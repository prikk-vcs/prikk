//! Object-store reading helpers for patch replay: block-chain walking and blob/patch/snapshot
//! loading. Split out of `patch_replay.rs` (DC-58) — no behaviour change, all items moved verbatim.

use std::collections::{BTreeMap, HashSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, NodeId, NodeKind, ObjectEnvelope, ObjectId,
    ObjectType, RefStatePayload,
};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::path::RepoPath;
use crate::refs::RefStore;
use crate::snapshot::{SnapshotEntry, SnapshotManifest};

use super::apply::ReplayLiveNode;
use super::{ReplayManifest, ReplayManifestEntry};

pub(super) fn current_target_block(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
) -> Result<ObjectId> {
    let ref_store = RefStore::new(layout.clone());
    let ref_state_id = ref_store
        .read_current_ref_state_id(ref_name)?
        .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} is not published")))?;
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {ref_name} points to missing RefState {ref_state_id}"
            ))
        })?;
    let ref_state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if ref_state.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "RefState name mismatch: expected {ref_name}, got {}",
            ref_state.ref_name
        )));
    }
    Ok(ref_state.target_object_id)
}

pub(super) fn single_parent_chain(
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

pub(super) fn read_block(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<BlockPayload> {
    let envelope = object_store
        .read_typed(block_id, ObjectType::Block)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Block {block_id}")))?;
    BlockPayload::decode_canonical(&envelope.canonical_payload)
}

pub(super) fn read_patch(
    object_store: &impl ObjectReader,
    patch_id: ObjectId,
) -> Result<ObjectEnvelope> {
    object_store
        .read_typed(patch_id, ObjectType::Patch)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Patch {patch_id}")))
}

pub(super) fn load_snapshot_files(
    object_store: &impl ObjectReader,
    snapshot_blob_ref: ObjectId,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let envelope = object_store
        .read_typed(snapshot_blob_ref, ObjectType::Blob)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!("missing snapshot Blob {snapshot_blob_ref}"))
        })?;
    let snapshot_content = crate::blob_access::decode_snapshot_blob(&envelope.canonical_payload)?;
    let manifest = SnapshotManifest::decode(&snapshot_content)?;
    let mut files = BTreeMap::new();
    for entry in manifest.files {
        files.insert(entry.path.as_str().to_string(), entry.bytes);
    }
    Ok(files)
}

pub(super) fn files_to_manifest(files: BTreeMap<String, Vec<u8>>) -> Result<SnapshotManifest> {
    let mut entries = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        entries.push(SnapshotEntry {
            path: RepoPath::parse(&path)?,
            bytes,
        });
    }
    Ok(SnapshotManifest { files: entries })
}

/// A snapshot-seeded live node's mode: legacy format-1 snapshot blobs (`SnapshotEntry`) carry no
/// mode field at all (see `ReplayManifestEntry`'s doc comment), so a path that entered `files`
/// through `load_snapshot_files` and was never subsequently touched by a `CreateFile`/`ChangePerm`
/// in the replayed window has no recorded mode to thread through. `open_new_regular`'s create-time
/// default was `0o600` before DC-73 (`fsutil/anchored/regular.rs`) and every such path materialized
/// at that mode unconditionally; this preserves that exact prior behavior rather than manufacturing
/// a mode the snapshot bytes never recorded.
const SNAPSHOT_SEEDED_FALLBACK_MODE: u32 = 0o600;

/// Build the final, mode-aware replay manifest (DC-73). `files` and `live_nodes` are one-to-one for
/// every path created or touched by a node-addressed operation within the current replay window —
/// but a path seeded by a snapshot block and never subsequently touched has no live-node entry at
/// all (`replay_supported_patch_chain` clears `live_nodes` at each snapshot boundary without
/// repopulating it from the loaded snapshot). `SNAPSHOT_SEEDED_FALLBACK_MODE` covers that case.
pub(super) fn files_to_replay_manifest(
    files: BTreeMap<String, Vec<u8>>,
    live_nodes: &BTreeMap<NodeId, ReplayLiveNode>,
) -> Result<ReplayManifest> {
    let modes_by_path: BTreeMap<&str, u32> = live_nodes
        .values()
        .map(|node| (node.path.as_str(), node.mode))
        .collect();
    let mut entries = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        let mode = modes_by_path
            .get(path.as_str())
            .copied()
            .unwrap_or(SNAPSHOT_SEEDED_FALLBACK_MODE);
        entries.push(ReplayManifestEntry {
            path: RepoPath::parse(&path)?,
            bytes,
            mode,
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
