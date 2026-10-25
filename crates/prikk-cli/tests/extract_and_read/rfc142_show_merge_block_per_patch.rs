//! RFC 142, stikk 020 (0.52.0 step 1 Part C): the ruling's own "stop and ask" clause names DC-75
//! `BlockKind::Merge` blocks as a shape to verify, not assume, before trusting that a block's
//! state is always "parent state, then its own patches in canonical order." This test builds a
//! real two-patch Merge block (`prikk merge`, DC-75) through the compiled binary and checks that
//! `show`'s new per-patch resolution agrees for it exactly as it does for an ordinary `Normal`
//! block: the adopted edit patch resolves its own node-addressed path against its own post-edit
//! state (not the merge block's end state, where the node is already deleted by the second
//! adopted patch), and that answer is identical to what `show` already gave that same patch
//! through its own pre-merge sealed block on the source ref.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::Path;

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

fn switch(repo: &Path, target: &str) {
    ok(
        &prikk(repo)
            .args(["branch", "switch", target])
            .output()
            .unwrap(),
        &format!("branch switch {target}"),
    );
}

fn merge(repo: &Path, baseline: ObjectId) -> std::process::Output {
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

/// `prikk show <id>`, prose form, as plain lines -- enough to check a `path:` line's value without
/// a JSON parse.
fn show_lines(repo: &Path, id: ObjectId) -> Vec<String> {
    let output = prikk(repo)
        .args(["show", &id.to_string()])
        .output()
        .unwrap();
    ok(&output, &format!("show {id}"));
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn merge_block_resolves_an_adopted_edit_patch_against_its_own_state_not_the_blocks_end() {
    let repo = unique_repo("rfc142-merge-per-patch");
    init(&repo);
    generation(&repo, "heads/main", "d.txt", b"d.txt 0\n", "baseline");
    let baseline = tip(&repo, "heads/main");

    ok(
        &branch_create(&repo, "heads/side", "heads/main"),
        "branch create",
    );
    switch(&repo, "heads/side");

    std::fs::write(repo.join("d.txt"), b"d.txt edited\n").unwrap();
    ok(&commit(&repo, "heads/side", "edit d.txt"), "commit edit");
    ok(&seal(&repo, "heads/side"), "seal edit");
    let side_edit_block = tip(&repo, "heads/side");

    std::fs::remove_file(repo.join("d.txt")).unwrap();
    ok(
        &commit(&repo, "heads/side", "delete d.txt"),
        "commit delete",
    );
    ok(&seal(&repo, "heads/side"), "seal delete");

    switch(&repo, "heads/main");
    ok(&merge(&repo, baseline), "merge");
    ok(&verify(&repo), "verify after merge");

    let merged_block = tip(&repo, "heads/main");

    // The pre-merge reference: patch 1's own sealed answer through its own one-patch block on
    // `heads/side`, before the merge ever adopted it.
    let pre_merge = show_lines(&repo, side_edit_block);
    let pre_merge_path_line = pre_merge
        .iter()
        .find(|line| line.trim_start().starts_with("path:"))
        .expect("pre-merge show must print a path line");
    assert_eq!(
        pre_merge_path_line.trim(),
        "path: d.txt",
        "precondition: before the merge, the edit patch's own block resolves to d.txt"
    );

    let merged = show_lines(&repo, merged_block);
    assert!(
        merged
            .iter()
            .any(|line| line == "patch 1:" || line.contains("operation 1: edit-text")),
        "expected the merged block's first adopted patch to carry the edit, got:\n{}",
        merged.join("\n")
    );
    let merged_path_line = merged
        .iter()
        .find(|line| line.trim_start().starts_with("path:"))
        .expect("merged block's show must print a path line for the edit operation");
    assert_eq!(
        merged_path_line.trim(),
        "path: d.txt",
        "a DC-75 Merge block's own adopted edit patch must resolve against its own post-edit \
         state (node still live), not the merge block's end state (node deleted by the second \
         adopted patch) -- got:\n{}",
        merged.join("\n")
    );
    assert_eq!(
        pre_merge_path_line.trim(),
        merged_path_line.trim(),
        "the edit patch's own resolved path must be identical whether read through its own \
         pre-merge block or the post-merge Merge block -- same patch, same parent-derived state, \
         regardless of which later block context it ends up embedded in"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
