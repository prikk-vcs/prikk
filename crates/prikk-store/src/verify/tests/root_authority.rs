//! Full verification must derive recovery authority from one retained repository root.

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload,
};

#[cfg(target_os = "windows")]
use crate::ObjectReader;
use crate::test_gates::test_support::{
    rollback_patch_envelope, signed_patch_blob_envelope, signed_patch_envelope, unique_temp_dir,
};
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectWriter,
    RefPublication, RefStore, RepositoryLayout, Wal, add_trusted_maintainer,
    derive_next_state_root, maintainer_signature, verify_repository, write_active_ref_metadata,
};

#[test]
fn full_verification_retains_wal_objects_trust_and_recovery_diagnosis_after_root_replacement()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("verify-retained-authority");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = Ed25519MaintainerSigner::from_seed("retained-maintainer", &[0x42; 32])?;
    let public_key = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    add_trusted_maintainer(&layout, signer.key_id(), &public_key)?;

    let patch = signed_patch_envelope();
    let patch_id = patch.object_id();
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    objects.write_object(&patch)?;
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: vec![patch_id],
        state_merkle_root: derive_next_state_root(&objects, None, &[patch_id])?,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block = signed_publication(
        ObjectType::Block,
        block_payload.to_canonical_bytes()?,
        &signer,
    )?;
    let block_id = objects.write_object(&block)?;
    let state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state = signed_publication(
        ObjectType::RefState,
        state_payload.to_canonical_bytes()?,
        &signer,
    )?;
    let state_id = ref_state.object_id();
    let update_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: block_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update = signed_publication(
        ObjectType::RefUpdate,
        update_payload.to_canonical_bytes()?,
        &signer,
    )?;
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: ref_state.clone(),
        ref_update,
    })?;
    write_active_ref_metadata(&layout, "heads/main")?;
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&patch)?;
    crate::refs::remove_pointer_entries_for_test(
        &layout,
        crate::foundation::layout::ref_name_key_bytes("heads/main"),
    )?;

    assert_retained_missing_pointer(&layout, patch_id)?;

    std::fs::rename(layout.prikk_dir(), root.join(".prikk-retained"))?;
    let replacement = RepositoryLayout::init(root.clone())?;
    write_active_ref_metadata(&replacement, "heads/main")?;
    Wal::for_layout(&replacement, DEFAULT_ACTIVE_NAME).append_patch(&rollback_patch_envelope())?;
    let mut replacement_objects = FileObjectStore::new(replacement.clone());
    replacement_objects.write_object(&patch)?;
    replacement_objects.write_object(&corrupt_signature_transport(block)?)?;
    replacement_objects.write_object(&corrupt_signature_transport(ref_state)?)?;

    assert_retained_missing_pointer(&layout, patch_id)?;
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-96 Windows Anchor Identity, design-v1.md §6.2: the `repository_mutation` negative control,
/// distinct from `snapshot::tests::worktree_checks_and_writes_remain_on_retained_root`'s worktree
/// one -- this scenario swaps `.prikk` itself, not the worktree root, so it exercises
/// `repository_mutation` (objects/refs/WAL, the durability-bearing anchor), not `worktree_mutation`.
///
/// **Asserts success, not refusal** -- corrected per
/// `.git-exclude/reviewed/DC-96-implementation-ruling-v1.md` §2-§4: the mechanism is prevention
/// (a retained handle follows the renamed directory), not detection, so a durability-bearing write
/// issued through the stale `layout` must continue to succeed, landing in the *retained*
/// `.prikk-retained` tree -- exactly what
/// `full_verification_retains_wal_objects_trust_and_recovery_diagnosis_after_root_replacement`
/// above already demonstrates for reads through the same retained authority. An earlier version of
/// this test asserted the opposite (refusal), matching the design's first, since-corrected
/// "detection only" shape; ruled wrong because it could never have made the acceptance tests above
/// pass while also passing this one -- the two would have required opposite outcomes for the same
/// class of scenario. Windows-only: on Linux/macOS this scenario has no interesting behavior to
/// pin beyond what the read test above already covers, via the same retained-fd mechanism.
#[cfg(target_os = "windows")]
#[test]
fn durability_bearing_write_succeeds_against_the_retained_repository_root_after_replacement()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("verify-durability-write-root-replacement");
    let layout = RepositoryLayout::init(root.clone())?;

    std::fs::rename(layout.prikk_dir(), root.join(".prikk-retained"))?;
    // The impostor: a fresh, empty .prikk at the same path the stale `layout` still points at.
    let _replacement = RepositoryLayout::init(root.clone())?;

    let impostor_bytes_before = total_bytes_recursively(&root.join(".prikk"));
    let retained_bytes_before = total_bytes_recursively(&root.join(".prikk-retained"));

    let mut objects = FileObjectStore::new(layout.clone());
    let write_result = objects.write_object(&signed_patch_blob_envelope());
    let read_back = write_result
        .as_ref()
        .ok()
        .and_then(|&id| objects.read_object(id).ok().flatten());

    let impostor_bytes_after = total_bytes_recursively(&root.join(".prikk"));
    let retained_bytes_after = total_bytes_recursively(&root.join(".prikk-retained"));

    let _ = std::fs::remove_dir_all(&root);

    assert!(
        write_result.is_ok(),
        "a durability-bearing write through a retained layout must succeed against the retained \
         directory after the anchor is renamed aside, matching the Linux/macOS guarantee: \
         {write_result:?}"
    );
    assert_eq!(
        impostor_bytes_before, impostor_bytes_after,
        "the impostor .prikk must not grow -- the write must not be redirected into it"
    );
    assert!(
        retained_bytes_after > retained_bytes_before,
        "the retained .prikk-retained tree must grow -- this is where the write must land"
    );
    assert_eq!(
        read_back.map(|envelope| envelope.object_id()),
        write_result.ok(),
        "the written object must be readable back through the same retained layout"
    );
    Ok(())
}

#[cfg(target_os = "windows")]
fn total_bytes_recursively(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0_u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += total_bytes_recursively(&path);
        } else if let Ok(metadata) = entry.metadata() {
            total += metadata.len();
        }
    }
    total
}

fn assert_retained_missing_pointer(
    layout: &RepositoryLayout,
    patch_id: prikk_object::ObjectId,
) -> prikk_error::Result<()> {
    let report = verify_repository(layout)?;
    assert!(report.publication_trust_issues.is_empty());
    assert!(report.ref_publication_issues.iter().any(|issue| {
        issue.code == "PRIKK-VERIFY-REF-DIVERGENCE"
            && issue.ref_name.as_deref() == Some("heads/main")
    }));
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
    assert_eq!(replay.records.len(), 1);
    assert_eq!(
        replay
            .records
            .first()
            .map(|record| record.envelope.object_id()),
        Some(patch_id)
    );
    Ok(())
}

fn signed_publication(
    object_type: ObjectType,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> prikk_error::Result<ObjectEnvelope> {
    let schema_version = if object_type == ObjectType::Block {
        2
    } else {
        1
    };
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    envelope.add_signature(maintainer_signature(
        signer,
        object_type,
        envelope.object_id(),
    )?)?;
    Ok(envelope)
}

fn corrupt_signature_transport(
    mut envelope: ObjectEnvelope,
) -> prikk_error::Result<ObjectEnvelope> {
    let byte = envelope
        .signatures
        .first_mut()
        .and_then(|signature| signature.signature_bytes.first_mut())
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("expected publication signature bytes".to_string())
        })?;
    *byte ^= 0xff;
    Ok(envelope)
}
