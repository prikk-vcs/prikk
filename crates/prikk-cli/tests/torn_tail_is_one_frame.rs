//! RFC 160 F3, end to end: **a torn tail is a prefix of one frame, and a repair never removes a sound frame.**
//!
//! `doctor --repair-wal-tail` truncates "the incomplete final record". Every framed reader used to take *any* frame claiming more bytes
//! than remain for that record, whatever sound records followed it, so a damaged length in the first of two queued commits made the
//! second look like part of a torn tail -- and the repair deleted it (0.47.0: `doctor --repair-wal-tail` on a 702-byte WAL: "truncated
//! 702 byte(s), preserved 0 record(s)"). These controls run the real binary on real repositories.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

const WAL_HEADER_LEN: usize = 8 + 2 + 8 + 8 + 32;

/// A repository with **two queued commits**: an active WAL of two records.
fn two_queued_commits(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    for (index, name) in ["one.txt", "two.txt"].iter().enumerate() {
        std::fs::write(repo.join(name), format!("queued commit {index}\n")).unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("queued {index}")),
            "commit",
        );
    }
    repo
}

fn wal_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/queue.wal")
}

/// The byte range of each record of a WAL (header, then a body whose length is the big-endian `u64` at bytes 18..26 of the header).
fn records_of(wal: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut at = 0;
    let mut ranges = Vec::new();
    while at + WAL_HEADER_LEN <= wal.len() {
        let body_len = u64::from_be_bytes(wal[at + 18..at + 26].try_into().unwrap()) as usize;
        let end = at + WAL_HEADER_LEN + body_len;
        if end > wal.len() {
            break;
        }
        ranges.push(at..end);
        at = end;
    }
    ranges
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

/// **Control 1 -- the data-loss case, end to end.** Two queued commits, the first record's length set to 2^62. `verify` and `doctor`
/// exit non-zero and name the damaged record; **`doctor --repair-wal-tail` refuses and leaves the WAL byte for byte as it was**, and the
/// second record is still there and still read (`checked WAL records: 1`).
/// **Perturb:** the old classification (`sound_frame_after_partial` returning `None`): `verify` and `doctor` exit 0, the repair
/// truncates the whole WAL ("preserved 0 record(s)"), and this goes red.
#[test]
fn a_damaged_first_wal_record_is_reported_and_the_repair_refuses_and_deletes_nothing() {
    let repo = two_queued_commits("f3-wal-damage");
    let wal = wal_path(&repo);
    let mut bytes = std::fs::read(&wal).unwrap();
    let records = records_of(&bytes);
    assert_eq!(records.len(), 2, "fixture: two queued commits");
    bytes[18..26].copy_from_slice(&(1_u64 << 62).to_be_bytes());
    std::fs::write(&wal, &bytes).unwrap();

    let (verify, verify_text) = run(&repo, &["verify"]);
    assert!(
        verify.is_some_and(|code| code != 0),
        "verify exits non-zero over a damaged WAL record: {verify:?}\n{verify_text}"
    );
    assert!(
        verify_text.contains("checked WAL records: 1"),
        "the second record is still read\n{verify_text}"
    );
    let (doctor, doctor_text) = run(&repo, &["doctor"]);
    assert!(
        doctor.is_some_and(|code| code != 0),
        "{doctor:?}\n{doctor_text}"
    );
    assert!(
        doctor_text.contains("WAL record at offset 0")
            && doctor_text.contains("1 sound record(s) follow it"),
        "doctor names the damaged record and what stands behind it\n{doctor_text}"
    );
    assert!(
        !doctor_text.contains("run `prikk doctor --repair-wal-tail`"),
        "doctor never recommends the truncating repair for damage\n{doctor_text}"
    );

    let (repair, repair_text) = run(&repo, &["doctor", "--repair-wal-tail"]);
    assert!(
        repair.is_some_and(|code| code != 0),
        "the repair refuses: {repair:?}\n{repair_text}"
    );
    assert!(
        repair_text.contains("damaged record")
            && repair_text.contains("1 sound record(s) follow it"),
        "and says what it will not touch\n{repair_text}"
    );
    assert_eq!(
        std::fs::read(&wal).unwrap(),
        bytes,
        "the WAL is byte for byte as it was"
    );

    std::fs::write(repo.join("three.txt"), "after damage\n").unwrap();
    let after = support::commit(&repo, "heads/main", "after damage");
    let commit_text = format!(
        "{}{}",
        String::from_utf8_lossy(&after.stdout),
        String::from_utf8_lossy(&after.stderr)
    );
    assert!(!after.status.success(), "{commit_text}");
    assert!(
        commit_text.contains("damaged record") && !commit_text.contains("--repair-wal-tail"),
        "commit says the record is damaged and points to doctor for diagnosis, not to the truncating repair\n{commit_text}"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// **Control 2 -- crash recovery is unchanged.** A true torn tail (a **prefix** of the last frame) is still tolerated by `verify`, still
/// warned about by `doctor`, and still truncated **exactly** by `--repair-wal-tail`, preserving the record before it.
/// **Perturb:** treat every partial frame as damage: the repair refuses and this goes red.
#[test]
fn a_true_torn_wal_tail_is_still_tolerated_and_still_truncated_by_the_repair() {
    let repo = two_queued_commits("f3-wal-torn");
    let wal = wal_path(&repo);
    let bytes = std::fs::read(&wal).unwrap();
    let records = records_of(&bytes);
    let first_end = records[0].end;
    let cut = first_end + (records[1].len() / 2);
    std::fs::write(&wal, &bytes[..cut]).unwrap();

    let (verify, verify_text) = run(&repo, &["verify"]);
    assert_eq!(
        verify,
        Some(0),
        "a torn tail is tolerated by verify\n{verify_text}"
    );
    assert!(
        verify_text.contains(&format!("trailing partial WAL bytes: {}", cut - first_end)),
        "and reported\n{verify_text}"
    );
    let (doctor, doctor_text) = run(&repo, &["doctor"]);
    assert_eq!(doctor, Some(0), "{doctor_text}");
    assert!(
        doctor_text.contains("--repair-wal-tail"),
        "doctor recommends the repair for a torn tail\n{doctor_text}"
    );

    let (repair, repair_text) = run(&repo, &["doctor", "--repair-wal-tail"]);
    assert_eq!(repair, Some(0), "{repair_text}");
    assert!(repair_text.contains("preserved 1 record"), "{repair_text}");
    assert_eq!(
        std::fs::read(&wal).unwrap(),
        bytes[..first_end].to_vec(),
        "exactly the torn bytes are gone and the record before them is untouched"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// **Control 3 -- the ambiguous case resolves to damage.** A genuinely torn frame whose *payload holds a sound frame*: the second
/// record's header rewritten to claim far more than remains, directly in front of the second record's own bytes. A sound frame
/// starts inside the "tail", so it is damage: `verify` and `doctor` exit non-zero, the repair refuses, and **nothing is deleted**.
/// The cost of a false "damage" is a refused repair and a manual step; the cost of a false "tail" is lost data.
#[test]
fn a_torn_frame_whose_payload_holds_a_sound_frame_is_damage_and_the_repair_refuses() {
    let repo = two_queued_commits("f3-wal-ambiguous");
    let wal = wal_path(&repo);
    let bytes = std::fs::read(&wal).unwrap();
    let records = records_of(&bytes);
    let mut ambiguous = bytes[..records[0].end].to_vec();
    let mut partial_header = bytes[records[1].clone()][..WAL_HEADER_LEN].to_vec();
    partial_header[18..26].copy_from_slice(&(1_u64 << 40).to_be_bytes());
    ambiguous.extend_from_slice(&partial_header);
    ambiguous.extend_from_slice(&bytes[records[1].clone()]); // the sound frame, inside the claimed payload
    std::fs::write(&wal, &ambiguous).unwrap();

    let (verify, verify_text) = run(&repo, &["verify"]);
    assert!(
        verify.is_some_and(|code| code != 0),
        "{verify:?}\n{verify_text}"
    );
    let (repair, repair_text) = run(&repo, &["doctor", "--repair-wal-tail"]);
    assert!(
        repair.is_some_and(|code| code != 0),
        "{repair:?}\n{repair_text}"
    );
    assert_eq!(
        std::fs::read(&wal).unwrap(),
        ambiguous,
        "nothing is deleted"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// **Control 2, the object index.** A true torn tail (a prefix of an entry) is still tolerated by `verify`. (The ref log container's
/// true torn tail is held at the reader, in `every_framed_reader_treats_a_partial_frame_before_a_sound_frame_as_damage`'s crash-recovery
/// half, and end to end by the publication-recovery tests; `verify` on a published ref whose log has a torn tail has always reported
/// the divergence, and still does.)
#[test]
fn a_true_torn_tail_on_the_object_index_is_still_tolerated() {
    let repo = support::unique_repo("f3-torn-index-and-log");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "one"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    assert_eq!(
        run(&repo, &["verify"]).0,
        Some(0),
        "fixture: a healthy repository"
    );

    // A prefix of the last index entry, appended: an interrupted index append.
    let index = repo.join(".prikk/containers/index.container");
    let mut bytes = std::fs::read(&index).unwrap();
    let entry_len = 50 + 32 + 2 + 1 + 8 + 8 + 32;
    let last = bytes[bytes.len() - entry_len..].to_vec();
    bytes.extend_from_slice(&last[..entry_len / 2]);
    std::fs::write(&index, bytes).unwrap();
    let (verify, text) = run(&repo, &["verify"]);
    assert_eq!(verify, Some(0), "a torn index tail is tolerated\n{text}");
    let _ = std::fs::remove_dir_all(repo);
}
