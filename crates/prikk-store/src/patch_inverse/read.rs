//! Read helpers for supported inverse planning.

use std::collections::{BTreeMap, HashSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlockKind, BlockPayload, ObjectEnvelope, ObjectId, ObjectType, RefStatePayload,
};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::refs::RefStore;
use crate::snapshot::SnapshotManifest;

/// Read the current target Block ID for a ref.
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
    // RFC 147 §3c: the fifth resolution site, and the one §3b missed -- this is a *separate*
    // function from `patch_replay/read.rs::current_target_block` despite the identical name and
    // nearly identical body, so fixing that one left `inverse-plan`, `rollback-preview` and
    // `rollback-draft-verify` reporting `object type mismatch: expected block, got tag` for a
    // perfectly valid tag ref. Same shared resolver as the other four; it resolves and never
    // validates, and `single_parent_chain` below is unchanged and still walks what it is given.
    let (block_id, _tag_envelope) = crate::refs::resolve_ref_tip_block(object_store, &ref_state)?;
    Ok(block_id)
}

/// Return the single-parent chain from oldest to newest.
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
                "inverse planning supports single-parent chains only; block has {} parents",
                block.parent_block_ids.len()
            ))
        })?;
        newest_first.push(block_id);
        current = next;
    }
    newest_first.reverse();
    Ok(newest_first)
}

/// The parent inverse planning continues through: mainline only for a `Merge` block (DC-75, same
/// state-derivation category as `block_state.rs`'s replay walk), the sole parent for `Normal`, none
/// for `Root`. `Ok(None)` at genesis is not an error; the outer `None` is reserved for a non-`Merge`
/// block with more than one parent, or a `Merge` block with no valid mainline parent.
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

/// Read and decode a Block payload.
pub(super) fn read_block(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<BlockPayload> {
    let envelope = object_store
        .read_typed(block_id, ObjectType::Block)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Block {block_id}")))?;
    BlockPayload::decode_canonical(&envelope.canonical_payload)
}

/// Read a Patch envelope.
pub(super) fn read_patch(
    object_store: &impl ObjectReader,
    patch_id: ObjectId,
) -> Result<ObjectEnvelope> {
    object_store
        .read_typed(patch_id, ObjectType::Patch)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Patch {patch_id}")))
}

/// Load a snapshot manifest into a path-to-bytes map.
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

/// Read a file-content Blob, returning the derived node kind and bytes. Used to
/// fill `DeleteNode.old_node_kind` when inverting a `CreateFile`.
pub(super) fn read_blob_bytes_with_kind(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
) -> Result<(prikk_object::NodeKind, Vec<u8>)> {
    let envelope = object_store
        .read_typed(blob_id, ObjectType::Blob)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Blob {blob_id}")))?;
    crate::blob_access::decode_file_content_blob_with_kind(&envelope.canonical_payload)
}
