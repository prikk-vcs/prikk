//! 0.56.0 step W3 (R4), through the compiled binary: a producer never moves a ref that a live
//! queue (the active WAL's own queued, unsealed work) already targets
//! (`.git-exclude/reviewed/0.56.0-step-3w1-design-review-v1.md`, "## R3"/"## R4", and
//! `0.56.0-step-3w1-design-report.md` §2.6).
//!
//! Three of the five producers needed a real code change for R4: `execute_merge`
//! (`crates/prikk-store/src/merge/execute.rs`), `try_adopt_local_ref`
//! (`crates/prikk-store/src/archive/import.rs`), and `run_create`
//! (`crates/prikk-cli/src/branch.rs`, `branch create --from`). The other two,
//! `seal_from_accepted_claim` and `append_rollback_draft`, already refuse on *any* non-empty
//! active WAL regardless of which ref owns it -- a strictly stronger condition that already
//! implies R4 -- confirmed by `crates/prikk-store/src/seal_from_accepted/tests.rs`'s own
//! `row10_a_non_empty_active_wal_refuses` (queue owns the exact ref being sealed) and
//! `crates/prikk-cli/tests/argument_and_error_handling/rfc132_part2_precondition_prefix.rs`'s own
//! rollback-draft case (queue owns `heads/main`, rollback-draft's own current-branch default).
//! Neither needed a new test here.

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

fn is_published(repo: &Path, ref_name: &str) -> bool {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .is_some()
}

/// Queue (commit, never seal) `path` onto `ref_name` -- `None` defaults to the current branch,
/// matching `commit`'s own CLI default.
fn queue_onto(repo: &Path, ref_name: Option<&str>, path: &str, message: &str) -> Output {
    std::fs::write(repo.join(path), b"queued\n").unwrap();
    let mut cmd = support::prikk(repo);
    cmd.env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID).env(
        "PRIKK_AUTHOR_SEED_FILE",
        support::seed_file(support::AUTHOR_SEED_HEX),
    );
    let mut args = vec!["commit", "--from-worktree"];
    if let Some(ref_name) = ref_name {
        args.push("--ref");
        args.push(ref_name);
    }
    args.push("-m");
    args.push(message);
    cmd.args(args).output().unwrap()
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
/// sealed, worktree left back on `heads/main`. Same idiom as `step3w2_worktree_baseline.rs`'s own
/// `setup_main_and_sealed_sibling`, not re-imported (that module is `pub(self)` to its own file).
/// Returns `heads/main`'s own genesis block id, for `merge --baseline-block`.
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

/// §2.6's own case: `merge --into heads/main` while a queued, unsealed patch already targets
/// `heads/main` refuses, naming `seal`, before any write -- `heads/main`'s own tip and the queue
/// are both untouched.
#[test]
fn merge_into_refuses_while_the_queue_owns_the_target_ref() {
    let repo = support::unique_repo("w3-r4-merge-same-ref");
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");
    ok(
        &queue_onto(
            &repo,
            None,
            "pending.txt",
            "queued edit to main, not yet sealed",
        ),
        "queue a patch onto heads/main",
    );
    let before = tip_block_id(&repo, "heads/main");

    let refused = merge(&repo, baseline, "heads/main", "heads/feature");
    assert!(
        !refused.status.success(),
        "merge --into must refuse while the queue owns heads/main: stdout={} stderr={}",
        stdout(&refused),
        stderr(&refused)
    );
    let message = stderr(&refused);
    assert!(
        message.contains("seal") && message.contains("heads/main"),
        "the refusal must name heads/main and the way out (seal): {message}"
    );
    assert_eq!(
        tip_block_id(&repo, "heads/main"),
        before,
        "the merge must not have moved heads/main's tip"
    );

    // The queue itself is untouched: sealing it still works afterward.
    let seal = support::seal(&repo, "heads/main");
    ok(&seal, "seal the still-queued patch after the refused merge");
    assert!(
        repo.join("pending.txt").exists(),
        "the queued patch the refusal protected must still be sealable"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// The selective half of the same predicate: a queue that targets a *different* ref does not
/// block a merge into `heads/main` -- R4 is "owned by the exact ref," not "any non-empty queue."
#[test]
fn merge_into_proceeds_when_the_queue_owns_a_different_ref() {
    let repo = support::unique_repo("w3-r4-merge-other-ref");
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");
    ok(
        &support::branch_create(&repo, "heads/unrelated", "heads/main"),
        "branch create heads/unrelated",
    );
    ok(
        &queue_onto(
            &repo,
            Some("heads/unrelated"),
            "pending.txt",
            "queued edit to a different ref",
        ),
        "queue a patch onto heads/unrelated",
    );

    let merged = merge(&repo, baseline, "heads/main", "heads/feature");
    ok(
        &merged,
        "merge --into heads/main with an unrelated ref queued",
    );
    assert!(
        is_published(&repo, "heads/main"),
        "heads/main must have been published by the merge"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `archive import --adopt` onto the unpublished, current `heads/main`: a queued, unsealed patch
/// already targeting `heads/main` refuses the adopt, naming `seal`, before any ref write -- the
/// archive's own exported tip is never landed.
#[test]
fn archive_import_adopt_refuses_while_the_queue_owns_the_adopted_ref() {
    let src = support::unique_repo("w3-r4-adopt-src");
    support::init(&src);
    support::generation(&src, "heads/main", "n.txt", b"n\n", "genesis");
    let archive_path = src.join("export.pkga");
    ok(
        &support::prikk(&src)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );

    let dest = support::unique_repo("w3-r4-adopt-dest");
    support::init(&dest);
    ok(
        &queue_onto(
            &dest,
            None,
            "pending.txt",
            "queued edit to unborn heads/main",
        ),
        "queue a patch onto the unborn heads/main",
    );

    let import = support::prikk(&dest)
        .args(["archive", "import", "--adopt"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        !import.status.success(),
        "archive import --adopt must refuse while the queue owns heads/main: stdout={} stderr={}",
        stdout(&import),
        stderr(&import)
    );
    let message = stderr(&import);
    assert!(
        message.contains("seal") && message.contains("heads/main"),
        "the refusal must name heads/main and the way out (seal): {message}"
    );
    assert!(
        !is_published(&dest, "heads/main"),
        "heads/main must still be unpublished -- the adopt must not have published a local ref"
    );
    // The refusal fires inside `try_adopt_local_ref`, downstream of this archive's own stage
    // 0-3 object admission (which is unconditional, --adopt or not, and leaves its own resumable
    // import journal behind on any later refusal -- orthogonal to R4, the same as any other
    // mid-pipeline refusal this command can hit). Directory teardown below does not depend on
    // that journal being resolved.

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dest);
}

/// `branch create heads/main --from heads/other` while a queued, unsealed patch already targets
/// the unborn `heads/main` refuses, naming `seal`, before any ref write.
#[test]
fn branch_create_from_refuses_while_the_queue_owns_the_created_ref() {
    let repo = support::unique_repo("w3-r4-create-from");
    support::init(&repo);
    support::generation(
        &repo,
        "heads/other",
        "o.txt",
        b"o\n",
        "genesis on heads/other",
    );
    // `commit --ref heads/other` packages whatever the worktree physically holds, regardless of
    // which ref it targets, and leaves it there afterward -- heads/other's own genesis content
    // physically on disk even though the pointer (heads/main, unborn) never moved. Removing it
    // again restores the worktree to heads/main's own true (empty) state, the same step the
    // design report's own §2.5 fixture takes (`rm o.txt`) before this round's R2 guard existed.
    std::fs::remove_file(repo.join("o.txt")).unwrap();
    ok(
        &queue_onto(
            &repo,
            None,
            "pending.txt",
            "queued edit to unborn heads/main",
        ),
        "queue a patch onto the unborn heads/main",
    );

    let create = support::prikk(&repo)
        .args(["branch", "create", "heads/main", "--from", "heads/other"])
        .output()
        .unwrap();
    assert!(
        !create.status.success(),
        "branch create --from must refuse while the queue owns heads/main: stdout={} stderr={}",
        stdout(&create),
        stderr(&create)
    );
    let message = stderr(&create);
    assert!(
        message.contains("seal") && message.contains("heads/main"),
        "the refusal must name heads/main and the way out (seal): {message}"
    );
    assert!(
        !is_published(&repo, "heads/main"),
        "heads/main must still be unpublished -- the create must not have written anything"
    );

    let seal = support::seal(&repo, "heads/main");
    ok(
        &seal,
        "seal the still-queued patch after the refused create",
    );

    let _ = std::fs::remove_dir_all(&repo);
}
