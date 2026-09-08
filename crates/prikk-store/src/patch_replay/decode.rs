//! Canonical patch-operation decoder: decodes every FDD-03 §9.3 operation kind
//! into a typed [`DecodedPatchOperation`]. Decoding is structural only; whether an
//! operation can be applied/replayed is a separate decision gated by
//! [`ensure_apply_supported`] (review erratum P1).
//!
//! Split across three files (DC-58): this file keeps the types and top-level dispatch;
//! `operations.rs` holds the seven per-operation-kind decoders; `tlv.rs` holds the low-level
//! canonical TLV cursor/field reading. No behaviour change.

use prikk_error::{PrikkError, Result};
use prikk_object::{
    NodeId, NodeKind, ObjectId, PATCH_MESSAGE_SCHEMA, PATCH_PARENT_IDS_RETIRED_SCHEMA,
    PatchPurpose, TEXT_SPAN_HASH_BYTES, WireType,
};

mod operations;
mod tlv;

use operations::{
    decode_change_perm, decode_create_file, decode_create_symlink, decode_delete_node,
    decode_edit_text, decode_rename_path, decode_replace_binary,
};
use tlv::TlvCursor;

/// One decoded operation: the validated `op_seq` envelope plus the typed body.
///
/// FDD-03 §9.2.1 already validated `op_seq == physical position + 1` during decode,
/// but the value is retained here (review erratum P2) for diagnostics, inverse
/// planning, and validator messages, so promoting the body into typed variants does
/// not discard the operation envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedPatchOperation {
    pub(crate) op_seq: u32,
    pub(crate) kind: DecodedOperationKind,
}

/// The decoded body of every FDD-03 §9.3 operation kind.
///
/// Decoding into a variant is a *structural* fact, not an *applicability* fact:
/// review erratum P1 requires that "decoded successfully" never be read as
/// "supported by replay/apply". The apply-supported subset is the single gate
/// [`ensure_apply_supported`]; the not-yet-wired kinds carry their decoded fields
/// for the node-model application increment (4.4) and are `dead_code`-allowed until
/// then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecodedOperationKind {
    /// Create a file from a persisted Blob.
    CreateFile {
        /// Repository-relative path.
        path: String,
        /// Node identity.
        node_id: NodeId,
        /// Initial Blob object ID.
        blob_id: ObjectId,
        /// Mode bits.
        mode: u32,
    },
    /// Delete a node, carrying its discriminated deletion preimage (§9.3).
    DeleteNode {
        /// Repository-relative path.
        path: String,
        /// Node identity.
        node_id: NodeId,
        /// Discriminated old-state preimage (file or symlink).
        preimage: DecodedDeletePreimage,
    },
    /// Span-anchored text edit (node-addressed; apply/inverse is FDD-01 §7.2.1, 4.4).
    EditText {
        node_id: NodeId,
        span_id: [u8; TEXT_SPAN_HASH_BYTES],
        old_span_hash: [u8; TEXT_SPAN_HASH_BYTES],
        left_anchor_hash: [u8; TEXT_SPAN_HASH_BYTES],
        right_anchor_hash: [u8; TEXT_SPAN_HASH_BYTES],
        replacement_text: Vec<u8>,
        old_span_text: Vec<u8>,
        /// RFC 134 §8: present (with `right_anchor_len`) only at
        /// `PATCH_TEXT_SPAN_V2_SCHEMA` and above; `None` means `span_id` is v1's
        /// positional identity.
        left_anchor_len: Option<u32>,
        right_anchor_len: Option<u32>,
    },
    /// Replace a binary node's blob (node-addressed; apply is 4.4).
    ReplaceBinary {
        node_id: NodeId,
        old_blob_id: ObjectId,
        new_blob_id: ObjectId,
    },
    /// Rename a node (node-addressed; apply is 4.4).
    RenamePath {
        node_id: NodeId,
        old_path: String,
        new_path: String,
    },
    /// Change a node's mode (node-addressed; apply is 4.4).
    ChangePerm {
        node_id: NodeId,
        old_mode: u32,
        new_mode: u32,
    },
    /// Create a symlink node (apply is 4.4; static target validation FDD-04 §5.4a).
    CreateSymlink {
        path: String,
        node_id: NodeId,
        target: String,
    },
}

/// Discriminated `DeleteNode` deletion preimage (§9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecodedDeletePreimage {
    /// Text/binary file: old blob + old mode.
    File {
        old_node_kind: NodeKind,
        old_blob_id: ObjectId,
        old_mode: u32,
    },
    /// Symlink: old target (apply/inverse is 4.4).
    Symlink { old_target: String },
}

/// Apply-time support gate (review erratum P1). Decoding a kind says nothing about
/// whether replay/apply can execute it; this is the *single* source of truth for the
/// apply-supported subset. Returns `Ok(())` only for the kinds whose application is
/// wired today (`CreateFile`, file-`DeleteNode`, `EditText`, `ReplaceBinary`, and `ChangePerm` —
/// DC-73); node-addressed kinds whose application is still deferred return
/// `UnsupportedObjectType`. Per review erratum P4, Phase 4 cannot be marked
/// implementation-reconciled while any kind still returns unsupported here.
pub(crate) fn ensure_apply_supported(operation: &DecodedPatchOperation) -> Result<()> {
    match &operation.kind {
        DecodedOperationKind::CreateFile { .. }
        | DecodedOperationKind::DeleteNode {
            preimage: DecodedDeletePreimage::File { .. },
            ..
        }
        | DecodedOperationKind::EditText { .. }
        | DecodedOperationKind::ReplaceBinary { .. }
        | DecodedOperationKind::ChangePerm { .. } => Ok(()),
        DecodedOperationKind::DeleteNode {
            preimage: DecodedDeletePreimage::Symlink { .. },
            ..
        } => Err(unsupported_operation("DeleteNode(symlink)")),
        // DC-73: unreachable in practice — `commit` never authors either kind (renames become
        // delete+create; symlink authoring is refused outright), so these apply paths stay
        // deferred pending an authoring path, not the node model.
        DecodedOperationKind::RenamePath { .. } => Err(unsupported_operation(
            "RenamePath (node-addressed apply pending a rename authoring path)",
        )),
        DecodedOperationKind::CreateSymlink { .. } => Err(unsupported_operation(
            "CreateSymlink (apply pending a symlink authoring path)",
        )),
    }
}

/// Stable label for one apply-supported operation kind (RFC 143 §6's coverage field). Mirrors
/// `show.rs`'s own `ShowOperation::kind` vocabulary exactly (`"create-file"`, `"delete-node"`,
/// `"edit-text"`, `"replace-binary"`, `"change-perm"`) rather than inventing a second one --
/// callable only after [`ensure_apply_supported`] has already confirmed the kind is one of these
/// five; the three refused kinds have no label here because they never reach a successful apply to
/// be recorded as covered.
pub(crate) fn applied_operation_kind_label(kind: &DecodedOperationKind) -> &'static str {
    match kind {
        DecodedOperationKind::CreateFile { .. } => "create-file",
        DecodedOperationKind::DeleteNode { .. } => "delete-node",
        DecodedOperationKind::EditText { .. } => "edit-text",
        DecodedOperationKind::ReplaceBinary { .. } => "replace-binary",
        DecodedOperationKind::ChangePerm { .. } => "change-perm",
        DecodedOperationKind::RenamePath { .. } => "rename-path",
        DecodedOperationKind::CreateSymlink { .. } => "create-symlink",
    }
}

/// Decode every FDD-03 §9.3 operation kind from canonical patch payload bytes into
/// typed [`DecodedPatchOperation`]s. Decoding validates structure/identity only;
/// applicability is gated separately by [`ensure_apply_supported`] (erratum P1).
///
/// `schema_version` comes from the object envelope carrying these bytes, mirroring
/// `RefStatePayload::decode_canonical`'s own shape (Patch schema 2 handoff): a present tag 2
/// (`parent_patch_ids`) is legal-but-ignored at schema 1 (every patch already written keeps
/// decoding unchanged) and refused outright at `PATCH_PARENT_IDS_RETIRED_SCHEMA` and above.
pub(crate) fn decode_patch_operations(
    bytes: &[u8],
    schema_version: u32,
) -> Result<Vec<DecodedPatchOperation>> {
    PatchPurpose::decode_from_patch_payload(bytes).map_err(|err| {
        PrikkError::MalformedData(format!("invalid PatchPurpose canonical form: {err}"))
    })?;
    let mut cursor = TlvCursor::new(bytes);
    let mut operations = Vec::new();
    while let Some(field) = cursor.next_field()? {
        match field.tag {
            1 => {
                field.require_wire(WireType::RecordListItem)?;
                // FDD-03 §9.2.1: the operation at physical position `i` (zero-based)
                // must carry `op_seq == i + 1`. Passing the position lets
                // decode_operation enforce one-based/contiguous/unique/ordered as a
                // single check, so raw persisted/imported bytes cannot carry an
                // alternate canonical operation order.
                let index = operations.len();
                operations.push(decode_operation(field.value, index, schema_version)?);
            }
            2 => {
                if schema_version >= PATCH_PARENT_IDS_RETIRED_SCHEMA {
                    return Err(PrikkError::MalformedData(format!(
                        "Patch schema {schema_version} must not carry parent_patch_ids (tag 2); \
                         retired at schema {PATCH_PARENT_IDS_RETIRED_SCHEMA}"
                    )));
                }
                // Schema 1: legal-but-ignored, unchanged from before this schema existed --
                // `decode_patch_parent_ids` is the function that actually reads this tag, for the
                // one caller (`patch_exchange/accept.rs`) that needs its value.
            }
            3 | 4 => {}
            5 => {
                field.require_wire(WireType::EnumU16)?;
                let _ = field.read_u16()?;
            }
            6 => {
                // RFC 123 §8.6: schema-gated exactly like EditText's own `left_anchor_len`/
                // `right_anchor_len` (`operations.rs`) -- a schema-3-or-below patch claiming
                // tag 6 is refused rather than silently decoded. The value itself is not
                // needed here; `decode_patch_message` is the single extraction point for
                // callers that need the text.
                if schema_version < PATCH_MESSAGE_SCHEMA {
                    return Err(PrikkError::MalformedData(format!(
                        "Patch schema {schema_version} must not carry message (tag 6); \
                         requires schema {PATCH_MESSAGE_SCHEMA}"
                    )));
                }
                field.require_wire(WireType::String)?;
                let _ = field.read_string()?;
            }
            other => {
                return Err(PrikkError::MalformedData(format!(
                    "unknown Patch field tag: {other}"
                )));
            }
        }
    }
    if operations.is_empty() {
        return Err(PrikkError::MalformedData(
            "Patch missing operations".to_string(),
        ));
    }
    Ok(operations)
}

/// Decode a Patch's retired `parent_patch_ids` (tag 2, repeated `object_id`) from its canonical
/// payload bytes -- legal only on a schema-1 patch, and every schema-1 authoring path has always
/// written it empty (RFC 115 Stage 3 handoff §0/§4.2 item 6; retired outright at
/// `PATCH_PARENT_IDS_RETIRED_SCHEMA`, Patch schema 2 handoff). `decode_patch_operations` above
/// does not extract this field's value; it only refuses tag 2's mere presence at schema 2 and
/// above. This function's one caller, the exchange accept path, needs the actual value, for exactly
/// one purpose: refuse the whole exchange if a carried patch ever has a non-empty one, because a
/// patch replay walk that silently ignored a real ancestry edge here would be the wrong kind of
/// quiet.
///
/// **Deliberately schema-blind, and that is stricter, not weaker.** `patch_exchange/accept.rs`
/// calls this *before* `decode_patch_operations`, so this check shadows the schema-aware one there
/// for both schemas: a non-empty tag 2 is refused whether the carrying patch claims schema 1 (where
/// the field is legal-but-must-be-empty) or schema 2 (where the field is not legal at all). Do not
/// reorder `accept.rs`, and do not read this function's continued existence as redundant with
/// `decode_patch_operations`'s own schema-2 refusal -- it is broader, not duplicate.
pub(crate) fn decode_patch_parent_ids(bytes: &[u8]) -> Result<Vec<ObjectId>> {
    let mut cursor = TlvCursor::new(bytes);
    let mut parent_patch_ids = Vec::new();
    while let Some(field) = cursor.next_field()? {
        if field.tag == 2 {
            parent_patch_ids.push(field.read_object_id_typed()?);
        }
    }
    Ok(parent_patch_ids)
}

/// Decode a Patch's optional `message` (tag 6, `WireType::String`, RFC 123 §8) from its canonical
/// payload bytes. `decode_patch_operations` above does not extract this field's value; it only
/// certifies tag 6's schema-tag legality. This function's callers need the actual text (`prikk log`
/// display), so it re-checks that legality itself rather than trusting a caller to have already run
/// `decode_patch_operations` first -- the same reasoning `decode_patch_parent_ids` gives for staying
/// independent of it, not duplicate: a second full scan of the same bytes for one field this file's
/// other functions do not need.
///
/// Refuses `Some("")` (§8.4: "absent" and "empty" must never both mean "no message") and a duplicate
/// tag 6, exactly as `PatchPayload::validate` does on the write side.
pub(crate) fn decode_patch_message(bytes: &[u8], schema_version: u32) -> Result<Option<String>> {
    let mut cursor = TlvCursor::new(bytes);
    let mut message = None;
    while let Some(field) = cursor.next_field()? {
        if field.tag == 6 {
            if schema_version < PATCH_MESSAGE_SCHEMA {
                return Err(PrikkError::MalformedData(format!(
                    "Patch schema {schema_version} must not carry message (tag 6); requires \
                     schema {PATCH_MESSAGE_SCHEMA}"
                )));
            }
            if message.is_some() {
                return Err(PrikkError::MalformedData(
                    "duplicate Patch message field".to_string(),
                ));
            }
            let value = field.read_string()?;
            if value.is_empty() {
                return Err(PrikkError::MalformedData(
                    "Patch message must not be empty".to_string(),
                ));
            }
            message = Some(value);
        }
    }
    Ok(message)
}

fn decode_operation(
    bytes: &[u8],
    index: usize,
    schema_version: u32,
) -> Result<DecodedPatchOperation> {
    let mut cursor = TlvCursor::new(bytes);
    let mut op_seq = None;
    // FDD-03 §9.2: an Operation record carries exactly one operation-kind field
    // (the oneof group, tags 10..=16). The Rust `OperationKind` enum enforces this
    // structurally on the write side; raw/imported bytes must be checked here, or a
    // malformed multi-kind record would decode "last-wins" and different readers
    // could interpret the same bytes differently. We claim the kind in the field
    // loop (rejecting a second kind before decoding it) and dispatch once after, so
    // the oneof check is independent of per-kind decode order or errors.
    let mut kind: Option<(u16, &[u8])> = None;
    while let Some(field) = cursor.next_field()? {
        match field.tag {
            1 => {
                if op_seq.is_some() {
                    return Err(PrikkError::MalformedData(
                        "duplicate Operation op_seq field".to_string(),
                    ));
                }
                op_seq = Some(field.read_u32()?);
            }
            2 | 3 => {}
            10..=16 => {
                if let Some((first, _)) = kind {
                    return Err(PrikkError::MalformedData(format!(
                        "Operation carries multiple kind records: tag {first} and tag {}",
                        field.tag
                    )));
                }
                field.require_wire(WireType::Record)?;
                kind = Some((field.tag, field.value));
            }
            other => {
                return Err(PrikkError::MalformedData(format!(
                    "unknown Operation field tag: {other}"
                )));
            }
        }
    }
    let op_seq =
        op_seq.ok_or_else(|| PrikkError::MalformedData("Operation missing op_seq".to_string()))?;
    // FDD-03 §9.2.1: op_seq is one-based and equals physical position + 1. This
    // single comparison enforces first==1, contiguous, unique, and physical order ==
    // ascending op_seq; any deviation is rejected, never normalized.
    if (op_seq as usize) != index + 1 {
        return Err(PrikkError::MalformedData(format!(
            "operation op_seq {op_seq} does not match physical position {} (expected {})",
            index,
            index + 1
        )));
    }
    let (kind_tag, value) =
        kind.ok_or_else(|| PrikkError::MalformedData("Operation missing kind".to_string()))?;
    let kind = match kind_tag {
        10 => decode_create_file(value),
        11 => decode_delete_node(value),
        12 => decode_edit_text(value, schema_version),
        13 => decode_rename_path(value),
        14 => decode_change_perm(value),
        15 => decode_create_symlink(value),
        16 => decode_replace_binary(value),
        _ => {
            return Err(PrikkError::MalformedData(format!(
                "Operation kind tag {kind_tag} is outside the constrained 10..=16 range"
            )));
        }
    }?;
    Ok(DecodedPatchOperation { op_seq, kind })
}

fn unsupported_operation(name: &str) -> PrikkError {
    PrikkError::UnsupportedObjectType(format!(
        "patch replay plan does not yet support {name}; patch algebra remains a later increment"
    ))
}
