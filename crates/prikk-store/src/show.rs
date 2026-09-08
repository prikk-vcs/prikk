//! `prikk show` (RFC 142): what a block or patch changed.
//!
//! Content comes straight from the patch payloads (RFC 142 §3 — no replay needed): `EditText`
//! carries its own before/after span text, and every other kind carries its own path, mode, or
//! blob reference inline. Only the three node-addressed kinds (`EditText`, `ChangePerm`,
//! `ReplaceBinary`) carry no path and need resolving, and only at a sealed block, against that
//! block's own lifecycle state (`merge_evidence::lifecycle_state_at`, RFC 142 §3/§4) — one replay
//! per invocation, not one per operation.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlobKind, BlobPayload, BlockPayload, NodeId, ObjectId, ObjectType};

use crate::RepositoryLayout;
use crate::merge_evidence::lifecycle_state_at;
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation, decode_patch_operations,
};

/// What `show` resolved a node-addressed operation's path to. RFC 140's "never fatal" rule
/// applies unchanged (RFC 142 §3, control 3): an unresolved node id is reported, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowPathResolution {
    /// A repository-relative path, read from the payload or resolved against the target block's
    /// lifecycle state.
    Path(String),
    /// A node-addressed operation whose node id is not live at the target — hex-encoded, matching
    /// `prikk_hash::to_hex` (RFC 140's own format). Reachable when a block both edits and deletes
    /// the same node, and whenever the target is a bare patch with no block to resolve against.
    Unresolved {
        /// Hex-encoded node id that failed to resolve.
        node_id: String,
    },
}

/// A blob's content, rendered only when it is text — RFC 142 §7 refuses binary rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowBlobContent {
    /// `BlobKind::Text` — the blob's own bytes, verbatim.
    Text(Vec<u8>),
    /// `BlobKind::Binary` — id and declared size only, never content.
    Binary {
        /// The blob's own object id.
        blob_id: ObjectId,
        /// Declared content size in bytes.
        size: u64,
    },
}

/// `DeleteNode`'s discriminated preimage, content-resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowDeletePreimage {
    /// File or binary node: the deleted blob's content (rendered per [`ShowBlobContent`]).
    File(ShowBlobContent),
    /// Symlink node: the deleted target, already inline in the operation.
    Symlink {
        /// The symlink's former target.
        old_target: String,
    },
}

/// What one operation changed, RFC 142 §2/§6: `EditText` renders its own before/after span,
/// never a synthesized line diff — the span is the truth, not a reconstructed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowOperationContent {
    /// A new file's initial content.
    CreateFile {
        /// The new blob's content.
        content: ShowBlobContent,
        /// Mode bits.
        mode: u32,
    },
    /// A deleted node's preimage.
    DeleteNode {
        /// What the node held before deletion.
        preimage: ShowDeletePreimage,
    },
    /// A span-anchored text edit: the exact bytes replaced and their replacement (RFC 134 — a
    /// content-anchored span, not a line range).
    EditText {
        /// The span's bytes before this edit.
        old_span_text: Vec<u8>,
        /// The span's bytes after this edit.
        replacement_text: Vec<u8>,
    },
    /// A binary blob replacement — ids and sizes only (RFC 142 §7: no binary rendering).
    ReplaceBinary {
        /// The replaced blob's own id.
        old_blob_id: ObjectId,
        /// The replaced blob's declared size.
        old_size: u64,
        /// The new blob's own id.
        new_blob_id: ObjectId,
        /// The new blob's declared size.
        new_size: u64,
    },
    /// A path rename; both endpoints are already in [`ShowOperation::paths`].
    RenamePath,
    /// A mode change.
    ChangePerm {
        /// Mode before the change.
        old_mode: u32,
        /// Mode after the change.
        new_mode: u32,
    },
    /// A new symlink node.
    CreateSymlink {
        /// The symlink's target.
        target: String,
    },
}

/// One operation `show` renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowOperation {
    /// Stable kind label, matching `QueuedOperationEntry::kind`'s own vocabulary (RFC 140):
    /// `create-file`, `delete-node`, `edit-text`, `rename-path`, `change-perm`, `create-symlink`,
    /// or `replace-binary`.
    pub kind: &'static str,
    /// The path(s) this operation affects, in payload order (two for `rename-path`, one
    /// otherwise).
    pub paths: Vec<ShowPathResolution>,
    /// What this operation changed.
    pub content: ShowOperationContent,
}

/// One patch's own operations, in canonical (`op_seq`) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowPatch {
    /// This patch's own signed object id.
    pub patch_id: ObjectId,
    /// This patch's operations.
    pub operations: Vec<ShowOperation>,
}

/// `prikk show <block-id|patch-id>` (RFC 142): read the target, decode its patch(es), resolve
/// node-addressed paths against the target block's own lifecycle state when the target is a
/// block. A block's output is the union of its patches in canonical (`patch_ids`) order (control
/// 5). A bare patch has no block context to resolve node-addressed operations against — those are
/// reported unresolved rather than fatal, the same mechanism §3 already requires for the
/// edit-then-delete-in-one-block case, applied here for a different reason (no target block at
/// all, not a node missing from one).
pub fn show(layout: &RepositoryLayout, id: ObjectId) -> Result<Vec<ShowPatch>> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let envelope = object_store
        .read_object(id)?
        .ok_or_else(|| PrikkError::Integrity(format!("no object {id}")))?;
    match envelope.object_type {
        ObjectType::Block => {
            let block = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
            let lifecycle = lifecycle_state_at(&object_store, id)?;
            block
                .patch_ids
                .iter()
                .map(|patch_id| show_patch(&object_store, *patch_id, Some(lifecycle.state())))
                .collect()
        }
        ObjectType::Patch => Ok(vec![show_patch(&object_store, id, None)?]),
        other => Err(PrikkError::UnsupportedObjectType(format!(
            "show requires a Block or Patch id; {id} is {other}"
        ))),
    }
}

fn show_patch(
    object_store: &impl ObjectReader,
    patch_id: ObjectId,
    lifecycle: Option<&NodeLifecycleState>,
) -> Result<ShowPatch> {
    let envelope = object_store
        .read_typed(patch_id, ObjectType::Patch)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Patch {patch_id}")))?;
    let operations = decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?;
    let operations = operations
        .iter()
        .map(|operation| show_operation(object_store, operation, lifecycle))
        .collect::<Result<Vec<_>>>()?;
    Ok(ShowPatch {
        patch_id,
        operations,
    })
}

fn show_operation(
    object_store: &impl ObjectReader,
    operation: &DecodedPatchOperation,
    lifecycle: Option<&NodeLifecycleState>,
) -> Result<ShowOperation> {
    match &operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            blob_id,
            mode,
            ..
        } => {
            let content = show_blob_content(object_store, *blob_id)?;
            Ok(ShowOperation {
                kind: "create-file",
                paths: vec![ShowPathResolution::Path(path.clone())],
                content: ShowOperationContent::CreateFile {
                    content,
                    mode: *mode,
                },
            })
        }
        DecodedOperationKind::DeleteNode { path, preimage, .. } => {
            let preimage = match preimage {
                DecodedDeletePreimage::File { old_blob_id, .. } => {
                    ShowDeletePreimage::File(show_blob_content(object_store, *old_blob_id)?)
                }
                DecodedDeletePreimage::Symlink { old_target } => ShowDeletePreimage::Symlink {
                    old_target: old_target.clone(),
                },
            };
            Ok(ShowOperation {
                kind: "delete-node",
                paths: vec![ShowPathResolution::Path(path.clone())],
                content: ShowOperationContent::DeleteNode { preimage },
            })
        }
        DecodedOperationKind::EditText {
            node_id,
            old_span_text,
            replacement_text,
            ..
        } => Ok(ShowOperation {
            kind: "edit-text",
            paths: vec![resolve_node_path(*node_id, lifecycle)],
            content: ShowOperationContent::EditText {
                old_span_text: old_span_text.clone(),
                replacement_text: replacement_text.clone(),
            },
        }),
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => {
            let old_size = read_blob(object_store, *old_blob_id)?.declared_size;
            let new_size = read_blob(object_store, *new_blob_id)?.declared_size;
            Ok(ShowOperation {
                kind: "replace-binary",
                paths: vec![resolve_node_path(*node_id, lifecycle)],
                content: ShowOperationContent::ReplaceBinary {
                    old_blob_id: *old_blob_id,
                    old_size,
                    new_blob_id: *new_blob_id,
                    new_size,
                },
            })
        }
        DecodedOperationKind::RenamePath {
            old_path, new_path, ..
        } => Ok(ShowOperation {
            kind: "rename-path",
            paths: vec![
                ShowPathResolution::Path(old_path.clone()),
                ShowPathResolution::Path(new_path.clone()),
            ],
            content: ShowOperationContent::RenamePath,
        }),
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => Ok(ShowOperation {
            kind: "change-perm",
            paths: vec![resolve_node_path(*node_id, lifecycle)],
            content: ShowOperationContent::ChangePerm {
                old_mode: *old_mode,
                new_mode: *new_mode,
            },
        }),
        DecodedOperationKind::CreateSymlink { path, target, .. } => Ok(ShowOperation {
            kind: "create-symlink",
            paths: vec![ShowPathResolution::Path(path.clone())],
            content: ShowOperationContent::CreateSymlink {
                target: target.clone(),
            },
        }),
    }
}

fn resolve_node_path(
    node_id: NodeId,
    lifecycle: Option<&NodeLifecycleState>,
) -> ShowPathResolution {
    lifecycle
        .and_then(|state| state.live_node(&node_id))
        .map(|node| ShowPathResolution::Path(node.path.as_str().to_string()))
        .unwrap_or_else(|| ShowPathResolution::Unresolved {
            node_id: prikk_hash::to_hex(node_id.as_bytes()),
        })
}

fn read_blob(object_store: &impl ObjectReader, blob_id: ObjectId) -> Result<BlobPayload> {
    let envelope = object_store
        .read_typed(blob_id, ObjectType::Blob)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Blob {blob_id}")))?;
    BlobPayload::decode_canonical(&envelope.canonical_payload)
}

fn show_blob_content(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
) -> Result<ShowBlobContent> {
    let blob = read_blob(object_store, blob_id)?;
    match blob.blob_kind {
        BlobKind::Text => Ok(ShowBlobContent::Text(blob.content)),
        BlobKind::Binary => Ok(ShowBlobContent::Binary {
            blob_id,
            size: blob.declared_size,
        }),
        BlobKind::Snapshot => Err(PrikkError::Integrity(format!(
            "content blob {blob_id} is a SNAPSHOT blob, not file content"
        ))),
    }
}

#[cfg(test)]
mod tests;
