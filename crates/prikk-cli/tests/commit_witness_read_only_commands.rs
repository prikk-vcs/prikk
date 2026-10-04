//! RFC 166, K5: `verify`, `status`, plain `doctor`, and `doctor --repair-wal-tail` over an already
//! sound WAL are all read-only with respect to the commit witness -- none of them writes, rewrites, or
//! clears it. Only `doctor --repair-tails` (over a damaged or stale witness, §13 item 5, not yet
//! implemented) and the three appenders (`commit`, `rollback-draft`, a session's own `ActiveSession::
//! append_patch`) and `seal`'s own drain are allowed to touch the witness file at all.
//!
//! **Perturb:** have any of the four commands below call `clear_witness` or `append_patch_and_witness`
//! on its own read-only path, and this goes red.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};

fn witness_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/witness")
}

#[test]
fn verify_status_and_doctor_leave_the_commit_witness_byte_identical() {
    let repo = support::unique_repo("rfc166-k5-read-only");
    support::init(&repo);
    std::fs::write(repo.join("one.txt"), "first queued commit\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued 0"), "commit");
    std::fs::write(repo.join("two.txt"), "second queued commit\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued 1"), "commit");

    let witness = witness_path(&repo);
    let before = std::fs::read(&witness).unwrap();
    assert!(
        !before.is_empty(),
        "fixture: two real commits leave a witness"
    );

    for args in [
        &["verify"][..],
        &["status"][..],
        &["doctor"][..],
        &["doctor", "--repair-wal-tail"][..],
    ] {
        let output = support::prikk(&repo).args(args).output().unwrap();
        assert_eq!(
            std::fs::read(&witness).unwrap(),
            before,
            "{args:?} must not touch the commit witness; exit {:?}\n{}{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let _ = std::fs::remove_dir_all(repo);
}
