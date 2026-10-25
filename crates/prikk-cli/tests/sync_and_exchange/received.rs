//! `prikk received remove` (0.54.0 step 3 Part 5, B2a), through the compiled binary.
//!
//! The write sequence, the exclusion lock, the case table, the kill sweep, the new
//! removal-then-reimport-then-restore-refusal row, and the end-to-end verify round trip are all
//! pinned directly at the `prikk-store` level
//! (`crates/prikk-store/src/received/remove/tests.rs`), where a fixture's own signing/state-root
//! crafting is reachable; this file covers what only the compiled binary and real argument
//! parsing can show: the CLI's own wiring, `--plan-only`, and its own refusals.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "../support/mod.rs"]
mod support;

use std::path::Path;

/// Writes a signed root Block and a received pointer naming it directly into `repo`'s own object
/// store and received index, via `write_received_pointer_for_test` (`test-support`-gated) -- the
/// identical construction `release_0_52_0_p3b_received_ref_failure_named.rs` already uses: no
/// `bundle`/`archive` round trip needed just to get a pointer the CLI can be pointed at.
fn write_received_fixture(repo: &Path, ref_name: &str) {
    use prikk_object::{
        BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope, ObjectId, ObjectType,
        RefKind, RefStatePayload, Signature, SignatureAlgorithm, SignerRole,
    };
    use prikk_store::{FileObjectStore, ObjectWriter, RepositoryLayout};

    let layout = RepositoryLayout::open(repo.to_path_buf()).expect("open repo layout");
    let mut store = FileObjectStore::new(layout.clone());

    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([0x42; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block = ObjectEnvelope::unsigned(
        ObjectType::Block,
        2,
        block_payload.to_canonical_bytes().unwrap(),
    );
    block
        .add_signature(Signature {
            algorithm: SignatureAlgorithm::Ed25519,
            key_id: "received-remove-cli-fixture-maintainer".to_string(),
            signature_bytes: vec![7; 64],
            created_at: 1,
            signer_role: SignerRole::Maintainer,
        })
        .unwrap();
    let block_id: ObjectId = store.write_object(&block).expect("write block");

    let state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        state_payload.to_canonical_bytes().unwrap(),
    );
    state
        .add_signature(Signature {
            algorithm: SignatureAlgorithm::Ed25519,
            key_id: "received-remove-cli-fixture-maintainer".to_string(),
            signature_bytes: vec![5; 64],
            created_at: 1,
            signer_role: SignerRole::Maintainer,
        })
        .unwrap();
    let ref_state_id: ObjectId = state.object_id();
    store.write_object(&state).expect("write ref state");

    prikk_store::write_received_pointer_for_test(&layout, ref_name, ref_state_id)
        .expect("write received pointer");
}

#[test]
fn removes_a_received_pointer() {
    let repo = support::unique_repo("received-remove-cli-basic");
    support::init(&repo);
    write_received_fixture(&repo, "remotes/heads/main");

    let output = support::prikk(&repo)
        .args(["received", "remove", "remotes/heads/main"])
        .output()
        .unwrap();
    support::ok(&output, "received remove");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("received removed: remotes/heads/main"),
        "{stdout}"
    );
    assert!(stdout.contains("1 -> 0 live record"), "{stdout}");

    let tree = support::prikk(&repo)
        .args(["tree", "--ref", "remotes/heads/main"])
        .output()
        .unwrap();
    assert!(!tree.status.success(), "the pointer must be gone");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn plan_only_reports_without_removing() {
    let repo = support::unique_repo("received-remove-cli-plan-only");
    support::init(&repo);
    write_received_fixture(&repo, "remotes/heads/main");

    let output = support::prikk(&repo)
        .args(["received", "remove", "remotes/heads/main", "--plan-only"])
        .output()
        .unwrap();
    support::ok(&output, "received remove --plan-only");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("received would remove: remotes/heads/main"),
        "{stdout}"
    );

    let after = support::prikk(&repo)
        .args(["received", "remove", "remotes/heads/main", "--plan-only"])
        .output()
        .unwrap();
    support::ok(
        &after,
        "received remove --plan-only must still see the pointer",
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn refuses_a_non_remotes_name() {
    let repo = support::unique_repo("received-remove-cli-non-remotes");
    support::init(&repo);

    let output = support::prikk(&repo)
        .args(["received", "remove", "heads/main"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not a received ref"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn refuses_when_nothing_is_named() {
    let repo = support::unique_repo("received-remove-cli-no-pointer");
    support::init(&repo);

    let output = support::prikk(&repo)
        .args(["received", "remove", "remotes/heads/nothing"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("nothing to remove"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn refuses_with_no_subcommand_or_an_unknown_one() {
    let repo = support::unique_repo("received-remove-cli-no-subcommand");
    support::init(&repo);

    let bare = support::prikk(&repo).args(["received"]).output().unwrap();
    assert!(!bare.status.success());
    assert!(
        String::from_utf8_lossy(&bare.stderr).contains("sub-command"),
        "stderr: {}",
        String::from_utf8_lossy(&bare.stderr)
    );

    let unknown = support::prikk(&repo)
        .args(["received", "frobnicate"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());

    let _ = std::fs::remove_dir_all(&repo);
}
