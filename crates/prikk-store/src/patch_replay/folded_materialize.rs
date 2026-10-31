//! Materialize a [`super::FoldedWorktreeBaseline`]'s live file nodes into real bytes (0.56.0 step
//! W2c).
//!
//! **Why this exists.** [`super::resolve_folded_worktree_baseline`] (and the lifecycle fold it
//! wraps) only ever produces a [`NodeLifecycleState`] -- identity only (a `blob_id` per node),
//! never bytes, by design (cheap replay). `commit`'s own authoring (`node_authoring.rs`) only ever
//! needed bytes for *one* path at a time (the one the worktree disagrees with), and built a small,
//! per-path fallback for that (`current_text_for_node`/`read_file_blob_bytes_if_present`). `branch
//! switch <the current branch>`'s self-switch resync needs the opposite shape: every live file
//! node's bytes at once, to write a whole worktree. This module is that whole-manifest version,
//! built from the same primitives (`TextCache`, `materialize_edited_text_anchored`,
//! `decode_file_content_blob`) rather than a new mechanism -- deliberately **not** sharing
//! `node_authoring.rs`'s own two helper functions verbatim: that module's `AuthorError` and
//! `commit_boundary`-local visibility are not meant to leak to `branch_switch` (a different crate
//! module entirely, outside `commit_boundary`), and duplicating the ~20 lines of per-node fallback
//! logic here is lower-risk than widening an already-reviewed authoring helper's visibility for one
//! new caller.
//!
//! **The gap this closes**, traced in `branch_switch.rs`: `switch_branch`'s `target_manifest` comes
//! only from `replay_for_verified_worktree_write` (the *sealed* tip's own block-chain walk) --
//! confirmed by reading that call site, which never touches `active_replay` at all. A producer that
//! moves only the **queued fold** without touching the sealed block (`rollback-draft
//! --append-inverse`) is therefore invisible to materialization even once the *comparison*
//! (`worktree_baseline.rs`, this same round) correctly detects it as stale: the self-switch resync
//! would still only ever write the sealed tip's files, never the fold on top. This module gives
//! `switch_branch` a manifest that **is** sealed-plus-fold, for the one case that needs it (a
//! self-switch, where refusal 2 already guarantees the active WAL, if non-empty, is owned by the
//! very ref being switched to).

use std::collections::BTreeMap;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, NodeKind, ObjectId};

use crate::foundation::layout::RepositoryLayout;
use crate::lifecycle_cache::replay::TextCache;
use crate::node::node_lifecycle::{NodeContent, NodeLifecycleState};
use crate::object_store::ObjectReader;

use super::{ReplayManifest, ReplayManifestEntry};

/// A stored file blob's bytes, or `None` when no object exists under `blob_id` -- the case for a
/// `TextFile` node whose most recent touching operation was an `EditText` (DC-65: its content
/// identity is a hash of the reconstructed text, not necessarily a stored object).
fn read_file_blob_bytes_if_present(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
) -> Result<Option<Vec<u8>>> {
    let Some(envelope) = object_store.read_object(blob_id)? else {
        return Ok(None);
    };
    crate::blob_access::decode_file_content_blob(&envelope.canonical_payload).map(Some)
}

/// One live `TextFile` node's current bytes: the queue's own materialized text first (every node
/// `apply_queued_patch_envelopes` touched while folding -- already resolved by the time this runs,
/// since `resolve_folded_worktree_baseline` is always called first), then a direct stored object,
/// then the lineage-anchored replay walk -- the exact fallback order `node_authoring.rs::
/// current_text_for_node` already established for the single-path case, applied here per node.
fn materialize_node_text(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    node_id: NodeId,
    blob_id: ObjectId,
    path: &str,
    lineage: Option<(ObjectId, ObjectId)>,
    queue_text_cache: &TextCache,
) -> Result<Vec<u8>> {
    if let Some(text) = queue_text_cache.get(&node_id) {
        return Ok(text.clone());
    }
    if let Some(bytes) = read_file_blob_bytes_if_present(object_store, blob_id)? {
        return Ok(bytes);
    }
    let (baseline_block_id, horizon_id) = lineage.ok_or_else(|| {
        PrikkError::Integrity(format!(
            "{path}: text node baseline blob {blob_id} is missing and no lineage is available to \
             materialize it (a live node implies a published baseline)"
        ))
    })?;
    crate::lifecycle_cache::materialize_edited_text_anchored(
        layout,
        object_store,
        baseline_block_id,
        horizon_id,
        node_id,
        blob_id,
    )?
    .ok_or_else(|| {
        PrikkError::Integrity(format!(
            "{path}: text node baseline blob {blob_id} is missing and could not be materialized \
             from its edit history"
        ))
    })
}

/// Materialize every live file node in `state` into a [`ReplayManifest`] with real bytes -- the
/// same shape `replay_for_verified_worktree_write`'s sealed-only walk already produces, but over a
/// state that may also carry the active queue's own fold on top (0.56.0 step W2c). Symlink nodes are
/// skipped (out of scope for worktree materialization today, same as `split_baseline`'s own
/// symlink-tracking-only treatment in `node_authoring.rs`).
pub(crate) fn materialize_folded_manifest(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    state: &NodeLifecycleState,
    lineage: Option<(ObjectId, ObjectId)>,
    queue_text_cache: &TextCache,
) -> Result<ReplayManifest> {
    let mut by_path: BTreeMap<String, ReplayManifestEntry> = BTreeMap::new();
    for (node_id, node) in state.live_nodes() {
        let NodeContent::File { blob_id, mode } = &node.content else {
            continue;
        };
        let path = node.path.clone();
        let bytes = match node.kind {
            NodeKind::TextFile => materialize_node_text(
                layout,
                object_store,
                *node_id,
                *blob_id,
                path.as_str(),
                lineage,
                queue_text_cache,
            )?,
            NodeKind::BinaryFile => read_file_blob_bytes_if_present(object_store, *blob_id)?
                .ok_or_else(|| {
                    PrikkError::Integrity(format!(
                        "{}: binary node baseline blob {blob_id} is missing",
                        path.as_str()
                    ))
                })?,
            NodeKind::Symlink => continue,
        };
        by_path.insert(
            path.as_str().to_string(),
            ReplayManifestEntry {
                node_id: Some(*node_id),
                path,
                bytes,
                mode: *mode,
                kind: Some(node.kind),
                blob_id: (node.kind == NodeKind::BinaryFile).then_some(*blob_id),
            },
        );
    }
    Ok(ReplayManifest {
        files: by_path.into_values().collect(),
    })
}
