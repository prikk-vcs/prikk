//! RFC 132 part 2, control 3: each of the six reclassified sites changes its prefix from
//! `lock conflict:` to `precondition not met:`, asserted against the compiled binary. The full-
//! queue site (`node_authoring.rs`/commit path) is covered by an extension to the existing
//! `dc57_active_patch_thresholds.rs::scaled_thresholds_warn_then_hard_block_then_seal_recovers`
//! (control 5's own "drive it through a real repository at the configured limit"); `active.rs`'s
//! own copy of the same check has no current CLI caller and is covered at the `prikk-store` level
//! instead (`active::tests::active_session_append_rejects_at_the_configured_limit_as_a_precondition`).
//! This file covers the other two CLI-reachable sites.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
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

    // 0.56.0 step W2b: the draft's own inverse is `delete-file genesis.txt` (the only operation
    // `genesis`'s one commit has to invert) -- queued on the active WAL, not yet sealed, and never
    // written to the worktree (`rollback-draft` only appends a patch; the note in its own report
    // says so: "rollback refs, audit policy, and worktree writes remain later PRs"). The worktree
    // still physically holds `genesis.txt` at this point, which `worktree-status` already reported
    // as "changed against baseline" (untracked) even before this round. Removing it here brings
    // the worktree's own physical state in line with what the draft already queues, exactly as a
    // caller who read that status would do by hand -- not a workaround for this round's own guard:
    // without it, this setup step's own `write_and_commit` would (correctly, as of W2b) refuse,
    // because authoring it would silently re-create `genesis.txt` as a brand-new node, undoing the
    // draft's own queued deletion the moment an unrelated file is committed alongside it. This test
    // is about `rollback-draft-verify`'s own refusal wording, not this interaction, so the setup is
    // adjusted to not exercise it.
    std::fs::remove_file(repo.join("genesis.txt")).unwrap();

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

/// RFC 132 per-site (0.40 item 5): `checkout --snapshot-plan` on a block with no snapshot is a
/// **precondition**, not damage.
///
/// **This site had no test at all before this one.** The reclassification was made, the whole suite
/// stayed green, and the ruled perturbation — flip it back, watch a test fail — had nothing to fail.
/// A classification nothing asserts is a classification the next round can undo by accident.
///
/// Only a checkpoint carries a snapshot (RFC 136 §10.2: a ref's first block and every 64 blocks
/// after), so the fixture seals a second block: an ordinary block that is not one.
#[test]
fn snapshot_plan_without_a_snapshot_is_a_precondition() {
    let repo = support::unique_repo("rfc132-snapshot-plan");
    support::init(&repo);
    write_and_commit(&repo, "a.txt", "hello", "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    write_and_commit(&repo, "b.txt", "second", "second");
    support::ok(
        &support::seal(&repo, "heads/main"),
        "seal a block that is not a checkpoint",
    );

    let out = support::prikk(&repo)
        .args(["checkout", "--snapshot-plan"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.starts_with(
            "error: precondition not met: checkout target for heads/main is not a \
                            checkpoint, so it carries no snapshot"
        ),
        "the class must be a precondition: {stderr}"
    );
    // The fact is kept and the route is added -- a refusal that names no way forward is where this
    // one started.
    assert!(
        stderr.contains("prikk checkout --patch-plan --ref heads/main"),
        "the refusal must name the route that works: {stderr}"
    );
    assert!(
        !stderr.contains("integrity error"),
        "the old class must be absent, not merely accompanied: {stderr}"
    );

    // And the route it names actually works from here, rather than being advice that fails.
    support::ok(
        &support::prikk(&repo)
            .args(["checkout", "--patch-plan", "--ref", "heads/main"])
            .output()
            .unwrap(),
        "the named route",
    );

    let _ = std::fs::remove_dir_all(&repo);
}
