//! DC-75 two-edits handoff §6, the inventory: each merge refusal the ruling moved names its class,
//! asserted against the compiled binary. The third site (a ref advanced during evidence gathering)
//! cannot be raced from the command line and is held at store level
//! (`merge::execute::tests::an_into_ref_advanced_during_evidence_is_a_lock_conflict`).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "support/mod.rs"]
mod support;

use std::path::Path;
use std::process::Output;

use support::*;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn tip(repo: &Path, ref_name: &str) -> ObjectId {
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

fn merge(repo: &Path, baseline: ObjectId) -> Output {
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            "heads/side",
        ])
        .output()
        .unwrap()
}

fn assert_refused_with(output: &Output, needle: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "must refuse: {stderr}");
    assert!(stderr.contains(needle), "expected {needle:?} in: {stderr}");
}

/// Genesis on `heads/main`, `heads/side` branched there, and one more sealed block on main.
fn repo_with_a_side(tag: &str) -> (std::path::PathBuf, ObjectId) {
    let repo = unique_repo(tag);
    init(&repo);
    generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    let genesis = tip(&repo, "heads/main");
    ok(
        &branch_create(&repo, "heads/side", "heads/main"),
        "branch create",
    );
    generation(&repo, "heads/main", "main.txt", b"main\n", "main moves on");
    (repo, genesis)
}

#[test]
fn a_side_with_nothing_to_adopt_is_a_precondition() {
    let (repo, genesis) = repo_with_a_side("merge-class-nothing-to-adopt");
    let before = tip(&repo, "heads/main");

    assert_refused_with(
        &merge(&repo, genesis),
        "precondition not met: heads/side has no patches to adopt since baseline",
    );
    assert_eq!(tip(&repo, "heads/main"), before, "refusal writes nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_baseline_that_is_not_an_ancestor_of_a_side_is_a_precondition() {
    let (repo, _) = repo_with_a_side("merge-class-not-an-ancestor");
    let main_only = tip(&repo, "heads/main");
    ok(
        &prikk(&repo)
            .args(["branch", "switch", "heads/side"])
            .output()
            .unwrap(),
        "switch side",
    );
    generation(&repo, "heads/side", "side.txt", b"side\n", "side work");
    ok(
        &prikk(&repo)
            .args(["branch", "switch", "heads/main"])
            .output()
            .unwrap(),
        "switch main",
    );

    assert_refused_with(
        &merge(&repo, main_only),
        &format!(
            "precondition not met: baseline Block {main_only} is not an ancestor of target Block"
        ),
    );
    let _ = std::fs::remove_dir_all(&repo);
}
