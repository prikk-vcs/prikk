//! RFC 164 round 2 Rule E: a stored object's own dangling reference is damage when the object
//! making it is reachable from committed state, an unreferenced remnant otherwise. One row per kind
//! of reacher named in RFC 164 §5 (a ref, a received pointer, a queued patch, a sealed block reached
//! from them), against the same defective shape each time -- a `Root` Block naming one missing
//! `patch_ids` entry -- plus the unreachable baseline (`verify/tests.rs`'s own `verify_repository_
//! detects_every_missing_referenced_object_as_a_remnant`) and a remnant-made-reachable transition.
//!
//! **"Reached by a queued patch", precisely.** A `Patch` payload carries no block reference at all
//! (`parent_patch_ids` retired at schema 2, RFC 114) -- its only outbound references are to blobs
//! (`patch_referenced_blob_ids`). So a block cannot be reached *through* a queued patch's own
//! payload; what this row tests instead is the realistic case a queued patch actually protects: the
//! active ref's own current tip, with a non-empty WAL open against it, which is already reachable
//! via the ref itself (the "branch" row below). This is a known scope question, not a silent
//! assumption -- flagged for the architect's ruling in the round's own report, not resolved here.

use prikk_error::Result;
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, CreateFile, MerkleRoot, NodeId, ObjectEnvelope,
    ObjectId, ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, RefKind,
    RefStatePayload, RefUpdatePayload,
};

use super::{assert_object_item_failed, assert_unreferenced_remnant};
use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::{rollback_author_signature, unique_temp_dir};
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, Wal, add_trusted_maintainer, maintainer_signature,
    verify_repository, write_active_ref_metadata,
};

fn trusted_signer() -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed("rfc164-reachability-verify", &[0x95; 32])
}

fn adopt(layout: &RepositoryLayout, signer: &Ed25519MaintainerSigner) -> Result<()> {
    let public_key_hex: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    add_trusted_maintainer(layout, signer.key_id(), &public_key_hex)?;
    Ok(())
}

/// A `Root` Block naming one missing `patch_ids` entry -- the defective shape every row below
/// shares. `distinguishing_root` keeps two otherwise-identical fixtures from colliding on the same
/// content-addressed id.
fn write_block_missing_a_patch(
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
    missing_patch_id: ObjectId,
    distinguishing_root: u8,
) -> Result<ObjectId> {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: vec![missing_patch_id],
        state_merkle_root: MerkleRoot([distinguishing_root; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, ObjectType::Block, id)?)?;
    objects.write_object(&envelope)
}

fn publish_branch_at(
    layout: &RepositoryLayout,
    signer: &Ed25519MaintainerSigner,
    ref_name: &str,
    target_object_id: ObjectId,
) -> Result<()> {
    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
    );
    let ref_state_id = ref_state.object_id();
    ref_state.add_signature(maintainer_signature(
        signer,
        ObjectType::RefState,
        ref_state_id,
    )?)?;

    let ref_update_payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: target_object_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut ref_update = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
    );
    let ref_update_id = ref_update.object_id();
    ref_update.add_signature(maintainer_signature(
        signer,
        ObjectType::RefUpdate,
        ref_update_id,
    )?)?;

    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

fn write_received_pointer_at(
    layout: &RepositoryLayout,
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
    ref_name: &str,
    target_object_id: ObjectId,
) -> Result<()> {
    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
    );
    let ref_state_id = ref_state.object_id();
    ref_state.add_signature(maintainer_signature(
        signer,
        ObjectType::RefState,
        ref_state_id,
    )?)?;
    objects.write_object(&ref_state)?;
    crate::received::write_received_pointer(layout, ref_name, ref_state_id)?;
    Ok(())
}

/// Queues one real, well-formed patch (referencing an arbitrary, never-dereferenced blob id --
/// irrelevant to this row, which is about the *block* the active ref targets, not the patch's own
/// blob) against `ref_name`, via the real `Wal::append_patch` path.
fn queue_one_patch_against(layout: &RepositoryLayout, ref_name: &str) -> Result<()> {
    write_active_ref_metadata(layout, ref_name)?;
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: NodeId::from_bytes([0x71; 32]),
                blob_id: crate::test_gates::test_support::sample_object_id("rfc164-reach-blob"),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(rollback_author_signature())?;
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).append_patch(&envelope)?;
    Ok(())
}

/// Row 1: reached by a branch.
#[test]
fn a_block_missing_its_patch_reached_by_a_branch_is_damage() -> Result<()> {
    let root = unique_temp_dir("rfc164-reach-branch");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = trusted_signer()?;
    adopt(&layout, &signer)?;

    let missing = crate::test_gates::test_support::sample_object_id("rfc164-reach-branch-patch");
    let block_id = write_block_missing_a_patch(&mut objects, &signer, missing, 0xD0)?;
    publish_branch_at(&layout, &signer, "heads/main", block_id)?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(&report, "references missing block patch");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 2: reached by a received pointer.
#[test]
fn a_block_missing_its_patch_reached_by_a_received_pointer_is_damage() -> Result<()> {
    let root = unique_temp_dir("rfc164-reach-received");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = trusted_signer()?;
    adopt(&layout, &signer)?;

    let missing = crate::test_gates::test_support::sample_object_id("rfc164-reach-received-patch");
    let block_id = write_block_missing_a_patch(&mut objects, &signer, missing, 0xD1)?;
    write_received_pointer_at(
        &layout,
        &mut objects,
        &signer,
        "remotes/heads/main",
        block_id,
    )?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(&report, "references missing block patch");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 3: reached by a queued patch -- the active ref's own tip, with a non-empty WAL open against
/// it (see this file's own module doc for why a block cannot be reached any other way through a
/// queued patch's own payload).
#[test]
fn a_block_missing_its_patch_reached_by_a_queued_patch_is_damage() -> Result<()> {
    let root = unique_temp_dir("rfc164-reach-queued");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = trusted_signer()?;
    adopt(&layout, &signer)?;

    let missing = crate::test_gates::test_support::sample_object_id("rfc164-reach-queued-patch");
    let block_id = write_block_missing_a_patch(&mut objects, &signer, missing, 0xD2)?;
    publish_branch_at(&layout, &signer, "heads/main", block_id)?;
    queue_one_patch_against(&layout, "heads/main")?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(&report, "references missing block patch");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 4: reached transitively, as a sealed block's parent -- RFC 164 §5's "a sealed block reached
/// from them". The defective block is never itself published; a second, healthy block names it as
/// `parent_block_ids`, and that second block is what a branch points at.
#[test]
fn a_block_missing_its_patch_reached_as_a_sealed_blocks_parent_is_damage() -> Result<()> {
    let root = unique_temp_dir("rfc164-reach-parent");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = trusted_signer()?;
    adopt(&layout, &signer)?;

    let missing = crate::test_gates::test_support::sample_object_id("rfc164-reach-parent-patch");
    let defective_block_id = write_block_missing_a_patch(&mut objects, &signer, missing, 0xD3)?;

    // A healthy, replay-correct child over the defective block -- `Normal`, no patches of its own
    // (an empty patch set replays trivially), so only the parent-reachability edge is under test.
    let child_payload = BlockPayload {
        parent_block_ids: vec![defective_block_id],
        kind: BlockKind::Normal,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([0xD4; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: Some(defective_block_id),
        merge_baseline_block_id: None,
    };
    let mut child_envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, child_payload.to_canonical_bytes()?);
    let child_id = child_envelope.object_id();
    child_envelope.add_signature(maintainer_signature(&signer, ObjectType::Block, child_id)?)?;
    let child_id = objects.write_object(&child_envelope)?;

    publish_branch_at(&layout, &signer, "heads/main", child_id)?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(&report, "references missing block patch");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Security requirement (RFC 164 §5 item 3): a remnant that later becomes reachable is damage on
/// the very next run -- reachability is never cached across runs, only ever recomputed from
/// committed state.
#[test]
fn a_remnant_made_reachable_afterwards_becomes_damage() -> Result<()> {
    let root = unique_temp_dir("rfc164-reach-transition");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = trusted_signer()?;
    adopt(&layout, &signer)?;

    let missing =
        crate::test_gates::test_support::sample_object_id("rfc164-reach-transition-patch");
    let block_id = write_block_missing_a_patch(&mut objects, &signer, missing, 0xD5)?;

    let before = verify_repository(&layout)?;
    assert!(
        !before.has_item_failure(),
        "unpublished, unreachable block must not fail verify yet: {before:?}"
    );
    assert_unreferenced_remnant(&before, block_id, "block patch");

    publish_branch_at(&layout, &signer, "heads/main", block_id)?;

    let after = verify_repository(&layout)?;
    assert_object_item_failed(&after, "references missing block patch");
    assert!(
        !after
            .unreferenced_remnants
            .iter()
            .any(|remnant| remnant.owner_object_id == block_id),
        "the same owner must not still be reported as a remnant once it is reachable: {after:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
