//! 0.56.0 step W3c-3b (`.git-exclude/reviewed/0.56.0-step-3w3c-3-review-v1.md`'s own "W3c-3b"
//! ruling, item 2): a file in the way of `merge --into` the current branch's own candidate state
//! must refuse *before* the merge publishes anything -- the same "before any write" guarantee
//! `branch create --from` already has (`step3w3c3_in_the_way_before_publish.rs`, 0.56.0 step
//! W3c-3). Before this round, `merge --into` only had the pre-existing, post-move backstop
//! (`materialize_after_publish`'s own `in_the_way` check): the merge Block, RefState, and RefUpdate
//! were already durably written and the ref already moved, by the time that backstop ever ran.
//!
//! `execute_merge` now derives its own merge Block's candidate state (`derive_candidate`,
//! `block_state.rs`) -- the identical derivation `seal_block_at` will make again moments later --
//! before `seal_block_at` ever runs, converts the derived `entries` plus `text_cache` into a
//! `ReplayManifest` the way `patch_replay::extract`'s own claim path builds its file list, and runs
//! the identical `require_nothing_in_the_way` planner `branch create --from` uses, against it,
//! before any write.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

fn verify_text(repo: &Path) -> String {
    let output = support::prikk(repo).arg("verify").output().unwrap();
    format!("{}{}", stdout(&output), stderr(&output))
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

/// Every file in the repository, worktree **and** `.prikk` both (except `.prikk/cache`), keyed by
/// its path relative to `repo` -- the identical idiom `step3w3c1_merge_refusal_writes_nothing.rs`'s
/// own `whole_repo` (and `step3w3c3_in_the_way_before_publish.rs`'s own copy of it) already
/// established, reused rather than re-derived. A byte-identical comparison also proves no dirty
/// marker was set, since the marker is itself a file under `.prikk/`.
fn whole_repo(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == "cache")
                && path
                    .parent()
                    .is_some_and(|parent| parent == repo.join(".prikk"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(repo).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

/// `heads/main` (one file, sealed) plus `sibling_name` branched from it with its own extra file,
/// sealed, worktree left back on `heads/main` -- the same fixture shape
/// `step3w3b_merge_into_current_branch.rs`'s and `step3w3c1_merge_refusal_writes_nothing.rs`'s own
/// `setup_main_and_sealed_sibling` build (not re-imported: each is private to its own file).
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

/// P5 (the ruling's own list): an untracked file sits at a path the merge's adopted side brings in;
/// `merge --into` the current branch refuses before any write, with the new sentence, leaving the
/// whole repository byte-identical (also proving no marker was set) and `heads/main` unmoved; after
/// the file in the way is moved aside, the identical command succeeds and materializes.
#[test]
fn merge_into_current_branch_an_untracked_file_in_the_way_refuses_before_publish_then_resyncs() {
    let repo = support::unique_repo("w3c3b-merge-in-the-way");
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");

    // The untracked file already in the way, at the exact path the adopted side (`heads/feature`)
    // brings in -- different content than `heads/feature` will adopt, so the collision is real.
    std::fs::write(repo.join("c.txt"), b"zzz\n").unwrap();

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );
    let before = whole_repo(&repo);
    let before_tip = tip_block_id(&repo, "heads/main");

    let refused = merge(&repo, baseline, "heads/main", "heads/feature");
    assert!(
        !refused.status.success(),
        "merge --into must refuse while a file is in the way of the merge's own candidate state: \
         stdout={} stderr={}",
        stdout(&refused),
        stderr(&refused)
    );
    let message = stderr(&refused);
    assert!(
        message.contains("are in the way of heads/main's files")
            && message.contains("c.txt")
            && message.contains("move them aside before merging heads/feature into heads/main")
            && message.contains("nothing was written"),
        "the new pre-write refusal must name the action, heads/main, c.txt, and say nothing was \
         written: {message}"
    );
    assert!(
        !message.contains("was published") && !message.contains("moved"),
        "the old post-publish backstop's own sentence must not be the one that fires here -- the \
         new pre-write check must refuse first: {message}"
    );

    // The ruling's own list: `heads/main` stays unmoved (1), the whole repository -- worktree AND
    // `.prikk` -- is byte-identical (2, which also proves no marker was set), and `verify` reports
    // no orphaned, unreferenced Block (3).
    assert_eq!(
        tip_block_id(&repo, "heads/main"),
        before_tip,
        "a refused merge must not move heads/main's tip -- this is exactly the defect this round \
         fixes for merge"
    );
    assert_eq!(
        whole_repo(&repo),
        before,
        "a refused merge must leave every file -- worktree AND .prikk -- byte-identical; a stray \
         object written to the object store, a moved pointer, or a set marker would all show up \
         here"
    );
    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "a refused merge must not leave an orphaned, unreferenced object behind: {}",
        verify_text(&repo)
    );
    assert_eq!(
        std::fs::read(repo.join("c.txt")).unwrap(),
        b"zzz\n",
        "the untracked file in the way must never be overwritten by a refused merge"
    );

    // After the offending file is moved aside, the same command succeeds and materializes.
    std::fs::rename(repo.join("c.txt"), repo.join("c.txt.aside")).unwrap();
    let retry = merge(&repo, baseline, "heads/main", "heads/feature");
    assert!(
        retry.status.success(),
        "merge --into must succeed once the file in the way is moved aside: stdout={} stderr={}",
        stdout(&retry),
        stderr(&retry)
    );
    assert!(
        stdout(&retry).contains("materialized into the worktree"),
        "the CLI output must say the merge materialized the worktree: {}",
        stdout(&retry)
    );
    assert_ne!(
        tip_block_id(&repo, "heads/main"),
        before_tip,
        "the retried merge must advance heads/main's tip to a new merge block"
    );
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"a\n");
    assert_eq!(
        std::fs::read(repo.join("c.txt")).unwrap(),
        b"sibling content\n"
    );

    std::fs::remove_file(repo.join("c.txt.aside")).unwrap();
    let status = support::prikk(&repo)
        .args(["worktree-status"])
        .output()
        .unwrap();
    ok(&status, "worktree-status after the retried merge");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the retried merge: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// The DC-65 case the ruling names specifically (ruling point 4): a merge whose adopted side
/// deletes a file edited by `EditText`, so that file's content was never stored as a Blob. The
/// pre-write check's own `derive_candidate` call -- unlike `seal_block_classified`'s internal one,
/// which runs `store_derived_content_for_candidate` (a write) first -- must still succeed here,
/// since adopted patches were already sealed on the source ref (their Blobs already present) and a
/// `DeleteNode`'s own preimage check never reads a Blob at all. This reuses the exact fixture shape
/// `dc78_export_after_deleting_an_edited_file.rs`'s own
/// `a_merge_whose_side_branch_deleted_an_edited_file_exports` already establishes and measures as
/// confluent, with `heads/main` left as the current branch so this round's own pre-write check runs
/// at all.
#[test]
fn merge_into_current_branch_whose_adopted_side_deleted_an_edited_file_succeeds() {
    let repo = support::unique_repo("w3c3b-merge-dc65-edit-then-delete");
    support::init(&repo);
    support::generation(
        &repo,
        "heads/main",
        "notes.txt",
        b"line one\nline two\n",
        "create notes.txt",
    );
    let baseline = tip_block_id(&repo, "heads/main");
    ok(
        &support::branch_create(&repo, "heads/dev", "heads/main"),
        "branch create heads/dev",
    );

    // The adopted side (`heads/dev`) edits notes.txt (`EditText`) and then deletes it: its content
    // is never stored as a Blob (DC-65).
    ok(&switch(&repo, "heads/dev"), "switch to heads/dev");
    support::generation(
        &repo,
        "heads/dev",
        "notes.txt",
        b"line one\nline two, edited\n",
        "edit notes.txt",
    );
    std::fs::remove_file(repo.join("notes.txt")).unwrap();
    ok(
        &support::commit(&repo, "heads/dev", "delete notes.txt"),
        "commit delete notes.txt",
    );
    ok(&support::seal(&repo, "heads/dev"), "seal delete notes.txt");

    // Mainline (the current branch, unaffected by the switch above once we switch back) does its
    // own, unrelated work -- never touching notes.txt itself, so the two sides are confluent.
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    support::generation(&repo, "heads/main", "main.txt", b"main\n", "main-only file");

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the merge"
    );

    let merged = merge(&repo, baseline, "heads/main", "heads/dev");
    assert!(
        merged.status.success(),
        "merge --into must succeed: the pre-write in-the-way check's own derive_candidate call \
         must not fail on an adopted deletion of a file edited by EditText, whose content was \
         never stored as a Blob (DC-65): stdout={} stderr={}",
        stdout(&merged),
        stderr(&merged)
    );
    assert!(
        stdout(&merged).contains("materialized into the worktree"),
        "the CLI output must say the merge materialized the worktree: {}",
        stdout(&merged)
    );
    assert!(
        !repo.join("notes.txt").exists(),
        "the merged tip must not hold notes.txt -- deleted on the adopted side"
    );
    assert_eq!(std::fs::read(repo.join("main.txt")).unwrap(), b"main\n");

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "a successful merge must not leave an orphaned object behind either: {}",
        verify_text(&repo)
    );
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
