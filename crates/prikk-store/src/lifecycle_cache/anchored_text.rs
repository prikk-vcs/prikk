//! Text from a replay-verified anchor (RFC 136 increment 2c).
//!
//! A `TextFile` node's `blob_id` after any `EditText` is a content identity, not necessarily a stored Blob (the
//! DC-65 invariant). A consumer that needs the node's bytes -- the one-block step of the DC-64 cache, `commit`
//! authoring an edit, `merge-evidence` reading a baseline, `diff` -- used to get them by replaying the whole
//! lineage from genesis. This module gets them from the nearest snapshot instead: read the node's text from
//! that snapshot's content Blobs, then carry it forward through the (at most `CHECKPOINT_CADENCE`) blocks after
//! it, editing only the nodes asked for.
//!
//! **What it is not.** It builds no lifecycle state. RFC 136 §10.3c ruling 3 governs an anchored *state* -- one
//! whose history fields (`seen_ids`, `latest_tombstone_by_id`) must equal full replay's. Nothing here supplies
//! either; a caller keeps the state it already has (the cached one, the baseline's) and takes *text* from here.
//!
//! **Two rules, each its own layer.**
//! 1. **Verified anchors only.** The nearest block at or before the starting block that carries a snapshot *and*
//!    is in this repository's replay-verified record (`verified_blocks`, §10.3c ruling 2). A snapshot on an
//!    unrecorded block is not consulted, however well it validates.
//! 2. **Self-certifying text.** A text is returned only if its content id equals the id the caller says that node
//!    has. A wrong anchor, a wrong fold or a stale cache cannot produce a text that is used.
//!
//! **Failure is classified.** No anchor in reach, an unreadable block, nothing wanted: `None`, silently -- the
//! ordinary case for a short history, and the full replay a caller then runs reports any real fault itself. But a
//! **verified** anchor whose manifest fails validation, whose forward blocks cannot be followed, or whose text
//! fails rule 2 is an integrity signal: it is recorded for the CLI to name (`anchor_fallback`), and the caller
//! still falls back, because the answer stays correct.

use std::collections::{BTreeMap, BTreeSet};

use prikk_object::{BlobKind, BlockKind, BlockPayload, NodeId, ObjectId};

use super::replay::{self, TextCache};
use super::{BlobContentResolver, StoreBackedResolver};
use crate::anchor_fallback::{SnapshotAnchorFallback, record_anchor_fallback};
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::snapshot::{CHECKPOINT_CADENCE, validate_snapshot_manifest};
use crate::state_root::StateRootContent;
use crate::text_span::text_blob_id;
use crate::verified_blocks::load_verified_blocks;

/// The block a state derivation continues through: the mainline of a `Merge`, the sole parent otherwise.
fn derivation_parent(block: &BlockPayload) -> Option<ObjectId> {
    match (block.kind, block.parent_block_ids.as_slice()) {
        (BlockKind::Merge, _) => block.mainline_parent_id,
        (_, [parent]) => Some(*parent),
        _ => None,
    }
}

/// The text each of `wanted` has **at `from_block`**, keyed by node. `wanted` maps a node to the content id it must
/// have there; every returned text hashes to it (rule 2), and the call succeeds only if it holds for all of them.
/// `None`: no verified anchor within reach, or the anchor could not supply every text (the second case is
/// recorded for the CLI to name).
pub(crate) fn anchored_texts(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    from_block: ObjectId,
    wanted: &BTreeMap<NodeId, ObjectId>,
) -> Option<TextCache> {
    #[cfg(test)]
    if ANCHORING_SUPPRESSED.with(std::cell::Cell::get) {
        return None;
    }
    if wanted.is_empty() {
        return None;
    }
    let verified = load_verified_blocks(layout);
    if verified.is_empty() {
        return None;
    }
    // Walk back to the nearest verified snapshot; the blocks passed on the way are the ones to carry through.
    let mut after_anchor: Vec<BlockPayload> = Vec::new();
    let mut current = from_block;
    let (anchor_id, anchor) = loop {
        let payload = replay::read_block(reader, current).ok()?;
        if payload.snapshot_blob_ref.is_some() && verified.contains(&current) {
            break (current, payload);
        }
        if after_anchor.len() >= CHECKPOINT_CADENCE as usize {
            return None;
        }
        let parent = derivation_parent(&payload);
        after_anchor.push(payload);
        current = parent?;
    };
    after_anchor.reverse();

    let refuse = |finding: String| {
        record_anchor_fallback(SnapshotAnchorFallback::for_text(anchor_id, finding));
        None
    };
    let manifest = match validate_snapshot_manifest(reader, anchor_id, &anchor) {
        Ok(Some(manifest)) => manifest,
        Ok(None) => return refuse("its snapshot reference names no manifest".to_string()),
        Err(err) => return refuse(err.to_string()),
    };
    let wanted_nodes: BTreeSet<NodeId> = wanted.keys().copied().collect();
    let resolver = StoreBackedResolver::new(reader);
    let mut seeds = TextCache::new();
    for entry in &manifest.entries {
        if !wanted_nodes.contains(&entry.node_id) {
            continue;
        }
        if let StateRootContent::Blob(blob_id) = &entry.content {
            if let Ok(Some((BlobKind::Text, bytes))) = resolver.blob_content(blob_id) {
                seeds.insert(entry.node_id, bytes);
            }
        }
    }
    let texts = match replay::materialize_wanted_text_forward(
        reader,
        seeds,
        &after_anchor,
        &wanted_nodes,
    ) {
        Ok(Some(texts)) => texts,
        Ok(None) => {
            return refuse(
                "the blocks after it could not be followed for the text asked for".to_string(),
            );
        }
        Err(err) => return refuse(err.to_string()),
    };
    let mut certified = TextCache::new();
    for (node_id, expected) in wanted {
        let Some(text) = texts.get(node_id) else {
            return refuse(format!(
                "it yielded no text for the file whose content id is {expected}"
            ));
        };
        if text_blob_id(text).ok().as_ref() != Some(expected) {
            return refuse(format!(
                "the text it yielded does not hash to the content id {expected} recorded for the file"
            ));
        }
        certified.insert(*node_id, text.clone());
    }
    #[cfg(test)]
    ANCHORED_TEXT_USES.with(|uses| uses.set(uses.get() + 1));
    Some(certified)
}

#[cfg(test)]
std::thread_local! {
    static ANCHORING_SUPPRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static ANCHORED_TEXT_USES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Test-only: run `body` with anchoring suppressed on this thread, so a test can compare any output with the
/// same output computed by full replay. `cfg(test)`, like `patch_replay::anchor::without_anchoring_for_test`:
/// compiled into this crate's own unit tests only.
#[cfg(test)]
pub(crate) fn without_anchoring_for_test<T>(body: impl FnOnce() -> T) -> T {
    ANCHORING_SUPPRESSED.with(|flag| flag.set(true));
    let result = body();
    ANCHORING_SUPPRESSED.with(|flag| flag.set(false));
    result
}

/// Test-only: how many times this thread's anchored-text requests were answered from an anchor.
#[cfg(test)]
pub(crate) fn anchored_text_uses_for_test() -> usize {
    ANCHORED_TEXT_USES.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests;

/// The fixture helpers a call-site control in another module shares with `tests`.
#[cfg(test)]
pub(crate) mod tests_support {
    pub(crate) use super::tests::{edited_a, write_tip_worktree};
}
