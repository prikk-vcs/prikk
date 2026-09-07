//! Verification helpers for rollback draft patches.
//!
//! This module keeps rollback publication non-mutating, but makes rollback drafts easier to audit before
//! seal. The active WAL verifier classifies rollback draft records by `PatchPurpose::RollbackDraft`
//! and validates that their Patch payload remains in the supported replay subset. The stronger
//! `verify_active_rollback_draft` API additionally compares the WAL
//! payload with the inverse Patch that would be derived from the currently published ref.

use prikk_crypto::ED25519_SIGNATURE_LEN;
use prikk_error::{PrikkError, Result};
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, PatchPurpose, Signature,
    SignatureAlgorithm, SignerRole,
};

use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::patch_inverse::prepare_patch_inverse_plan;
use crate::patch_replay::decode::{
    decode_patch_message, decode_patch_operations, ensure_apply_supported,
};
use crate::rollback_draft::is_rollback_draft_envelope;
use crate::wal::{Wal, WalRecord};

const LEGACY_ROLLBACK_MARKER_KEY_ID: &str = "dev-placeholder-rollback-author";

/// Verification result for one active rollback draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackDraftVerification {
    /// Ref used to derive the expected inverse Patch.
    pub ref_name: String,
    /// WAL sequence containing the rollback draft Patch envelope.
    pub wal_sequence: u64,
    /// Signed rollback draft Patch ID currently present in the active WAL.
    pub draft_patch_id: ObjectId,
    /// Real AUTHOR key id recorded in the rollback draft signature.
    pub author_key_id: String,
    /// Published block that was used as the rollback target.
    pub target_block_id: ObjectId,
    /// Number of blocks inspected while deriving the expected inverse.
    pub block_count: usize,
    /// Number of patch objects inspected while deriving the expected inverse.
    pub patch_count: usize,
    /// Number of supported inverse operations expected for this rollback draft.
    pub inverse_operation_count: usize,
    /// Number of supported operations decoded from the active WAL payload.
    pub decoded_operation_count: usize,
}

/// Verify that the active WAL contains exactly one rollback draft matching the current ref.
///
/// This is intentionally a pre-seal validation helper. It does not write objects, publish refs,
/// or mutate the worktree. It refuses trailing partial WAL bytes, non-rollback WAL records, and
/// rollback payloads that no longer match the inverse Patch derived from the selected ref.
pub fn verify_active_rollback_draft(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<RollbackDraftVerification> {
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
    if replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "active WAL has {} trailing partial bytes; run doctor before rollback-draft-verify",
            replay.trailing_partial_bytes
        )));
    }
    // RFC 102 Stage 2: `single_wal_record` below only sees the surviving records -- a genuinely
    // two-record WAL with one damaged record would otherwise pass as "exactly one," and this
    // function's result is trusted to authorize seal.
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "active WAL has a damaged record; run doctor before rollback-draft-verify".to_string(),
        ));
    }
    let Some(record) = single_wal_record(&replay.records)? else {
        return Err(PrikkError::Integrity(
            "rollback-draft-verify requires exactly one active WAL record".to_string(),
        ));
    };
    verify_active_rollback_record(record)?;

    let mut inverse = prepare_patch_inverse_plan(layout, ref_name)?;
    inverse.inverse_payload.purpose = PatchPurpose::RollbackDraft;
    // RFC 123 §8: `prepare_patch_inverse_plan` cannot derive the message -- `append_rollback_draft`
    // sets it from the caller's `-m` value after the plan returns (`rollback_draft.rs`), the same
    // way it sets `purpose` above. Read it back from the actual record before reconstructing the
    // expected payload, or every rollback draft with a message would fail this comparison for a
    // reason that has nothing to do with whether the draft matches the current inverse plan.
    inverse.inverse_payload.message = decode_patch_message(
        &record.envelope.canonical_payload,
        record.envelope.schema_version,
    )?;
    let expected_payload = inverse.inverse_payload.to_canonical_bytes()?;
    if record.envelope.canonical_payload != expected_payload {
        return Err(PrikkError::Integrity(
            "active rollback draft payload does not match the current inverse plan".to_string(),
        ));
    }
    let decoded = decode_patch_operations(
        &record.envelope.canonical_payload,
        record.envelope.schema_version,
    )?;
    // Erratum P1: decoding all §9.3 kinds does not prove the draft is replayable. A
    // rollback draft must consist only of apply-supported operations; gate explicitly
    // rather than relying on decode success.
    for operation in &decoded {
        ensure_apply_supported(operation)?;
    }
    if decoded.len() != inverse.inverse_operation_count {
        return Err(PrikkError::Integrity(format!(
            "rollback draft decoded {} operations but inverse plan has {}",
            decoded.len(),
            inverse.inverse_operation_count
        )));
    }

    Ok(RollbackDraftVerification {
        ref_name: ref_name.to_string(),
        wal_sequence: record.seq,
        draft_patch_id: record.envelope.object_id(),
        author_key_id: rollback_author_key_id(&record.envelope)?,
        target_block_id: inverse.target_block_id,
        block_count: inverse.block_count,
        patch_count: inverse.patch_count,
        inverse_operation_count: inverse.inverse_operation_count,
        decoded_operation_count: decoded.len(),
    })
}

pub(crate) fn verify_rollback_draft_wal_records(records: &[WalRecord]) -> Result<usize> {
    let mut rollback_drafts = 0_usize;
    for record in records {
        let context = format!("rollback draft WAL record {}", record.seq);
        if verify_rollback_patch_envelope(&record.envelope, &context)? {
            rollback_drafts = rollback_drafts.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("rollback draft WAL count overflow".to_string())
            })?;
        }
    }
    Ok(rollback_drafts)
}

/// Verify a rollback-marked Patch envelope and return whether it is a rollback patch.
///
/// Non-rollback Patch envelopes return `Ok(false)`. Rollback-marked envelopes must decode under
/// the currently supported replay subset and must contain at least one inverse operation. This
/// helper is shared by active-WAL verification and sealed Block/history classification.
pub(crate) fn verify_rollback_patch_envelope(
    envelope: &ObjectEnvelope,
    context: &str,
) -> Result<bool> {
    if !is_rollback_draft_envelope(envelope)? {
        return Ok(false);
    }
    if envelope.object_type != ObjectType::Patch {
        return Err(PrikkError::Integrity(format!(
            "{context} is {}, expected patch",
            envelope.object_type
        )));
    }
    let decoded = decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?;
    // Erratum P1: require apply-support, not merely decodability.
    for operation in &decoded {
        ensure_apply_supported(operation)?;
    }
    if decoded.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "{context} has no supported inverse operations"
        )));
    }
    require_rollback_author_signature(envelope, context)?;
    Ok(true)
}

fn single_wal_record(records: &[WalRecord]) -> Result<Option<&WalRecord>> {
    match records {
        [] => Ok(None),
        [record] => Ok(Some(record)),
        // RFC 132 part 2: an active WAL holding more than the rollback draft is a caller
        // precondition, not a lock -- nothing is held and no other writer is racing this one.
        _ => Err(PrikkError::Precondition(
            "rollback-draft-verify requires an active WAL containing only the rollback draft"
                .to_string(),
        )),
    }
}

fn verify_active_rollback_record(record: &WalRecord) -> Result<()> {
    if record.envelope.object_type != ObjectType::Patch {
        return Err(PrikkError::Integrity(format!(
            "rollback draft WAL record {} contains {}, expected patch",
            record.seq, record.envelope.object_type
        )));
    }
    if !is_rollback_draft_envelope(&record.envelope)? {
        return Err(PrikkError::InvalidSignature(format!(
            "active WAL record {} is not a rollback draft PatchPurpose",
            record.seq
        )));
    }
    require_rollback_author_signature(
        &record.envelope,
        &format!("active WAL record {}", record.seq),
    )?;
    Ok(())
}

fn rollback_author_key_id(envelope: &ObjectEnvelope) -> Result<String> {
    Ok(
        require_rollback_author_signature(envelope, "rollback draft Patch")?
            .key_id
            .clone(),
    )
}

fn require_rollback_author_signature<'a>(
    envelope: &'a ObjectEnvelope,
    context: &str,
) -> Result<&'a Signature> {
    envelope
        .signatures
        .iter()
        .find(|signature| signature.signer_role == SignerRole::Author)
        .ok_or_else(|| {
            PrikkError::InvalidSignature(
                "rollback draft Patch must carry an AUTHOR signature".to_string(),
            )
        })
        .and_then(|signature| {
            if signature.algorithm != SignatureAlgorithm::Ed25519 {
                return Err(PrikkError::InvalidSignature(format!(
                    "{context} rollback draft AUTHOR signature must use Ed25519"
                )));
            }
            if signature.key_id == LEGACY_ROLLBACK_MARKER_KEY_ID {
                return Err(PrikkError::InvalidSignature(format!(
                    "{context} uses legacy rollback marker key id"
                )));
            }
            // RFC 103, following DC-95 Stage 1 round 11's own finding: this arm was already reachable
            // end to end only under format-1 -- `Wal::replay()` calls `validate_read_schema` on every
            // record before this function ever runs, and under `RepositoryFormat::CurrentV6` that
            // call already hard-errors on a malformed-length signature via `envelope.validate_strict()`.
            // With formats 1 through 4 all retired (RFC 102 Stages 3-5), `CurrentV6` is the only
            // format left, so this arm is now provably unreachable through `verify_repository`'s
            // pipeline, not merely untested. Kept, per round
            // 6's ruling on unreachable checks: unreachable today is not unreachable by design, and the
            // unit-level coverage (`rollback_purpose_with_short_ed25519_author_signature_is_rejected`)
            // still proves the function's own logic is correct in isolation.
            if signature.signature_bytes.len() != ED25519_SIGNATURE_LEN {
                return Err(PrikkError::InvalidSignature(format!(
                    "{context} rollback draft AUTHOR signature must be {ED25519_SIGNATURE_LEN} bytes"
                )));
            }
            let _preimage = Signature::signed_bytes(
                signature.algorithm,
                ObjectType::Patch,
                envelope.object_id(),
                SignerRole::Author,
                &signature.key_id,
            )?;
            Ok(signature)
        })
}

#[cfg(test)]
mod tests;
