//! Format-2 Block shape and authoritative clean-state derivation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockKind, BlockPayload, MerkleRoot, ObjectId, ObjectType};

use crate::lifecycle_cache::replay::{
    LifecycleReplayError, TextCache, apply_candidate_patches, apply_one_block_with_text_cache,
};
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::ObjectReader;
use crate::state_root::{compute_state_root, entries_from_state};

/// Validate the format-2 Block kind and parent cardinality contract.
pub fn validate_block_v2_shape(payload: &BlockPayload) -> Result<()> {
    match (payload.kind, payload.parent_block_ids.as_slice()) {
        (BlockKind::Root, []) | (BlockKind::Normal, [_]) => validate_non_merge_shape(payload),
        (BlockKind::Merge, [_, _]) => validate_merge_shape(payload),
        (BlockKind::Root, _) => Err(PrikkError::Integrity(
            "format-2 Root Block must have zero parents".to_string(),
        )),
        (BlockKind::Normal, _) => Err(PrikkError::Integrity(
            "format-2 Normal Block must have exactly one parent".to_string(),
        )),
        (BlockKind::Merge, _) => Err(PrikkError::Integrity(
            "format-2 Merge Block must have exactly two parents".to_string(),
        )),
        (BlockKind::Repair | BlockKind::Import, _) => Err(PrikkError::Integrity(
            "format-2 Block kind is not authorized".to_string(),
        )),
    }
}

/// `Root`/`Normal` blocks carry neither DC-75 field — only a `Merge` block has two parents to
/// disambiguate or a proven baseline to record.
fn validate_non_merge_shape(payload: &BlockPayload) -> Result<()> {
    if payload.mainline_parent_id.is_some() || payload.merge_baseline_block_id.is_some() {
        return Err(PrikkError::Integrity(format!(
            "format-2 {:?} Block must not carry a mainline parent or merge baseline",
            payload.kind
        )));
    }
    Ok(())
}

/// A `Merge` block additionally names, and is bound to, its mainline parent (DC-75): one of its two
/// `parent_block_ids`, designating which side state derivation and replay follow. Every other kind
/// carries neither field — `parent_block_ids`' own cardinality already says everything a `Root` or
/// `Normal` block needs.
fn validate_merge_shape(payload: &BlockPayload) -> Result<()> {
    let Some(mainline) = payload.mainline_parent_id else {
        return Err(PrikkError::Integrity(
            "format-2 Merge Block must name a mainline parent".to_string(),
        ));
    };
    if !payload.parent_block_ids.contains(&mainline) {
        return Err(PrikkError::Integrity(
            "format-2 Merge Block mainline parent must be one of its own parents".to_string(),
        ));
    }
    if payload.merge_baseline_block_id.is_none() {
        return Err(PrikkError::Integrity(
            "format-2 Merge Block must record the baseline confluence was proven against"
                .to_string(),
        ));
    }
    Ok(())
}

/// The parent state derivation and replay follow for an already shape-validated payload: mainline
/// only for a `Merge` block (DC-75), the sole parent otherwise. Callers must have already run
/// [`validate_block_v2_shape`] on `payload` — this trusts `mainline_parent_id` is `Some` and names a
/// real parent for `Merge`, exactly as that validation requires.
fn state_derivation_parent(payload: &BlockPayload) -> Option<ObjectId> {
    if payload.kind == BlockKind::Merge {
        payload.mainline_parent_id
    } else {
        payload.parent_block_ids.first().copied()
    }
}

/// DC-92: a per-process-invocation memo of already-verified block states, shared across every
/// [`derive_next_state_root`]/[`verify_block_v2_state`] call within one `verify`, `seal`, or `merge`
/// invocation. **Never persisted, never read across process invocations** — constructed empty by
/// its caller and dropped when that invocation ends. This is why it does not engage NFR-PERF-04 or
/// DC-64's trust-ladder ruling: there is no file, no cross-run state, nothing for either to govern.
///
/// **The load-bearing invariant.** An entry is inserted for block X only once X has passed *every*
/// check the unmemoized path performs for it: shape validation ([`validate_block_v2_shape`]), schema
/// version (`schema_version == 2`), and replay-and-compare against X's own recorded
/// `state_merkle_root`. A memo entry means "X passed everything verification currently checks,"
/// never "X's state happened to match." [`validate_v2_lineage`] enforces the shape/schema half
/// before an entry can even be produced; [`verify_v2_lineage_roots`] and [`verify_block_v2_state`]
/// enforce the replay-and-compare half before inserting. Neither ever writes a memo entry for a
/// block whose own check failed or was skipped.
///
/// **Carries a `TextCache` alongside each state, not just the state itself.** A `TextFile` node's
/// content identity after an `EditText` is not necessarily a stored blob (DC-65's invariant); a
/// single continuous replay materializes it into a `TextCache` as it walks the lineage, and a later
/// `EditText` against the same node depends on finding it there. Splitting a from-genesis replay
/// into separately-memoized steps without carrying that cache forward reproduces exactly the gap
/// `crate::lifecycle_cache::incremental`'s own one-block step already found and falls back to full
/// replay for (see its module doc) — this was caught here by the existing test suite, not
/// anticipated in the original design, and fixed by carrying the cache rather than by falling back,
/// since blocks memoized here really are visited in lineage order and a real cache exists to carry.
#[derive(Debug, Default)]
pub(crate) struct LineageStateMemo {
    verified: BTreeMap<ObjectId, (NodeLifecycleState, TextCache)>,
}

impl LineageStateMemo {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Number of entries currently live. Read by [`verify_blocks_topological`] to track its own
    /// peak-concurrency diagnostic, and by the frontier-boundedness test that checks it.
    pub(crate) fn len(&self) -> usize {
        self.verified.len()
    }

    /// Drop `block_id`'s entry once [`verify_blocks_topological`] has determined no remaining block
    /// still needs it (DC-92 §4.2) — the mechanism that turns O(N) live entries into O(frontier).
    /// Not exposed outside this module: callers elsewhere in `verify`/`seal`/`merge` never share a
    /// memo across more than the one derivation they asked for, so they have no reason to evict.
    fn evict(&mut self, block_id: &ObjectId) {
        self.verified.remove(block_id);
    }
}

/// Derive the state root for a proposed format-2 Block from its parent and ordered Patches.
///
/// Convenience entry point for callers that only need one derivation and do not track a
/// `LineageStateMemo` of their own — every current caller except `verify`'s own per-object loop.
/// Constructs a fresh, call-scoped memo and delegates to `derive_next_state_root_with_memo`: this
/// still gets the full benefit of DC-92's per-call fix (O(i²) → O(i) in lineage depth), just without
/// sharing work across a *later*, separate call — which is exactly what a single `seal` or `merge`
/// invocation is.
pub fn derive_next_state_root(
    reader: &impl ObjectReader,
    parent: Option<ObjectId>,
    patch_ids: &[ObjectId],
) -> Result<MerkleRoot> {
    derive_next_state_root_with_memo(reader, parent, patch_ids, &mut LineageStateMemo::new())
}

/// Same guarantee as [`derive_next_state_root`], but threading a caller-supplied
/// [`LineageStateMemo`] through the whole derivation (DC-92). `verify`'s outer per-object loop is
/// the one caller that needs this directly: constructing one memo before that loop and passing it to
/// every block's [`verify_block_v2_state`] call is what takes `verify`'s total cost from O(N²) (one
/// O(i) derivation per block, summed) to O(N) (every block's own state derived at most once for the
/// whole invocation, however many later blocks' lineages reference it).
pub(crate) fn derive_next_state_root_with_memo(
    reader: &impl ObjectReader,
    parent: Option<ObjectId>,
    patch_ids: &[ObjectId],
    memo: &mut LineageStateMemo,
) -> Result<MerkleRoot> {
    let (mut state, mut text_cache) = resolved_parent_state(reader, parent, memo)?;
    apply_candidate_patches(reader, &mut state, &mut text_cache, patch_ids)?;
    compute_state_root(&entries_from_state(&state)?)
}

/// Failure of [`derive_next_state_root_for_candidate`], split at exactly the point RFC 115 Stage 4
/// needs classified: whether *the parent's own already-sealed lineage* failed to resolve (always an
/// integrity failure -- this repository's own history is broken), or whether *applying the
/// candidate patches themselves* failed (needs further classification by the caller, since an
/// accepted-but-unsealed patch failing to apply to a receiver's own tip is an ordinary divergence,
/// not corruption -- ordinary [`derive_next_state_root`] cannot tell these apart because
/// `From<LifecycleReplayError> for PrikkError` flattens the variant away before a caller ever sees
/// it).
#[derive(Debug)]
pub(crate) enum CandidateStateDerivationError {
    /// The parent Block's own lineage did not resolve. This repository's own sealed history is
    /// broken; the candidate patches were never reached.
    Lineage(PrikkError),
    /// Applying `patch_ids` onto the parent's resolved state failed. The caller must classify this
    /// variant -- see RFC 115 Stage 4 handoff §4's ruled table.
    Patch(LifecycleReplayError),
}

/// Same derivation as [`derive_next_state_root`], but for a caller that must distinguish *why* it
/// failed rather than receive one flattened [`PrikkError`] (RFC 115 Stage 4 handoff §4). The only
/// caller today is the seal-from-accepted path: the first place prikk applies patches that were not
/// authored against the state they are being applied to, where conflating "this repository's own
/// history is broken" with "these two histories merely diverged" would be a serious diagnostic
/// defect. Every other caller of state derivation replays already-sealed history, where a patch
/// failing to apply always does mean corruption -- this function changes nothing about that; it only
/// stops discarding the distinction for the one caller that needs it.
pub(crate) fn derive_next_state_root_for_candidate(
    reader: &impl ObjectReader,
    parent: Option<ObjectId>,
    patch_ids: &[ObjectId],
) -> std::result::Result<MerkleRoot, CandidateStateDerivationError> {
    let (mut state, mut text_cache) =
        resolved_parent_state(reader, parent, &mut LineageStateMemo::new())
            .map_err(CandidateStateDerivationError::Lineage)?;
    apply_candidate_patches(reader, &mut state, &mut text_cache, patch_ids)
        .map_err(CandidateStateDerivationError::Patch)?;
    compute_state_root(&entries_from_state(&state).map_err(CandidateStateDerivationError::Lineage)?)
        .map_err(CandidateStateDerivationError::Lineage)
}

/// Shared by [`derive_next_state_root_with_memo`] and [`verify_block_v2_state`]: resolve `parent`'s
/// state and carried `TextCache` (DC-92), verifying and memoizing anything not already known-good
/// for this invocation. `None` (genesis parent) returns empty state and an empty cache, matching
/// what a from-genesis replay starts from today.
fn resolved_parent_state(
    reader: &impl ObjectReader,
    parent: Option<ObjectId>,
    memo: &mut LineageStateMemo,
) -> Result<(NodeLifecycleState, TextCache)> {
    match parent {
        Some(parent_id) => {
            let lineage = validate_v2_lineage(reader, parent_id, memo)?;
            verify_v2_lineage_roots(reader, &lineage, memo)?;
            memo.verified.get(&parent_id).cloned().ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "format-2 parent Block {parent_id} was not verified before state derivation"
                ))
            })
        }
        None => Ok((NodeLifecycleState::new(), TextCache::new())),
    }
}

/// Recompute and compare one persisted format-2 Block's state root, threading a shared
/// [`LineageStateMemo`] (DC-92) so a caller checking many blocks — `verify`'s outer per-object loop
/// — never re-derives a block's state twice across the whole run. On success, this block's own
/// verified state is inserted into `memo`, so a *later* block whose lineage passes through this one
/// reuses it instead of re-deriving. Never inserted before success — see [`LineageStateMemo`]'s own
/// doc for why that ordering is the entire point.
pub(crate) fn verify_block_v2_state(
    reader: &impl ObjectReader,
    block_id: ObjectId,
    payload: &BlockPayload,
    memo: &mut LineageStateMemo,
) -> Result<()> {
    validate_block_v2_shape(payload)?;
    let parent = state_derivation_parent(payload);
    let (mut state, mut text_cache) = resolved_parent_state(reader, parent, memo)?;
    apply_candidate_patches(reader, &mut state, &mut text_cache, &payload.patch_ids)?;
    let computed = compute_state_root(&entries_from_state(&state)?)?;
    if computed != payload.state_merkle_root {
        return Err(PrikkError::Integrity(format!(
            "format-2 Block {block_id} state root does not match authoritative replay"
        )));
    }
    memo.verified.insert(block_id, (state, text_cache));
    Ok(())
}

/// Outcome of attempting to verify one `CurrentV6` Block's state root during
/// `verify_blocks_topological`'s whole-batch pass (DC-95 Stage 2 Level 2). Distinct from
/// `verify::StageOutcome`/`StageStatus` (Level 1): there is no operator-requested halt at block
/// granularity, so there is no `Halted` analogue — a block's non-evaluation is always because its
/// own state-derivation parent did not itself evaluate, never because an unrelated walk stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockStateStatus {
    /// The block's state root was independently re-derived and matches its recorded value.
    Verified,
    /// The block's own state-root check failed.
    Failed {
        /// The error the check raised.
        message: String,
    },
    /// This block's state-derivation parent did not itself evaluate (`Failed` or `NotEvaluated`), so
    /// this block's own state is undefined by construction and `verify_block_v2_state` was never
    /// attempted for it — attempting anyway would mean either trusting an unsound parent or
    /// re-deriving from genesis per descendant, defeating DC-92's whole memoization point.
    /// `blocked_by` names this block's *immediate* state-derivation parent, not the root cause
    /// (implementation review v1 §4 / Level 2 handoff §7 Q2: each record asserts only what it
    /// knows — a reader follows the chain one hop at a time, exactly as `StageStatus::NotEvaluated`
    /// requires at the stage level).
    NotEvaluated {
        /// This block's own state-derivation parent.
        blocked_by: ObjectId,
    },
}

/// One block's resolved outcome from `verify_blocks_topological`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStateOutcome {
    /// The block this outcome is for.
    pub block_id: ObjectId,
    /// How that block's state-root check resolved.
    pub status: BlockStateStatus,
}

/// Result of [`verify_blocks_topological`]: one outcome per input block, plus its pre-existing
/// diagnostic. Always exactly `blocks.len()` outcomes — no block may be silently absent, the same
/// invariant Level 1's `StageOutcome` carries at the stage level.
#[derive(Debug, Clone)]
pub(crate) struct TopologicalVerification {
    /// One outcome per block in `blocks`, in the order each was resolved (topological order, not
    /// input order).
    pub(crate) outcomes: Vec<BlockStateOutcome>,
    /// Peak number of entries [`LineageStateMemo`] held live at any point during this call —
    /// diagnostic only, unchanged by Level 2, read by the frontier-boundedness test and ignored by
    /// every production caller (unread outside `#[cfg(test)]`, hence the attribute below).
    #[allow(dead_code)]
    pub(crate) peak_memo_entries: usize,
}

/// Verify every format-2 Block in `blocks` — `verify`'s own outer loop's batch, collected in
/// ObjectId scan order by its Phase A pass — in **state-dependency order** rather than that scan
/// order (DC-92 §4.2). `state_derivation_parent` reduces every block, including `Merge` (mainline
/// parent only), to a single state-derivation parent, so the dependency structure here is always a
/// tree/forest, never a general multi-parent DAG — the same simplification
/// [`validate_v2_lineage`]'s own single-parent walk already relies on.
///
/// **Why this bounds memory, not just avoids re-deriving.** `verify_block_v2_state` is called on a
/// block only once its state-derivation parent is already memoized (or it has none — a root). The
/// instant every block that depends on a given memo entry has consumed it, that entry is evicted —
/// so at most a handful of entries are ever live at once: exactly the "frontier" of the traversal,
/// not the total block count. For a strict linear history the frontier is a small constant (two
/// entries momentarily coexist right as a new tip is verified, before its now-fully-consumed parent
/// is evicted) regardless of how deep the history is; for `B` concurrently open, never-merged
/// branches, it is `O(B)`. This is the mechanism the implementation review's §4 measurement asked
/// for: turning `LineageStateMemo` from something that grows with every block `verify` ever checks
/// into something that only ever holds what the *traversal in progress* still needs.
///
/// Uses Kahn's algorithm — in-degree map, children map, FIFO queue — the same shape already
/// established in this codebase by [`crate::merge::evidence::topological_order`] for a related but
/// distinct purpose (ordering candidate blocks for merge evidence, over full `parent_block_ids`
/// rather than the single state-derivation parent used here).
///
/// A block whose `state_derivation_parent` is not itself present in `blocks` (a format-1 ancestor at
/// a format transition boundary, or a missing/wrong-schema parent under corruption) is treated as
/// immediately ready — nothing in *this* batch to wait for — and `verify_block_v2_state`'s own
/// internal lineage walk still runs for it exactly as before, so a genuine defect there is still
/// caught with the same error it always was; this function adds no new trust in that path, only
/// reordering the batch that *is* self-contained.
///
/// **DC-95 Stage 2 Level 2: item-contained.** A block whose own check fails no longer aborts the
/// whole pass — it is recorded [`BlockStateStatus::Failed`] and the walk continues. Every block
/// whose state-derivation parent resolved to anything but [`BlockStateStatus::Verified`] is recorded
/// [`BlockStateStatus::NotEvaluated`] *without* attempting `verify_block_v2_state` at all: its state
/// is undefined by construction (§ above), so nothing is gained by attempting it and failing a second,
/// less informative way. The topological order Kahn's algorithm already establishes guarantees a
/// block's parent (if in-batch) is always resolved before the block itself, so looking up the
/// parent's already-recorded status is always safe. The batch-level cycle detection below remains a
/// genuine whole-pass failure — a cycle violates the tree/forest structure every other guarantee in
/// this function assumes, the same footing as a directory-shape violation one level up in `verify`'s
/// own pipeline (DC-95 Stage 2 Level 2 Step 0 §1.1's structural/semantic split) — and this check is
/// provably unreachable in practice regardless (round 6's ruling, kept for defense).
pub(crate) fn verify_blocks_topological(
    reader: &impl ObjectReader,
    blocks: &[(ObjectId, BlockPayload)],
    memo: &mut LineageStateMemo,
) -> Result<TopologicalVerification> {
    let by_id: BTreeMap<ObjectId, &BlockPayload> =
        blocks.iter().map(|(id, payload)| (*id, payload)).collect();

    let mut children: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    let mut pending_parent: BTreeMap<ObjectId, bool> = BTreeMap::new();
    for (id, payload) in blocks {
        let has_in_batch_parent = match state_derivation_parent(payload) {
            Some(parent_id) if by_id.contains_key(&parent_id) => {
                children.entry(parent_id).or_default().push(*id);
                true
            }
            _ => false,
        };
        pending_parent.insert(*id, has_in_batch_parent);
    }
    let mut remaining_children: BTreeMap<ObjectId, usize> = blocks
        .iter()
        .map(|(id, _)| (*id, children.get(id).map_or(0, Vec::len)))
        .collect();

    let mut ready: Vec<ObjectId> = pending_parent
        .iter()
        .filter(|&(_, has_parent)| !has_parent)
        .map(|(id, _)| *id)
        .collect();
    ready.sort();
    let mut queue: VecDeque<ObjectId> = ready.into();

    let mut peak = memo.len();
    let mut processed = BTreeSet::new();
    let mut resolved: BTreeMap<ObjectId, BlockStateStatus> = BTreeMap::new();
    let mut outcomes: Vec<BlockStateOutcome> = Vec::with_capacity(blocks.len());
    while let Some(id) = queue.pop_front() {
        let payload = by_id.get(&id).ok_or_else(|| {
            PrikkError::Integrity("format-2 topological pass lost a block".into())
        })?;
        let in_batch_parent =
            state_derivation_parent(payload).filter(|parent_id| by_id.contains_key(parent_id));
        let blocking_parent =
            in_batch_parent.and_then(|parent_id| match resolved.get(&parent_id) {
                Some(BlockStateStatus::Verified) | None => None,
                Some(BlockStateStatus::Failed { .. } | BlockStateStatus::NotEvaluated { .. }) => {
                    Some(parent_id)
                }
            });
        let status = if let Some(blocked_by) = blocking_parent {
            BlockStateStatus::NotEvaluated { blocked_by }
        } else {
            match verify_block_v2_state(reader, id, payload, memo) {
                Ok(()) => BlockStateStatus::Verified,
                Err(err) => BlockStateStatus::Failed {
                    message: err.to_string(),
                },
            }
        };
        peak = peak.max(memo.len());
        resolved.insert(id, status.clone());
        outcomes.push(BlockStateOutcome {
            block_id: id,
            status,
        });
        processed.insert(id);

        if let Some(parent_id) = state_derivation_parent(payload) {
            if let Some(count) = remaining_children.get_mut(&parent_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    memo.evict(&parent_id);
                }
            }
        }
        if remaining_children.get(&id).copied() == Some(0) {
            memo.evict(&id);
        }

        for child in children.get(&id).into_iter().flatten() {
            let entry = pending_parent.get_mut(child).ok_or_else(|| {
                PrikkError::Integrity("format-2 topological pass lost a tracked child".into())
            })?;
            *entry = false;
            queue.push_back(*child);
        }
    }

    if processed.len() != blocks.len() {
        return Err(
            match blocks
                .iter()
                .map(|(id, _)| *id)
                .find(|id| !processed.contains(id))
            {
                Some(stuck) => {
                    PrikkError::Integrity(format!("format-2 Block lineage cycle at {stuck}"))
                }
                None => PrikkError::Integrity(
                    "format-2 topological pass detected an inconsistent cycle count".to_string(),
                ),
            },
        );
    }
    Ok(TopologicalVerification {
        outcomes,
        peak_memo_entries: peak,
    })
}

/// Walk parent pointers from `tip` back toward genesis, stopping at genesis *or* at the first
/// ancestor already present in `memo` (DC-92) — whichever comes first. Reading, decoding, and
/// shape-validating a block already known-good for this invocation is exactly the redundant work
/// memoization exists to eliminate, so the walk itself stops there rather than only the replay that
/// follows it; without this, `verify`'s outer loop would still cost O(N²) in lineage-pointer walks
/// alone, even with every replay memoized.
///
/// Returns the *unresolved* suffix, ordered tip-to-boundary — every entry the caller still needs to
/// verify. An empty result means `tip` itself was already in `memo`; the caller has nothing left to
/// do for this lineage.
fn validate_v2_lineage(
    reader: &impl ObjectReader,
    tip: ObjectId,
    memo: &LineageStateMemo,
) -> Result<Vec<(ObjectId, BlockPayload)>> {
    let mut visited = BTreeSet::new();
    let mut lineage = Vec::new();
    let mut current = Some(tip);
    while let Some(block_id) = current {
        if memo.verified.contains_key(&block_id) {
            break;
        }
        if !visited.insert(block_id) {
            return Err(PrikkError::Integrity(format!(
                "format-2 Block lineage cycle at {block_id}"
            )));
        }
        let envelope = reader.read_object(block_id)?.ok_or_else(|| {
            PrikkError::Integrity(format!("format-2 parent Block {block_id} is missing"))
        })?;
        if envelope.object_type != ObjectType::Block {
            return Err(PrikkError::ObjectTypeMismatch {
                expected: ObjectType::Block.to_string(),
                actual: envelope.object_type.to_string(),
            });
        }
        if envelope.schema_version != 2 {
            return Err(PrikkError::Integrity(format!(
                "format-2 lineage contains Block {block_id} with schema {}",
                envelope.schema_version
            )));
        }
        let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
        validate_block_v2_shape(&payload)?;
        current = state_derivation_parent(&payload);
        lineage.push((block_id, payload));
    }
    Ok(lineage)
}

/// Verify and memoize every not-yet-memoized entry `validate_v2_lineage` returned, genesis-to-tip
/// (DC-92). The starting state is either `memo`'s entry for the boundary ancestor
/// `validate_v2_lineage` stopped at, or an empty state if the walk reached true genesis (`None`
/// parent) — `validate_v2_lineage`'s own stopping rule guarantees one of those two is always the
/// case, never a boundary with no recorded state.
fn verify_v2_lineage_roots(
    reader: &impl ObjectReader,
    lineage_from_tip: &[(ObjectId, BlockPayload)],
    memo: &mut LineageStateMemo,
) -> Result<()> {
    let Some((_, deepest)) = lineage_from_tip.last() else {
        // Everything in this lineage was already memoized; validate_v2_lineage returned nothing
        // left to do.
        return Ok(());
    };
    let (mut state, mut text_cache) = match state_derivation_parent(deepest) {
        Some(parent_id) => memo.verified.get(&parent_id).cloned().ok_or_else(|| {
            PrikkError::Integrity(format!(
                "format-2 parent Block {parent_id} was not verified before state derivation"
            ))
        })?,
        None => (NodeLifecycleState::new(), TextCache::new()),
    };
    for (block_id, payload) in lineage_from_tip.iter().rev() {
        apply_one_block_with_text_cache(reader, payload, &mut state, &mut text_cache)?;
        let computed = compute_state_root(&entries_from_state(&state)?)?;
        if computed != payload.state_merkle_root {
            return Err(PrikkError::Integrity(format!(
                "format-2 parent Block {block_id} state root does not match authoritative replay"
            )));
        }
        memo.verified
            .insert(*block_id, (state.clone(), text_cache.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
