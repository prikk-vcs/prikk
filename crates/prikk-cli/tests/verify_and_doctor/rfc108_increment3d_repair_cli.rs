//! RFC 108 increment 3d, review v1's condition (`.git-exclude/reviewed/per-active-repair-review-v1.md`):
//! `prikk doctor --repair-wal-tail` used to report success, print "truncated 0", and exit 0 while
//! leaving a repairable trailing partial WAL untouched whenever an active session's lock was busy --
//! the review's own probe measured exactly this. `repair_repository`'s `active_repairs` field
//! already carried the skip and its reason; nothing in `prikk-cli` read it. This file reproduces the
//! review's own scenario end-to-end through the real binary and asserts both halves of the fix: the
//! skip and its reason reach stdout, and the exit code goes non-zero.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "../support/mod.rs"]
mod support;

use prikk_store::{ActiveLock, DEFAULT_ACTIVE_NAME, RepositoryLayout};

/// Same technique as `prikk-store::doctor::tests::doctor_reports_trailing_partial_wal_warning`:
/// seven raw bytes written straight into a freshly-initialized repository's WAL file are read back
/// as a single trailing-partial tail with zero preserved records -- the smallest fixture that is
/// genuinely repairable by `--repair-wal-tail`.
fn plant_repairable_trailing_partial(layout: &RepositoryLayout) {
    std::fs::write(layout.default_queue_wal_path(), b"partial").unwrap();
}

#[test]
fn repair_wal_tail_reports_skip_and_fails_when_default_lock_is_busy() {
    let repo = support::unique_repo("rfc108-3d-cli-busy-lock");
    support::init(&repo);
    let layout = RepositoryLayout::open(&repo).unwrap();
    plant_repairable_trailing_partial(&layout);

    // Hold `default`'s own lock across the CLI invocation below, exactly the review's probe
    // scenario: a busy lock on the one active session that also has repairable damage.
    let lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
        .expect("acquire default's lock to simulate it being busy");

    let busy_run = support::prikk(&repo)
        .args(["doctor", "--repair-wal-tail"])
        .output()
        .unwrap();
    let busy_stdout = String::from_utf8_lossy(&busy_run.stdout).into_owned();

    assert!(
        !busy_run.status.success(),
        "a requested repair that skipped an active session must exit non-zero: {busy_stdout}"
    );
    assert!(
        busy_stdout.contains("skipped"),
        "stdout must name the skip: {busy_stdout}"
    );
    assert!(
        busy_stdout.contains("default"),
        "stdout must name which active session was skipped: {busy_stdout}"
    );
    assert!(
        busy_stdout.to_lowercase().contains("lock"),
        "the skip reason (a lock conflict) must reach stdout, not just the struct: {busy_stdout}"
    );

    drop(lock);
    let wal_after_busy_run = std::fs::read(layout.default_queue_wal_path()).unwrap();
    assert_eq!(
        wal_after_busy_run.len(),
        7,
        "the busy-lock run above must not have truncated anything: {wal_after_busy_run:?}"
    );

    // Once the lock is free, the exact same repair request must succeed and actually truncate.
    let retry = support::prikk(&repo)
        .args(["doctor", "--repair-wal-tail"])
        .output()
        .unwrap();
    let retry_stdout = String::from_utf8_lossy(&retry.stdout).into_owned();
    assert!(
        retry.status.success(),
        "once the lock is free the same repair must succeed: {retry_stdout}"
    );
    assert!(
        retry_stdout.contains("repaired"),
        "the retry must report an actual repair, not another skip: {retry_stdout}"
    );
    assert!(
        !retry_stdout.contains("skipped"),
        "the retry must not report any skip once the lock is free: {retry_stdout}"
    );
    let wal_after_retry = std::fs::read(layout.default_queue_wal_path()).unwrap();
    assert!(
        wal_after_retry.is_empty(),
        "the retry must actually truncate the trailing partial: {wal_after_retry:?}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
