//! 0.56.0 step W2c, through the compiled binary.
//!
//! Two independent fixes, each with its own test(s):
//! 1. The ref-name gate (W2b's own `recorded_names_canonical_ref`) is removed: a cross-ref commit
//!    (`commit --ref <other>`) that would revert a diverged *edit* -- not just a deletion, W1's
//!    case 5 -- now refuses regardless of which ref the worktree's record names, and a real
//!    `branch switch` to the ref actually being committed onto is what lets the legitimate commit
//!    through (case 1/case 2 below).
//! 2. The resync (`branch switch <the current branch>`) now detects **and can apply** a moved
//!    *fold*, not only a moved sealed block: `rollback-draft --append-inverse` moves only the
//!    active WAL's own queued fold, never the sealed block, so it was invisible to the old
//!    sealed-block-only comparison and unmaterializable by the old sealed-tip-only resync writer
//!    (case 3 below).

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

/// Case 3 (the second gap this round fixes): `rollback-draft --append-inverse` moves only the
/// active WAL's own queued fold, never the sealed block and never the worktree. Before this round,
/// `compare_worktree_baseline` read `Current` regardless (it compared the sealed block alone), and
/// even a caller that somehow asked for a resync would not have gotten one, because the
/// self-switch materializer only ever wrote the sealed tip's own files. This round fixes both: the
/// comparison (content-identity digest over the queued patch ids) and the materializer (folds the
/// active WAL into what a self-switch writes, when it is owned by the ref being switched to).
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

    // The gap this round closes, part 1 (detection): the worktree is now behind its own ref's
    // fold, even though the sealed block never moved.
    let status = worktree_status(&repo);
    assert!(
        stdout(&status).contains("behind heads/main"),
        "worktree-status must report the moved fold as behind, not current: {}",
        stdout(&status)
    );

    // The gap this round closes, part 2 (materialization): a real self-switch must actually apply
    // the queued inverse's deletion, not just detect staleness.
    let resync = switch(&repo, "heads/main");
    ok(
        &resync,
        "branch switch heads/main (self, applies the queued fold)",
    );
    assert!(
        !repo.join("a.txt").exists(),
        "the resync must apply the inverse's deletion of a.txt: {}",
        stdout(&resync)
    );

    let status_after = worktree_status(&repo);
    assert_eq!(
        status_after.status.code(),
        Some(0),
        "worktree-status must be clean after the resync applied the fold: {}",
        stdout(&status_after)
    );
    assert!(
        stdout(&status_after).contains("worktree: clean against baseline"),
        "no residual staleness after the resync: {}",
        stdout(&status_after)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
