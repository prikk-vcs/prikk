//! Operation-application logic for patch replay: folding one decoded operation into the in-memory
//! file/live-node state. Split out of `patch_replay.rs` (DC-58) — no behaviour change, all items
//! moved verbatim.

use std::collections::BTreeMap;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, NodeKind, ObjectId, text_span_hash};

use crate::object_store::ObjectReader;
use crate::path::RepoPath;
use crate::text_span;

use super::PatchReplayDeletedFile;
use super::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation, ensure_apply_supported,
};
use super::read::read_blob_bytes_with_kind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReplayLiveNode {
    pub(super) path: String,
    pub(super) kind: NodeKind,
    /// Current mode bits, as recorded by the operation that most recently set them
    /// (`CreateFile`, then any `ChangePerm`) — DC-73. Threaded into materialization so a
    /// checked-out file's permission bits match what was authored, not the anchored write
    /// primitive's create-time default.
    pub(super) mode: u32,
    /// The blob id backing this node's *current* content, as recorded by the operation that most
    /// recently set it (`CreateFile`, then any `ReplaceBinary`) — RFC 143 §5. Meaningful only when
    /// `kind` is `BinaryFile`: a `TextFile` node's tracked id goes stale the moment an `EditText`
    /// touches it (DC-65 — the pre-edit blob is deliberately never rewritten), so RFC 143's own
    /// content report never reads this field for text. `BinaryFile` nodes never take that path
    /// (`EditText` is text-only; binary content only ever changes via `ReplaceBinary`, which always
    /// updates this to a real, current, apply-validated blob), so for them this id never goes
    /// stale.
    pub(super) blob_id: ObjectId,
}

pub(super) fn apply_decoded_operation(
    object_store: &impl ObjectReader,
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &mut BTreeMap<NodeId, ReplayLiveNode>,
    deleted_files: &mut BTreeMap<String, PatchReplayDeletedFile>,
    operation: DecodedPatchOperation,
) -> Result<()> {
    // Erratum P1: decode success does not imply applicability. The apply-supported
    // subset is gated here as the single source of truth; the match below only needs
    // to handle the kinds the gate admits.
    ensure_apply_supported(&operation)?;
    match operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => {
            if files.contains_key(&path) {
                return Err(PrikkError::Integrity(format!(
                    "CreateFile would overwrite existing path {path}"
                )));
            }
            if live_nodes.contains_key(&node_id) {
                return Err(PrikkError::Integrity(
                    "CreateFile would introduce an already-live node_id".to_string(),
                ));
            }
            let (kind, bytes) = read_blob_bytes_with_kind(object_store, blob_id)?;
            deleted_files.remove(&path);
            files.insert(path.clone(), bytes);
            live_nodes.insert(
                node_id,
                ReplayLiveNode {
                    path,
                    kind,
                    mode,
                    blob_id,
                },
            );
        }
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage:
                DecodedDeletePreimage::File {
                    old_node_kind,
                    old_blob_id,
                    old_mode: _,
                },
        } => {
            let old_bytes = files.get(&path).ok_or_else(|| {
                PrikkError::Integrity(format!("DeleteNode path is absent: {path}"))
            })?;
            crate::blob_access::ensure_blob_matches_node_kind(
                old_bytes,
                old_blob_id,
                old_node_kind,
            )?;
            let repo_path = RepoPath::parse(&path)?;
            let deleted = PatchReplayDeletedFile {
                path: repo_path,
                old_blob_id,
                old_bytes: old_bytes.clone(),
            };
            files.remove(&path);
            if let Some(live) = live_nodes.remove(&node_id) {
                if live.path != path {
                    return Err(PrikkError::Integrity(format!(
                        "DeleteNode path {path} does not match live node path {}",
                        live.path
                    )));
                }
                if live.kind != old_node_kind {
                    return Err(PrikkError::Integrity(
                        "DeleteNode old_node_kind does not match live node kind".to_string(),
                    ));
                }
            }
            deleted_files.insert(path, deleted);
        }
        DecodedOperationKind::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            replacement_text,
            old_span_text,
            left_anchor_len,
            right_anchor_len,
        } => {
            apply_edit_text(
                files,
                live_nodes,
                node_id,
                &span_id,
                &old_span_hash,
                &left_anchor_hash,
                &right_anchor_hash,
                &replacement_text,
                &old_span_text,
                left_anchor_len,
                right_anchor_len,
            )?;
        }
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => {
            apply_replace_binary(
                object_store,
                files,
                live_nodes,
                node_id,
                old_blob_id,
                new_blob_id,
            )?;
        }
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => {
            apply_change_perm(live_nodes, node_id, old_mode, new_mode)?;
        }
        _ => {
            return Err(PrikkError::MalformedData(
                "apply_decoded_operation received an operation kind ensure_apply_supported \
                 should have refused"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Apply a `ReplaceBinary` operation (DC-73): fold the node's content forward from `old_blob_id` to
/// `new_blob_id`. Node-addressed with no path field — `live_nodes` supplies the current path.
fn apply_replace_binary(
    object_store: &impl ObjectReader,
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &mut BTreeMap<NodeId, ReplayLiveNode>,
    node_id: NodeId,
    old_blob_id: ObjectId,
    new_blob_id: ObjectId,
) -> Result<()> {
    let live = live_nodes.get(&node_id).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "ReplaceBinary target node {} is not live",
            hex32(node_id.as_bytes())
        ))
    })?;
    if live.kind != NodeKind::BinaryFile {
        return Err(PrikkError::Integrity(format!(
            "ReplaceBinary target node {} is {:?}, not BinaryFile",
            hex32(node_id.as_bytes()),
            live.kind
        )));
    }
    let current_bytes = files.get(&live.path).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "ReplaceBinary target path {} is absent for live node {}",
            live.path,
            hex32(node_id.as_bytes())
        ))
    })?;
    crate::blob_access::ensure_blob_matches_node_kind(current_bytes, old_blob_id, live.kind)?;
    let (new_kind, new_bytes) = read_blob_bytes_with_kind(object_store, new_blob_id)?;
    if new_kind != NodeKind::BinaryFile {
        return Err(PrikkError::Integrity(format!(
            "ReplaceBinary new blob {new_blob_id} is not a binary-file blob"
        )));
    }
    let path = live.path.clone();
    files.insert(path, new_bytes);
    // Safe to unwrap: the `live_nodes.get(&node_id)` lookup above already proved this key exists,
    // and nothing between there and here removes it.
    if let Some(live) = live_nodes.get_mut(&node_id) {
        live.blob_id = new_blob_id;
    }
    Ok(())
}

/// Apply a `ChangePerm` operation (DC-73): fold the node's mode forward from `old_mode` to
/// `new_mode`. Node-addressed with no path field or content change — only `live_nodes`' recorded
/// mode is affected; materialization reads it from there.
fn apply_change_perm(
    live_nodes: &mut BTreeMap<NodeId, ReplayLiveNode>,
    node_id: NodeId,
    old_mode: u32,
    new_mode: u32,
) -> Result<()> {
    let live = live_nodes.get_mut(&node_id).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "ChangePerm target node {} is not live",
            hex32(node_id.as_bytes())
        ))
    })?;
    if live.mode != old_mode {
        return Err(PrikkError::Integrity(format!(
            "ChangePerm old_mode does not match live node {}'s current mode",
            hex32(node_id.as_bytes())
        )));
    }
    live.mode = new_mode;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_edit_text(
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &BTreeMap<NodeId, ReplayLiveNode>,
    node_id: NodeId,
    span_id: &[u8; 32],
    old_span_hash: &[u8; 32],
    left_anchor_hash: &[u8; 32],
    right_anchor_hash: &[u8; 32],
    replacement_text: &[u8],
    old_span_text: &[u8],
    left_anchor_len: Option<u32>,
    right_anchor_len: Option<u32>,
) -> Result<()> {
    if text_span_hash(old_span_text) != *old_span_hash {
        return Err(PrikkError::Integrity(format!(
            "EditText hash verification failed before localization for node {} span {}",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        )));
    }
    let live = live_nodes.get(&node_id).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "EditText failed before blob load: node {} is not live (span {})",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        ))
    })?;
    if live.kind != NodeKind::TextFile {
        return Err(PrikkError::Integrity(format!(
            "EditText failed before blob load: node {} is {:?}, not TextFile (span {})",
            hex32(node_id.as_bytes()),
            live.kind,
            hex32(span_id)
        )));
    }
    let current_text = files.get(&live.path).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "EditText failed before blob load: live node {} path {} is absent (span {})",
            hex32(node_id.as_bytes()),
            live.path,
            hex32(span_id)
        ))
    })?;
    if core::str::from_utf8(current_text).is_err() {
        return Err(PrikkError::Integrity(format!(
            "EditText failed during UTF-8 validation for node {} span {}",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        )));
    }
    let (start, end) = text_span::resolve_text_span(
        current_text,
        old_span_text,
        left_anchor_hash,
        right_anchor_hash,
        span_id,
        node_id,
        old_span_hash,
        left_anchor_len,
        right_anchor_len,
    )
    .map_err(|reason| {
        PrikkError::Integrity(format!(
            "EditText failed during localization for node {} span {}: {reason}",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        ))
    })?;
    let new_text =
        text_span::splice_text(current_text, start, end, replacement_text).map_err(|err| {
            PrikkError::Integrity(format!(
                "EditText failed during splice for node {} span {}: {err}",
                hex32(node_id.as_bytes()),
                hex32(span_id)
            ))
        })?;
    files.insert(live.path.clone(), new_text);
    Ok(())
}

fn hex32(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
