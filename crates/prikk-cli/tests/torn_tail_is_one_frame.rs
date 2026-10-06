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

/// RFC 166: the commit witness for `default`'s own active session.
fn witness_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/witness")
}

/// Like [`two_queued_commits`], but also returns the commit witness exactly as the first commit left
/// it -- before the second commit's own witness write ever happened. RFC 166: a byte-truncated second
/// record whose witness still names it as acknowledged is not an unacknowledged crash tail, it is
/// acknowledged damage (row 4) -- a genuinely *never-acknowledged* torn second record additionally
/// needs the witness rolled back to what it was after only the first commit, the state a real crash
/// mid-way through the second commit's own WAL append (before that commit's own witness write, which
/// is strictly ordered after it) would actually have left behind.
fn two_queued_commits_with_witness_after_first(tag: &str) -> (PathBuf, Vec<u8>) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    let mut witness_after_first = None;
    for (index, name) in ["one.txt", "two.txt"].iter().enumerate() {
        std::fs::write(repo.join(name), format!("queued commit {index}\n")).unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("queued {index}")),
            "commit",
        );
        if index == 0 {
            witness_after_first = Some(std::fs::read(witness_path(&repo)).unwrap());
        }
    }
    (repo, witness_after_first.unwrap())
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

/// The entry a repair names: its 16-hex-character id, from `recovery/log, entry <id>`.
fn named_entry(output: &str) -> String {
    let start = output
        .find("recovery/log, entry ")
        .unwrap_or_else(|| panic!("the repair names its recovery entry\n{output}"))
        + "recovery/log, entry ".len();
    output[start..].chars().take(16).collect()
}

/// RFC 168 §3.2: the recovery log holds exactly `expected` as the removed bytes of the entry the repair named, and the
/// listing shows that entry. The removed bytes are stored raw in the frame, so a byte search finds them.
fn assert_log_holds_named_entry(repo: &Path, output: &str, expected: &[u8]) {
    let id = named_entry(output);
    let log = std::fs::read(repo.join(".prikk/recovery/log")).unwrap();
    assert!(
        log.windows(expected.len()).any(|window| window == expected),
        "the recovery log holds the removed bytes"
    );
    let (code, listing) = run(repo, &["doctor", "--recovery-list"]);
    assert_eq!(code, Some(0), "{listing}");
    assert!(
        listing.contains(&id),
        "the listing shows the entry the repair named ({id}):\n{listing}"
    );
}

/// **Addendum 1, control 1 -- a lone damaged record with no witness: the repair truncates it, and keeps it.** One queued commit, its
/// length set to 2^62, **and its commit witness removed** (RFC 166 row 1: no witness file is rule 3 for this session, exactly as
/// 0.48.0 -- a legacy session, or one whose witness was itself removed): nothing sound is behind it, so it reads as a torn tail (the
/// ambiguity no rule can resolve without a witness) and `--repair-wal-tail` removes it. **The recovery file the output names holds
/// exactly the removed bytes, byte for byte**: a repair that was wrong about what it removed lost nothing, and the record can be read
/// back.
/// **Perturb:** truncate without saving: the output names no file and this goes red.
#[test]
fn a_repair_of_a_lone_damaged_record_with_no_witness_saves_the_record_byte_for_byte() {
    let repo = support::unique_repo("f3-wal-lone");
    support::init(&repo);
    std::fs::write(repo.join("one.txt"), "the only queued commit\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued"), "commit");
    let wal = wal_path(&repo);
    let mut bytes = std::fs::read(&wal).unwrap();
    bytes[18..26].copy_from_slice(&(1_u64 << 62).to_be_bytes());
    std::fs::write(&wal, &bytes).unwrap();
    std::fs::remove_file(witness_path(&repo)).unwrap();

    let (repair, text) = run(&repo, &["doctor", "--repair-wal-tail"]);
    assert_eq!(repair, Some(0), "{text}");
    assert!(
        std::fs::read(&wal).unwrap().is_empty(),
        "the lone record is removed"
    );
    assert_log_holds_named_entry(&repo, &text, &bytes);
    let _ = std::fs::remove_dir_all(repo);
}

/// **RFC 166, N6, end to end: a lone damaged record that WAS witnessed is acknowledged damage, not an ambiguous tail.** Same byte
/// damage as the no-witness control above, but the commit witness is left in place, naming this record as acknowledged -- the
/// ambiguity the no-witness control relies on does not exist here, so `--repair-wal-tail` must refuse rather than silently discard an
/// acknowledged commit (0.48.0's own defect this RFC exists to close: `doctor --repair-wal-tail` used to remove this record, exit 0,
/// and leave the user down one queued commit with no record anything was ever wrong).
/// **Perturb:** delete the witness first (the control above): the repair succeeds and this distinction disappears.
#[test]
fn a_witnessed_lone_damaged_record_is_acknowledged_damage_and_the_repair_refuses() {
    let repo = support::unique_repo("f3-wal-lone-witnessed");
    support::init(&repo);
    std::fs::write(repo.join("one.txt"), "the only queued commit\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued"), "commit");
    let wal = wal_path(&repo);
    let bytes = std::fs::read(&wal).unwrap();
    let mut damaged = bytes.clone();
    damaged[18..26].copy_from_slice(&(1_u64 << 62).to_be_bytes());
    std::fs::write(&wal, &damaged).unwrap();

    let (verify, verify_text) = run(&repo, &["verify"]);
    assert!(
        verify.is_some_and(|code| code != 0),
        "verify refuses over acknowledged damage: {verify:?}\n{verify_text}"
    );
    let (repair, repair_text) = run(&repo, &["doctor", "--repair-wal-tail"]);
    assert!(
        repair.is_some_and(|code| code != 0),
        "the repair refuses rather than discard an acknowledged commit: {repair:?}\n{repair_text}"
    );
    assert_eq!(
        std::fs::read(&wal).unwrap(),
        damaged,
        "nothing is removed; the WAL is byte for byte as it was"
    );
    let _ = std::fs::remove_dir_all(repo);
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
    let (repo, witness_after_first) = two_queued_commits_with_witness_after_first("f3-wal-torn");
    let wal = wal_path(&repo);
    let bytes = std::fs::read(&wal).unwrap();
    let records = records_of(&bytes);
    let first_end = records[0].end;
    let cut = first_end + (records[1].len() / 2);
    std::fs::write(&wal, &bytes[..cut]).unwrap();
    // RFC 166: a genuine crash mid-way through the second commit's own WAL append happens strictly
    // before that commit's own witness write, so the witness must still read exactly as the first
    // commit left it -- not what a real, completed second commit actually wrote.
    std::fs::write(witness_path(&repo), &witness_after_first).unwrap();

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
    // RFC 160 F3 Addendum 1, RFC 168: the removed bytes are kept, exactly, in the log entry the output names.
    assert_log_holds_named_entry(&repo, &repair_text, &bytes[first_end..cut]);
    assert_eq!(
        run(&repo, &["verify"]).0,
        Some(0),
        "verify ignores the recovery file (never authority)"
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
