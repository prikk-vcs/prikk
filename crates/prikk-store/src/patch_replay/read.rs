//! Object-store reading helpers for patch replay: block-chain walking and blob/patch/snapshot
//! loading. Split out of `patch_replay.rs` (DC-58) — no behaviour change, all items moved verbatim.

use std::collections::{BTreeMap, HashSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, NodeId, NodeKind, ObjectEnvelope, ObjectId,
    ObjectType,
};

use crate::object_store::ObjectReader;
use crate::path::RepoPath;
use crate::snapshot::SnapshotFile;

use super::apply::ReplayLiveNode;
use super::{ReplayManifest, ReplayManifestEntry};

/// The single-parent ancestry of `target`, oldest first.
///
/// `None` where the chain forks: a block with more than one parent has no single-parent chain, and
/// guessing one would silently pick a side of a merge.
pub(crate) fn single_parent_chain(
    object_store: &impl ObjectReader,
    target: ObjectId,
) -> Result<Vec<ObjectId>> {
    let mut newest_first = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(target);
    while let Some(block_id) = current {
        if !seen.insert(block_id) {
            return Err(PrikkError::Integrity(format!(
                "block parent chain contains a cycle at {block_id}"
            )));
        }
        let block = read_block(object_store, block_id)?;
        let next = mainline_or_sole_parent(&block).ok_or_else(|| {
            PrikkError::UnsupportedObjectType(format!(
                "patch replay supports only single-parent chains; block {block_id} has {} parents",
                block.parent_block_ids.len()
            ))
        })?;
        newest_first.push(block_id);
        current = next;
    }
    newest_first.reverse();
    Ok(newest_first)
}

/// The parent materialization continues through: mainline only for a `Merge` block (DC-75, same
/// state-derivation category as `block_state.rs`'s replay walk), the sole parent for `Normal`, none
/// for `Root`. `Ok(None)` at genesis is not an error; the outer `Err` return is reserved for a
/// non-`Merge` block with more than one parent, or a `Merge` block with no valid mainline parent —
/// both indicate a shape this walk cannot follow, distinguished from `Ok(None)` by the caller.
fn mainline_or_sole_parent(block: &BlockPayload) -> Option<Option<ObjectId>> {
    if block.kind == BlockKind::Merge {
        let mainline = block.mainline_parent_id?;
        if !block.parent_block_ids.contains(&mainline) {
            return None;
        }
        return Some(Some(mainline));
    }
    match block.parent_block_ids.as_slice() {
        [] => Some(None),
        [parent] => Some(Some(*parent)),
        _ => None,
    }
}

/// Read and decode one Block, or fail with an integrity error naming the missing id.
pub(crate) fn read_block(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<BlockPayload> {
    let envelope = object_store
        .read_typed(block_id, ObjectType::Block)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Block {block_id}")))?;
    BlockPayload::decode_canonical(&envelope.canonical_payload)
}

/// Read one Patch envelope, or fail with an integrity error naming the missing id.
///
/// Returns the envelope rather than a decoded payload: the signature is checked against the
/// envelope's own bytes, so a caller that needs to verify must not be handed a decoded copy.
pub(crate) fn read_patch(
    object_store: &impl ObjectReader,
    patch_id: ObjectId,
) -> Result<ObjectEnvelope> {
    object_store
        .read_typed(patch_id, ObjectType::Patch)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Patch {patch_id}")))
}

/// Replay state seeded from a loaded snapshot (RFC 136 §10.1a): path-keyed bytes and node-keyed live
/// nodes, carrying each entry's real node id, kind, mode and Blob id. Used by every replay walk that
/// meets a snapshot, which then skips that block's patches.
pub(crate) fn replay_state_from_snapshot(
    snapshot: Vec<SnapshotFile>,
) -> (BTreeMap<String, Vec<u8>>, BTreeMap<NodeId, ReplayLiveNode>) {
    let mut files = BTreeMap::new();
    let mut live_nodes = BTreeMap::new();
    for file in snapshot {
        live_nodes.insert(
            file.node_id,
            ReplayLiveNode {
                path: file.path.clone(),
                kind: file.kind,
                mode: file.mode,
                blob_id: file.blob_id,
            },
        );
        files.insert(file.path, file.bytes);
    }
    (files, live_nodes)
}

/// Build the final, mode-aware replay manifest (DC-73). `files` and `live_nodes` are one-to-one:
/// every operation that adds, moves or removes a path does the same to its live node, and a snapshot
/// seeds both together ([`replay_state_from_snapshot`]). A path with no live node is therefore a
/// replay defect, refused as `Integrity` rather than given a guessed mode.
pub(crate) fn files_to_replay_manifest(
    files: BTreeMap<String, Vec<u8>>,
    live_nodes: &BTreeMap<NodeId, ReplayLiveNode>,
) -> Result<ReplayManifest> {
    let live_by_path: BTreeMap<&str, (&NodeId, &ReplayLiveNode)> = live_nodes
        .iter()
        .map(|(node_id, node)| (node.path.as_str(), (node_id, node)))
        .collect();
    let mut entries = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        let (node_id, live) = live_by_path.get(path.as_str()).copied().ok_or_else(|| {
            PrikkError::Integrity(format!("replayed path {path} has no live node"))
        })?;
        entries.push(ReplayManifestEntry {
            node_id: Some(*node_id),
            path: RepoPath::parse(&path)?,
            bytes,
            mode: live.mode,
            kind: Some(live.kind),
            blob_id: (live.kind == NodeKind::BinaryFile).then_some(live.blob_id),
        });
    }
    Ok(ReplayManifest { files: entries })
}

pub(super) fn read_blob_bytes_with_kind(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
) -> Result<(NodeKind, Vec<u8>)> {
    let envelope = object_store
        .read_typed(blob_id, ObjectType::Blob)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Blob {blob_id}")))?;
    let blob = BlobPayload::decode_canonical(&envelope.canonical_payload)?;
    if blob.blob_kind == BlobKind::Snapshot {
        return Err(PrikkError::Integrity(
            "file content reference points to a SNAPSHOT blob".to_string(),
        ));
    }
    let kind = NodeKind::from_file_blob_kind(blob.blob_kind)?;
    Ok((kind, blob.content))
}
