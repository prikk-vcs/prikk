//! Public read-only merge/conflict evidence display boundary (DC-22).

mod display;
mod merge_plan;

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockKind, BlockPayload, ObjectId, ObjectType, RefStatePayload};

pub use display::{
    MergeEvidenceDisplay, MergeEvidenceDisplayItem, MergeEvidenceDisplayOperation,
    MergeEvidenceDisplayOperationContent, MergeEvidenceDisplaySelector,
};
pub use merge_plan::MergePlanDisplay;

use crate::author::author_signing::require_author_key_id;
use crate::lifecycle_cache::{ReplayDerivedLifecycleState, replay_derived_state};
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_algebra::{EvidenceScope, StorePatchAlgebraEvidence, analyze_merge_evidence};
use crate::patch_replay::decode::{
    DecodedOperationKind, DecodedPatchOperation, decode_patch_operations,
};
use crate::received::read_received_pointer;
use crate::refs::RefStore;
use crate::trust::{MaintainerTrustPolicy, verify_trusted_publication_envelope};
use crate::{RepositoryLayout, validate_local_branch_ref};

/// Target selector for `prikk merge-evidence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeEvidenceTarget {
    /// Select a sealed target block directly.
    Block(ObjectId),
    /// Select the current target block of a local branch ref.
    Ref(String),
    /// Select the current target block of a received ref (`remotes/<name>`, DC-85). Never valid as
    /// a merge's `into_ref` — only as a source.
    ReceivedRef(String),
}

/// Prepare a read-only merge evidence display report.
pub fn prepare_merge_evidence(
    layout: &RepositoryLayout,
    baseline_block_id: ObjectId,
    left_target: MergeEvidenceTarget,
    right_target: MergeEvidenceTarget,
) -> Result<MergeEvidenceDisplay> {
    // RFC 111 §6.1: this function is read-only end to end (it never calls `write_object`; shared as
    // infrastructure by `execute_merge`, which writes separately, afterward, through its own store --
    // see `merge_execute.rs`), so it takes one decoded index snapshot here instead of paying a fresh
    // decode per object read.
    let object_store = ObjectReadSnapshot::open(layout)?;
    let baseline_horizon = lineage_horizon(&object_store, baseline_block_id)?;
    let replay = replay_derived_state(&object_store, baseline_block_id, baseline_horizon)?;
    let evidence =
        StorePatchAlgebraEvidence::from_replay_derived(&object_store, baseline_horizon, replay)
            .map_err(|err| PrikkError::Integrity(format!("merge evidence baseline: {err:?}")))?;
    let left_selector = resolve_target(layout, &object_store, left_target)?;
    let right_selector = resolve_target(layout, &object_store, right_target)?;
    let (left_operations, left_author_key_ids) = candidate_sequence(
        &object_store,
        baseline_block_id,
        left_selector.target_block_id,
    )?;
    let (right_operations, right_author_key_ids) = candidate_sequence(
        &object_store,
        baseline_block_id,
        right_selector.target_block_id,
    )?;
    let report = analyze_merge_evidence(
        baseline_block_id,
        Some(baseline_horizon),
        evidence.baseline_state(),
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left_operations,
        &right_operations,
    );
    Ok(MergeEvidenceDisplay::from_report(
        report,
        left_selector,
        right_selector,
        &left_author_key_ids,
        &right_author_key_ids,
    ))
}

/// Prepare a read-only merge plan display report.
pub fn prepare_merge_plan(
    layout: &RepositoryLayout,
    baseline_block_id: ObjectId,
    left_target: MergeEvidenceTarget,
    right_target: MergeEvidenceTarget,
) -> Result<MergePlanDisplay> {
    let evidence = prepare_merge_evidence(layout, baseline_block_id, left_target, right_target)?;
    Ok(MergePlanDisplay::from_evidence(evidence))
}

fn resolve_target(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    target: MergeEvidenceTarget,
) -> Result<MergeEvidenceDisplaySelector> {
    match target {
        MergeEvidenceTarget::Block(block_id) => {
            read_block(object_store, block_id)?;
            Ok(MergeEvidenceDisplaySelector {
                selector: format!("block {block_id}"),
                target_block_id: block_id,
            })
        }
        MergeEvidenceTarget::Ref(ref_name) => {
            let ref_name = validate_local_branch_ref(&ref_name)?;
            let ref_store = RefStore::new(layout.clone());
            let ref_state_id = ref_store
                .read_current_ref_state_id(&ref_name)?
                .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} is not published")))?;
            let envelope = object_store
                .read_typed(ref_state_id, ObjectType::RefState)?
                .ok_or_else(|| {
                    PrikkError::Integrity(format!("ref {ref_name} points to missing RefState"))
                })?;
            let ref_state = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            if ref_state.ref_name != ref_name {
                return Err(PrikkError::Integrity(format!(
                    "RefState name mismatch: expected {ref_name}, got {}",
                    ref_state.ref_name
                )));
            }
            read_block(object_store, ref_state.target_object_id)?;
            Ok(MergeEvidenceDisplaySelector {
                selector: format!("ref {ref_name}"),
                target_block_id: ref_state.target_object_id,
            })
        }
        MergeEvidenceTarget::ReceivedRef(ref_name) => {
            let pointer = read_received_pointer(layout, &ref_name)?.ok_or_else(|| {
                PrikkError::Integrity(format!("received ref {ref_name} does not exist"))
            })?;
            let envelope = object_store
                .read_typed(pointer.ref_state_id, ObjectType::RefState)?
                .ok_or_else(|| {
                    PrikkError::Integrity(format!(
                        "received ref {ref_name} points to missing RefState"
                    ))
                })?;
            let ref_state = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            // Deliberately no name-equality check here (unlike the local-ref arm above): a received
            // RefState's embedded `ref_name` is the *origin's* own name (e.g. "heads/main"), never
            // the local "remotes/..." label — DC-85 §3A carries this asymmetry forward from DC-78's
            // received-ref design, where it's why received refs cannot reuse `refs/by-id/`'s pointer
            // format at all.
            read_block(object_store, ref_state.target_object_id)?;
            Ok(MergeEvidenceDisplaySelector {
                selector: format!("received ref {ref_name}"),
                target_block_id: ref_state.target_object_id,
            })
        }
    }
}

fn lineage_horizon(object_store: &impl ObjectReader, baseline: ObjectId) -> Result<ObjectId> {
    let mut visited = BTreeSet::new();
    let mut current = baseline;
    loop {
        if !visited.insert(current) {
            return Err(PrikkError::Integrity(format!(
                "block parent chain contains a cycle at {current}"
            )));
        }
        let block = read_block(object_store, current)?;
        // State derivation, same category as `block_state.rs`'s replay walk (DC-75): a `Merge`
        // block's own state is replayed from its mainline parent only, never its secondary.
        match mainline_or_sole_parent(&block) {
            Some(None) => return Ok(current),
            Some(Some(parent)) => current = parent,
            None => {
                return Err(PrikkError::UnsupportedObjectType(format!(
                    "merge evidence requires a single-parent baseline lineage; block {current} has {} parents",
                    block.parent_block_ids.len()
                )));
            }
        }
    }
}

/// RFC 142 §3's narrow boundary: `show` needs `lineage_horizon` + `replay_derived_state` to
/// resolve a node-addressed operation's path at a sealed block, but `lineage_horizon` stays
/// private to this file and `replay_derived_state`/`ReplayDerivedLifecycleState` stay at their
/// existing `pub(crate)` visibility (RFC 131 narrowed internals two days before this one; widening
/// them back out would work directly against that). This is the one new function instead — same
/// shape as RFC 140's `enumerate_queued_patches`, which called `resolve_folded_worktree_baseline`
/// directly rather than exporting it further. One replay per call: callers resolve as many node
/// ids as they need against the single returned value ("resolve once per invocation, not once per
/// operation").
pub(crate) fn lifecycle_state_at(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<ReplayDerivedLifecycleState> {
    let horizon = lineage_horizon(object_store, block_id)?;
    replay_derived_state(object_store, block_id, horizon)
}

/// The parent state derivation continues through: mainline only for a `Merge` block (DC-75), the
/// sole parent for `Normal`, none for `Root`. `Some(None)` is genesis, not an error; `None` is a
/// shape this walk cannot follow — a non-`Merge` block with more than one parent, or a `Merge`
/// block with no valid mainline parent.
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

/// Full-DAG ancestor closure of `start` (inclusive), following **all** parents — the reachability
/// primitive (DC-75), distinct from state derivation's mainline-only walk. A cycle (cryptographically
/// impossible in honestly-generated data, since parent references are content hashes computed before
/// the referencing block exists) is not distinguished from a legitimate diamond re-visit here: this
/// closure is a set, used only for reachability/exclusion, never to decide state, so under-detecting a
/// cycle can at worst leave a set slightly incomplete — it cannot make an unsound merge succeed. The
/// state-derivation walks (`block_state.rs`, `lineage_horizon` above) retain explicit cycle errors,
/// since those sit on the actual trust boundary.
pub(crate) fn ancestors_inclusive(
    object_store: &impl ObjectReader,
    start: ObjectId,
) -> Result<std::collections::BTreeMap<ObjectId, BlockPayload>> {
    let mut ancestors = std::collections::BTreeMap::new();
    let mut stack = vec![start];
    while let Some(current) = stack.pop() {
        if ancestors.contains_key(&current) {
            continue;
        }
        let block = read_block(object_store, current)?;
        crate::validate_block_v2_shape(&block)?;
        stack.extend(block.parent_block_ids.iter().copied());
        ancestors.insert(current, block);
    }
    Ok(ancestors)
}

/// Topologically order `new_ids` (parents before children), restricted to edges within `new_ids`
/// itself — a parent outside the set is, by construction, already satisfied (it is an ancestor of
/// `baseline`). Kahn's algorithm, O(V+E): for the overwhelmingly common case (a simple single-parent
/// chain since baseline, no repeated merge involved) this is linear, same cost as the walk it
/// replaces — verified in `baseline-recording-answer-v1.md` §1.
fn topological_order(
    new_ids: &BTreeSet<ObjectId>,
    ancestors: &std::collections::BTreeMap<ObjectId, BlockPayload>,
) -> Result<Vec<ObjectId>> {
    let mut remaining_parents = std::collections::BTreeMap::new();
    let mut children: std::collections::BTreeMap<ObjectId, Vec<ObjectId>> =
        std::collections::BTreeMap::new();
    for id in new_ids {
        let Some(block) = ancestors.get(id) else {
            return Err(PrikkError::Integrity(format!(
                "candidate block set references untracked block {id}"
            )));
        };
        let count = block
            .parent_block_ids
            .iter()
            .filter(|parent| new_ids.contains(parent))
            .count();
        remaining_parents.insert(*id, count);
        for parent in &block.parent_block_ids {
            if new_ids.contains(parent) {
                children.entry(*parent).or_default().push(*id);
            }
        }
    }
    let mut ready: Vec<ObjectId> = remaining_parents
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    ready.sort();
    let mut queue: std::collections::VecDeque<ObjectId> = ready.into();
    let mut order = Vec::with_capacity(new_ids.len());
    while let Some(id) = queue.pop_front() {
        order.push(id);
        for child in children.get(&id).into_iter().flatten() {
            let Some(entry) = remaining_parents.get_mut(child) else {
                return Err(PrikkError::Integrity(
                    "candidate block set topological sort lost a tracked child".to_string(),
                ));
            };
            *entry -= 1;
            if *entry == 0 {
                queue.push_back(*child);
            }
        }
    }
    if order.len() != new_ids.len() {
        return Err(PrikkError::Integrity(
            "candidate block set contains a cycle".to_string(),
        ));
    }
    Ok(order)
}

/// Blocks strictly between `baseline` (exclusive) and `target` (inclusive), oldest first, paired with
/// their own object ids — the ancestor-closure difference `ancestors(target) \ ancestors(baseline)`,
/// following **all** parents (DC-75; previously a single-parent-only walk, replaced because a `Merge`
/// block's secondary parent can be the true, and only, path back to a repeated merge's baseline).
/// Shared by `candidate_sequence` (decoded operations, for evidence), `candidate_patch_ids` (patch
/// identity, for merge execution's adoption set — DC-74), and `verify_candidate_blocks_trusted` (DC-85
/// — the same candidate set, not a second walk of the ancestor graph) so the walk is defined exactly
/// once.
fn candidate_blocks(
    object_store: &impl ObjectReader,
    baseline: ObjectId,
    target: ObjectId,
) -> Result<Vec<(ObjectId, BlockPayload)>> {
    let target_ancestors = ancestors_inclusive(object_store, target)?;
    if !target_ancestors.contains_key(&baseline) {
        return Err(PrikkError::Integrity(format!(
            "baseline Block {baseline} is not an ancestor of target Block {target}"
        )));
    }
    let baseline_ancestors = ancestors_inclusive(object_store, baseline)?;
    let new_ids: BTreeSet<ObjectId> = target_ancestors
        .keys()
        .filter(|id| !baseline_ancestors.contains_key(id))
        .copied()
        .collect();
    topological_order(&new_ids, &target_ancestors)?
        .into_iter()
        .map(|id| {
            let payload = target_ancestors.get(&id).cloned().ok_or_else(|| {
                PrikkError::Integrity(format!("missing Block {id} in candidate set"))
            })?;
            Ok((id, payload))
        })
        .collect()
}

/// Patch ids already reachable from `baseline` via **any** parent path (DC-75 addendum-5): the set a
/// side's candidate patches must exclude. A patch appearing here is either literally the baseline's
/// own content or was already adopted into it by an earlier merge — replaying it again is not new
/// content, and feeding it to confluence analysis breaks the proof rather than refusing cleanly
/// (`reachability-vs-state-derivation-answer-v1.md` §2: `PairReplayFailed`, not a conflict).
fn baseline_reachable_patch_ids(
    object_store: &impl ObjectReader,
    baseline: ObjectId,
) -> Result<BTreeSet<ObjectId>> {
    let ancestors = ancestors_inclusive(object_store, baseline)?;
    Ok(ancestors
        .values()
        .flat_map(|block| block.patch_ids.iter().copied())
        .collect())
}

/// The candidate operation sequence plus, in lockstep (one entry per operation, same order,
/// never separately indexed), the AUTHOR key id that asserted each `RenamePath` operation --
/// RFC 144 §4o.6a's honesty invariant. Built for every other kind too, always `None`: only a
/// `RenamePath` operand ever needs its asserting signer resolved, so only that arm pays
/// `require_author_key_id`'s cost (and only that arm can fail the whole call on a missing AUTHOR
/// signature -- a patch with no rename in it is never affected).
fn candidate_sequence(
    object_store: &impl ObjectReader,
    baseline: ObjectId,
    target: ObjectId,
) -> Result<(Vec<DecodedPatchOperation>, Vec<Option<String>>)> {
    let excluded = baseline_reachable_patch_ids(object_store, baseline)?;
    let mut operations = Vec::new();
    let mut author_key_ids = Vec::new();
    for (_, block) in candidate_blocks(object_store, baseline, target)? {
        for patch_id in block.patch_ids {
            if excluded.contains(&patch_id) {
                continue;
            }
            let envelope = object_store
                .read_typed(patch_id, ObjectType::Patch)?
                .ok_or_else(|| PrikkError::Integrity(format!("missing Patch {patch_id}")))?;
            for operation in
                decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?
            {
                let author_key_id = match &operation.kind {
                    DecodedOperationKind::RenamePath { .. } => {
                        Some(require_author_key_id(&envelope)?)
                    }
                    _ => None,
                };
                operations.push(operation);
                author_key_ids.push(author_key_id);
            }
        }
    }
    Ok((operations, author_key_ids))
}

/// Patch identities strictly between `baseline` (exclusive) and `target` (inclusive), in the order
/// they were sealed — the set merge execution adopts verbatim onto the other side (DC-74). Excludes
/// any patch already reachable from `baseline` (DC-75 addendum-5) — see `baseline_reachable_patch_ids`.
pub(crate) fn candidate_patch_ids(
    object_store: &impl ObjectReader,
    baseline: ObjectId,
    target: ObjectId,
) -> Result<Vec<ObjectId>> {
    let excluded = baseline_reachable_patch_ids(object_store, baseline)?;
    Ok(candidate_blocks(object_store, baseline, target)?
        .into_iter()
        .flat_map(|(_, block)| block.patch_ids)
        .filter(|patch_id| !excluded.contains(patch_id))
        .collect())
}

/// DC-85 §3A.1's mandatory acceptance criterion: every candidate block a merge would adopt must carry
/// a currently-trusted MAINTAINER signature, checked here — over the exact same candidate set
/// `candidate_patch_ids` computes (`candidate_blocks`, not a second walk of the ancestor graph), and
/// using the same trust machinery `verify` uses (`verify_trusted_publication_envelope`), not a new
/// check invented for this path.
///
/// Required specifically because a received ref's blocks arrive via `import_bundle`, which performs
/// no trust check at all (DC-78 Stage 3 §4, deliberate — "no automatic trust adoption on import").
/// A local-to-local merge's adopted blocks are safe by induction: every block reachable from a local
/// ref was itself created through this repository's own seal/merge path, each gated by
/// `verify_signer_trusted` at creation. That induction does not hold for imported content, which was
/// never gated on entry — so it must be gated here, before `into_ref` advances, not deferred to a
/// later `verify` run.
pub(crate) fn verify_candidate_blocks_trusted(
    object_store: &impl ObjectReader,
    policy: &MaintainerTrustPolicy,
    baseline: ObjectId,
    target: ObjectId,
) -> Result<()> {
    for (block_id, _) in candidate_blocks(object_store, baseline, target)? {
        let envelope = object_store
            .read_typed(block_id, ObjectType::Block)?
            .ok_or_else(|| PrikkError::Integrity(format!("missing Block {block_id}")))?;
        verify_trusted_publication_envelope(policy, &envelope).map_err(|issue| {
            PrikkError::InvalidSignature(format!(
                "adopted Block {block_id} has no trusted MAINTAINER signature ({}: {})",
                issue.code, issue.message
            ))
        })?;
    }
    Ok(())
}

fn read_block(object_store: &impl ObjectReader, block_id: ObjectId) -> Result<BlockPayload> {
    let envelope = object_store
        .read_typed(block_id, ObjectType::Block)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Block {block_id}")))?;
    BlockPayload::decode_canonical(&envelope.canonical_payload)
}

#[cfg(test)]
mod tests;
