//! RFC 132 part 2, control 3: each of the six reclassified sites changes its prefix from
//! `lock conflict:` to `precondition not met:`, asserted against the compiled binary. The full-
//! queue site (`node_authoring.rs`/commit path) is covered by an extension to the existing
//! `dc57_active_patch_thresholds.rs::scaled_thresholds_warn_then_hard_block_then_seal_recovers`
//! (control 5's own "drive it through a real repository at the configured limit"); `active.rs`'s
//! own copy of the same check has no current CLI caller and is covered at the `prikk-store` level
//! instead (`active::tests::active_session_append_rejects_at_the_configured_limit_as_a_precondition`).
//! This file covers the other two CLI-reachable sites.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;

fn write_and_commit(repo: &Path, path: &str, content: &str, message: &str) {
    std::fs::write(repo.join(path), content).unwrap();
    support::ok(&support::commit(repo, "heads/main", message), message);
}

/// `rollback-draft requires an empty active WAL` (`rollback_draft.rs`).
#[test]
fn rollback_draft_prefix_changes_to_precondition() {
    let repo = support::unique_repo("rfc132-part2-rollback-draft");
    support::init(&repo);
    write_and_commit(&repo, "genesis.txt", "genesis", "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    // Queue a second, unsealed patch -- the active WAL is now non-empty.
    write_and_commit(&repo, "queued.txt", "queued", "queued, not sealed");

    let out = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["rollback-draft", "--append-inverse", "-m", "undo"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "rollback-draft over a non-empty active WAL must be refused"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("precondition not met: rollback-draft requires an empty active WAL"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// `rollback-draft-verify requires an active WAL containing only the rollback draft`
/// (`rollback_verify.rs`).
#[test]
fn rollback_draft_verify_prefix_changes_to_precondition() {
    let repo = support::unique_repo("rfc132-part2-rollback-verify");
    support::init(&repo);
    write_and_commit(&repo, "genesis.txt", "genesis", "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    let draft = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["rollback-draft", "--append-inverse", "-m", "undo"])
        .output()
        .unwrap();
    support::ok(&draft, "rollback-draft onto an empty active WAL");

    // Queue a second record alongside the rollback draft -- the active WAL is no longer *only*
    // the draft.
    write_and_commit(
        &repo,
        "alongside.txt",
        "alongside",
        "queued alongside the draft",
    );

    let out = support::prikk(&repo)
        .args(["rollback-draft-verify"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "rollback-draft-verify over more than the draft alone must be refused"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "precondition not met: rollback-draft-verify requires an active WAL containing only \
             the rollback draft"
        ),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
