//! RFC 144 §4m — the bundle-impact preview mechanism: what would a bundle's own history do to a
//! repository's current state, without admitting the bundle's objects into the store.
//!
//! **Lives under `bundle`, not `patch_replay`, deliberately.** This needs both
//! `crate::patch_replay`'s own replay primitives and `crate::merge_evidence`'s ancestor/candidate-
//! patch machinery. `merge_evidence` already depends on `patch_replay` (`decode_patch_operations`);
//! putting this module *inside* `patch_replay` instead would have made `patch_replay` depend on
//! `merge_evidence` too, a genuine new cycle release-policy's own coupling gate caught immediately.
//! `bundle` already depends on `patch_replay` one-way (its own closure-validation decode path), so
//! adding a `bundle -> merge_evidence` edge here costs nothing that did not already exist.
//!
//! The design's own words (§4m.3): **"One replay of current state, plus the bundle's own patches
//! applied in memory."** This module does exactly that, reusing
//! [`crate::patch_replay::apply_operation_sequence`] (the same per-operation dispatch every other
//! replay path in this crate uses) rather than a second implementation of "how an operation folds
//! into state."
//!
//! **Connectivity has no existing machinery to reuse.** Every real merge command in this
//! repository (`merge`, `merge-plan`, `merge-evidence`) takes an operator-supplied
//! `--baseline-block` — there is no automatic common-ancestor search anywhere in this codebase
//! (confirmed by reading `merge_evidence.rs`/`verify.rs`'s own `verify_merge_baseline`, which only
//! ever *checks* a claimed baseline, never *finds* one). This module is therefore the first place
//! that needs to. It reuses [`crate::merge_evidence::ancestors_inclusive`] (already `pub(crate)`,
//! already multi-parent-aware) for the underlying graph walk, and adds its own lowest-common-
//! ancestor search on top -- see [`find_lowest_common_ancestors`] for why that search can return
//! more than one candidate, and what this module does when it does.
//!
//! **"Would it conflict" is answered by the same replay, not a second analysis.** Rather than
//! reusing `patch_algebra`'s confluence/witness machinery (built for a *true* three-way merge
//! question -- would two independent change sets, replayed against a shared baseline, agree), this
//! asks a narrower, more directly answerable question: does replaying the bundle's own new
//! operations *directly onto the local ref's current state* succeed? Every conflict shape
//! `patch_algebra` tracks (a path already occupied, a stale precondition, an overlapping text
//! span, a node no longer live) is also exactly the kind of precondition
//! `apply_operation_sequence`'s own dispatch already enforces -- so a genuine conflict surfaces as
//! a real `Err` from this replay, not a missed signal. What this narrower question cannot express:
//! two changes that are individually harmless against the *current* state but would not compose
//! correctly under a *true* three-way merge's own resolution rules. That is out of this preview's
//! stated scope (§4m.5: no change to `merge`/`merge-plan`, and no claim beyond "does the bundle's
//! own history apply cleanly here"), so it is not attempted.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectId};

use crate::merge_evidence::{ancestors_inclusive, candidate_patch_ids};
use crate::object_store::ObjectReader;
use crate::patch_replay::apply::ReplayLiveNode;
use crate::patch_replay::apply_operation_sequence;
use crate::patch_replay::decode::{DecodedPatchOperation, decode_patch_operations};
use crate::patch_replay::read::{load_snapshot_files, read_block, read_patch, single_parent_chain};

/// Path-keyed file bytes, as `patch_replay`'s own replay loop builds them.
type ReplayedFiles = BTreeMap<String, Vec<u8>>;
/// Node-keyed live-node state, as `patch_replay`'s own replay loop builds it.
type ReplayedLiveNodes = BTreeMap<prikk_object::NodeId, ReplayLiveNode>;

/// How the bundle's own history relates to the local ref it is being previewed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BundleConnectivity {
    /// The local ref has never been published (§4n.1) -- there is no local target to compare
    /// against at all, not merely an empty one. Named for what is true of the **local** side,
    /// deliberately distinct from `FastForward`: fast-forward asserts *local history is a prefix
    /// of the bundle's*, which is not a true statement when local has no history to be a prefix
    /// of. The whole of the bundle's own content previews as `Created`, the same way
    /// `FastForward` reports new nodes, and there is nothing to conflict with.
    NoLocalHistory,
    /// No shared ancestry at all -- a legitimate, common answer (§4m.3 #1), reported rather than
    /// treated as an error.
    DoesNotConnect,
    /// The bundle's own target block is already an ancestor of the local ref's current target:
    /// this bundle brings nothing this repository does not already have.
    AlreadyIncluded,
    /// The local ref's current target is an ancestor of the bundle's own target: the bundle
    /// strictly extends local history. No conflict is possible by construction -- a fast-forward.
    FastForward,
    /// Both sides carry commits since their common ancestor. Whether this would conflict is
    /// answered separately -- see [`ConflictAnswer`].
    Diverged,
}

/// The answer to "would it conflict" (§4m.3 #2), computed only when [`BundleConnectivity::Diverged`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictAnswer {
    /// The bundle's own new operations replayed cleanly onto the local ref's current state.
    AppliesCleanly,
    /// Replaying the bundle's own new operations onto the local ref's current state failed --
    /// naming the underlying replay error, not a witness classification (see this module's own
    /// doc comment for why a raw apply-time failure is treated as the conflict signal).
    Conflict { detail: String },
    /// More than one maximal common ancestor exists (an ambiguous merge base) -- this module does
    /// not guess among them. An honest "not answerable today" (per the handoff's own instruction)
    /// rather than a field that is always populated and sometimes wrong.
    Undetermined { reason: String },
}

/// One node-granularity effect the bundle's own new operations would have on the local ref's
/// current state, were they applied in memory (§4m.3's own vocabulary: created, deleted, edited,
/// permission-changed -- **not** renamed, see this field's own doc and §4m.4's honesty limit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundleImpactEffect {
    pub(crate) path: String,
    pub(crate) kind: BundleImpactEffectKind,
    pub(crate) current_bytes: Option<u64>,
    pub(crate) after_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BundleImpactEffectKind {
    Created,
    Deleted,
    Edited,
    PermissionChanged,
}

/// Full result of previewing a bundle's target block against a local ref's current target.
pub(crate) struct BundlePreviewResult {
    pub(crate) connectivity: BundleConnectivity,
    pub(crate) conflict: Option<ConflictAnswer>,
    pub(crate) effects: Vec<BundleImpactEffect>,
}

/// Walk `block_ids` (oldest-first, as [`single_parent_chain`] returns) and replay every patch's
/// operations into fresh state -- the same block-walk `replay_supported_patch_chain` performs,
/// factored out so it can run over an arbitrary chain, not only one backed by a local ref.
fn walk_and_replay(
    reader: &impl ObjectReader,
    block_ids: &[ObjectId],
) -> Result<(ReplayedFiles, ReplayedLiveNodes)> {
    let mut files = BTreeMap::new();
    let mut live_nodes = BTreeMap::new();
    let mut deleted_files = BTreeMap::new();
    for block_id in block_ids {
        let block = read_block(reader, *block_id)?;
        if let Some(snapshot_blob_ref) = block.snapshot_blob_ref {
            files = load_snapshot_files(reader, snapshot_blob_ref)?;
            live_nodes.clear();
            deleted_files.clear();
        }
        for patch_id in block.patch_ids {
            let patch = read_patch(reader, patch_id)?;
            let operations =
                decode_patch_operations(&patch.canonical_payload, patch.schema_version)?;
            apply_operation_sequence(
                reader,
                &mut files,
                &mut live_nodes,
                &mut deleted_files,
                operations,
            )?;
        }
    }
    Ok((files, live_nodes))
}

/// Every candidate patch strictly between `baseline` and `target`, decoded and concatenated in
/// sealed order -- the bundle's own new operations, for whichever baseline connectivity picked.
fn candidate_operations(
    reader: &impl ObjectReader,
    baseline: ObjectId,
    target: ObjectId,
) -> Result<Vec<DecodedPatchOperation>> {
    let patch_ids = candidate_patch_ids(reader, baseline, target)?;
    let mut operations = Vec::new();
    for patch_id in patch_ids {
        let envelope = reader
            .read_typed(patch_id, prikk_object::ObjectType::Patch)?
            .ok_or_else(|| PrikkError::Integrity(format!("missing Patch {patch_id}")))?;
        operations.extend(decode_patch_operations(
            &envelope.canonical_payload,
            envelope.schema_version,
        )?);
    }
    Ok(operations)
}

/// The maximal elements of `local_ancestors ∩ bundle_ancestors`: common-ancestor blocks that are
/// not themselves a (transitive) ancestor of any other block in the intersection. In a simple
/// history there is exactly one -- the lowest common ancestor a real merge would use as its
/// baseline. A diamond-shaped history (independent merges of the same two lines) can legitimately
/// produce more than one; this function returns all of them and lets the caller decide (this
/// module's own policy: more than one means "undetermined," never a guess).
///
/// **Complexity, stated plainly rather than left implicit**: this is O(|shared| × (V+E)) in the
/// worst case -- one bounded graph walk per shared candidate. Every input here is already bounded
/// (`DEFAULT_BUNDLE_MAX_OBJECT_COUNT`), and a real repository's own shared-ancestor set is
/// overwhelmingly one element in practice; a tighter single-pass topological algorithm exists but
/// is not warranted at these sizes, and would cost readability this module does not need to spend.
fn find_lowest_common_ancestors(
    local_ancestors: &BTreeMap<ObjectId, BlockPayload>,
    bundle_ancestors: &BTreeMap<ObjectId, BlockPayload>,
) -> Vec<ObjectId> {
    let shared: BTreeSet<ObjectId> = local_ancestors
        .keys()
        .filter(|id| bundle_ancestors.contains_key(id))
        .copied()
        .collect();
    let mut dominated: BTreeSet<ObjectId> = BTreeSet::new();
    for root in &shared {
        // Every block reachable from `root`'s own parents, using whichever ancestor map already
        // has `root` (both do, by construction of `shared`) -- no new reads, this is a pure
        // in-memory walk over already-fetched payloads.
        let source = if local_ancestors.contains_key(root) {
            local_ancestors
        } else {
            bundle_ancestors
        };
        let mut stack: Vec<ObjectId> = source
            .get(root)
            .map(|block| block.parent_block_ids.clone())
            .unwrap_or_default();
        let mut visited: BTreeSet<ObjectId> = BTreeSet::new();
        while let Some(current) = stack.pop() {
            if !visited.insert(current) {
                continue;
            }
            if shared.contains(&current) {
                dominated.insert(current);
            }
            if let Some(block) = source.get(&current) {
                stack.extend(block.parent_block_ids.iter().copied());
            }
        }
    }
    shared.difference(&dominated).copied().collect()
}

/// The preview computation for §4n.1's fifth state: the local ref named has never been
/// published, so there is no `local_target` to compare against at all. `bundle_only_reader` must
/// resolve only the bundle's own object set -- there is no local ref state to combine it with,
/// and reusing the bundle-only reader here (rather than a composed one) keeps this path
/// consistent with `preview_impact`'s own "never credit local objects the bundle did not itself
/// carry" discipline, even though local has nothing to contribute in this state by construction.
///
/// Walks the bundle's own full chain from genesis to `bundle_target` and reports every path in
/// it as `Created` against an empty starting state -- "all of it arrives," per §4n.1's own
/// instruction -- with no conflict question to answer (there is no local state to conflict
/// with).
pub(crate) fn preview_new_repository_impact(
    bundle_only_reader: &impl ObjectReader,
    bundle_target: ObjectId,
) -> Result<BundlePreviewResult> {
    let (after_files, after_live) = walk_and_replay(
        bundle_only_reader,
        &single_parent_chain(bundle_only_reader, bundle_target)?,
    )?;
    let after_modes = modes_by_path(&after_live);
    let effects = diff_effects(
        &BTreeMap::new(),
        &BTreeMap::new(),
        &after_files,
        &after_modes,
    );
    Ok(BundlePreviewResult {
        connectivity: BundleConnectivity::NoLocalHistory,
        conflict: Some(ConflictAnswer::AppliesCleanly),
        effects,
    })
}

/// The full preview computation: connectivity, conflict answer, and node-level effects, for a
/// bundle whose own target block is `bundle_target` against a local ref whose current target is
/// `local_target`. `reader` must resolve both bundle-only and local objects (the composed
/// `BundleAndLocalReader`, `local: Some(...)`); `bundle_only_reader` must resolve only the
/// bundle's own object set, used for the bundle's own ancestor walk so a partially-overlapping
/// bundle can never be credited with local objects it did not itself carry.
pub(crate) fn preview_impact(
    reader: &impl ObjectReader,
    bundle_only_reader: &impl ObjectReader,
    local_target: ObjectId,
    bundle_target: ObjectId,
) -> Result<BundlePreviewResult> {
    let local_ancestors = ancestors_inclusive(reader, local_target)?;
    let bundle_ancestors = ancestors_inclusive(bundle_only_reader, bundle_target)?;

    if !local_ancestors
        .keys()
        .any(|id| bundle_ancestors.contains_key(id))
    {
        return Ok(BundlePreviewResult {
            connectivity: BundleConnectivity::DoesNotConnect,
            conflict: None,
            effects: Vec::new(),
        });
    }

    if local_ancestors.contains_key(&bundle_target) {
        return Ok(BundlePreviewResult {
            connectivity: BundleConnectivity::AlreadyIncluded,
            conflict: None,
            effects: Vec::new(),
        });
    }

    if bundle_ancestors.contains_key(&local_target) {
        // Fast-forward: local_target is a real ancestor of bundle_target, so the bundle's own
        // candidate operations since local_target are, by construction, exactly what is new.
        let (current_files, current_live) =
            walk_and_replay(reader, &single_parent_chain(reader, local_target)?)?;
        let operations = candidate_operations(reader, local_target, bundle_target)?;
        let (effects, _) =
            apply_new_operations_and_diff(reader, current_files, current_live, operations)?;
        return Ok(BundlePreviewResult {
            connectivity: BundleConnectivity::FastForward,
            conflict: Some(ConflictAnswer::AppliesCleanly),
            effects,
        });
    }

    let lowest_common_ancestors = find_lowest_common_ancestors(&local_ancestors, &bundle_ancestors);
    let [baseline] = lowest_common_ancestors.as_slice() else {
        return Ok(BundlePreviewResult {
            connectivity: BundleConnectivity::Diverged,
            conflict: Some(ConflictAnswer::Undetermined {
                reason: format!(
                    "{} common ancestors share equal claim to being the merge base -- a genuinely \
                     ambiguous history this preview does not guess among",
                    lowest_common_ancestors.len()
                ),
            }),
            effects: Vec::new(),
        });
    };

    let (current_files, current_live) =
        walk_and_replay(reader, &single_parent_chain(reader, local_target)?)?;
    let operations = candidate_operations(reader, *baseline, bundle_target)?;
    match apply_new_operations_and_diff(reader, current_files, current_live, operations) {
        Ok((effects, _)) => Ok(BundlePreviewResult {
            connectivity: BundleConnectivity::Diverged,
            conflict: Some(ConflictAnswer::AppliesCleanly),
            effects,
        }),
        Err(err) => Ok(BundlePreviewResult {
            connectivity: BundleConnectivity::Diverged,
            conflict: Some(ConflictAnswer::Conflict {
                detail: err.to_string(),
            }),
            effects: Vec::new(),
        }),
    }
}

/// Apply `operations` onto a copy of the current replayed state and diff the result -- "current
/// state, plus the bundle's own patches applied in memory," in the design's own words. Returns
/// `Err` (never panics, never silently drops the failure) if applying fails partway; the caller
/// decides what that means for its own connectivity case.
fn apply_new_operations_and_diff(
    reader: &impl ObjectReader,
    current_files: ReplayedFiles,
    current_live: ReplayedLiveNodes,
    operations: Vec<DecodedPatchOperation>,
) -> Result<(Vec<BundleImpactEffect>, ReplayedFiles)> {
    let mut after_files = current_files.clone();
    let mut after_live = current_live.clone();
    let mut after_deleted = BTreeMap::new();
    apply_operation_sequence(
        reader,
        &mut after_files,
        &mut after_live,
        &mut after_deleted,
        operations,
    )?;
    let current_modes = modes_by_path(&current_live);
    let after_modes = modes_by_path(&after_live);
    Ok((
        diff_effects(&current_files, &current_modes, &after_files, &after_modes),
        after_files,
    ))
}

fn modes_by_path(live_nodes: &ReplayedLiveNodes) -> BTreeMap<String, u32> {
    live_nodes
        .values()
        .map(|node| (node.path.clone(), node.mode))
        .collect()
}

fn diff_effects(
    current_files: &ReplayedFiles,
    current_modes: &BTreeMap<String, u32>,
    after_files: &ReplayedFiles,
    after_modes: &BTreeMap<String, u32>,
) -> Vec<BundleImpactEffect> {
    let mut effects = Vec::new();
    for (path, current_bytes) in current_files {
        match after_files.get(path) {
            None => effects.push(BundleImpactEffect {
                path: path.clone(),
                kind: BundleImpactEffectKind::Deleted,
                current_bytes: Some(current_bytes.len() as u64),
                after_bytes: None,
            }),
            Some(after_bytes) if after_bytes != current_bytes => {
                effects.push(BundleImpactEffect {
                    path: path.clone(),
                    kind: BundleImpactEffectKind::Edited,
                    current_bytes: Some(current_bytes.len() as u64),
                    after_bytes: Some(after_bytes.len() as u64),
                });
            }
            Some(after_bytes) => {
                let current_mode = current_modes.get(path).copied();
                let after_mode = after_modes.get(path).copied();
                if current_mode != after_mode {
                    effects.push(BundleImpactEffect {
                        path: path.clone(),
                        kind: BundleImpactEffectKind::PermissionChanged,
                        current_bytes: Some(current_bytes.len() as u64),
                        after_bytes: Some(after_bytes.len() as u64),
                    });
                }
            }
        }
    }
    for (path, after_bytes) in after_files {
        if current_files.contains_key(path) {
            continue;
        }
        effects.push(BundleImpactEffect {
            path: path.clone(),
            kind: BundleImpactEffectKind::Created,
            current_bytes: None,
            after_bytes: Some(after_bytes.len() as u64),
        });
    }
    effects.sort_by(|left, right| left.path.cmp(&right.path));
    effects
}

#[cfg(test)]
mod tests;
