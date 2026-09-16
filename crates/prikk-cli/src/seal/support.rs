//! Seal object construction and interrupted-publication helpers.

use prikk_object::{
    BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectType, RefStatePayload, RefUpdatePayload,
};
use prikk_store::{
    ActiveLock, MaintainerSigner, ObjectReader, ObjectWriter, RefPublication, RefStore,
    RepositoryFormat, RepositoryLayout, maintainer_signature,
};

pub(super) fn persist_wal_patches(
    object_store: &mut impl ObjectWriter,
    records: &[prikk_store::WalRecord],
) -> Result<Vec<prikk_object::ObjectId>, String> {
    let mut patch_ids = Vec::with_capacity(records.len());
    for record in records {
        require_patch_record(record)?;
        let id = object_store
            .write_object(&record.envelope)
            .map_err(|err| err.to_string())?;
        patch_ids.push(id);
    }
    Ok(patch_ids)
}

pub(super) fn collect_wal_patch_ids(
    records: &[prikk_store::WalRecord],
) -> Result<Vec<prikk_object::ObjectId>, String> {
    records
        .iter()
        .map(|record| {
            require_patch_record(record)?;
            Ok(record.envelope.object_id())
        })
        .collect()
}

fn require_patch_record(record: &prikk_store::WalRecord) -> Result<(), String> {
    if record.envelope.object_type == ObjectType::Patch {
        return Ok(());
    }
    Err(format!(
        "active WAL record {} is {}, expected patch",
        record.seq, record.envelope.object_type
    ))
}

pub(super) fn current_tip_matches_wal_patches(
    object_store: &impl ObjectReader,
    current: &CurrentRefState,
    wal_patch_ids: &[prikk_object::ObjectId],
) -> Result<bool, String> {
    let envelope = object_store
        .read_typed(current.target_block_id, ObjectType::Block)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            format!(
                "current ref targets missing block {}",
                current.target_block_id
            )
        })?;
    let block = BlockPayload::decode_canonical(&envelope.canonical_payload)
        .map_err(|err| err.to_string())?;
    Ok(block.patch_ids == wal_patch_ids)
}

pub(super) fn current_ref_state(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_store: &RefStore,
    ref_name: &str,
) -> Result<Option<CurrentRefState>, String> {
    let Some(ref_state_id) = ref_store
        .read_current_ref_state_id(ref_name)
        .map_err(|err| err.to_string())?
    else {
        let log = ref_store
            .replay_log(ref_name)
            .map_err(|err| err.to_string())?;
        // RFC 102 Stage 2: a damaged sole record would otherwise read as `log.records.is_empty()`
        // below, misclassifying a ref with corrupted history as one with none at all (genesis).
        if log.has_item_failure() {
            return Err(format!(
                "ref {ref_name} pointer is missing and its log has a damaged record; \
                 run `prikk doctor` before seal"
            ));
        }
        if log.trailing_partial_bytes != 0 {
            return Err(format!(
                "ref {ref_name} pointer is missing and its log has trailing partial bytes; \
                 run `prikk doctor` before seal"
            ));
        }
        if !log.records.is_empty()
            && (matches!(
                layout.format(),
                RepositoryFormat::CurrentV6 | RepositoryFormat::V7
            ) || log.records.len() > 1)
        {
            return Err(format!(
                "ref {ref_name} pointer/log state does not match the expected publication \
                 transition; run `prikk doctor` before seal"
            ));
        }
        return Ok(None);
    };
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            format!("current ref {ref_name} points to missing RefState {ref_state_id}")
        })?;
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
            .map_err(|err| err.to_string())?;
    if payload.ref_name != ref_name {
        return Err(format!(
            "current RefState name mismatch: expected {ref_name}, got {}",
            payload.ref_name
        ));
    }
    if object_store
        .read_typed(payload.target_object_id, ObjectType::Block)
        .map_err(|err| err.to_string())?
        .is_none()
    {
        return Err(format!(
            "current RefState {ref_state_id} targets missing block {}",
            payload.target_object_id
        ));
    }
    Ok(Some(CurrentRefState {
        ref_state_id,
        target_block_id: payload.target_object_id,
        update_seq: payload.update_seq,
        previous_ref_state_id: payload.previous_ref_state_id,
        ref_state_envelope: envelope,
    }))
}

pub(super) fn finish_current_publication(
    ref_store: &RefStore,
    object_store: &mut impl ObjectWriter,
    active_lock: &ActiveLock,
    ref_name: &str,
    current: &CurrentRefState,
    signer: &impl MaintainerSigner,
) -> Result<(), String> {
    let update = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: current.previous_ref_state_id,
        new_ref_state_id: current.ref_state_id,
        new_target_object_id: current.target_block_id,
        update_seq: current.update_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update = signed_envelope(
        ObjectType::RefUpdate,
        1,
        update.to_canonical_bytes().map_err(|err| err.to_string())?,
        signer,
    )?;
    ref_store
        .finish_interrupted_publication_with_object_store(
            object_store,
            active_lock,
            &RefPublication {
                ref_name: ref_name.to_string(),
                expected_previous_ref_state_id: current.previous_ref_state_id,
                ref_state: current.ref_state_envelope.clone(),
                ref_update,
            },
        )
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub(super) fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> Result<ObjectEnvelope, String> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope
        .add_signature(
            maintainer_signature(signer, object_type, object_id).map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    Ok(envelope)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CurrentRefState {
    pub(super) ref_state_id: prikk_object::ObjectId,
    pub(super) target_block_id: prikk_object::ObjectId,
    pub(super) update_seq: u64,
    pub(super) previous_ref_state_id: Option<prikk_object::ObjectId>,
    ref_state_envelope: ObjectEnvelope,
}
