//! Retained active-state comparison used by mutation guards.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectType, RefStatePayload};

use super::{RefPublication, RefStore};
use crate::commit_boundary::active::{ActiveRefMetadata, read_active_ref_metadata};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::object_store::{FileObjectStore, ObjectReader};
use crate::trust::{load_maintainer_trust_policy, verify_trusted_publication_envelope};
use crate::wal::Wal;

/// `exclude_ref_name`: RFC 165 R3 -- a publication retrying its own interrupted work is exactly the
/// shape this function's own fixture describes (a settled Block whose WAL has not drained yet), so a
/// publication excludes its own ref the same way [`super::ensure_no_incomplete_publication_except`]
/// excludes it from the pointer/log agreement check -- otherwise a `seal` retry would refuse behind
/// the very state its own retry exists to resolve.
pub(super) fn has_incomplete_active_cleanup(
    layout: &RepositoryLayout,
    exclude_ref_name: Option<&str>,
) -> Result<bool> {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
    let ActiveRefMetadata::Valid(ref_name) = read_active_ref_metadata(layout)? else {
        return Ok(false);
    };
    // RFC 102 Stage 2: a damaged record silently missing from `replay.records` could make the
    // `patch_ids` comparison below pass or fail for the wrong reason -- fail closed instead of
    // reasoning from a reduced view of the WAL. Checked after the metadata check above (matching
    // this function's own established shape: only report on an issue once metadata already
    // implicates this WAL), before the emptiness check below. **Never excluded**: a damaged WAL
    // refuses regardless of which ref's own retry is in progress -- only the ordinary "settled but
    // not drained" shape below is this ref's own business to resolve.
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(format!(
            "active WAL has a damaged record ({}); run doctor for diagnosis before mutating this repository (a damaged record with \
             sound ones behind it is not a torn tail, and no automatic repair applies to it)",
            replay.damage_summary().unwrap_or_default()
        )));
    }
    if Some(ref_name.as_str()) == exclude_ref_name {
        return Ok(false);
    }
    if replay.records.is_empty() {
        return Ok(false);
    }
    let store = RefStore::new(layout.clone());
    let Some(state_id) = store.read_current_ref_state_id(&ref_name)? else {
        return Ok(false);
    };
    let objects = FileObjectStore::new(layout.clone());
    let state = objects
        .read_typed(state_id, ObjectType::RefState)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing RefState object: {state_id}")))?;
    let target = RefStatePayload::decode_canonical(&state.canonical_payload, state.schema_version)?
        .target_object_id;
    let block = objects
        .read_typed(target, ObjectType::Block)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Block object: {target}")))?;
    let payload = BlockPayload::decode_canonical(&block.canonical_payload)?;
    Ok(payload.patch_ids
        == replay
            .records
            .iter()
            .map(|record| record.envelope.object_id())
            .collect::<Vec<_>>())
}

pub(super) fn validate_signer_backed_recovery(
    layout: &RepositoryLayout,
    publication: &RefPublication,
) -> Result<()> {
    match read_active_ref_metadata(layout)? {
        ActiveRefMetadata::Valid(ref_name) if ref_name == publication.ref_name => {}
        _ => {
            return Err(PrikkError::Integrity(
                "signer-backed ref recovery requires matching retained active-ref metadata"
                    .to_string(),
            ));
        }
    }
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
    if replay.records.is_empty() || replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(
            "signer-backed ref recovery requires a complete non-empty active WAL".to_string(),
        ));
    }
    // RFC 102 Stage 2: `wal_patch_ids` below is built from `replay.records` alone -- a damaged
    // record silently missing from it could make the `patch_ids` comparison pass or fail for the
    // wrong reason.
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "active WAL has a damaged record; run doctor before signer-backed ref recovery"
                .to_string(),
        ));
    }
    let state = RefStatePayload::decode_canonical(
        &publication.ref_state.canonical_payload,
        publication.ref_state.schema_version,
    )?;
    let objects = FileObjectStore::new(layout.clone());
    let block = objects
        .read_typed(state.target_object_id, ObjectType::Block)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!("missing Block object: {}", state.target_object_id))
        })?;
    let payload = BlockPayload::decode_canonical(&block.canonical_payload)?;
    let wal_patch_ids = replay
        .records
        .iter()
        .map(|record| record.envelope.object_id())
        .collect::<Vec<_>>();
    if payload.patch_ids != wal_patch_ids {
        return Err(PrikkError::Integrity(
            "retained active WAL does not prove the proposed publication Block".to_string(),
        ));
    }
    let policy = load_maintainer_trust_policy(layout)?;
    for envelope in [&block, &publication.ref_state, &publication.ref_update] {
        verify_trusted_publication_envelope(&policy, envelope)
            .map_err(|issue| PrikkError::InvalidSignature(issue.message))?;
    }
    Ok(())
}
