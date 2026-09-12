//! Merge execution (DC-74/DC-75): seals the other side's patches verbatim onto the target ref when
//! the two sides are proven confluent from a common baseline.
//!
//! **A merge authors nothing** — the adopted patches are the exact objects already sealed on the
//! source ref, same canonical bytes, same `ObjectId`, same author signature; this module never
//! decodes, re-derives, or re-signs a patch. Only the new `Block`, `RefState`, and `RefUpdate` are
//! signed here, with the maintainer key, exactly as an ordinary `seal` signs them.
//!
//! Two-parent `BlockKind::Merge` blocks (DC-75): `parent_block_ids` names both `into_ref`'s prior
//! tip (recorded again as `mainline_parent_id`) and `from_ref`'s adopted tip, sorted per the format's
//! uniqueness invariant. `merge_baseline_block_id` records the baseline confluence was proven
//! against — a claim `verify` independently re-derives and cross-checks rather than trusts. State
//! derivation and replay follow the mainline parent only; the secondary parent's own chain is
//! verified independently by the ordinary full-object-store scan. DC-74 sealed merges as
//! indistinguishable `BlockKind::Normal` blocks; this is what discharges its release condition.

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind,
    RefStatePayload, RefUpdatePayload,
};

use crate::merge::evidence::{
    MergeEvidenceTarget, candidate_patch_ids, prepare_merge_evidence,
    verify_candidate_blocks_trusted,
};
use crate::received::validate_received_ref;
use crate::{
    GatedOperation, MaintainerSigner, ObjectReader, ObjectWriteSession, ObjectWriter,
    RefPublication, RefStore, RepositoryLayout, derive_next_state_root, maintainer_signature,
    validate_local_branch_ref, verify_signer_trusted,
};

/// Result of a completed merge execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeExecutionReport {
    /// Ref advanced by the merge (the "into" side).
    pub into_ref: String,
    /// Ref merged in (the "from" side).
    pub from_ref: String,
    /// Baseline block confluence was proven against.
    pub baseline_block_id: ObjectId,
    /// `into_ref`'s target block immediately before the merge (the new block's mainline parent).
    pub parent_block_id: ObjectId,
    /// `from_ref`'s target block the adopted patches were collected up to.
    pub adopted_target_block_id: ObjectId,
    /// Patch IDs adopted verbatim, in sealed order.
    pub adopted_patch_ids: Vec<ObjectId>,
    /// New block ID.
    pub block_id: ObjectId,
    /// New RefState ID.
    pub ref_state_id: ObjectId,
}

/// Execute a merge: seal `from_ref`'s patches since `baseline_block_id` verbatim onto `into_ref`.
///
/// Refuses cleanly with no object, WAL, or ref write of any kind unless the two sides are proven
/// confluent from the given baseline — conflict detection is `patch_algebra`'s existing evidence
/// machinery (the same analysis `merge-evidence`/`merge-plan` already report), reused rather than
/// duplicated.
pub fn execute_merge(
    layout: &RepositoryLayout,
    baseline_block_id: ObjectId,
    into_ref: &str,
    from_ref: &str,
    signer: &impl MaintainerSigner,
) -> Result<MergeExecutionReport> {
    layout.require_current_format()?;
    let into_ref = validate_local_branch_ref(into_ref)?;
    // DC-85: `from_ref` may be a local branch or a received ref (`remotes/<name>`) — never widen
    // `validate_local_branch_ref` itself to accept `remotes/`, since it also gates `into_ref` here
    // and `branch create --from` elsewhere; a merge source that happens to be received gets its own
    // resolution path instead (§3A.3). `into_ref` is never eligible: `RefStore::publish` only ever
    // writes `refs/by-id/`, so the side being advanced must remain a genuine local branch.
    let from_is_received = from_ref.starts_with("remotes/");
    // Carry the local arm's own canonical string forward for the comparison below, rather than
    // re-deriving `from_ref` from the raw argument a second time — today `validate_local_branch_ref`
    // happens to return its input unchanged, so the two forms agree, but that is not guaranteed to
    // stay true (`refs.rs`'s own comment on `validate_local_tag_ref` already records NFR-SEC-03's
    // case-collision rule as unmet and tracked for later); comparing a validated name against an
    // unvalidated one would silently stop catching `heads/Main` vs `heads/main` the day it lands.
    let (from_target, from_ref) = if from_is_received {
        validate_received_ref(from_ref)?;
        (
            MergeEvidenceTarget::ReceivedRef(from_ref.to_string()),
            from_ref.to_string(),
        )
    } else {
        let canonical = validate_local_branch_ref(from_ref)?;
        (MergeEvidenceTarget::Ref(canonical.clone()), canonical)
    };
    if into_ref == from_ref {
        return Err(PrikkError::InvalidName(
            "merge into_ref and from_ref must differ".to_string(),
        ));
    }
    // Read-only evidence gathering, requiring no signing credential at all — confluence is
    // determinable exactly like `merge-plan`'s, before any question of who may seal is asked.
    let evidence = prepare_merge_evidence(
        layout,
        baseline_block_id,
        MergeEvidenceTarget::Ref(into_ref.clone()),
        from_target,
    )?;
    if !evidence.is_confluent() {
        return Err(PrikkError::Integrity(format!(
            "merge refused: {from_ref} is not confluent with {into_ref} from baseline \
             {baseline_block_id} (outcome: {}{})",
            evidence.outcome,
            evidence
                .reason
                .map(|reason| format!(", reason: {reason}"))
                .unwrap_or_default(),
        )));
    }

    // Only proceeding to seal needs a trusted signer.
    let policy = verify_signer_trusted(layout, signer, GatedOperation::Merge)?;

    let mut object_store = ObjectWriteSession::open(layout)?;

    // DC-85 §3A.1's mandatory criterion: a received ref's blocks never passed a trust check on the
    // way in (`import_bundle` performs none, deliberately). Checked here, before any write, reusing
    // the signer's already-loaded policy rather than a second load.
    if from_is_received {
        verify_candidate_blocks_trusted(
            &object_store,
            &policy,
            baseline_block_id,
            evidence.right_selector.target_block_id,
        )?;
    }

    let adopted_patch_ids = candidate_patch_ids(
        &object_store,
        baseline_block_id,
        evidence.right_selector.target_block_id,
    )?;
    if adopted_patch_ids.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "{from_ref} has no patches to adopt since baseline {baseline_block_id}"
        )));
    }

    let ref_store = RefStore::new(layout.clone());
    let into_ref_state_id = ref_store
        .read_current_ref_state_id(&into_ref)?
        .ok_or_else(|| PrikkError::Integrity(format!("ref {into_ref} is not published")))?;
    let into_ref_state_envelope = object_store
        .read_typed(into_ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!("ref {into_ref} points to missing RefState"))
        })?;
    let into_ref_state = RefStatePayload::decode_canonical(
        &into_ref_state_envelope.canonical_payload,
        into_ref_state_envelope.schema_version,
    )?;
    let parent_block_id = into_ref_state.target_object_id;
    if parent_block_id != evidence.left_selector.target_block_id {
        return Err(PrikkError::Integrity(format!(
            "ref {into_ref} advanced during merge evidence gathering; retry"
        )));
    }

    let state_merkle_root =
        derive_next_state_root(&object_store, Some(parent_block_id), &adopted_patch_ids)?;
    let adopted_target_block_id = evidence.right_selector.target_block_id;
    let mut parent_block_ids = vec![parent_block_id, adopted_target_block_id];
    parent_block_ids.sort();
    let block_payload = BlockPayload {
        parent_block_ids,
        kind: BlockKind::Merge,
        patch_ids: adopted_patch_ids.clone(),
        state_merkle_root,
        snapshot_blob_ref: None,
        mainline_parent_id: Some(parent_block_id),
        merge_baseline_block_id: Some(baseline_block_id),
    };
    let block_envelope = signed_envelope(
        ObjectType::Block,
        2,
        block_payload.to_canonical_bytes()?,
        signer,
    )?;
    let block_id = object_store.write_object(&block_envelope)?;

    let update_seq = into_ref_state.update_seq.checked_add(1).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "merge into {into_ref}: ref-state update_seq overflow"
        ))
    })?;
    let ref_state_payload = RefStatePayload {
        ref_name: into_ref.clone(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq,
        previous_ref_state_id: Some(into_ref_state_id),
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
        signer,
    )?;
    let ref_state_id = ref_state_envelope.object_id();

    let ref_update_payload = RefUpdatePayload {
        ref_name: into_ref.clone(),
        old_ref_state_id: Some(into_ref_state_id),
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
        signer,
    )?;

    let publication = RefPublication {
        ref_name: into_ref.clone(),
        expected_previous_ref_state_id: Some(into_ref_state_id),
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    let published_ref_state_id =
        ref_store.publish_with_object_store(&mut object_store, &publication)?;

    Ok(MergeExecutionReport {
        into_ref,
        from_ref,
        baseline_block_id,
        parent_block_id,
        adopted_target_block_id,
        adopted_patch_ids,
        block_id,
        ref_state_id: published_ref_state_id,
    })
}

fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> Result<ObjectEnvelope> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, object_type, object_id)?)?;
    Ok(envelope)
}

#[cfg(test)]
mod tests;
