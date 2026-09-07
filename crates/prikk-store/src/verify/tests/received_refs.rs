//! RFC 115 Stage 3 §6: the received-namespace verification gap, closed. Before this stage existed,
//! `verify_repository` never scanned `remotes/*` at all -- a received ref whose target object was
//! never shipped dangled invisibly. These tests prove the new `ReceivedRefs` stage catches it, and
//! that a genuinely healthy received ref still passes.

use prikk_error::Result;
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind,
    RefStatePayload,
};

use super::assert_ref_failed;
use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RepositoryLayout,
    add_trusted_maintainer, derive_next_state_root, maintainer_signature, verify_repository,
};

fn trusted_signer() -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed("received-refs-verify", &[0x91; 32])
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

fn write_trusted_block(
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: derive_next_state_root(objects, None, &[])?,
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

fn write_received_ref_state(
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
    ref_name: &str,
    target_object_id: ObjectId,
) -> Result<ObjectId> {
    let payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, ObjectType::RefState, id)?)?;
    objects.write_object(&envelope)
}

/// A healthy received ref -- its RefState targets a Block genuinely present -- must not be reported
/// as a failure by the new stage.
#[test]
fn verify_repository_accepts_a_healthy_received_ref() -> Result<()> {
    let root = unique_temp_dir("verify-received-refs-healthy");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = trusted_signer()?;
    adopt(&layout, &signer)?;

    let block_id = write_trusted_block(&mut objects, &signer)?;
    let ref_state_id =
        write_received_ref_state(&mut objects, &signer, "remotes/heads/main", block_id)?;
    crate::received::write_received_pointer(&layout, "remotes/heads/main", ref_state_id)?;

    let report = verify_repository(&layout)?;
    assert!(
        !report.has_item_failure(),
        "a healthy received ref must not fail: {report:?}"
    );
    assert!(
        report
            .received_ref_item_outcomes
            .iter()
            .any(|outcome| outcome.ref_name == "remotes/heads/main"
                && matches!(outcome.status, crate::refs::RefItemStatus::Evaluated)),
        "expected an Evaluated outcome for remotes/heads/main: {:?}",
        report.received_ref_item_outcomes
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The property this stage exists for: a received ref whose target Block was never shipped now
/// reports an item failure -- invisible before this stage existed (RFC 115 Stage 3 §6, following
/// from the DC-78 bundle-tag-gap review's own finding).
#[test]
fn verify_repository_detects_a_dangling_received_ref() -> Result<()> {
    let root = unique_temp_dir("verify-received-refs-dangling");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = trusted_signer()?;
    adopt(&layout, &signer)?;

    // A RefState naming a Block id that was never written -- the sender's own shipped closure was
    // incomplete, or the artifact never carried it. Either way, nothing under this id exists.
    let missing_block_id = ObjectId::from_bytes([0x92; 32]);
    let ref_state_id = write_received_ref_state(
        &mut objects,
        &signer,
        "remotes/heads/main",
        missing_block_id,
    )?;
    crate::received::write_received_pointer(&layout, "remotes/heads/main", ref_state_id)?;

    let report = verify_repository(&layout)?;
    assert_ref_failed(&report, "targets missing block");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
