//! The state a `seal` continues from, derived from the nearest **authenticated snapshot** instead of by walking the
//! whole lineage (RFC 159, accepted 2026-09-26).
//!
//! [`anchored_parent_state`] returns exactly what `resolved_parent_state` returns -- the parent's lifecycle state and its
//! carried text cache -- or `None`, in which case the caller runs today's full walk. **Every doubt is `None`.** Nothing
//! here can make a seal fail that the full walk would not also fail, and nothing here can make one succeed that the
//! full walk would refuse, because the full walk is what runs whenever this is not certain.
//!
//! 1. **The anchor** is the nearest block on the derivation line that the **anchor-trust function** admits
//!    (`anchor_trust::AnchorTrust::admit`): in the replay-verified record, signed by an adopted maintainer key, its
//!    snapshot passing `validate_snapshot_manifest`, and at most 63 blocks behind the parent. A recorded anchor whose
//!    manifest or signature fails is *named* (`record_anchor_fallback`) and the full walk runs; any other unusable
//!    candidate is silent.
//! 2. **Live state** is the snapshot's entries, seeded through `seed_live_node` (the same gates a create passes).
//! 3. **History fields** (`latest_tombstone_by_id`, `seen_ids`) come from an id-only walk of the anchor and every block
//!    below it (`replay::id_only_tombstones`), which also re-reads and re-validates each block (present, a Block,
//!    schema 2, decodes, shape-valid, no cycle, reaches genesis) and each patch (present, a Patch, an admitted schema,
//!    decodes) on the way -- the checks that are (b) in RFC 159's classification. A snapshot carries no tombstones.
//! 4. **The blocks after the anchor** are folded with the ordinary fold and each one's state root is recomputed and
//!    compared with its signed root, exactly as `verify_v2_lineage_roots` does. A mismatch is `None`.
//!
//! **What it does not re-check below the anchor** -- operation effects, state validity and roots (class (a): the
//! record's writers ran them on this whole ancestry) and the *content* of blobs no live node names (C1, accepted: a
//! state root commits to blob ids, never content; `verify` still reports damage) -- is why the anchor must be
//! authenticated: the record alone decides nothing.
//!
//! The text cache starts empty. A snapshot names a stored blob for every file (`write_checkpoint_snapshot` stores each
//! one, `validate_snapshot_manifest` checks each is present), so an `EditText` on a node no earlier block in the fold
//! has edited reads its current blob, and every later one hits the cache the fold fills.

use std::collections::BTreeSet;

use prikk_object::{BlockPayload, ObjectId, ObjectType};

use super::{
    NodeLifecycleState, TextCache, apply_one_block_with_text_cache, compute_state_root,
    entries_from_state, state_derivation_parent, validate_block_v2_shape,
};
use crate::anchor_fallback::{SnapshotAnchorFallback, record_anchor_fallback};
use crate::anchor_trust::{Admission, AnchorSite, AnchorTrust, MAX_ANCHOR_DISTANCE};
use crate::lifecycle_cache::replay::id_only_tombstones;
use crate::node::node_lifecycle::{LiveNode, NodeContent};
use crate::object_store::ObjectReader;
use crate::state_root::StateRootContent;

/// What an anchored derivation produced.
pub(super) struct AnchoredParent {
    pub(super) state: NodeLifecycleState,
    pub(super) text_cache: TextCache,
    /// The blocks after the anchor whose roots this derivation recomputed and matched.
    pub(super) folded: Vec<ObjectId>,
    /// Blocks the id-only walk read: the anchor and everything below it.
    pub(super) id_only_blocks: usize,
}

/// A block read as `validate_v2_lineage` reads one: present, a Block, schema 2, decodes, shape-valid.
fn read_checked_block(reader: &impl ObjectReader, block_id: ObjectId) -> Option<BlockPayload> {
    let envelope = reader.read_object(block_id).ok()??;
    if envelope.object_type != ObjectType::Block || envelope.schema_version != 2 {
        return None;
    }
    let payload = BlockPayload::decode_canonical(&envelope.canonical_payload).ok()?;
    validate_block_v2_shape(&payload).ok()?;
    Some(payload)
}

/// See the module documentation. `parent` is the block a new block is sealed on (its state-derivation parent); `site`
/// names the caller for the per-site controls.
pub(super) fn anchored_parent_state(
    layout: &crate::RepositoryLayout,
    reader: &impl ObjectReader,
    parent: ObjectId,
    site: AnchorSite,
) -> Option<AnchoredParent> {
    let trust = AnchorTrust::load(layout);
    if !trust.has_record() {
        return None;
    }

    // 1. Walk back to the nearest admitted anchor; the blocks passed on the way are the ones to fold.
    let mut after_anchor: Vec<(ObjectId, BlockPayload)> = Vec::new();
    let mut visited = BTreeSet::new();
    let mut current = parent;
    let (anchor, manifest) = loop {
        if !visited.insert(current) {
            return None;
        }
        let payload = read_checked_block(reader, current)?;
        if payload.snapshot_blob_ref.is_some() {
            match trust.admit(reader, current, after_anchor.len(), site) {
                Admission::Usable(manifest) => break (payload, manifest),
                Admission::Signal(finding) => {
                    record_anchor_fallback(SnapshotAnchorFallback::for_state(current, finding));
                    return None;
                }
                Admission::NotUsable => {}
            }
        }
        if after_anchor.len() >= MAX_ANCHOR_DISTANCE {
            return None;
        }
        let next = state_derivation_parent(&payload)?;
        after_anchor.push((current, payload));
        current = next;
    };
    after_anchor.reverse();

    // 2. The blocks from genesis to the anchor, for the id-only walk. Reaching genesis is required: a lineage that
    //    ends anywhere else is the full walk's to report.
    let mut below: Vec<BlockPayload> = vec![anchor.clone()];
    let mut cursor = state_derivation_parent(&anchor);
    while let Some(block_id) = cursor {
        if !visited.insert(block_id) {
            return None;
        }
        let payload = read_checked_block(reader, block_id)?;
        cursor = state_derivation_parent(&payload);
        below.push(payload);
    }
    below.reverse();
    let tombstones = id_only_tombstones(reader, &below).ok()?;

    // 3. Seed the state: live nodes from the snapshot, history fields from the id-only walk.
    let mut state = NodeLifecycleState::new();
    for entry in manifest.entries {
        let content = match entry.content {
            StateRootContent::Blob(blob_id) => NodeContent::File {
                blob_id,
                mode: entry.mode,
            },
            StateRootContent::Symlink(target) => NodeContent::Symlink { target },
        };
        let node = LiveNode {
            path: entry.path,
            kind: entry.kind,
            content,
        };
        state.seed_live_node(entry.node_id, node).ok()?;
    }
    for (node_id, tombstone) in tombstones {
        if state.live_node(&node_id).is_none() {
            state.seed_tombstone(node_id, tombstone).ok()?;
        }
    }

    // 4. Fold the blocks after the anchor, checking each root against its signed root.
    let mut text_cache = TextCache::new();
    let mut folded = Vec::with_capacity(after_anchor.len());
    for (block_id, payload) in &after_anchor {
        apply_one_block_with_text_cache(reader, payload, &mut state, &mut text_cache).ok()?;
        let computed = compute_state_root(&entries_from_state(&state).ok()?).ok()?;
        if computed != payload.state_merkle_root {
            return None;
        }
        folded.push(*block_id);
    }
    Some(AnchoredParent {
        state,
        text_cache,
        folded,
        id_only_blocks: below.len(),
    })
}

// ---- Test-support probe: identity against full replay, at every block of a history ----------------------

/// What [`rfc159_identity_probe`] found.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Default)]
pub struct Rfc159IdentityReport {
    /// Blocks in the history (each one is sealed again on its own parent).
    pub blocks: usize,
    /// Blocks whose parent state came from an anchor.
    pub anchored: usize,
    /// Blocks that fell back to the full walk (no anchor in reach).
    pub fell_back: usize,
    /// The most blocks folded after an anchor.
    pub max_folded: usize,
    /// Blocks the id-only walks read, summed.
    pub id_only_blocks_total: usize,
    /// Blocks also compared against the literal full walk (`derive_next_state_for_candidate_choosing(.., StateAnchoring::Never)`).
    pub literal_full_compared: usize,
    /// Wall time of every anchored derivation, summed, in seconds.
    pub anchored_seconds: f64,
    /// Wall time of the literal full derivations, summed, in seconds (only the compared blocks).
    pub literal_full_seconds: f64,
    /// Wall time of the anchored derivation at those same compared blocks, in seconds.
    pub anchored_seconds_at_compared: f64,
    /// Every difference found; empty means identical everywhere. Capped at 20.
    pub differences: Vec<String>,
}

/// For every block of the history ending at `tip_hex` (a Block id in hex), seal-derive it again on its own parent and
/// compare, three ways: (1) the anchored derivation against an **independent forward replay from genesis** (one
/// pass, roots checked against each block's signed root): the whole `NodeLifecycleState`, tombstones and
/// `seen_ids` included, the state entries, the state root against the block's signed `state_merkle_root`, and the
/// text of every file whose content blob is not stored (what a checkpoint writer needs); (2) every
/// `literal_stride`-th block against the literal full walk that `seal` runs today; (3) that every block the anchored
/// derivation says it verified is in the lineage. The record is read as it stands: a block with no verified
/// anchor in reach falls back and is counted, not compared.
///
/// # Errors
///
/// The history does not replay, or a store read fails.
#[cfg(any(test, feature = "test-support"))]
pub fn rfc159_identity_probe(
    layout: &crate::RepositoryLayout,
    tip_hex: &str,
    literal_stride: usize,
) -> prikk_error::Result<Rfc159IdentityReport> {
    use std::str::FromStr;

    use super::{
        CandidateDerivation, StateAnchoring, apply_candidate_patches,
        derive_next_state_for_candidate_choosing,
    };
    use crate::object_store::ObjectReadSnapshot;

    let tip = ObjectId::from_str(tip_hex).map_err(|_| {
        prikk_error::PrikkError::Precondition(format!("probe: {tip_hex} is not a block id"))
    })?;

    let reader = ObjectReadSnapshot::open(layout)?;
    let mut chain: Vec<(ObjectId, BlockPayload)> = Vec::new();
    let mut cursor = Some(tip);
    while let Some(block_id) = cursor {
        let payload = read_checked_block(&reader, block_id).ok_or_else(|| {
            prikk_error::PrikkError::Integrity(format!("probe: block {block_id} is unreadable"))
        })?;
        cursor = state_derivation_parent(&payload);
        chain.push((block_id, payload));
    }
    chain.reverse();

    // An independent forward replay from genesis: state and text cache after each block, roots checked.
    let mut forward: Vec<(NodeLifecycleState, TextCache)> = Vec::with_capacity(chain.len());
    let mut state = NodeLifecycleState::new();
    let mut cache = TextCache::new();
    for (block_id, payload) in &chain {
        apply_one_block_with_text_cache(&reader, payload, &mut state, &mut cache)?;
        if compute_state_root(&entries_from_state(&state)?)? != payload.state_merkle_root {
            return Err(prikk_error::PrikkError::Integrity(format!(
                "probe: block {block_id} does not replay to its signed root"
            )));
        }
        forward.push((state.clone(), cache.clone()));
    }

    let mut report = Rfc159IdentityReport {
        blocks: chain.len(),
        ..Rfc159IdentityReport::default()
    };
    let note = |report: &mut Rfc159IdentityReport, message: String| {
        if report.differences.len() < 20 {
            report.differences.push(message);
        }
    };
    let lineage_ids: BTreeSet<ObjectId> = chain.iter().map(|(id, _)| *id).collect();
    for (index, (block_id, payload)) in chain.iter().enumerate() {
        let parent = index
            .checked_sub(1)
            .and_then(|p| chain.get(p))
            .map(|(id, _)| *id);
        let start = std::time::Instant::now();
        let derived: CandidateDerivation = derive_next_state_for_candidate_choosing(
            layout,
            &reader,
            parent,
            &payload.patch_ids,
            StateAnchoring::Anchored(AnchorSite::Seal),
        )
        .map_err(|_| {
            prikk_error::PrikkError::Integrity(format!(
                "probe: anchored derivation of {block_id} failed"
            ))
        })?;
        let anchored_elapsed = start.elapsed().as_secs_f64();
        report.anchored_seconds += anchored_elapsed;
        if derived.anchored {
            report.anchored += 1;
            report.max_folded = report.max_folded.max(derived.folded);
            report.id_only_blocks_total += derived.id_only_blocks;
        } else {
            report.fell_back += 1;
        }

        // (1) against the independent forward replay.
        let (mut expected_state, mut expected_cache) = match index.checked_sub(1) {
            Some(p) => forward.get(p).cloned().ok_or_else(|| {
                prikk_error::PrikkError::Integrity(format!("probe: no forward state at block {p}"))
            })?,
            None => (NodeLifecycleState::new(), TextCache::new()),
        };
        apply_candidate_patches(
            &reader,
            &mut expected_state,
            &mut expected_cache,
            &payload.patch_ids,
        )?;
        let expected_entries = entries_from_state(&expected_state)?;
        if derived.state != expected_state {
            note(
                &mut report,
                format!(
                    "block {index} ({block_id}): lifecycle state differs from the forward replay"
                ),
            );
        }
        if derived.entries != expected_entries {
            note(
                &mut report,
                format!("block {index} ({block_id}): state entries differ from the forward replay"),
            );
        }
        if compute_state_root(&derived.entries)? != payload.state_merkle_root {
            note(
                &mut report,
                format!(
                    "block {index} ({block_id}): derived root differs from the block's signed root"
                ),
            );
        }
        for entry in &derived.entries {
            if let StateRootContent::Blob(blob_id) = &entry.content {
                if !reader.has_object(*blob_id, ObjectType::Blob)?
                    && (!derived.text_cache.contains_key(&entry.node_id)
                        || derived.text_cache.get(&entry.node_id)
                            != expected_cache.get(&entry.node_id))
                {
                    note(
                        &mut report,
                        format!(
                            "block {index} ({block_id}): text of {} differs or is missing",
                            entry.path.as_str()
                        ),
                    );
                }
            }
        }
        // (3) what the anchored derivation claims to have verified.
        if let Some(stray) = derived.verified.iter().find(|id| !lineage_ids.contains(id)) {
            note(
                &mut report,
                format!(
                    "block {index} ({block_id}): claims to have verified {stray}, not in the lineage"
                ),
            );
        }

        // (2) every `literal_stride`-th block against the literal full walk.
        if literal_stride > 0 && index % literal_stride == 0 {
            let start = std::time::Instant::now();
            let literal = derive_next_state_for_candidate_choosing(
                layout,
                &reader,
                parent,
                &payload.patch_ids,
                StateAnchoring::Never,
            )
            .map_err(|_| {
                prikk_error::PrikkError::Integrity(format!(
                    "probe: full derivation of {block_id} failed"
                ))
            })?;
            report.literal_full_seconds += start.elapsed().as_secs_f64();
            report.anchored_seconds_at_compared += anchored_elapsed;
            report.literal_full_compared += 1;
            if literal.state != derived.state || literal.entries != derived.entries {
                note(
                    &mut report,
                    format!(
                        "block {index} ({block_id}): anchored derivation differs from the literal full walk"
                    ),
                );
            }
            let full_ids: BTreeSet<ObjectId> = literal.verified.iter().copied().collect();
            if derived.verified.iter().any(|id| !full_ids.contains(id)) {
                note(
                    &mut report,
                    format!(
                        "block {index} ({block_id}): anchored verified set is not within the full walk's"
                    ),
                );
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
