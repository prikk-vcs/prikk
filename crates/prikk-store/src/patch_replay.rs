//! Minimal patch replay planning for supported file-level operations.
//!
//! PR-024 keeps a deliberately narrow replay boundary. It can reconstruct an in-memory snapshot
//! manifest by walking a single-parent block chain and applying `CreateFile`, `DeleteNode`,
//! `EditText`, `ReplaceBinary`, and `ChangePerm` operations (DC-73 wired the last two — see
//! `apply.rs` and `decode.rs::ensure_apply_supported`). Renames and symlinks remain unauthored
//! (`node_authoring.rs` never produces `RenamePath`; symlink authoring is refused outright), so
//! their apply paths stay deferred pending an authoring path, not the node model; merge algebra and
//! conflict handling remain later increments.
//!
//! Split across three files (DC-58): this file keeps the public API and baseline resolution;
//! `read.rs` holds object-store reading helpers (block-chain walking, blob/patch/snapshot
//! loading); `apply.rs` holds the per-operation state-fold logic. `decode.rs` (pre-existing) is
//! unchanged. No behaviour or public path changed by the split.

use std::collections::BTreeMap;

mod apply;
pub(crate) mod decode;
mod read;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeKind, ObjectId};

use crate::foundation::layout::RepositoryLayout;
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::path::RepoPath;
use crate::refs::RefStore;
use crate::snapshot::SnapshotManifest;
use crate::validate_local_branch_ref;
use crate::wal::WalReplay;

use apply::apply_decoded_operation;
use decode::decode_patch_operations;
use read::{
    current_target_block, files_to_manifest, files_to_replay_manifest, load_snapshot_files,
    read_block, read_patch, single_parent_chain,
};

/// Read-only result of replaying supported patch operations to an in-memory snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchReplayPlan {
    /// Ref used as the checkout target.
    pub ref_name: String,
    /// Target block ID.
    pub target_block_id: ObjectId,
    /// Number of blocks replayed from oldest to newest.
    pub block_count: usize,
    /// Number of patch objects replayed.
    pub patch_count: usize,
    /// Number of supported operations applied.
    pub applied_operation_count: usize,
    /// Number of files in the resulting manifest.
    pub file_count: usize,
    /// Total content bytes in the resulting manifest.
    pub total_content_bytes: u64,
    /// Repository-relative paths in the resulting manifest.
    pub paths: Vec<String>,
}

/// Replay the supported operation subset for a ref without writing the worktree.
pub fn prepare_patch_replay_plan(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<PatchReplayPlan> {
    let snapshot = replay_supported_patch_chain(layout, ref_name)?;
    let paths = snapshot
        .manifest
        .files
        .iter()
        .map(|entry| entry.path.as_str().to_string())
        .collect();
    Ok(PatchReplayPlan {
        ref_name: snapshot.ref_name,
        target_block_id: snapshot.target_block_id,
        block_count: snapshot.block_count,
        patch_count: snapshot.patch_count,
        applied_operation_count: snapshot.applied_operation_count,
        file_count: snapshot.manifest.files.len(),
        total_content_bytes: snapshot.manifest.total_content_bytes(),
        paths,
    })
}

/// RFC 143 §6: the replay's own structural coverage, exposed so a consumer can tell "the complete
/// state at this point" from "what the supported subset reconstructs" without parsing prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanCoverage {
    /// Operation kinds actually applied while reconstructing this point, in stable
    /// (`ShowOperation::kind`-style: `"create-file"`, `"edit-text"`, ...) form -- a subset of the
    /// five kinds this replay mechanism knows how to apply at all. Never includes `"rename-path"`
    /// or `"create-symlink"`: those, and a symlink `"delete-node"`, are refused outright
    /// (`ensure_apply_supported`) rather than silently applied and omitted, so a *successful*
    /// response is never a partial one that went unreported (RFC 143 §6b: the unsupported-kind
    /// case is an `Err`, propagated by `prepare_patch_plan_content_report` below, not folded into
    /// this field).
    pub applied_operation_kinds: Vec<&'static str>,
    /// Always `"single-parent"`: this replay mechanism walks one parent only at a block with more
    /// than one (a well-formed `Merge` block's own mainline, never the other sides). A future
    /// merge-aware replay would report something else here; today there is only one value.
    pub walk: &'static str,
}

/// One path's content, RFC 143 §5: binary is never rendered, exactly as `show` (RFC 142 §7)
/// already refuses to render it -- one project, one answer to that question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchPlanContent {
    /// A text file's own bytes, verbatim.
    Text(Vec<u8>),
    /// A binary file: id and declared size only, never content.
    Binary {
        /// The blob backing this file's current content.
        blob_id: ObjectId,
        /// Content length in bytes.
        size: u64,
    },
    /// Content the replayed window cannot classify as text or binary: a path seeded by a snapshot
    /// boundary and never subsequently touched by a node-addressed operation in the replayed
    /// window (see `ReplayManifestEntry`'s own doc for why -- `SnapshotEntry` itself carries no
    /// kind field to fall back on). Real bytes exist and their length is reported; rendering them
    /// as text would risk emitting non-UTF-8 bytes into a JSON string on a guess this crate has no
    /// basis for, and reporting them as binary would claim an identity (`blob_id`) this code does
    /// not actually have for them.
    Opaque {
        /// Content length in bytes.
        size: u64,
    },
}

/// Content at one requested path, at the point [`PatchPlanContentReport`] resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanContentEntry {
    /// Repository-relative path, exactly as requested.
    pub path: String,
    /// Mode bits.
    pub mode: u32,
    /// The content itself.
    pub content: PatchPlanContent,
}

/// RFC 143 §5: content at a replayed point, for exactly the requested paths -- never the whole
/// tree by default. An empty `requested_paths` yields metadata only: `entries` and `not_found` are
/// both empty, `coverage` is still populated. Deliberately not `ReplayManifest`/
/// `ReplayManifestEntry` widened to `pub`: RFC 131 applies, and this narrow, purpose-built type is
/// the minimum this feature needs -- the `show` round's own restraint (one narrow function,
/// nothing else widened) is the standard matched here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanContentReport {
    /// Ref used as the checkout target.
    pub ref_name: String,
    /// Target block ID.
    pub target_block_id: ObjectId,
    /// The replay's own structural coverage.
    pub coverage: PatchPlanCoverage,
    /// Content for every requested path found in the resulting manifest, in the order requested.
    pub entries: Vec<PatchPlanContentEntry>,
    /// Requested paths absent from the resulting manifest -- never existed, or removed by this
    /// point in history. Degraded, not an error: absence is the RFC 140 §7b / RFC 142 §6b case,
    /// distinguished from a real error, which still propagates via this function's own `Result`.
    pub not_found: Vec<String>,
}

/// Replay the supported operation subset for a ref and report content at exactly the requested
/// paths (RFC 143 §5) -- read-only, same as [`prepare_patch_replay_plan`]. An unsupported
/// operation anywhere in the walked chain still fails the whole call (`Err`; RFC 143 §6b/§6 names
/// this the third instance of RFC 140 §7b's rule): this never degrades a real error into a
/// partial-but-successful response, only absence (a requested path the manifest does not contain)
/// degrades, into `not_found`.
pub fn prepare_patch_plan_content_report(
    layout: &RepositoryLayout,
    ref_name: &str,
    requested_paths: &[String],
) -> Result<PatchPlanContentReport> {
    let snapshot = replay_supported_patch_chain(layout, ref_name)?;
    let by_path: BTreeMap<&str, &ReplayManifestEntry> = snapshot
        .manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut entries = Vec::with_capacity(requested_paths.len());
    let mut not_found = Vec::new();
    for requested in requested_paths {
        match by_path.get(requested.as_str()) {
            Some(entry) => {
                let content = match (entry.kind, entry.blob_id) {
                    (Some(NodeKind::BinaryFile), Some(blob_id)) => PatchPlanContent::Binary {
                        blob_id,
                        size: entry.bytes.len() as u64,
                    },
                    (Some(NodeKind::TextFile), _) => PatchPlanContent::Text(entry.bytes.clone()),
                    _ => PatchPlanContent::Opaque {
                        size: entry.bytes.len() as u64,
                    },
                };
                entries.push(PatchPlanContentEntry {
                    path: requested.clone(),
                    mode: entry.mode,
                    content,
                });
            }
            None => not_found.push(requested.clone()),
        }
    }
    Ok(PatchPlanContentReport {
        ref_name: snapshot.ref_name,
        target_block_id: snapshot.target_block_id,
        coverage: PatchPlanCoverage {
            applied_operation_kinds: snapshot.applied_operation_kinds.into_iter().collect(),
            walk: "single-parent",
        },
        entries,
        not_found,
    })
}

/// One file entry in a replay-derived manifest, carrying the mode bits `CreateFile`/`ChangePerm`
/// recorded (DC-73). Deliberately **not** `crate::snapshot::SnapshotEntry`: that type is also what
/// `SnapshotManifest::decode` reads from a stored snapshot Blob's wire bytes, which have no mode
/// field of their own — adding one to the shared type would force a default on the decode side for
/// a value the stored bytes never contained. This type exists only in memory, built by replaying
/// operations, and never crosses the object-format boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplayManifestEntry {
    /// Validated repository-relative path.
    pub(crate) path: RepoPath,
    /// File content bytes.
    pub(crate) bytes: Vec<u8>,
    /// Mode bits, as recorded by the most recent `CreateFile`/`ChangePerm` for this node.
    pub(crate) mode: u32,
    /// This entry's node kind, when the replayed window's own live-node tracking has one --
    /// `None` for a path seeded by a snapshot boundary and never subsequently touched by a
    /// node-addressed operation (`replay_supported_patch_chain` clears `live_nodes` at each
    /// snapshot without repopulating it from the loaded snapshot, and `SnapshotEntry` itself
    /// carries no kind field to fall back on -- RFC 143 §5's own content report treats this as a
    /// third state, neither text nor binary, rather than guessing). Never `Symlink`: a symlink
    /// node never enters `files`/`live_nodes` at all, since both `DeleteNode(symlink)` and
    /// `CreateSymlink` are refused by `ensure_apply_supported` before application.
    pub(crate) kind: Option<NodeKind>,
    /// The blob id backing this entry's *current* content -- `Some` only when `kind` is
    /// `Some(NodeKind::BinaryFile)` (see [`super::apply::ReplayLiveNode`]'s own doc for why a
    /// text node's tracked id would not be trustworthy here even if carried through).
    pub(crate) blob_id: Option<ObjectId>,
}

/// Replay-derived manifest, sorted by path. See [`ReplayManifestEntry`] for why this is not
/// `crate::snapshot::SnapshotManifest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplayManifest {
    /// File entries, sorted by path.
    pub(crate) files: Vec<ReplayManifestEntry>,
}

impl ReplayManifest {
    pub(crate) fn total_content_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|entry| entry.bytes.len() as u64)
            .sum()
    }
}

/// In-memory replay result used by patch checkout materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchReplaySnapshot {
    /// Ref used as the replay target.
    pub(crate) ref_name: String,
    /// Target block ID.
    pub(crate) target_block_id: ObjectId,
    /// Number of blocks replayed from oldest to newest.
    pub(crate) block_count: usize,
    /// Number of patch objects replayed.
    pub(crate) patch_count: usize,
    /// Number of supported operations applied.
    pub(crate) applied_operation_count: usize,
    /// Distinct kinds of operation actually applied in this replay window (RFC 143 §6) -- a
    /// subset of the five `ensure_apply_supported` admits, since a given repository's own history
    /// may never exercise all five.
    pub(crate) applied_operation_kinds: std::collections::BTreeSet<&'static str>,
    /// Resulting file manifest, mode-aware (DC-73).
    pub(crate) manifest: ReplayManifest,
    /// Files explicitly removed by replayed patches and still absent in the final manifest.
    pub(crate) deleted_files: Vec<PatchReplayDeletedFile>,
    /// Latest snapshot baseline used as the rollback-preview target for the supported replay
    /// window. Mode-unaware like the wire-decoded snapshot format it is seeded from — the
    /// rollback-preview consumer compares paths and bytes only.
    pub(crate) baseline_manifest: SnapshotManifest,
}

/// A file explicitly deleted while replaying the supported patch subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchReplayDeletedFile {
    /// Validated repository-relative path that was removed.
    pub(crate) path: RepoPath,
    /// Blob ID recorded as the delete precondition.
    pub(crate) old_blob_id: ObjectId,
    /// Bytes that must still be present before an opt-in destructive delete may occur.
    pub(crate) old_bytes: Vec<u8>,
}

/// Replay the supported operation subset into a validated in-memory manifest.
pub(crate) fn replay_supported_patch_chain(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<PatchReplaySnapshot> {
    // RFC 111 §6.1: safe as a read-only snapshot because every production caller
    // (`patch_checkout.rs`, `rollback_preview.rs`) only reads -- neither ever writes an object.
    // If a future caller reaches this from a writing operation, confirm its own write happens
    // after this function returns (Stage 1 review v1 §4) before assuming this stays safe.
    let object_store = ObjectReadSnapshot::open(layout)?;
    let target_block_id = current_target_block(layout, &object_store, ref_name)?;
    let block_ids = single_parent_chain(&object_store, target_block_id)?;
    let mut files = BTreeMap::new();
    let mut live_nodes = BTreeMap::new();
    let mut deleted_files = BTreeMap::new();
    let mut patch_count = 0_usize;
    let mut applied_operation_count = 0_usize;
    let mut applied_operation_kinds = std::collections::BTreeSet::new();
    let mut baseline_files = BTreeMap::new();

    for block_id in &block_ids {
        let block = read_block(&object_store, *block_id)?;
        if let Some(snapshot_blob_ref) = block.snapshot_blob_ref {
            files = load_snapshot_files(&object_store, snapshot_blob_ref)?;
            live_nodes.clear();
            baseline_files = files.clone();
            deleted_files.clear();
        }
        for patch_id in block.patch_ids {
            let patch = read_patch(&object_store, patch_id)?;
            let operations =
                decode_patch_operations(&patch.canonical_payload, patch.schema_version)?;
            for operation in operations {
                // Captured before the operation is consumed below, and only kept if the apply
                // that follows actually succeeds -- a kind `ensure_apply_supported` admits can
                // still fail apply's own further validation (a real Integrity error), and that
                // must propagate rather than being recorded as "covered".
                let kind_label = decode::applied_operation_kind_label(&operation.kind);
                apply_decoded_operation(
                    &object_store,
                    &mut files,
                    &mut live_nodes,
                    &mut deleted_files,
                    operation,
                )?;
                applied_operation_kinds.insert(kind_label);
                applied_operation_count += 1;
            }
            patch_count += 1;
        }
    }

    Ok(PatchReplaySnapshot {
        ref_name: ref_name.to_string(),
        target_block_id,
        block_count: block_ids.len(),
        patch_count,
        applied_operation_count,
        applied_operation_kinds,
        manifest: files_to_replay_manifest(files, &live_nodes)?,
        deleted_files: deleted_files.into_values().collect(),
        baseline_manifest: files_to_manifest(baseline_files)?,
    })
}

/// Resolve the node-addressed lineage bounds for a ref: the current target block (baseline) and the
/// lineage genesis (horizon). Worktree authoring (4.4a-2) supplies these to `replay_derived_state`
/// so the baseline node lifecycle state is reconstructed from authoritative node-addressed history,
/// never from a snapshot manifest. For a well-formed `Merge` block (DC-75), `single_parent_chain`
/// follows its mainline parent only; it fails closed only on a malformed multi-parent block (a
/// non-`Merge` block with more than one parent, or a `Merge` block with a missing or invalid
/// mainline) — see `read::mainline_or_sole_parent`.
pub(crate) fn resolve_node_lineage_bounds(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(ObjectId, ObjectId)> {
    // RFC 111 §6.1: safe as a read-only snapshot even though this is reached from a writing
    // operation (`node_authoring.rs::author_worktree_patch`, via `resolve_worktree_baseline`) --
    // this function's own read completes and the snapshot is dropped entirely before that
    // caller's own writes begin later in the same call (Stage 1 review v1 §4). If this call is
    // ever hoisted to live across that caller's writes, or a new caller writes before calling
    // this, that guarantee breaks silently -- check this comment still describes reality before
    // assuming it's still safe.
    let object_store = ObjectReadSnapshot::open(layout)?;
    let baseline = current_target_block(layout, &object_store, ref_name)?;
    let chain = single_parent_chain(&object_store, baseline)?;
    let horizon = *chain
        .first()
        .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} lineage is empty")))?;
    Ok((baseline, horizon))
}

/// Baseline context for worktree authoring: either a published node-addressed lineage, or a genesis
/// (first-commit) context with no baseline at all.
pub(crate) enum WorktreeBaseline {
    /// The ref is published; author against replay-derived node lifecycle state.
    Published {
        /// Current target block (baseline).
        baseline_block: ObjectId,
        /// Lineage genesis (horizon).
        horizon: ObjectId,
    },
    /// The ref has never been published; author against an empty baseline (all `CreateFile`).
    Genesis,
}

/// Decide whether worktree authoring runs against a published lineage or a genesis (first-commit)
/// context (DC-09 4.4b). Genesis is selected **only** when the target ref has never been published:
/// the ref pointer is absent **and** the ref log is readable and empty. A missing pointer with any
/// ref-log history, or an unreadable/partial ref log, is treated as corruption - not genesis - and
/// fails closed with preserve/restore guidance (design §4, review E2). The active-WAL guard (review E1) is the
/// authoring caller's responsibility and is enforced there.
pub(crate) fn resolve_worktree_baseline(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<WorktreeBaseline> {
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    let ref_store = RefStore::new(layout.clone());
    if ref_store
        .read_current_ref_state_id(&canonical_ref)?
        .is_some()
    {
        let (baseline_block, horizon) = resolve_node_lineage_bounds(layout, &canonical_ref)?;
        return Ok(WorktreeBaseline::Published {
            baseline_block,
            horizon,
        });
    }
    // Pointer absent: genesis only if the log is readable and empty; otherwise corruption.
    // An absent log is decoded as an empty log by `RefStore::replay_log`; unreadable, malformed, or
    // partial logs remain corruption, not genesis.
    let log = ref_store.replay_log(&canonical_ref).map_err(|err| {
        PrikkError::Integrity(format!(
            "ref {canonical_ref} log is unreadable; run `prikk doctor` before committing ({err})"
        ))
    })?;
    if log.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "ref {canonical_ref} pointer is missing and its log has trailing partial bytes; \
             run `prikk doctor` (this is not a genesis repository)"
        )));
    }
    // RFC 102 Stage 2: checked before the emptiness check below -- a damaged sole record would
    // otherwise read as `log.records.is_empty()`, and the worst possible misclassification here is
    // exactly that: authoring against an empty (Genesis) baseline for a ref that actually has
    // history.
    if log.has_item_failure() {
        return Err(PrikkError::Integrity(format!(
            "ref {canonical_ref} pointer is missing and its log has a damaged record; \
             run `prikk doctor` (this is not a genesis repository)"
        )));
    }
    if !log.records.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "ref {canonical_ref} pointer is missing but ref-log history exists; \
             preserve the repository and restore from backup (this is not a genesis repository)"
        )));
    }
    Ok(WorktreeBaseline::Genesis)
}

/// The baseline lifecycle state a worktree operation should compare or author against: the sealed
/// baseline (or an empty genesis state), with any already-queued (unsealed) patches for this ref
/// folded on top (DC-66) exactly as `commit` folds them.
pub(crate) struct FoldedWorktreeBaseline {
    /// Baseline lifecycle state, with the active queue folded on top when it belongs to this ref.
    pub(crate) state: NodeLifecycleState,
    /// `Some((baseline_block, horizon))` when the ref is published; `None` for a genesis baseline.
    pub(crate) lineage: Option<(ObjectId, ObjectId)>,
    /// `Some(other_ref)` when the active WAL is non-empty but owned by a ref other than the one
    /// requested — correctly *not* folded into `state` (another ref's queue is not part of this
    /// ref's baseline), but a caller may still want to say so: that queue is real, committed work,
    /// not a stray file, even though it plays no part in this baseline (RFC 122
    /// `replay-baseline-handoff-v2-amendment.md` §4).
    pub(crate) queued_on_other_ref: Option<String>,
}

/// **The single derivation every worktree-comparing command uses** (RFC 122 §3,
/// `replay-baseline-handoff-v1.md`): `commit` (`node_authoring.rs`) and `worktree-status`
/// (`worktree_status.rs`) both call this rather than each reconstructing baseline state their own
/// way — a second implementation that happens to agree today is the defect RFC 122 fixes, one
/// release later. `active_replay` is a parameter, not read here, because `commit` already has its
/// own copy in scope for its own, authoring-specific checks (WAL tail/damage/patch-limit) and
/// reading it twice would cost a second WAL replay for no reason; `worktree-status` has no such
/// copy yet and reads one itself before calling this.
///
/// **Folding decision, deliberately not `require_active_ref_for_non_empty_wal`'s own decision**:
/// that function refuses outright when the active WAL belongs to a different ref, which is correct
/// for `commit` (it is about to append to that WAL and single-active-session ownership must be
/// unambiguous before it does) but wrong for a read-only query about one specific ref — a queue
/// that belongs to some *other* ref is simply irrelevant to this ref's status, not an error. Only
/// genuinely ambiguous ownership (non-empty WAL, no readable owner) is refused here, with the same
/// classification and wording `require_active_ref_for_non_empty_wal` already uses for that case —
/// by the time `commit` reaches this function it has already called that stricter check itself
/// (`author_inner`, before baseline resolution begins), so a non-empty queue there is always
/// already known to belong to this ref, and this function's own ownership check is a harmless,
/// always-true re-confirmation for that caller, not a behavior change.
///
/// `text_cache` is a parameter, not created here, because `commit` (`node_authoring.rs`) reuses its
/// own cache afterward for `plan_edit_text`'s own text materialization — folding into a cache the
/// caller keeps, not a throwaway internal to this call, so that later reuse still sees what folding
/// already materialized. `worktree-status` has no further use for it and passes a fresh, empty one.
pub(crate) fn resolve_folded_worktree_baseline(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    active_replay: &WalReplay,
    text_cache: &mut crate::lifecycle_cache::replay::TextCache,
) -> Result<FoldedWorktreeBaseline> {
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    let baseline = resolve_worktree_baseline(layout, &canonical_ref)?;
    let lineage = match &baseline {
        WorktreeBaseline::Published {
            baseline_block,
            horizon,
        } => Some((*baseline_block, *horizon)),
        WorktreeBaseline::Genesis => None,
    };
    let mut state = match &baseline {
        WorktreeBaseline::Published {
            baseline_block,
            horizon,
        } => crate::lifecycle_cache::incremental::resolve_baseline_state(
            layout,
            object_store,
            *baseline_block,
            *horizon,
        )?
        .state()
        .clone(),
        WorktreeBaseline::Genesis => NodeLifecycleState::new(),
    };

    let mut queued_on_other_ref = None;
    if !active_replay.records.is_empty() {
        match crate::read_active_ref_metadata(layout)? {
            crate::ActiveRefMetadata::Valid(actual) if actual == canonical_ref => {
                crate::lifecycle_cache::replay::apply_queued_patch_envelopes(
                    object_store,
                    &active_replay.records,
                    &mut state,
                    text_cache,
                    lineage,
                )?;
            }
            crate::ActiveRefMetadata::Valid(other) => {
                queued_on_other_ref = Some(other);
            }
            crate::ActiveRefMetadata::Missing => {
                return Err(PrikkError::Integrity(
                    "active WAL has records but active ref metadata is missing".to_string(),
                ));
            }
            crate::ActiveRefMetadata::Invalid(reason) => {
                return Err(PrikkError::Integrity(format!(
                    "active WAL has records but active ref metadata is malformed: {reason}"
                )));
            }
        };
    }

    Ok(FoldedWorktreeBaseline {
        state,
        lineage,
        queued_on_other_ref,
    })
}

#[cfg(test)]
pub(crate) mod tests;
