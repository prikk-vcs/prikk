//! Shared fixtures for `sync_negotiation`'s own test modules (`summary::tests`, `have_list::tests`,
//! `tests`). Builds Blocks and RefStates directly (`test_support::signed_block`,
//! `test_support::signed_ref_state_envelope`) rather than through `simulate_one_seal` or any WAL
//! path -- patch ids here are bare `ObjectId` literals, never real Patch objects, matching
//! `patch_set_digest/tests.rs`'s own precedent: `patch_ids_reachable_from_block` only reads Block
//! payloads' own `patch_ids` fields, never dereferences a Patch object. No `RecognitionClaimPayload`
//! is constructed anywhere in this file (the parent module's own §6 constraint).

use prikk_error::Result;
use prikk_object::{BlockKind, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, TagPayload};

use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::ContainerSlot;
use crate::test_gates::test_support::{
    maintainer_signature, signed_block, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};

pub(super) fn fresh_repo(name: &str) -> Result<RepositoryLayout> {
    RepositoryLayout::init(unique_temp_dir(name))
}

/// Publish a `heads/*` branch ref at a fresh Block carrying `patch_ids` verbatim, parented on
/// whatever `heads/*` already names (root block if none). Returns the new Block's own id.
pub(super) fn publish_branch(
    layout: &RepositoryLayout,
    ref_name: &str,
    patch_ids: Vec<ObjectId>,
) -> Result<ObjectId> {
    let ref_store = RefStore::new(layout.clone());
    let mut store = FileObjectStore::new(layout.clone());
    let current = ref_store.read_current_ref_state_id(ref_name)?;
    let (parent_block_ids, previous_ref_state_id, update_seq) = match &current {
        Some(ref_state_id) => {
            let envelope = crate::object_store::ObjectReader::read_typed(
                &store,
                *ref_state_id,
                ObjectType::RefState,
            )?
            .ok_or_else(|| prikk_error::PrikkError::Integrity("missing RefState".to_string()))?;
            let payload = prikk_object::RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            (
                vec![payload.target_object_id],
                Some(*ref_state_id),
                payload.update_seq + 1,
            )
        }
        None => (Vec::new(), None, 1),
    };
    let kind = if parent_block_ids.is_empty() {
        BlockKind::Root
    } else {
        BlockKind::Normal
    };
    let block = signed_block(kind, parent_block_ids, patch_ids, None);
    let block_id = store.write_object(&block)?;

    let ref_state =
        signed_ref_state_envelope(ref_name, previous_ref_state_id, block_id, update_seq);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(
        ref_name,
        previous_ref_state_id,
        ref_state_id,
        block_id,
        update_seq,
    );
    ref_store.publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: previous_ref_state_id,
        ref_state,
        ref_update,
    })?;
    Ok(block_id)
}

/// Publish a `tags/*` ref pointing (through a Tag object) at `target_block_id` -- the same two-hop
/// shape `patch_set_digest/tests.rs`'s own tag fixture uses.
pub(super) fn publish_tag(
    layout: &RepositoryLayout,
    ref_name: &str,
    tag_name: &str,
    target_block_id: ObjectId,
) -> Result<()> {
    let mut store = FileObjectStore::new(layout.clone());
    let (patch_set_digest, patch_count) =
        crate::compute_patch_set_digest_and_count_from_block(&store, target_block_id)?;
    let tag_payload = TagPayload {
        name: tag_name.to_string(),
        target_block_id,
        message: None,
        created_at: 0,
        author_key_id: "maintainer-key".to_string(),
        patch_set_digest,
        patch_count,
    };
    let mut tag_envelope =
        ObjectEnvelope::unsigned(ObjectType::Tag, 1, tag_payload.to_canonical_bytes()?);
    tag_envelope.add_signature(maintainer_signature())?;
    let tag_id = store.write_object(&tag_envelope)?;

    let payload = prikk_object::RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: prikk_object::RefKind::Tag,
        target_object_id: tag_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, payload.to_canonical_bytes()?);
    ref_state.add_signature(maintainer_signature())?;
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(ref_name, None, ref_state_id, tag_id, 1);

    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// Record a `remotes/*` received pointer at a fresh RefState naming `target_block_id` -- the same
/// received-index path `bundle.rs`'s own import uses, not the ordinary `RefStore::publish`.
pub(super) fn publish_received(
    layout: &RepositoryLayout,
    ref_name: &str,
    target_block_id: ObjectId,
) -> Result<()> {
    let mut store = FileObjectStore::new(layout.clone());
    let payload = prikk_object::RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: prikk_object::RefKind::Branch,
        target_object_id: target_block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, payload.to_canonical_bytes()?);
    ref_state.add_signature(maintainer_signature())?;
    let ref_state_id = store.write_object(&ref_state)?;
    crate::received::write_received_pointer(layout, ref_name, ref_state_id)
}

pub(super) fn cleanup(layout: &RepositoryLayout) {
    let _ = std::fs::remove_dir_all(layout.root());
}

/// The raw bytes of `object_type`'s primary container slot -- a byte-for-byte proof nothing was
/// written, not merely that some higher-level view looks unchanged (`seal_from_accepted/tests.rs`'s
/// own `block_container_bytes` precedent, generalised over `ObjectType`).
pub(super) fn container_bytes(
    layout: &RepositoryLayout,
    object_type: ObjectType,
) -> Result<Vec<u8>> {
    let relative =
        layout.repository_relative(&layout.container_slot_path(object_type, ContainerSlot::A))?;
    Ok(read_file_if_exists(layout.repository_mutation_root(), &relative)?.unwrap_or_default())
}

/// Every persisted object type's own container bytes, in `persisted_object_types()`'s own fixed
/// order -- row 1's "changes no state" must hold for every kind of object this repository can ever
/// write, not only the ones a healthy read path happens to touch (`crate::foundation::layout::persisted_object_types`).
pub(super) fn all_container_bytes(layout: &RepositoryLayout) -> Result<Vec<Vec<u8>>> {
    crate::foundation::layout::persisted_object_types()
        .into_iter()
        .map(|object_type| container_bytes(layout, object_type))
        .collect()
}
