//! Repository-format schema admission rules.

use prikk_error::{PrikkError, Result};
use prikk_object::{
    ObjectEnvelope, ObjectType, PATCH_MESSAGE_SCHEMA, PATCH_PARENT_IDS_RETIRED_SCHEMA,
    PATCH_TEXT_SPAN_V2_SCHEMA, REF_STATE_CLOSED_SCHEMA,
};

use crate::foundation::layout::RepositoryFormat;

pub(crate) fn validate_object_envelope(
    format: RepositoryFormat,
    envelope: &ObjectEnvelope,
) -> Result<()> {
    envelope.validate_strict()?;
    match format {
        RepositoryFormat::CurrentV6 => validate_format2_schema(envelope),
    }
}

/// The admitted envelope schemas for `object_type` at format 2 — the single authoritative source
/// every schema-admission check must read from, never a second hand-maintained list (Patch schema
/// 2 handoff v2 amendment §2: `lifecycle_cache`'s own `require_schema_one` checks used to duplicate
/// this table with a literal `!= 1`, which is exactly the defect this function exists to prevent).
/// `None` means the type is never authorized in a format-2 identity position at all.
///
/// Every type but `RefState` and `Patch` accepts exactly one schema. `RefState` accepts schema 1
/// (open, the pre-DC-61 shape) or `REF_STATE_CLOSED_SCHEMA` (closed, DC-61). `Patch` accepts schema
/// 1 (may carry a `parent_patch_ids` field, tag 2, for backward compatibility with every patch
/// already written), `PATCH_PARENT_IDS_RETIRED_SCHEMA` (tag 2 retired outright),
/// `PATCH_TEXT_SPAN_V2_SCHEMA` (RFC 134 §8: `EditText` may additionally carry tags 10/11, the
/// content-unique anchor lengths), or `PATCH_MESSAGE_SCHEMA` (RFC 123 §8: the payload may
/// additionally carry an optional `message`, tag 6). `Tag` gained two fields in place at schema 1
/// after it shipped (`patch_set_digest`, RFC 117 stage 1; `patch_count`, T7) rather than minting a
/// new schema — the owner ruled `Tag`'s schema window closed (2026-08-23), so `RefState` and `Patch`
/// are the only types admitting more than one schema.
pub(crate) fn admitted_schemas(object_type: ObjectType) -> Option<&'static [u32]> {
    match object_type {
        ObjectType::Block => Some(&[2]),
        ObjectType::RefState => Some(&[1, REF_STATE_CLOSED_SCHEMA]),
        ObjectType::Patch => Some(&[
            1,
            PATCH_PARENT_IDS_RETIRED_SCHEMA,
            PATCH_TEXT_SPAN_V2_SCHEMA,
            PATCH_MESSAGE_SCHEMA,
        ]),
        ObjectType::RefUpdate
        | ObjectType::Tag
        | ObjectType::Attestation
        | ObjectType::Blob
        | ObjectType::RecognitionClaim => Some(&[1]),
        ObjectType::BlockSummaryCache | ObjectType::RecoveryNote => None,
    }
}

pub(crate) fn validate_format2_schema(envelope: &ObjectEnvelope) -> Result<()> {
    let Some(accepted) = admitted_schemas(envelope.object_type) else {
        return Err(PrikkError::Integrity(format!(
            "{} is not authorized in a format-2 identity position",
            envelope.object_type
        )));
    };
    if !accepted.contains(&envelope.schema_version) {
        return Err(PrikkError::Integrity(format!(
            "format-2 {} does not accept envelope schema {} (accepted: {accepted:?})",
            envelope.object_type, envelope.schema_version
        )));
    }
    Ok(())
}

pub(crate) fn validate_read_schema(
    format: RepositoryFormat,
    envelope: &ObjectEnvelope,
) -> Result<()> {
    match format {
        RepositoryFormat::CurrentV6 => {
            envelope.validate_strict()?;
            validate_format2_schema(envelope)
        }
    }
}

#[cfg(test)]
mod tests;
