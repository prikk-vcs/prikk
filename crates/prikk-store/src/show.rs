//! `prikk show` (RFC 142): what a block or patch changed.
//!
//! Content comes straight from the patch payloads (RFC 142 §3 — no replay needed): `EditText`
//! carries its own before/after span text, and every other kind carries its own path, mode, or
//! blob reference inline. Only the three node-addressed kinds (`EditText`, `ChangePerm`,
//! `ReplaceBinary`) carry no path and need resolving, and only at a sealed block, against that
//! block's own lifecycle state (`merge_evidence::lifecycle_state_at`, RFC 142 §3/§4) — one replay
//! per invocation, not one per operation.

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, BlockPayload, NodeId, ObjectEnvelope, ObjectId, ObjectType,
};

use crate::RepositoryLayout;
use crate::author::author_signing::require_author_key_id;
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
///
/// RFC 142 §6a/§6b: a blob a patch payload names is a content-addressed *id*, not a promise the
/// object store holds it (§3a's correction — DC-65 leaves a text node's pre-edit blob id
/// deliberately unbacked once the node has been edited without a fresh blob write). `show` is a
/// read surface, not `verify`: an *absent* blob (`Ok(None)`) is ambiguous at this layer — the
/// overwhelmingly common cause is DC-65's by-design gap, and `show` cannot tell that case apart
/// from any other blob that simply never made it into the store without replaying the algebra to
/// decide what *should* be stored, a verification path this RFC's own §7 refuses to add — so
/// absence alone degrades to this variant rather than failing the command. **A real error does
/// not degrade**: the object store affirmatively reporting damage (a content-hash mismatch, a type
/// disagreement, a malformed payload, a `SNAPSHOT`-kind blob) is not ambiguous the way absence is,
/// and propagates as a `PrikkError` instead (§6b). A `doctor`-level surface wanting to separate
/// DC-65's specific cause from ordinary absence would need to independently check whether this
/// exact node's lifecycle includes an `EditText` since the blob was last live -- evidence `show`
/// does not gather because nothing here needs it otherwise.
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
    /// The blob id is absent from the object store — named and machine-branchable, carrying the
    /// id that failed, never an omitted field or an empty value a consumer could mistake for real
    /// empty content. A real error (wrong type, malformed payload, content-hash mismatch) is not
    /// this variant: it propagates instead (§6b).
    Unavailable {
        /// The blob id that could not be read.
        blob_id: ObjectId,
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
    /// A binary blob replacement. Each side is [`ShowBlobContent`] the same way `CreateFile`'s
    /// content is: id and declared size only, since binary content is never rendered (RFC 142
    /// §7) — reusing the one degradation idiom rather than a second, `Option`-shaped one for this
    /// operation alone (RFC 142 §6a's follow-up: "follow the local precedent").
    ReplaceBinary {
        /// The replaced blob.
        old: ShowBlobContent,
        /// The new blob.
        new: ShowBlobContent,
    },
    /// A path rename; both endpoints are already in [`ShowOperation::paths`]. RFC 144 §4o.6a's
    /// honesty invariant: a signed `RenamePath` asserts *this signer* rewrote `old` to `new`, so
    /// the asserting signer must be recoverable in the same answer, not a fact a surface can
    /// choose to drop -- this variant cannot be constructed without it.
    RenamePath {
        /// The patch's AUTHOR signature key id -- the signer who asserted this rename. Not a
        /// trust or verification judgement (that is a separate, local question); carries identity
        /// only.
        author_key_id: String,
    },
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
        .map(|operation| show_operation(object_store, &envelope, operation, lifecycle))
        .collect::<Result<Vec<_>>>()?;
    Ok(ShowPatch {
        patch_id,
        operations,
    })
}

/// RFC 142 §6b: fallible again, unlike round 2's infallible shape -- three of the seven kinds may
/// now propagate a real `PrikkError` (an unreadable blob's *error* case, not its absence), and a
/// `Result` at this layer is the cleanest way to let that reach `show_patch`'s own `?` rather than
/// inventing a second signalling path alongside [`ShowBlobContent::Unavailable`].
fn show_operation(
    object_store: &impl ObjectReader,
    envelope: &ObjectEnvelope,
    operation: &DecodedPatchOperation,
    lifecycle: Option<&NodeLifecycleState>,
) -> Result<ShowOperation> {
    Ok(match &operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            blob_id,
            mode,
            ..
        } => {
            let content = show_blob_content(object_store, *blob_id)?;
            ShowOperation {
                kind: "create-file",
                paths: vec![ShowPathResolution::Path(path.clone())],
                content: ShowOperationContent::CreateFile {
                    content,
                    mode: *mode,
                },
            }
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
            ShowOperation {
                kind: "delete-node",
                paths: vec![ShowPathResolution::Path(path.clone())],
                content: ShowOperationContent::DeleteNode { preimage },
            }
        }
        DecodedOperationKind::EditText {
            node_id,
            old_span_text,
            replacement_text,
            ..
        } => ShowOperation {
            kind: "edit-text",
            paths: vec![resolve_node_path(*node_id, lifecycle)],
            content: ShowOperationContent::EditText {
                old_span_text: old_span_text.clone(),
                replacement_text: replacement_text.clone(),
            },
        },
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => ShowOperation {
            kind: "replace-binary",
            paths: vec![resolve_node_path(*node_id, lifecycle)],
            content: ShowOperationContent::ReplaceBinary {
                old: show_blob_content(object_store, *old_blob_id)?,
                new: show_blob_content(object_store, *new_blob_id)?,
            },
        },
        DecodedOperationKind::RenamePath {
            old_path, new_path, ..
        } => ShowOperation {
            kind: "rename-path",
            paths: vec![
                ShowPathResolution::Path(old_path.clone()),
                ShowPathResolution::Path(new_path.clone()),
            ],
            content: ShowOperationContent::RenamePath {
                author_key_id: require_author_key_id(envelope)?,
            },
        },
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => ShowOperation {
            kind: "change-perm",
            paths: vec![resolve_node_path(*node_id, lifecycle)],
            content: ShowOperationContent::ChangePerm {
                old_mode: *old_mode,
                new_mode: *new_mode,
            },
        },
        DecodedOperationKind::CreateSymlink { path, target, .. } => ShowOperation {
            kind: "create-symlink",
            paths: vec![ShowPathResolution::Path(path.clone())],
            content: ShowOperationContent::CreateSymlink {
                target: target.clone(),
            },
        },
    })
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

/// RFC 142 §6b: keeps the `Option` the object store hands out instead of collapsing it into a bare
/// `Result` the way round 2's own `read_blob` did -- that collapse was the structural cause of
/// treating a content-hash-verified corrupt object the same as a merely-absent one. `Ok(None)` is
/// the only case this function's caller may degrade; every `Err` here is the store affirmatively
/// reporting damage (`object_store.rs`'s own id-recomputation check among them) and must reach the
/// caller as one.
fn read_blob(object_store: &impl ObjectReader, blob_id: ObjectId) -> Result<Option<BlobPayload>> {
    let Some(envelope) = object_store.read_typed(blob_id, ObjectType::Blob)? else {
        return Ok(None);
    };
    BlobPayload::decode_canonical(&envelope.canonical_payload).map(Some)
}

/// RFC 142 §6a/§6b: a blob id a patch payload names is a reference, not a guarantee the object
/// store holds it (§3a) -- absence (`Ok(None)`) degrades to [`ShowBlobContent::Unavailable`], the
/// same "report, do not fail" treatment [`resolve_node_path`] already gives an unresolvable node
/// id, because DC-65 makes a text node's pre-edit blob id absent by design and `show` cannot tell
/// that case apart from a lost object by absence alone. **An `Err` is different**: the object store
/// affirmatively reporting damage (a content-hash mismatch, a type disagreement, a malformed
/// payload, an `Io` failure) is not "unbacked by design" under any cause this crate knows, so it
/// propagates rather than degrading -- round 1 called an intact repository damaged; degrading every
/// `Err` here would have called a damaged one intact, which is worse because it is quieter. A
/// `SNAPSHOT`-kind blob is the same: no file-content operation legitimately names one, so decoding
/// one here is itself an integrity problem, not an absence.
fn show_blob_content(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
) -> Result<ShowBlobContent> {
    match read_blob(object_store, blob_id)? {
        None => Ok(ShowBlobContent::Unavailable { blob_id }),
        Some(blob) => match blob.blob_kind {
            BlobKind::Text => Ok(ShowBlobContent::Text(blob.content)),
            BlobKind::Binary => Ok(ShowBlobContent::Binary {
                blob_id,
                size: blob.declared_size,
            }),
            BlobKind::Snapshot => Err(PrikkError::Integrity(format!(
                "content blob {blob_id} is a SNAPSHOT blob, not file content"
            ))),
        },
    }
}

#[cfg(test)]
mod tests;
