//! 0.56.0 step W2c, through the compiled binary.
//!
//! Two independent fixes, each with its own test(s):
//! 1. The ref-name gate (W2b's own `recorded_names_canonical_ref`) is removed: a cross-ref commit
//!    (`commit --ref <other>`) that would revert a diverged *edit* -- not just a deletion, W1's
//!    case 5 -- now refuses regardless of which ref the worktree's record names, and a real
//!    `branch switch` to the ref actually being committed onto is what lets the legitimate commit
//!    through (case 1/case 2 below).
//! 2. The resync (`branch switch <the current branch>`) detects a moved *fold*, not only a moved
//!    sealed block: `rollback-draft --append-inverse` moves only the active WAL's own queued fold,
//!    never the sealed block, so it was invisible to the old sealed-block-only comparison (case 3
//!    below). **0.56.0 step W2d-2 correction:** this round (W2c) also added a self-switch
//!    materializer that applied the fold directly through the lifecycle cache, bypassing RFC 136's
//!    verified-anchor write gate -- removed by W2d-2. Case 3 (below) now asserts the corrected
//!    behavior: a bare self-switch over a non-empty, behind-reading queue refuses, naming `seal`
//!    then `branch switch` again. **A found, not fixed, residual gap**: after `seal`, the
//!    self-switch does not actually apply the deletion either -- a pre-existing, unrelated no-op
//!    early-return (block-identity only, no real content check) reports `already_current` first.
//!    See case 3's own final section for the full account; reported in the W2d report.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::Path;
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
        .output()
        .unwrap()
}

fn worktree_status(repo: &Path) -> Output {
    support::prikk(repo)
        .args(["worktree-status"])
        .output()
        .unwrap()
}

/// `commit --ref <ref_name>`, signed by the fixed test author -- distinct from `support::commit`
/// only in being explicit that the worktree may not actually be on `ref_name` (the cross-ref shape
/// this file's first two cases are about).
fn commit_onto(repo: &Path, ref_name: &str, message: &str) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--ref", ref_name, "-m", message])
        .output()
        .unwrap()
}

/// Case 1 (the ruling's own measured W2c hazard): `main` has `a.txt` = `a1`; `feature` changes it to
/// `fx` (sealed, a genuine divergence, not merely new content). Back on `main`'s worktree (still
/// physically holding `a1`), an unrelated edit plus `commit --ref heads/feature` must refuse --
/// before this round, it silently authored `edit-text a.txt` (plus the unrelated file), reverting
/// `feature`'s own real, sealed change the moment `seal` ran.
#[test]
fn case1_cross_ref_commit_would_revert_a_diverged_edit_refuses() {
    let repo = support::unique_repo("w2c-case1-cross-ref-edit");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a1\n", "genesis a1");
    ok(
        &support::branch_create(&repo, "heads/feature", "heads/main"),
        "branch create feature",
    );
    ok(&switch(&repo, "heads/feature"), "switch to feature");
    support::generation(
        &repo,
        "heads/feature",
        "a.txt",
        b"fx\n",
        "feature diverges a.txt",
    );
    ok(&switch(&repo, "heads/main"), "switch back to main");
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a1\n",
        "back on main, the worktree holds main's own a1, not feature's fx"
    );

    // The hazard: an unrelated edit, committed directly onto `heads/feature` from this worktree,
    // which still holds main's pre-divergence bytes for `a.txt`.
    std::fs::write(repo.join("b.txt"), b"unrelated\n").unwrap();
    let refused = commit_onto(
        &repo,
        "heads/feature",
        "unrelated edit, redirected onto feature",
    );
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit --ref heads/feature must refuse: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit")
            && stderr(&refused).contains("a.txt")
            && stderr(&refused).contains("does not match heads/feature's current state"),
        "the refusal must name a.txt as present-but-stale against heads/feature, not merely \
         absent: {}",
        stderr(&refused)
    );

    // Confirm feature's own sealed content is untouched: the refusal happened before any write.
    let status = support::prikk(&repo).args(["status"]).output().unwrap();
    ok(&status, "status after the refusal");
    assert!(
        stdout(&status).contains("queued patches: 0") || !stdout(&status).contains("queued"),
        "nothing was queued by the refused commit: {}",
        stdout(&status)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Case 2: the same divergent history as case 1, but the worktree genuinely switches to
/// `heads/feature` first (the way a user must) before staging its own further, unrelated edit.
/// Authored cleanly -- no stray revert of `a.txt`, and `feature`'s own `fx` survives a seal.
#[test]
fn case2_after_a_real_switch_the_same_commit_is_authored_cleanly() {
    let repo = support::unique_repo("w2c-case2-cross-ref-edit-after-switch");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a1\n", "genesis a1");
    ok(
        &support::branch_create(&repo, "heads/feature", "heads/main"),
        "branch create feature",
    );
    ok(&switch(&repo, "heads/feature"), "switch to feature");
    support::generation(
        &repo,
        "heads/feature",
        "a.txt",
        b"fx\n",
        "feature diverges a.txt",
    );
    ok(&switch(&repo, "heads/main"), "switch back to main");

    // The fix: a real switch to feature before staging feature's own next change.
    ok(
        &switch(&repo, "heads/feature"),
        "switch to feature for real",
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"fx\n",
        "the switch materializes feature's own real content"
    );
    std::fs::write(repo.join("b.txt"), b"unrelated\n").unwrap();
    let clean = commit_onto(
        &repo,
        "heads/feature",
        "unrelated edit, now genuinely on feature",
    );
    ok(&clean, "commit after a real switch");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("edit-text a.txt"),
        "no stray revert of a.txt: {clean_stdout}"
    );
    assert!(
        clean_stdout.contains("create-file b.txt"),
        "only the real, unrelated change is authored: {clean_stdout}"
    );

    ok(&support::seal(&repo, "heads/feature"), "seal feature");
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"fx\n",
        "after seal, feature's a.txt still reads fx -- the divergence was not reverted"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Case 3 (the second gap this round fixes, then corrected again by 0.56.0 step W2d-2):
/// `rollback-draft --append-inverse` moves only the active WAL's own queued fold, never the sealed
/// block and never the worktree. W2c fixed detection (the comparison reads `Behind`, not
/// `Current`, for a fold-only move) and, at the time, also added a self-switch materializer that
/// *applied* the fold directly via the lifecycle cache -- bypassing RFC 136's verified-anchor
/// write gate every other overwrite of the worktree in this file goes through. **W2d-2 removed
/// that materializer**: a self-switch whose active WAL is non-empty and whose comparison reads
/// `Behind` now refuses instead, naming the way out (`seal`, then `branch switch` again, which
/// then resyncs through the ordinary, verified-anchor path once the queue is empty). This test is
/// W2c's own case3, re-pointed at the corrected expectation in place, rather than duplicated
/// beside a new one for the same scenario -- the fixture (the `rollback-draft --append-inverse`
/// setup) is identical; only what happens after the bare self-switch, and the new `seal` step
/// before the one that actually applies the deletion, changed.
#[test]
fn case3_rollback_draft_append_inverse_then_self_switch_applies_the_deletion() {
    let repo = support::unique_repo("w2c-case3-rollback-draft-fold-resync");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a1\n", "genesis a1");

    let draft = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["rollback-draft", "--append-inverse", "-m", "undo a1"])
        .output()
        .unwrap();
    ok(&draft, "rollback-draft --append-inverse");
    assert!(
        stdout(&draft).contains("delete-file a.txt"),
        "the inverse of the one create is a delete: {}",
        stdout(&draft)
    );
    // The draft never touches the worktree -- confirmed, not assumed.
    assert!(
        repo.join("a.txt").exists(),
        "rollback-draft only queues a patch; the worktree is untouched"
    );

    // Detection (kept from W2c, unaffected by W2d-2): the worktree is now behind its own ref's
    // fold, even though the sealed block never moved.
    let status = worktree_status(&repo);
    assert!(
        stdout(&status).contains("behind heads/main"),
        "worktree-status must report the moved fold as behind, not current: {}",
        stdout(&status)
    );

    // 0.56.0 step W2d-2: a bare self-switch no longer materializes the fold -- it refuses, naming
    // `seal` then `branch switch` again as the way out, because applying the fold here would mean
    // writing the sealed portion of the manifest through the lifecycle cache rather than RFC 136's
    // verified-anchor gate.
    let refused = switch(&repo, "heads/main");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a self-switch over a non-empty, behind-reading queue must refuse, not materialize: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("queued fold")
            && stderr(&refused).contains("seal")
            && stderr(&refused).contains("branch switch heads/main"),
        "the refusal must name seal, then branch switch, as the way out: {}",
        stderr(&refused)
    );
    assert!(
        repo.join("a.txt").exists(),
        "the refused self-switch must not have written anything: a.txt is still there"
    );

    // Drain the queue for real: after `seal`, the active WAL is empty, so this round's own new
    // refusal (self-switch + non-empty queue + `Behind`) no longer applies.
    ok(
        &support::seal(&repo, "heads/main"),
        "seal --allow-no-audit --ref heads/main (drains the queued inverse)",
    );

    // 0.56.0 step W2e closes the dead end 0.56.0-step-3w2d-report.md's own §2c found (W2d-2's own
    // "found, not fixed" residual gap): `seal` no longer advances the worktree-baseline record
    // unconditionally. Here, right before this `seal` drained the queue, the record (still naming
    // the pre-rollback-draft genesis block, queued_fold empty) disagreed with the fold about to be
    // sealed (the same genesis block, but with the inverse patch folded on top) -- a `Behind`
    // comparison, exactly what `worktree-status` already reported above. Because it disagreed,
    // `seal` left the record untouched rather than claiming the worktree now matched a tip it was
    // never actually written to match. So, immediately after `seal`, the record still names the
    // genesis block -- genuinely behind the ref's new, post-seal tip (where `a.txt` is deleted) --
    // and the self-switch below's own pre-existing no-op early-return (block-identity only) no
    // longer misreads this as `Current`: it correctly falls through to the ordinary,
    // verified-anchor materialization path (`replay_for_verified_worktree_write`), which deletes
    // `a.txt` for real. This is the ruling's own originally-described outcome ("seal, then branch
    // switch again -- succeeds, and the file the inverse deleted is now actually gone from the
    // worktree"), which W2d-2's report had to document as not completing; it completes now.
    let after_seal_switch = switch(&repo, "heads/main");
    ok(
        &after_seal_switch,
        "branch switch heads/main, after seal, resyncs for real",
    );
    assert!(
        !stdout(&after_seal_switch).contains("already"),
        "this is a real resync, not the block-identity no-op: {}",
        stdout(&after_seal_switch)
    );
    assert!(
        !repo.join("a.txt").exists(),
        "the inverse's own deletion is now actually applied to the worktree"
    );

    // `worktree-status` independently confirms clean: the record now matches heads/main's live
    // (post-seal) tip, which really is what the worktree holds.
    let status_after = worktree_status(&repo);
    ok(&status_after, "worktree-status is clean after the resync");
    assert!(
        stdout(&status_after).contains("clean against baseline"),
        "the worktree genuinely matches the baseline now, not merely by a stale record's say-so: {}",
        stdout(&status_after)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
