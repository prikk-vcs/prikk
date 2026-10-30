//! Patch payload types.
//!
//! Split into two files (DC-58): this file keeps `PatchPayload`/`PatchPurpose`/`Operation`/
//! `OperationKind` and the text-span helpers; `patch/operations.rs` holds the seven per-kind
//! payload structs (`CreateFile`, `DeleteNode`, …). All items stay `pub` and are re-exported here
//! at the same path (`payload::patch::*`), so every existing caller — including the crate-root
//! re-export at `payload.rs` — is unaffected. No behaviour change.

use prikk_error::{PrikkError, Result};

use crate::canonical::{is_contiguous_op_seq, is_strictly_sorted};
use crate::payload::common::{Intent, OperationCondition, OperationConditionEntry};
use crate::{CanonicalEncode, CanonicalWriter, WireType};

mod operations;

pub use operations::{
    ChangePerm, CreateFile, CreateSymlink, DeleteNode, DeleteNodePreimage, EditText, RenamePath,
    ReplaceBinary, authoring_path_error, is_canonical_file_mode,
};

/// Number of bytes in a content-anchored text span hash.
pub const TEXT_SPAN_HASH_BYTES: usize = 32;

/// Compute the stable hash used by content-anchored text edit preconditions.
#[must_use]
pub fn text_span_hash(bytes: &[u8]) -> [u8; TEXT_SPAN_HASH_BYTES] {
    prikk_hash::sha256(bytes)
}

/// Validate a stable content-anchor identifier.
pub fn validate_text_anchor_id(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(PrikkError::CanonicalEncoding(
            "text anchor id must not be empty".to_string(),
        ));
    }
    if !value.is_ascii() {
        return Err(PrikkError::CanonicalEncoding(
            "text anchor id must be ASCII in v1".to_string(),
        ));
    }
    if value.bytes().any(|byte| byte < 0x21 || byte == 0x7f) {
        return Err(PrikkError::CanonicalEncoding(
            "text anchor id must not contain whitespace or control characters".to_string(),
        ));
    }
    Ok(())
}

/// `Patch` schema at and above which field 2 (`parent_patch_ids`) is retired — the opposite
/// direction from [`crate::REF_STATE_CLOSED_SCHEMA`], which admits a field starting at its
/// threshold rather than retiring one. Schema 1 (frozen forever, RFC 114) keeps decoding a
/// present field 2 without inspecting it, exactly as before this schema existed, so every patch
/// already written keeps decoding unchanged. Schema 2 refuses field 2's mere presence outright —
/// see `decode_patch_operations` (`prikk-store`). **Field number 2 is retired, not reused**: no
/// future schema may repurpose tag 2 for something else, since a schema-1 reader would silently
/// misinterpret it as the old `parent_patch_ids` shape.
pub const PATCH_PARENT_IDS_RETIRED_SCHEMA: u32 = 2;

/// `Patch` schema at and above which `EditText` may carry tags 10/11 (`left_anchor_len`,
/// `right_anchor_len`) — RFC 134 §8's content-unique span identity. Below this schema the fields
/// are absent, exactly as before this schema existed, so no existing object's bytes move. At and
/// above it a present pair of anchor-length fields switches resolution from v1's positional
/// `dup_index` to v2's content-unique identity; v1 resolution stays available forever for
/// operations that omit the fields (RFC 114: keep every version ever written decodable).
pub const PATCH_TEXT_SPAN_V2_SCHEMA: u32 = 3;

/// `Patch` schema at and above which the payload may carry `message` (tag 6, `WireType::String`) --
/// RFC 123 §8. Below this schema the field is absent, exactly as before this schema existed, so no
/// existing object's bytes move. Optional even at this schema and above (§8.2): RFC 113's Git
/// import must be able to represent a commit that genuinely had no message. `Some("")` is refused
/// by [`PatchPayload::validate`] regardless of schema, so "absent" and "empty" never both mean "no
/// message". This is the current schema for every newly authored patch -- both authoring sites
/// (`worktree_patch/node_authoring.rs`, `patch_inverse.rs`/`rollback_draft.rs`) mint it
/// unconditionally, exactly as they minted [`PATCH_TEXT_SPAN_V2_SCHEMA`] before it.
pub const PATCH_MESSAGE_SCHEMA: u32 = 4;

/// Minimum length, in bytes, of a v2 `EditText` anchor (`left_anchor_len`/`right_anchor_len`).
/// Matches v1's fixed anchor window (`TEXT_ANCHOR_WINDOW` in `prikk-store`) — v2 anchors are never
/// shorter than v1's, only ever longer when uniqueness demands it. Defined here, not in
/// `prikk-store`, because `prikk-store` depends on `prikk-object` and not the reverse.
pub const TEXT_SPAN_ANCHOR_MIN_LEN: u32 = 64;

/// Patch payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPayload {
    /// Operations in semantic order. `op_seq` must be contiguous from 1.
    pub operations: Vec<Operation>,
    /// Advisory intent.
    pub intent: Option<Intent>,
    /// Patch-level preconditions, sorted by key.
    pub preconditions: Vec<OperationConditionEntry>,
    /// Identity-bearing patch purpose. `Normal` is canonical by omission.
    pub purpose: PatchPurpose,
    /// Commit message (RFC 123 §8): identity-bearing, optional, present only at
    /// [`PATCH_MESSAGE_SCHEMA`] and above. Mirrors `TagPayload.message` exactly. `Some("")` is
    /// refused by [`Self::validate`] -- "absent" and "empty" must never both mean "no message".
    pub message: Option<String>,
}

impl PatchPayload {
    /// Validate ordering and duplicate constraints.
    pub fn validate(&self) -> Result<()> {
        if self.operations.is_empty() {
            return Err(PrikkError::CanonicalEncoding(
                "patch operations must contain at least one operation".to_string(),
            ));
        }
        let op_seq: Vec<u32> = self.operations.iter().map(|op| op.op_seq).collect();
        if !is_contiguous_op_seq(&op_seq) {
            return Err(PrikkError::CanonicalEncoding(
                "patch operations must have contiguous op_seq values starting at 1".to_string(),
            ));
        }
        if !is_strictly_sorted(&self.preconditions) {
            return Err(PrikkError::CanonicalEncoding(
                "patch preconditions must be sorted and unique".to_string(),
            ));
        }
        if self.message.as_deref() == Some("") {
            return Err(PrikkError::CanonicalEncoding(
                "patch message must not be Some(\"\") -- omit it instead".to_string(),
            ));
        }
        Ok(())
    }
}

impl CanonicalEncode for PatchPayload {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.repeated_record_list(1, &self.operations)?;
        // Tag 2 (`parent_patch_ids`) is retired at `PATCH_PARENT_IDS_RETIRED_SCHEMA` and above --
        // every construction site now writes that schema (or later), so tag 2 is never emitted
        // here at all. See `PATCH_PARENT_IDS_RETIRED_SCHEMA`'s own doc.
        if let Some(intent) = self.intent {
            writer.field_enum_u16(3, intent.code())?;
        }
        writer.repeated_record(4, &self.preconditions)?;
        if self.purpose != PatchPurpose::Normal {
            writer.field_enum_u16(5, self.purpose.code())?;
        }
        writer.field_string_opt(6, self.message.as_deref())?;
        Ok(())
    }
}

/// Identity-bearing patch purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum PatchPurpose {
    /// Ordinary Patch. This is the default when tag 5 is absent and must not be encoded explicitly.
    Normal = 1,
    /// Rollback draft Patch. This survives WAL-to-object persistence for classification.
    RollbackDraft = 2,
}

impl PatchPurpose {
    /// Stable numeric code.
    #[must_use]
    pub const fn code(self) -> u16 {
        self as u16
    }

    /// Parse a stable code from a present tag-5 purpose field.
    pub fn from_present_code(code: u16) -> Result<Self> {
        match code {
            1 => Err(PrikkError::CanonicalEncoding(
                "PatchPurpose::Normal must be omitted, not encoded explicitly".to_string(),
            )),
            2 => Ok(Self::RollbackDraft),
            other => Err(PrikkError::CanonicalEncoding(format!(
                "unknown patch purpose code: {other}"
            ))),
        }
    }

    /// Decode only the top-level `PatchPayload` purpose field, validating tag order and rejecting
    /// an explicitly encoded `Normal` default. Absence means `Normal`. Schema-blind by design, like
    /// `decode_patch_parent_ids` in `prikk-store`: tag 6 (`message`, RFC 123 §8) is accepted here
    /// unconditionally, regardless of schema -- the schema-gated legality check belongs to
    /// `decode_patch_operations`, the one caller of this function that also knows the envelope's
    /// declared schema.
    pub fn decode_from_patch_payload(bytes: &[u8]) -> Result<Self> {
        let mut cursor = PatchPayloadFieldCursor::new(bytes);
        let mut purpose = Self::Normal;
        let mut seen_purpose = false;
        while let Some(field) = cursor.next_field()? {
            match field.tag {
                1..=4 | 6 => {}
                5 => {
                    if seen_purpose {
                        return Err(PrikkError::CanonicalEncoding(
                            "duplicate PatchPurpose field".to_string(),
                        ));
                    }
                    seen_purpose = true;
                    field.require_wire(WireType::EnumU16)?;
                    purpose = Self::from_present_code(field.read_u16()?)?;
                }
                other => {
                    return Err(PrikkError::CanonicalEncoding(format!(
                        "unknown PatchPayload field tag: {other}"
                    )));
                }
            }
        }
        Ok(purpose)
    }
}

struct PatchPayloadFieldCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    last_tag: Option<u16>,
}

impl<'a> PatchPayloadFieldCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            last_tag: None,
        }
    }

    fn next_field(&mut self) -> Result<Option<PatchPayloadField<'a>>> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }
        let tag = u16::from_be_bytes(self.read_array::<2>()?);
        if tag == 0 {
            return Err(PrikkError::CanonicalEncoding(
                "field tag 0 is reserved".to_string(),
            ));
        }
        if let Some(last) = self.last_tag {
            if tag < last {
                return Err(PrikkError::CanonicalEncoding(format!(
                    "field tag order violation: {tag} after {last}"
                )));
            }
        }
        self.last_tag = Some(tag);
        let wire_type = self.read_u8()?;
        let len = usize::try_from(u64::from_be_bytes(self.read_array::<8>()?)).map_err(|_| {
            PrikkError::CanonicalEncoding("canonical field length does not fit usize".to_string())
        })?;
        let value = self.read_exact(len)?;
        Ok(Some(PatchPayloadField {
            tag,
            wire_type,
            value,
        }))
    }

    fn read_u8(&mut self) -> Result<u8> {
        let bytes = self.read_exact(1)?;
        let Some(byte) = bytes.first() else {
            return Err(PrikkError::CanonicalEncoding(
                "unexpected empty byte".to_string(),
            ));
        };
        Ok(*byte)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.read_exact(N)?;
        let mut out = [0_u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| PrikkError::CanonicalEncoding("canonical range overflow".to_string()))?;
        let Some(slice) = self.bytes.get(self.pos..end) else {
            return Err(PrikkError::CanonicalEncoding(
                "unexpected end of canonical payload".to_string(),
            ));
        };
        self.pos = end;
        Ok(slice)
    }
}

struct PatchPayloadField<'a> {
    tag: u16,
    wire_type: u8,
    value: &'a [u8],
}

impl PatchPayloadField<'_> {
    fn require_wire(&self, expected: WireType) -> Result<()> {
        if self.wire_type == expected as u8 {
            return Ok(());
        }
        Err(PrikkError::CanonicalEncoding(format!(
            "field {} has wrong wire type: expected {}, got {}",
            self.tag, expected as u8, self.wire_type
        )))
    }

    fn read_u16(&self) -> Result<u16> {
        if self.value.len() != 2 {
            return Err(PrikkError::CanonicalEncoding(format!(
                "field {} expected 2 bytes, got {}",
                self.tag,
                self.value.len()
            )));
        }
        let mut out = [0_u8; 2];
        out.copy_from_slice(self.value);
        Ok(u16::from_be_bytes(out))
    }
}

/// A single operation inside a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// Strict operation sequence, starting at 1 inside the patch.
    pub op_seq: u32,
    /// Optional stable label for UI/debugging.
    pub op_id: Option<String>,
    /// Inline operation preconditions.
    pub preconditions: Vec<OperationCondition>,
    /// Operation kind.
    pub kind: OperationKind,
}

impl CanonicalEncode for Operation {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        writer.field_u32(1, self.op_seq)?;
        writer.field_string_opt(2, self.op_id.as_deref())?;
        writer.repeated_record(3, &self.preconditions)?;
        match &self.kind {
            OperationKind::CreateFile(value) => writer.field_record(10, value)?,
            OperationKind::DeleteNode(value) => writer.field_record(11, value)?,
            OperationKind::EditText(value) => writer.field_record(12, value)?,
            OperationKind::RenamePath(value) => writer.field_record(13, value)?,
            OperationKind::ChangePerm(value) => writer.field_record(14, value)?,
            OperationKind::CreateSymlink(value) => writer.field_record(15, value)?,
            OperationKind::ReplaceBinary(value) => writer.field_record(16, value)?,
        }
        Ok(())
    }
}

/// Operation variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationKind {
    /// Create a text or binary file.
    CreateFile(CreateFile),
    /// Delete a node.
    DeleteNode(DeleteNode),
    /// Edit text using content-anchored spans.
    EditText(EditText),
    /// Rename a path.
    RenamePath(RenamePath),
    /// Change Unix-like permissions.
    ChangePerm(ChangePerm),
    /// Create a symbolic link.
    CreateSymlink(CreateSymlink),
    /// Replace an opaque binary blob.
    ReplaceBinary(ReplaceBinary),
}
