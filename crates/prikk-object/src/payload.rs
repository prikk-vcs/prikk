//! Canonical payload shapes for Prikk object types.

pub mod attestation;
pub mod blob;
pub mod block;
pub mod common;
pub mod node;
pub mod patch;
pub mod recognition_claim;
pub mod refs;
pub mod tag;

#[cfg(test)]
mod tests;

pub use attestation::{AttestationPayload, AttestationStatus, PluginResultEntry};
pub use blob::{BlobKind, BlobPayload};
pub use block::{BlockKind, BlockPayload};
pub use common::{Intent, MerkleRoot, OperationCondition, OperationConditionEntry, PatchSetDigest};
pub use node::{NODE_ID_BYTES, NodeId, NodeKind};
pub use patch::{
    ChangePerm, CreateFile, CreateSymlink, DeleteNode, DeleteNodePreimage, EditText, Operation,
    OperationKind, PATCH_MESSAGE_SCHEMA, PATCH_PARENT_IDS_RETIRED_SCHEMA,
    PATCH_TEXT_SPAN_V2_SCHEMA, PatchPayload, PatchPurpose, RenamePath, ReplaceBinary,
    TEXT_SPAN_ANCHOR_MIN_LEN, TEXT_SPAN_HASH_BYTES, authoring_path_error, is_canonical_file_mode,
    text_span_hash, validate_text_anchor_id,
};
pub use recognition_claim::{RECOGNITION_CLAIM_MAX_PATCH_IDS, RecognitionClaimPayload};
pub use refs::{REF_STATE_CLOSED_SCHEMA, RefKind, RefStatePayload, RefUpdatePayload};
pub use tag::TagPayload;
