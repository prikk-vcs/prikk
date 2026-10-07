//! 0.50.0 step 1, A6 item 1 (N6, RFC 163 §4, 019 §5.5): on the N6 state (an acknowledged record
//! damaged, nothing queued after it), `verify` used to also print *"trailing partial WAL bytes: N"*
//! and *"warning: active WAL contains an incomplete trailing record"*, and `doctor`'s first
//! recommendation was `--repair-wal-tail`, which then skips -- two recommendations, one a dead end.
//! Rule 3's own tail classification (`wal.rs`) is unchanged and correct for the general case (an
//! *unacknowledged* damaged last record genuinely is a repairable tail, per the existing
//! `wal_truncate_reports_when_the_removed_tail_includes_a_complete_damaged_record` test); what
//! changes is only the generic warning sentence and `doctor` issue, suppressed specifically when the
//! commit witness already explains the identical bytes as acknowledged damage or loss.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

fn wal_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/queue.wal")
}

/// One queued, witnessed, then damaged commit in the active session -- acknowledged damage
/// (row 4), the same fixture shape `rfc166_round2_refusals_speak_first.rs`'s own `damaged_repo`
/// builds, reproduced here rather than shared across files (that file's own `damaged_repo` is
/// private to it, matching this test tree's convention of small per-file fixtures).
fn damaged_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("queued.txt"), "queued\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "queued, acknowledged"),
        "commit",
    );

    let wal = wal_path(&repo);
    let mut bytes = std::fs::read(&wal).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal, &bytes).unwrap();
    repo
}

#[test]
fn verify_no_longer_also_reports_a_trailing_partial_over_acknowledged_damage() {
    let repo = damaged_repo("rfc163-n6-verify-no-tail-warning");
    let output = support::verify(&repo);
    let text = String::from_utf8_lossy(&output.stdout);

    assert!(
        text.contains("trailing partial WAL bytes:"),
        "the byte count itself is still printed, as a fact: {text}"
    );
    assert!(
        !text.contains("warning: active WAL contains an incomplete trailing record"),
        "the misleading benign-tail warning must not print over acknowledged damage: {text}"
    );
    assert!(
        text.contains("run `prikk doctor --discard-damaged-commits`"),
        "the one way out is still named: {text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn doctor_no_longer_recommends_repair_wal_tail_over_acknowledged_damage() {
    let repo = damaged_repo("rfc163-n6-doctor-no-repair-wal-tail");
    let mut cmd = support::prikk(&repo);
    let output = cmd.arg("doctor").output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);

    assert!(
        !text.contains("PRIKK-DOCTOR-WAL-TRAILING-PARTIAL"),
        "the dead-end recommendation must not appear: {text}"
    );
    assert!(
        !text.contains("--repair-wal-tail"),
        "doctor's first (and only) recommendation here must not be a repair that then skips: {text}"
    );
    assert!(
        text.contains("PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-DAMAGE"),
        "the real finding is still reported: {text}"
    );
    assert!(
        text.contains("--discard-damaged-commits"),
        "the one way out is still named: {text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

// Control for rule 3's own classification staying unchanged: every `commit` through the production
// commit flow writes a witness (confirmed by reading this file's own `damaged_repo` fixture against
// `rfc166_round2_refusals_speak_first.rs`'s identical one, and by the CLI itself -- there is no way
// to reach an unwitnessed, genuinely *unacknowledged* damaged-last-record state through `prikk
// commit` at all; only the raw `Wal::append_patch` primitive, bypassing the witness-writing commit
// flow entirely, can). That lower-level case is exactly what `wal.rs`'s own, pre-existing,
// unmodified-by-this-round test already covers and still passes unchanged:
// `wal_truncate_reports_when_the_removed_tail_includes_a_complete_damaged_record`
// (`crates/prikk-store/src/wal/tests.rs`) -- asserting `!replay.has_item_failure()` and a nonzero
// `trailing_partial_bytes` for that case, the intentional behavior this round's own fix must not
// (and does not) touch.
