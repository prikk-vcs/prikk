//! 0.56.0 step W2b: the worktree-baseline guard generalized from deletions alone to every stale
//! path (`.git-exclude/reviewed/0.56.0-step-3w2-review-v1.md`, "W2b. The guard must cover every
//! stale path, not only a missing one").
//!
//! W2 (`step3w2_worktree_baseline.rs`) only caught a path **absent** from the worktree whose
//! absence this worktree's own record never explains. This round generalizes the same mechanism
//! (`node_authoring.rs::path_is_stale`) to every per-path authoring site -- a path is **stale**
//! when its state in the live baseline (what `canonical_ref` currently, really resolves to)
//! disagrees with its state in this worktree's own recorded one, in content, mode, kind, presence,
//! or the node it belongs to -- and refuses whenever a stale path would otherwise have something
//! authored for it (the worktree does not already hold the live state), rather than silently
//! authoring an edit, a mode change, a create, or a rename that would revert a producer's change
//! or record a conflict as an ordinary one.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::Path;
use std::process::Output;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_state_id = RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
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

/// Write a brand-new, previously untracked file and commit the whole worktree -- the "innocent,
/// unrelated commit" shape every one of W2's own hazard cases (and this round's) uses to trigger
/// the guard: the new file itself is never stale (neither side names it), so a refusal that names
/// it instead of the real stale path would itself be a bug.
fn commit_unrelated(repo: &Path, path: &str, message: &str) -> Output {
    std::fs::write(repo.join(path), b"unrelated\n").unwrap();
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--from-worktree", "-m", message])
        .output()
        .unwrap()
}

fn merge_into_main(repo: &Path, baseline: ObjectId, from_ref: &str) -> Output {
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            from_ref,
        ])
        .output()
        .unwrap()
}

/// `heads/main` with two sealed files (`a.txt`, `b.txt`), a sealed sibling branch created from the
/// same tip, and a real `branch switch` round trip there and back -- not a bare `branch create`
/// plus a worktree write left lying around (see `step3w2_worktree_baseline.rs`'s own
/// `setup_main_and_sealed_sibling` doc for why that distinction matters: a real round trip is what
/// makes the worktree's record genuinely describe `heads/main`, so a later producer-side change can
/// be reproduced as a *staleness*, not merely as a file the worktree happens to already lack).
/// Returns `heads/main`'s own genesis block id (for `merge --baseline-block`).
fn setup_main_and_sibling(repo: &Path, sibling_name: &str) -> ObjectId {
    support::generation(repo, "heads/main", "a.txt", b"a1\n", "add a.txt");
    support::generation(repo, "heads/main", "b.txt", b"b1\n", "add b.txt");
    let baseline = tip_block_id(repo, "heads/main");
    ok(
        &support::branch_create(repo, sibling_name, "heads/main"),
        "branch create sibling",
    );
    baseline
}

/// Test 1 (the ruling's own measured case): a merge changes `a.txt`'s **content** on `heads/main`;
/// the worktree still physically holds the pre-merge bytes (the merge never touches it, exactly as
/// W2's own deletion cases prove for a missing path). Before this round, the next unrelated commit
/// silently authored `edit-text a.txt` that reverted the merge's own change back to the old bytes.
/// Now it refuses, naming `a.txt`; `branch switch heads/main` resyncs; the following commit is
/// clean.
#[test]
fn case1_merge_changes_content_edit_would_revert_it_refuses_then_resyncs() {
    let repo = support::unique_repo("w2b-case1-edit-reverted");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    support::generation(
        &repo,
        "heads/feature",
        "a.txt",
        b"a2-from-feature\n",
        "feature changes a.txt",
    );
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a1\n",
        "heads/main is unaffected by feature's own change so far"
    );

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a1\n",
        "the merge must not touch the worktree -- a.txt stays at its old bytes on disk"
    );

    // The hazard: before this round, this commit silently authored `edit-text a.txt`, reverting
    // the merge's own change.
    let refused = commit_unrelated(&repo, "extra1.txt", "unrelated edit after merge");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must now refuse: {}",
        stdout(&refused)
    );
    let refused_stderr = stderr(&refused);
    assert!(
        refused_stderr.contains("refusing to commit") && refused_stderr.contains("a.txt"),
        "the refusal must name a.txt: {refused_stderr}"
    );
    assert!(
        !refused_stderr.contains("never were"),
        "a.txt is present in the worktree, with stale content -- not the deletion-shaped \
         wording: {refused_stderr}"
    );

    // The resync.
    let resync = switch(&repo, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a2-from-feature\n",
        "the resync must materialize the merge's own real content: {}",
        stdout(&resync)
    );

    std::fs::remove_file(repo.join("extra1.txt")).unwrap();
    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must be clean after the resync: {}",
        stdout(&status)
    );

    let clean = commit_unrelated(&repo, "extra1.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("edit-text a.txt"),
        "no stray revert of the merge's own change: {clean_stdout}"
    );
    assert!(
        clean_stdout.contains("create-file extra1.txt"),
        "the only authored operation is the real create: {clean_stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 2: the mirror image of W2's own deletion case. There, the worktree was **missing** a file
/// the live tip had **added**. Here, a merge **removes** `a.txt` from `heads/main`'s live tip, but
/// the worktree still physically holds it (the merge never touches it). Before this round, the
/// next unrelated commit treated the still-present file as brand new and silently authored
/// `create-file a.txt` -- re-materializing stale content under a fresh node id, the "producer's
/// deletion" shape the ruling names by reasoning. Now it refuses; the resync removes the file from
/// disk; the following commit is clean.
#[test]
fn case2_merge_deletes_path_worktree_still_holds_it_refuses_then_resyncs() {
    let repo = support::unique_repo("w2b-case2-merged-deletion");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "feature deletes a.txt"),
        "commit deletion on feature",
    );
    ok(&support::seal(&repo, "heads/feature"), "seal deletion");
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    assert!(
        repo.join("a.txt").exists(),
        "heads/main is unaffected by feature's own deletion so far"
    );

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    assert!(
        repo.join("a.txt").exists(),
        "the merge must not touch the worktree -- a.txt stays present on disk"
    );

    // The hazard: before this round, this commit silently authored `create-file a.txt`.
    let refused = commit_unrelated(&repo, "extra2.txt", "unrelated edit after merge");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must now refuse: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit") && stderr(&refused).contains("a.txt"),
        "the refusal must name a.txt: {}",
        stderr(&refused)
    );

    let resync = switch(&repo, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    assert!(
        !repo.join("a.txt").exists(),
        "the resync must remove a.txt, matching the merge's own real deletion: {}",
        stdout(&resync)
    );

    std::fs::remove_file(repo.join("extra2.txt")).unwrap();
    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must be clean after the resync: {}",
        stdout(&status)
    );

    let clean = commit_unrelated(&repo, "extra2.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("create-file a.txt"),
        "no stray re-creation of the merge's own deletion: {clean_stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 3: a merge changes `a.txt`'s **mode** (not its content) on `heads/main`; the worktree still
/// physically holds the old mode. Before this round, the next unrelated commit silently authored
/// `change-perm a.txt`, reverting the merge's own mode change back to the old mode. Now it refuses;
/// the resync materializes the real mode; the following commit is clean.
///
/// DC-71: Windows has no Unix permission-bit model.
#[cfg(unix)]
#[test]
fn case3_merge_changes_mode_change_perm_would_revert_it_refuses_then_resyncs() {
    use std::os::unix::fs::PermissionsExt;

    let repo = support::unique_repo("w2b-case3-mode-reverted");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    std::fs::set_permissions(
        repo.join("a.txt"),
        std::fs::Permissions::from_mode(0o100_755),
    )
    .unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "feature makes a.txt executable"),
        "commit mode change on feature",
    );
    ok(&support::seal(&repo, "heads/feature"), "seal mode change");
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    let mode_on_main = std::fs::symlink_metadata(repo.join("a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode_on_main & 0o777,
        0o644,
        "heads/main is unaffected by feature's own mode change so far"
    );

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    let mode_still_old = std::fs::symlink_metadata(repo.join("a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode_still_old & 0o777,
        0o644,
        "the merge must not touch the worktree -- a.txt keeps its old mode on disk"
    );

    // The hazard: before this round, this commit silently authored `change-perm a.txt`, reverting
    // the merge's own mode change.
    let refused = commit_unrelated(&repo, "extra3.txt", "unrelated edit after merge");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must now refuse: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit") && stderr(&refused).contains("a.txt"),
        "the refusal must name a.txt: {}",
        stderr(&refused)
    );

    let resync = switch(&repo, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    let mode_after_resync = std::fs::symlink_metadata(repo.join("a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode_after_resync & 0o777,
        0o755,
        "the resync must materialize the merge's own real mode: {}",
        stdout(&resync)
    );

    std::fs::remove_file(repo.join("extra3.txt")).unwrap();
    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must be clean after the resync: {}",
        stdout(&status)
    );

    let clean = commit_unrelated(&repo, "extra3.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("change-perm"),
        "no stray revert of the merge's own mode change: {clean_stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 4: the user edits the same stale file themselves -- the worktree holds **neither** the
/// record's old state **nor** the live state, but a third, user-authored one. Refused as a
/// conflict (naming `a.txt`), not silently authored as an edit against the wrong baseline.
#[test]
fn case4_user_edit_of_stale_path_refused_as_conflict() {
    let repo = support::unique_repo("w2b-case4-conflict");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    support::generation(
        &repo,
        "heads/feature",
        "a.txt",
        b"a2-from-feature\n",
        "feature changes a.txt",
    );
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );

    // The user, not knowing the worktree is already stale, edits a.txt themselves -- a third
    // state, matching neither the record's old bytes ("a1\n") nor the merge's real new ones
    // ("a2-from-feature\n").
    std::fs::write(repo.join("a.txt"), b"a3-user-edit\n").unwrap();

    let refused = commit_unrelated(&repo, "extra4.txt", "unrelated edit after merge");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse the conflict, not author it as an ordinary edit: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit") && stderr(&refused).contains("a.txt"),
        "the refusal must name a.txt: {}",
        stderr(&refused)
    );

    // `branch switch` (the ordinary resync) also refuses here, for the same reason `commit` just
    // did -- a real conflict (a third state matching neither side) is not something a resync may
    // silently discard. This is the expected, doubly-guarded shape: the worktree's own dirty check
    // (unchanged by W2/W2b) still applies on top of the staleness guard.
    let switch_refused = switch(&repo, "heads/main");
    assert_eq!(
        switch_refused.status.code(),
        Some(1),
        "branch switch must also refuse the conflict, not silently discard it: {}",
        stdout(&switch_refused)
    );

    // Discard the user's own conflicting edit by hand, restoring exactly what this worktree's own
    // record says it was last written from ("a1\n") -- the ordinary way out of a dirty-worktree
    // refusal, which `require_clean` here judges against the *recorded* replay, not the live one,
    // because the record is the one genuinely stale against `heads/main` (W2's own rule, §5 of its
    // report). Only then does the resync land on the real, current tip.
    std::fs::write(repo.join("a.txt"), b"a1\n").unwrap();
    let resync = switch(&repo, "heads/main");
    ok(
        &resync,
        "branch switch heads/main (resync, after discarding the conflict)",
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a2-from-feature\n",
        "the resync lands on the merge's own real content: {}",
        stdout(&resync)
    );

    std::fs::remove_file(repo.join("extra4.txt")).unwrap();
    let clean = commit_unrelated(&repo, "extra4.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 5: a declared local rename (`prikk mv`) whose **source** node is itself stale -- a merge
/// changed `a.txt`'s content on `heads/main` (the worktree still holds the old bytes, exactly as
/// test 1), and the user also renames `a.txt` to `renamed.txt` in this same commit, carrying the
/// stale bytes along under the new name. Before a per-path check on the rename's own source path,
/// this would have authored `rename-path a.txt -> renamed.txt` *and* an `edit-text` reverting the
/// merge's change -- a revert disguised as a rename, not merely exempted from the deletion loop by
/// `renamed_away.contains(old_path)`. Refused, naming `a.txt` (the rename's own source), not
/// `renamed.txt`.
#[test]
fn case5_rename_of_a_stale_node_refuses() {
    let repo = support::unique_repo("w2b-case5-stale-rename");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    support::generation(
        &repo,
        "heads/feature",
        "a.txt",
        b"a2-from-feature\n",
        "feature changes a.txt",
    );
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a1\n",
        "the merge must not touch the worktree -- a.txt stays at its old, stale bytes on disk"
    );

    ok(
        &support::prikk(&repo)
            .args(["mv", "a.txt", "renamed.txt"])
            .output()
            .unwrap(),
        "mv a.txt renamed.txt",
    );
    assert!(!repo.join("a.txt").exists());
    assert_eq!(
        std::fs::read(repo.join("renamed.txt")).unwrap(),
        b"a1\n",
        "mv carries the stale bytes along under the new name"
    );

    let refused = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--from-worktree", "-m", "rename the stale file"])
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse: the rename's own source node is stale: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit") && stderr(&refused).contains("a.txt"),
        "the refusal names the rename's own source path, a.txt: {}",
        stderr(&refused)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
