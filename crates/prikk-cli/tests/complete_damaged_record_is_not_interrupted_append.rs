//! 0.49.0 step 5, D11/U5: **a full-length frame whose checksum fails is a damaged record, not an
//! interrupted append.** RFC 164 §9's principle (RFC 165 R5 §9.2 applied it to the ref container),
//! applied here to the object container: `foundation/container.rs`'s checksum-mismatch arm used to
//! set `complete: false`, the same flag a genuine torn tail (fewer bytes physically present than the
//! header claims) sets -- so a fully-present but corrupted record was reported, wrongly, as the
//! "interrupted appends" warning `verify`/`doctor` otherwise reserve for a crash mid-write. The two
//! shapes below are built byte-for-byte to be the **same size** and differ only in which bytes are
//! wrong, so the test names exactly what distinguishes them: physical presence, not the checksum
//! outcome.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

/// The container path and frame offset a printed line names after `prefix`, the path split off at `end`. The path is
/// compared by identity (`support::assert_same_path`), never as text: a temp directory prints differently on macOS
/// and Windows.
fn assert_printed_frame(
    text: &str,
    marker: &str,
    prefix: &str,
    end: &str,
    container: &Path,
    offset: usize,
) {
    let line = text
        .lines()
        .find(|line| line.contains(marker))
        .unwrap_or_else(|| panic!("no line contains {marker:?}\n{text}"));
    let rest = &line[line.find(prefix).unwrap() + prefix.len()..];
    let location = rest.split(end).next().unwrap();
    let (printed, printed_offset) = location
        .rsplit_once(&format!("{}#", std::path::MAIN_SEPARATOR))
        .unwrap_or_else(|| panic!("no separator-#offset in {location:?}"));
    support::assert_same_path(printed, container);
    assert_eq!(printed_offset, offset.to_string(), "{line}");
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

/// One committed blob, returning the repo and the container file's one frame as `(header_and_body,
/// frame_offset)` -- the frame starts at offset 0, since this is the container's first and only
/// record.
fn one_committed_blob(tag: &str) -> (PathBuf, PathBuf, Vec<u8>) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("first.txt"), "the body of the only object\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "first"),
        "first commit",
    );
    let container = repo.join(".prikk/containers/blob/a.container");
    let whole = std::fs::read(&container).unwrap();
    (repo, container, whole)
}

/// **Shape 1 -- a torn tail: fewer bytes are physically present than the frame's own header claims.**
/// Truncate the frame mid-body, then append a second, sound commit's frame past the cut -- the same
/// shape `interrupted_append.rs` uses, so `sound_frame_after_partial` finds something after the torn
/// bytes and reports a `Failed{complete: false}` outcome instead of silently swallowing the rest of
/// the file as one trailing partial. This is the shape `verify`/`doctor` still call "interrupted
/// append".
#[test]
fn a_torn_tail_is_reported_as_an_interrupted_append() {
    let (repo, container, whole) = one_committed_blob("u5-torn-tail");
    let body_len = u64::from_be_bytes(whole[10..18].try_into().unwrap()) as usize;
    let torn_at = whole.len();
    let torn = whole[..50 + body_len / 2].to_vec();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&container)
        .unwrap();
    std::io::Write::write_all(&mut file, &torn).unwrap();
    std::fs::write(repo.join("second.txt"), "a later, sound commit\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "second"),
        "a commit past the torn frame",
    );

    let (code, text) = run(&repo, &["verify"]);
    assert_eq!(
        code,
        Some(0),
        "a torn tail is a warning, not a failure\n{text}"
    );
    assert!(
        text.contains("object items: 2 scanned, 0 failed"),
        "the torn frame is not a failed item; the later blob is still scanned\n{text}"
    );
    assert!(
        text.contains("interrupted appends: 1"),
        "the torn frame is counted as an interrupted append\n{text}"
    );
    assert_printed_frame(
        &text,
        "warning: interrupted append in Blob at ",
        "warning: interrupted append in Blob at ",
        ": ",
        &container,
        torn_at,
    );
    assert!(
        text.contains("connectivity finds nothing that still needs it"),
        "the warning says nothing references the torn frame\n{text}"
    );
    assert!(
        !text.contains("damaged record"),
        "a torn tail is never worded as a damaged record\n{text}"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// **Shape 2 -- a complete damaged record: every byte the header claims is physically present, and
/// the checksum fails anyway.** Flip one byte inside the body, in place, so the file's length is
/// unchanged -- there is nothing "torn" about it; `parse_frame_at` reads the full header and the full
/// claimed body before the checksum comparison fails. This must be a failed object item, not an
/// interrupted-append warning (the bug this unit fixes: `foundation/container.rs` used to set
/// `complete: false` here too, the same flag a torn tail sets).
#[test]
fn a_complete_record_with_a_bad_checksum_is_a_failed_item_not_an_interrupted_append() {
    let (repo, container, mut whole) = one_committed_blob("u5-damaged-record");
    let last = whole.len() - 1;
    whole[last] ^= 0xFF;
    std::fs::write(&container, &whole).unwrap();

    let (code, text) = run(&repo, &["verify"]);
    assert_eq!(
        code,
        Some(1),
        "a complete record with a bad checksum is damage, not a tolerated warning\n{text}"
    );
    assert!(
        text.contains("object items: 1 scanned, 1 failed"),
        "the one record is a failed item\n{text}"
    );
    assert_printed_frame(
        &text,
        ": failed: container checksum mismatch",
        "object ",
        " (blob)",
        &container,
        0,
    );
    assert!(
        text.contains(": failed: container checksum mismatch at byte offset 0"),
        "the failure names the checksum mismatch, worded as a failed object, not an interrupted \
         append\n{text}"
    );
    assert!(
        text.contains("interrupted appends: 0") && !text.contains("warning: interrupted append"),
        "a complete damaged record must not be counted or worded as an interrupted append\n{text}"
    );
    let (doctor_code, doctor_text) = run(&repo, &["doctor"]);
    assert!(
        doctor_code.is_some_and(|c| c != 0),
        "doctor also treats it as damage\n{doctor_text}"
    );
    assert!(
        !doctor_text.contains("PRIKK-DOCTOR-OBJECT-INTERRUPTED-APPEND"),
        "doctor must not report the interrupted-append code for a complete damaged record\n{doctor_text}"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// **0.49.0 step 5, round 2 U2: an earlier object that rots, followed by later commits, is damage.** The first
/// object's frame is complete and its checksum fails over a body that is fully present; the next, sound frame
/// starts exactly at that frame's claimed end, not inside it, so nothing overran it -- it is a damaged record.
/// **Perturb:** make the borrowed-tail rule always answer "torn" (`checksum_mismatch_is_complete` returning
/// `false`): this test goes red (the rotted object is then an interrupted append, not a failed item).
#[test]
fn an_earlier_object_that_rots_before_later_commits_is_a_failed_item() {
    let repo = support::unique_repo("u2-rot-then-later");
    support::init(&repo);
    std::fs::write(repo.join("first.txt"), "the first object, which will rot\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "first"),
        "first commit",
    );
    for index in 0..2 {
        std::fs::write(
            repo.join(format!("later{index}.txt")),
            format!("a later commit {index}\n"),
        )
        .unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("later {index}")),
            "a later commit",
        );
    }
    let container = repo.join(".prikk/containers/blob/a.container");
    let mut bytes = std::fs::read(&container).unwrap();
    bytes[60] ^= 0x01;
    std::fs::write(&container, &bytes).unwrap();

    let (code, text) = run(&repo, &["verify"]);
    assert_eq!(code, Some(1), "a rotted object is damage\n{text}");
    assert!(
        text.contains("object items: 3 scanned, 1 failed")
            && text.contains("#0 (blob): failed: container checksum mismatch at byte offset 0"),
        "the rotted first object is the failed item\n{text}"
    );
    assert!(
        text.contains("interrupted appends: 0") && !text.contains("warning: interrupted append"),
        "nothing overran the rotted frame, so it is not an interrupted append\n{text}"
    );
    let _ = std::fs::remove_dir_all(repo);
}
