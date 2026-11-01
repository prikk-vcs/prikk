//! 0.56.0 step W3c-1 (`.git-exclude/reviewed/0.56.0-step-3w3b-review-v1.md`'s own "W3c-1" ruling): a
//! refused `merge --into` the current branch must write nothing at all -- not even an orphaned,
//! unreferenced Block. Measured on the build of `c9659ece` (W3b's own final commit): a dirty-worktree
//! merge refused correctly (no ref move), but `verify` afterward reported `unreached objects: 1`,
//! naming the merge's own stray Block -- because W3b's own clean check ran *after* `seal_block_at`
//! had already built and durably written the Block/RefState/RefUpdate objects. This round moves the
//! clean check (and the capture of the old baseline) to immediately after the R4 queue-ownership
//! check, before any object write at all, and moves the dirty-marker set to immediately before the
//! publish -- so a refusal between those two points now leaves nothing written.
//!
//! This test snapshots the **entire** `.prikk` directory (not merely "no new file in the worktree"),
//! so a stray Block written to the object store but never referenced by any ref would be caught even
//! though it is invisible to every other assertion the W3b round's own test made.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::ObjectId;
use prikk_store::RepositoryLayout;

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

fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    use prikk_object::{ObjectType, RefStatePayload};
    use prikk_store::{FileObjectStore, ObjectReader, RefStore};
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

/// Every file in the repository, worktree **and** `.prikk` both (except `.prikk/cache`, which is
/// pure memoization -- derived caches like `lifecycle-state.v1`/`commit-index.v1` are rebuilt
/// opportunistically on ordinary reads, including the evidence-gathering a refused merge performs
/// before it refuses, and are not part of any durability or "nothing written" claim; confirmed by
/// running this exact test once without the exclusion, which failed only on cache-directory bytes,
/// never on `.prikk/containers`, `.prikk/refs` or `.prikk/active`), keyed by its path relative to
/// `repo`. A byte-identical comparison of this map before and after a refusal is a stronger claim
/// than "no new worktree file appeared": it also catches an orphaned object the refusal's own write
/// path left behind inside the object store, which nothing else in this test file would see.
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

fn verify_text(repo: &Path) -> String {
    let output = support::prikk(repo).arg("verify").output().unwrap();
    format!("{}{}", stdout(&output), stderr(&output))
}

/// `heads/main` (one file, sealed) plus `sibling_name` branched from it with its own extra file,
/// sealed, worktree left back on `heads/main` -- the identical fixture shape
/// `step3w3b_merge_into_current_branch.rs`'s own `setup_main_and_sealed_sibling` builds (not
/// re-imported: that module is private to its own file). Returns `heads/main`'s own genesis block id.
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

/// The W3c-1 test (the ruling's own words): a dirty-worktree merge leaves `.prikk` byte-identical
/// (confirmed by a whole-repository checksum/directory comparison, not merely "no new file
/// appeared"), and `verify` reports `unreached_objects: 0` afterward -- there is no stray Block.
#[test]
fn a_refused_merge_into_current_branch_writes_nothing_at_all() {
    let repo = support::unique_repo("w3c1-merge-refusal-writes-nothing");
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );

    // Dirty the worktree: modify the tracked `a.txt` without committing it.
    std::fs::write(repo.join("a.txt"), b"a-modified-uncommitted\n").unwrap();
    let before = whole_repo(&repo);
    let before_tip = tip_block_id(&repo, "heads/main");

    let refused = merge(&repo, baseline, "heads/main", "heads/feature");
    assert!(
        !refused.status.success(),
        "merge --into must refuse while the worktree is dirty: stdout={} stderr={}",
        stdout(&refused),
        stderr(&refused)
    );
    assert!(
        stderr(&refused).contains("not clean") && stderr(&refused).contains("heads/main"),
        "{}",
        stderr(&refused)
    );

    assert_eq!(
        tip_block_id(&repo, "heads/main"),
        before_tip,
        "a refused merge must not move heads/main's tip"
    );
    assert_eq!(
        whole_repo(&repo),
        before,
        "a refused merge must leave every file -- worktree AND .prikk -- byte-identical; a stray \
         object written to the object store before the refusal would show up here even though no \
         ref ever named it"
    );
    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "a refused merge must not leave an orphaned, unreferenced Block behind: {}",
        verify_text(&repo)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
