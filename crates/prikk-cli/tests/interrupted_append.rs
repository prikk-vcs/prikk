//! RFC 160 F3 Addendum 1: **an interrupted append followed by later writes is not damage when nothing names it.**
//!
//! An object container's record is made durable *before* its index entry is appended, so a frame no index entry names was never
//! committed: it is what a crash between the two leaves. A crash-torn frame followed by later commits (the container writer appends
//! past it by design) used to make `verify` fail for good when the later bytes ran past the torn frame's claim ("container checksum
//! mismatch", no repair), and to silently skip the later objects when they fell short. It is now a **warning** that names the offset and
//! says nothing references it; every later object is still scanned and read. **A frame an index entry names stays a failed item.**

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

/// A repository with one committed blob of `big` bytes, then a crash-torn copy of its frame (header and half its body) appended to the
/// blob container, then `later` more commits appended past it. Returns the repository and the torn frame's offset.
fn torn_then_appended(tag: &str, big: usize, later: usize) -> (PathBuf, usize) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("first.txt"), "x".repeat(big)).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "first"),
        "first commit",
    );
    let container = repo.join(".prikk/containers/blob/a.container");
    let whole = std::fs::read(&container).unwrap();
    let body_len = u64::from_be_bytes(whole[10..18].try_into().unwrap()) as usize;
    let torn_at = whole.len();
    let mut torn = whole[..50 + body_len / 2].to_vec();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&container)
        .unwrap();
    std::io::Write::write_all(&mut file, &torn).unwrap();
    torn.clear();
    for index in 0..later {
        std::fs::write(
            repo.join(format!("later{index}.txt")),
            format!("later {index}\n"),
        )
        .unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("later {index}")),
            "a commit past the torn frame",
        );
    }
    (repo, torn_at)
}

fn run(repo: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = support::prikk(repo).args(args).output().unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// **Control 1 -- both of the probe's shapes.** The later bytes *exceed* the torn frame's claim (0.47.0: `verify` exit 1, "container
/// checksum mismatch") and *fall short* of it (0.47.0: exit 0, but the later objects silently swallowed). In both: `verify` exits 0
/// with the warning naming the torn frame's offset, **every** later object is scanned (`0 failed`), and `doctor` exits 0 with a warning.
/// **Perturb:** treat an unindexed frame as failed: both shapes exit 1 and this goes red.
#[test]
fn an_interrupted_append_that_nothing_names_is_a_warning_and_every_later_object_is_scanned() {
    for (shape, big, later) in [
        ("later bytes exceed the claim", 200, 4),
        ("later bytes fall short of the claim", 200_000, 1),
    ] {
        let (repo, torn_at) = torn_then_appended("f3-interrupted-append", big, later);
        let (verify, text) = run(&repo, &["verify"]);
        assert_eq!(
            verify,
            Some(0),
            "{shape}: verify tolerates an interrupted append\n{text}"
        );
        assert!(
            text.contains(&format!("#{torn_at}"))
                && text.contains("connectivity finds nothing that still needs it"),
            "{shape}: the warning names the offset and says nothing references it\n{text}"
        );
        assert!(
            text.contains(&format!("object items: {} scanned, 0 failed", 1 + later)),
            "{shape}: the first blob and every later one are scanned, none failed\n{text}"
        );
        let (doctor, doctor_text) = run(&repo, &["doctor"]);
        assert_eq!(doctor, Some(0), "{shape}: {doctor_text}");
        assert!(
            doctor_text.contains("PRIKK-DOCTOR-OBJECT-INTERRUPTED-APPEND"),
            "{shape}: doctor warns\n{doctor_text}"
        );
        // Every later object is readable (the commit index and worktree read them back).
        assert_eq!(run(&repo, &["status"]).0, Some(0), "{shape}");
        let _ = std::fs::remove_dir_all(repo);
    }
}

/// **Control 2 -- the same torn frame, named by an index entry: a failed item.** An index entry (a copy of a real one, its offset and
/// length pointed at the torn frame, its checksum recomputed) names the frame, so it was supposedly committed and is unreadable:
/// `verify` exits 1 and `doctor` exits non-zero. **Never silence a record something names.**
/// **Perturb:** treat an indexed frame as unindexed: this goes red (exit 0).
#[test]
fn the_same_torn_frame_named_by_an_index_entry_is_a_failed_item() {
    let (repo, torn_at) = torn_then_appended("f3-interrupted-append-named", 200, 2);
    let index = repo.join(".prikk/containers/index.container");
    let mut bytes = std::fs::read(&index).unwrap();
    // The first entry's frame: magic(8) version(2) body_len(8) checksum(32), then the body: id(32) type(2) slot(1) offset(8) length(8) checksum(32).
    let header = bytes[..10].to_vec();
    let mut body = bytes[50..50 + 83].to_vec();
    body[35..43].copy_from_slice(&(torn_at as u64).to_be_bytes());
    body[43..51].copy_from_slice(&1000_u64.to_be_bytes());
    let body_len = (body.len() as u64).to_be_bytes();
    let mut preimage = header.clone();
    preimage.extend_from_slice(&body_len);
    preimage.extend_from_slice(&body);
    let mut entry = header;
    entry.extend_from_slice(&body_len);
    entry.extend_from_slice(&prikk_hash::sha256(&preimage));
    entry.extend_from_slice(&body);
    bytes.extend_from_slice(&entry);
    std::fs::write(&index, bytes).unwrap();

    let (verify, text) = run(&repo, &["verify"]);
    assert_eq!(
        verify,
        Some(1),
        "an indexed unreadable frame is damage\n{text}"
    );
    assert!(
        text.contains("failed") && !text.contains("object items: 3 scanned, 0 failed"),
        "{text}"
    );
    // Named by an entry, the frame is a failed item and **not** an interrupted append: two independent guards hold this (the index pass
    // reports an entry it cannot read; the container scan reports the frame), so the control asserts the second's half too -- the
    // frame is not among the interrupted-append warnings.
    assert!(
        text.contains("interrupted appends: 0") && !text.contains("warning: interrupted append"),
        "{text}"
    );
    let (doctor, doctor_text) = run(&repo, &["doctor"]);
    assert!(doctor.is_some_and(|code| code != 0), "{doctor_text}");
    let _ = std::fs::remove_dir_all(repo);
}
