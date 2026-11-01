//! 0.56.0 step W3b, R3 for `merge --into` the current branch
//! (`.git-exclude/reviewed/0.56.0-step-3w3-review-v1.md`'s ruling): a clean worktree is brought
//! along -- the merged tip's files are materialized into the worktree -- and a dirty one refuses,
//! before any write. Through the compiled binary, reusing
//! `step3w3_r4_queue_ownership.rs`'s own fixture idiom (`setup_main_and_sealed_sibling`, `merge`,
//! `switch`).
//!
//! The crash-between-publish-and-materialize case needs an in-process failpoint and lives in
//! `crates/prikk-store/src/worktree_materialize/tests.rs` instead (exercising the shared
//! `require_clean_and_mark_current_branch_switch`/`materialize_after_publish` primitives directly).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::Path;
use std::process::Output;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
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

fn merge(repo: &Path, baseline: ObjectId, into_ref: &str, from_ref: &str) -> Output {
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
            into_ref,
            "--from",
            from_ref,
        ])
        .output()
        .unwrap()
}

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
        .output()
        .unwrap()
}

/// `heads/main` (one file, sealed) plus `sibling_name` branched from it with its own extra file,
/// sealed, worktree left back on `heads/main` -- the same fixture
/// `step3w3_r4_queue_ownership.rs`'s own `setup_main_and_sealed_sibling` builds (not re-imported:
/// that module is private to its own file). Returns `heads/main`'s own genesis block id, for
/// `merge --baseline-block`.
fn setup_main_and_sealed_sibling(repo: &Path, sibling_name: &str, extra_path: &str) -> ObjectId {
    support::init(repo);
    support::generation(repo, "heads/main", "a.txt", b"a\n", "genesis");
    let baseline = tip_block_id(repo, "heads/main");
    ok(
        &support::branch_create(repo, sibling_name, "heads/main"),
        "branch create sibling",
    );
    ok(&switch(repo, sibling_name), "switch to sibling");
    support::generation(
        repo,
        sibling_name,
        extra_path,
        b"sibling content\n",
        "sibling adds its own file",
    );
    ok(&switch(repo, "heads/main"), "switch back to heads/main");
    baseline
}

/// Test 1 (the ruling's own list): a clean worktree is brought along -- `merge --into heads/main`
/// while `heads/main` is the current branch and the worktree is clean materializes `heads/feature`'s
/// adopted file into the worktree, the record ends current, and an ordinary commit afterward has
/// nothing to do.
#[test]
fn merge_into_current_branch_materializes_a_clean_worktree() {
    let repo = support::unique_repo("w3b-merge-into-clean");
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");

    let merged = merge(&repo, baseline, "heads/main", "heads/feature");
    assert!(
        merged.status.success(),
        "merge --into heads/main must succeed on a clean worktree: stdout={} stderr={}",
        stdout(&merged),
        stderr(&merged)
    );
    assert!(
        stdout(&merged).contains("materialized into the worktree"),
        "the CLI output must say the merge materialized the worktree: {}",
        stdout(&merged)
    );

    // The adopted file is really on disk, not merely recorded as adopted.
    assert_eq!(
        std::fs::read(repo.join("c.txt")).unwrap(),
        b"sibling content\n",
        "the merged file must be materialized into the worktree"
    );
    // The pre-existing file is untouched.
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"a\n");

    // The record is current: `worktree-status` reports clean, and an ordinary commit has nothing
    // queued (RFC 147 §2e's own "clean" meaning).
    let status = support::prikk(&repo)
        .args(["worktree-status"])
        .output()
        .unwrap();
    ok(&status, "worktree-status after the merge");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the merge: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 2 (the ruling's own list): a dirty worktree refuses, before any write -- `heads/main`'s own
/// tip is unmoved and the merged file never appears.
#[test]
fn merge_into_current_branch_refuses_a_dirty_worktree() {
    let repo = support::unique_repo("w3b-merge-into-dirty");
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");

    // Dirty the worktree: modify the tracked `a.txt` without committing it.
    std::fs::write(repo.join("a.txt"), b"a-modified-uncommitted\n").unwrap();
    let before = tip_block_id(&repo, "heads/main");

    let refused = merge(&repo, baseline, "heads/main", "heads/feature");
    assert!(
        !refused.status.success(),
        "merge --into must refuse while the worktree is dirty: stdout={} stderr={}",
        stdout(&refused),
        stderr(&refused)
    );
    let message = stderr(&refused);
    assert!(
        message.contains("not clean") && message.contains("heads/main"),
        "the refusal must say the worktree is not clean against heads/main: {message}"
    );

    // Nothing was written: the ref is unmoved and the merged file never appeared.
    assert_eq!(
        tip_block_id(&repo, "heads/main"),
        before,
        "a refused merge must not move heads/main's tip"
    );
    assert!(
        !repo.join("c.txt").exists(),
        "a refused merge must not materialize any file"
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a-modified-uncommitted\n",
        "the refusal must not have touched the dirty file either"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `merge --into` a ref that is **not** the current branch is entirely unaffected by R3: no
/// materialization, no clean-worktree precondition -- the worktree (left on `heads/main`) is never
/// even read.
#[test]
fn merge_into_a_ref_that_is_not_current_is_unaffected_by_r3() {
    let repo = support::unique_repo("w3b-merge-into-not-current");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    let baseline = tip_block_id(&repo, "heads/main");
    ok(
        &support::branch_create(&repo, "heads/target", "heads/main"),
        "branch create heads/target",
    );
    ok(
        &support::branch_create(&repo, "heads/feature", "heads/main"),
        "branch create heads/feature",
    );
    ok(&switch(&repo, "heads/feature"), "switch to heads/feature");
    support::generation(
        &repo,
        "heads/feature",
        "c.txt",
        b"sibling content\n",
        "feature adds its own file",
    );
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");

    // Dirty the worktree deliberately -- must not matter at all, since `heads/target` is not the
    // current branch (`heads/main` is).
    std::fs::write(repo.join("a.txt"), b"dirty, but irrelevant\n").unwrap();

    let merged = merge(&repo, baseline, "heads/target", "heads/feature");
    assert!(
        merged.status.success(),
        "merging into a ref that is not the current branch must ignore worktree cleanliness: \
         stdout={} stderr={}",
        stdout(&merged),
        stderr(&merged)
    );
    assert!(
        !stdout(&merged).contains("materialized into the worktree"),
        "a merge into a ref that is not current must not claim to materialize anything: {}",
        stdout(&merged)
    );
    assert!(
        !repo.join("c.txt").exists(),
        "heads/target's own adopted file must not appear in heads/main's worktree"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
