//! Shared worktree-materialization helpers for a producer that moves the **current branch** (0.56.0
//! step W3b, `.git-exclude/reviewed/0.56.0-step-3w3-review-v1.md`'s own ruling for R3).
//!
//! **The ordering the ruling gives, so this module's own split matches it exactly** (0.56.0 step
//! W3c-1, `.git-exclude/reviewed/0.56.0-step-3w3b-review-v1.md`'s own correction to W3b: the clean
//! check and the capture must run *before any object write*, since neither one reads anything about
//! the producer's own new state, and the marker must not be set until immediately before the
//! publish, so a refusal between the two leaves nothing written at all):
//! 1. Judge the worktree clean against the record, under the producer's existing lock, before any
//!    write at all -- not merely before the publish ([`require_worktree_clean_for_current_branch`]).
//! 2. Capture the files the current branch's own live tip is about to stop being (judged against the
//!    record exactly like [`crate::branch_switch`]'s own resync, since after step 3 that tip is no
//!    longer reachable as "the ref's live state" at all). Steps 1 and 2 are combined in
//!    [`require_worktree_clean_and_capture`], called once, after every one of the producer's own
//!    read-only refusals (its R4 check, any RFC 164 Rule D tail/damage check, and so on -- each of
//!    those must still refuse before this call ever runs, since it derives and caches lifecycle
//!    state as a side effect, which a refusal must not leave behind either), but before it builds
//!    or writes anything of its own. Separately, immediately before the producer's own publish
//!    (step 3 below) -- not bundled with steps 1-2 any more -- [`mark_current_branch_switch_pending`]
//!    marks the worktree dirty as a `Switch` to the current branch, using the
//!    `PendingCurrentBranchMaterialization` steps 1-2 already captured. The marker's own recorded
//!    way out, `branch switch <current branch>`, is exactly what finishes a crash anywhere from
//!    that point through [`materialize_after_publish`].
//! 3. The producer's own publish call -- unchanged, not this module's concern.
//! 4. Materialize the now-published tip through the ordinary verified path
//!    ([`materialize_after_publish`]): `replay_for_verified_worktree_write`, the identical function
//!    `branch_switch.rs`'s own ordinary (non-self-switch-exempted) path already uses, now reading the
//!    tip step 3 just published. Write, delete what the captured old baseline held but the new tip
//!    does not, record the new baseline, then clear the marker.
//!
//! **Why steps 1-2 need nothing about the producer's own new state:** both only read
//! `current_branch`'s own *current* (pre-producer) record and its live worktree files -- confirmed
//! by their own signatures, neither of which takes anything describing the producer's new content,
//! Block, RefState, or RefUpdate. This is exactly why they can run this early, with the producer's
//! own object-building work (e.g. `seal_block_at`) happening entirely in between the capture and the
//! marker being set.
//!
//! **Shared, not copied**, with `crate::branch_switch`: the dirty-entry classification
//! ([`dirty_worktree_entries`]), the record-aware "what did this ref's worktree last actually hold"
//! derivation ([`baseline_files_against_record_or_live`]), and the write/delete/in-the-way planning a
//! materialization performs ([`plan_materialization`]/[`apply_materialization_plan`]) are the exact
//! logic `branch_switch.rs`'s own `require_clean` and its ordinary materialization loop use -- moved
//! here so both files call one copy, confirmed by `branch_switch.rs` now calling every one of these
//! instead of its own former inline versions.
//!
//! **A file in the way must refuse before the first write too** (0.56.0 step W3c-3,
//! `.git-exclude/reviewed/0.56.0-step-3w3c-2-review-v1.md`'s own "W3c-3" correction): before this
//! round, [`plan_materialization`]'s own `in_the_way` check only ran inside
//! [`materialize_after_publish`] -- step 4, *after* the producer's own publish (step 3) -- so a
//! collision published the ref (or moved the tip), then refused, exactly what `branch_switch.rs`'s
//! own pre-write refusal (`branch_switch.rs:303-310`) has never done. [`require_nothing_in_the_way`]
//! runs the identical planner one step earlier, between steps 1-2 and step 3, against the target's
//! own manifest -- not `pending`'s `old_files` (step 2's own capture, unaffected) -- so a collision
//! refuses while nothing has been written or published yet. [`materialize_after_publish`]'s own
//! `in_the_way` check stays exactly as it is: the backstop for the one window this earlier check
//! cannot see into (a file created by a process outside prikk, between the two checks, under the
//! same lock).

use std::collections::BTreeMap;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeKind, ObjectId};

use crate::blob_access::ensure_blob_matches_node_kind;
use crate::foundation::fsutil::{
    EntryKind, MutationRoot, ensure_directory_required, inspect_entry, read_file_required,
    remove_worktree_file_required, set_regular_file_mode_required, stat_file_state_if_exists,
    write_worktree_file_atomically,
};
use crate::foundation::layout::RepositoryLayout;
use crate::node::node_lifecycle::{NodeContent, NodeLifecycleState};
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::{
    ReplayManifest, ReplayManifestEntry, SnapshotAnchorFallback,
    replay_for_verified_worktree_write, replay_for_verified_worktree_write_by_block_id,
    resolve_folded_worktree_baseline_with_own_cache, resolve_state_at_recorded_baseline,
};
use crate::wal::WalReplay;
use crate::worktree_baseline::{
    WorktreeBaselineComparison, compare_worktree_baseline, record_worktree_baseline,
};
use crate::worktree_marker::{MarkedTarget, clear_worktree_dirty, mark_worktree_dirty};
use crate::worktree_status::{
    WorktreeChange, WorktreeChangeKind, WorktreeStatusReport, worktree_status,
    worktree_status_against_recorded_baseline,
};

/// Entries in `report` that make the worktree not clean, in the shape every caller's own refusal
/// renders: every `Missing`/`Modified`/`UnsupportedPath` change not excused by `resumed`, plus every
/// live rename declaration. `Untracked` changes are never a reason to refuse. `branch_switch.rs`'s own
/// `require_clean` has a genuine resumption case (a switch already interrupted and run again); every
/// W3b producer has none, so its own `resumed` always answers `false`.
pub(crate) fn dirty_worktree_entries(
    report: &WorktreeStatusReport,
    resumed: impl Fn(&WorktreeChange) -> bool,
) -> Vec<String> {
    let mut dirty = Vec::new();
    for change in &report.changes {
        match change.kind {
            WorktreeChangeKind::Untracked => {}
            _ if resumed(change) => {}
            kind => dirty.push(format!("{} ({})", change.path, kind.as_str())),
        }
    }
    for declaration in &report.declarations {
        dirty.push(format!(
            "rename declaration {} -> {}",
            declaration.old_path, declaration.new_path
        ));
    }
    dirty
}

/// Step 1 of the R3 ruling: refuse unless `current_branch`'s worktree is clean against its own
/// record (or, lacking one, its live tip) -- the same comparison `branch_switch.rs`'s own
/// `require_clean` uses for a self-switch, with no resumption case (see [`dirty_worktree_entries`]'s
/// own doc for why).
pub(crate) fn require_worktree_clean_for_current_branch(
    layout: &RepositoryLayout,
    current_branch: &str,
    active_replay: &WalReplay,
    verb: &str,
) -> Result<()> {
    let comparison = compare_worktree_baseline(layout, current_branch, active_replay)?;
    let report = match &comparison {
        WorktreeBaselineComparison::Behind { recorded, .. } => {
            worktree_status_against_recorded_baseline(
                layout,
                current_branch,
                recorded,
                active_replay,
            )?
        }
        _ => worktree_status(layout, current_branch)?,
    };
    let dirty = dirty_worktree_entries(&report, |_| false);
    if dirty.is_empty() {
        return Ok(());
    }
    Err(PrikkError::Precondition(format!(
        "the worktree is not clean against {current_branch}: {}; commit it, or restore those files, \
         before {verb} (`prikk worktree-status` lists them)",
        dirty.join(", ")
    )))
}

/// [`crate::branch_switch`]'s own "the branch being left"'s files, judged against the worktree's
/// record whenever one exists and is stale, never only against the ref's own live tip -- moved here
/// (0.56.0 step W3b) since the W3b producers need exactly this calculation themselves, *before* they
/// publish: the instant they do, `ref_name`'s own live tip stops being the state this call must
/// describe.
pub(crate) fn baseline_files_against_record_or_live(
    layout: &RepositoryLayout,
    ref_name: &str,
    active_replay: &WalReplay,
) -> Result<BTreeMap<String, (ObjectId, NodeKind)>> {
    if let WorktreeBaselineComparison::Behind { recorded, .. } =
        compare_worktree_baseline(layout, ref_name, active_replay)?
    {
        let object_store = ObjectReadSnapshot::open(layout)?;
        let mut text_cache = crate::lifecycle_cache::replay::TextCache::new();
        let resolved = resolve_state_at_recorded_baseline(
            layout,
            &object_store,
            recorded.baseline,
            recorded.wal_sequence,
            active_replay,
            &mut text_cache,
        )?;
        return Ok(file_map_from_state(&resolved.state));
    }
    baseline_files(layout, ref_name, active_replay)
}

fn baseline_files(
    layout: &RepositoryLayout,
    ref_name: &str,
    active_replay: &WalReplay,
) -> Result<BTreeMap<String, (ObjectId, NodeKind)>> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let resolved = resolve_folded_worktree_baseline_with_own_cache(
        layout,
        &object_store,
        ref_name,
        active_replay,
    )?;
    Ok(file_map_from_state(&resolved.state))
}

fn file_map_from_state(state: &NodeLifecycleState) -> BTreeMap<String, (ObjectId, NodeKind)> {
    state
        .live_nodes()
        .filter_map(|(_, node)| match &node.content {
            NodeContent::File { blob_id, .. } => {
                Some((node.path.as_str().to_string(), (*blob_id, node.kind)))
            }
            _ => None,
        })
        .collect()
}

fn matches_blob(bytes: &[u8], blob: ObjectId, kind: NodeKind) -> bool {
    ensure_blob_matches_node_kind(bytes, blob, kind).is_ok()
}

/// The write/delete/in-the-way plan for replacing `old_files` with `target_manifest`'s files --
/// `branch_switch.rs`'s own ordinary-materialization loop (R2), moved here (0.56.0 step W3b) since the
/// W3b producers need the identical plan (a new tip's manifest replacing a ref's own prior baseline)
/// with nothing about "the branch being left" (a second, different ref) in it at all.
pub(crate) struct MaterializationPlan<'a> {
    pub(crate) writes: Vec<&'a ReplayManifestEntry>,
    pub(crate) unchanged: Vec<&'a ReplayManifestEntry>,
    pub(crate) deletes: Vec<String>,
    pub(crate) in_the_way: Vec<String>,
}

pub(crate) fn plan_materialization<'a>(
    root: &MutationRoot,
    target_manifest: &'a ReplayManifest,
    old_files: &BTreeMap<String, (ObjectId, NodeKind)>,
) -> Result<MaterializationPlan<'a>> {
    let target_files: BTreeMap<&str, &ReplayManifestEntry> = target_manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut writes = Vec::new();
    let mut unchanged = Vec::new();
    let mut deletes = Vec::new();
    let mut in_the_way = Vec::new();
    for entry in &target_manifest.files {
        let path = entry.path.as_str();
        match inspect_entry(root, Path::new(path))? {
            None => writes.push(entry),
            Some(EntryKind::Regular) => {
                let current = read_file_required(root, Path::new(path))?;
                if current == entry.bytes {
                    unchanged.push(entry);
                } else if old_files
                    .get(path)
                    .is_some_and(|(blob, kind)| matches_blob(&current, *blob, *kind))
                {
                    writes.push(entry);
                } else {
                    in_the_way.push(format!(
                        "{path} (a file that is not part of the prior state)"
                    ));
                }
            }
            Some(_) => in_the_way.push(format!("{path} (not a regular file)")),
        }
    }
    for (path, (blob, kind)) in old_files {
        if target_files.contains_key(path.as_str()) {
            continue;
        }
        match inspect_entry(root, Path::new(path))? {
            None => {}
            Some(EntryKind::Regular) => {
                let current = read_file_required(root, Path::new(path))?;
                if matches_blob(&current, *blob, *kind) {
                    deletes.push(path.clone());
                } else {
                    in_the_way.push(format!("{path} (changed since the prior state)"));
                }
            }
            Some(_) => in_the_way.push(format!("{path} (not a regular file)")),
        }
    }
    Ok(MaterializationPlan {
        writes,
        unchanged,
        deletes,
        in_the_way,
    })
}

pub(crate) fn apply_materialization_plan(
    root: &MutationRoot,
    plan: &MaterializationPlan<'_>,
) -> Result<()> {
    for entry in &plan.writes {
        let relative = Path::new(entry.path.as_str());
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        ensure_directory_required(root, parent)?;
        write_worktree_file_atomically(root, relative, &entry.bytes)?;
        set_regular_file_mode_required(root, relative, entry.mode)?;
    }
    for entry in &plan.unchanged {
        let relative = Path::new(entry.path.as_str());
        let mode = stat_file_state_if_exists(root, relative)?
            .and_then(|stat| stat.mode)
            .map(|mode| mode & 0o7777);
        if mode != Some(entry.mode & 0o7777) {
            set_regular_file_mode_required(root, relative, entry.mode)?;
        }
    }
    for path in &plan.deletes {
        remove_worktree_file_required(root, Path::new(path))?;
    }
    Ok(())
}

/// What step 1-2 capture, for step 4 ([`materialize_after_publish`]) to consume across the
/// producer's own publish (step 3) in between.
///
/// 0.56.0 step W3c-2: `pub`, not `pub(crate)` -- `branch create --from`'s own publish logic lives
/// in `prikk-cli`, not `prikk-store` (unlike `merge`/`archive import`/`sync seal`), so this type and
/// the two functions that produce/consume it are re-exported narrowly from `lib.rs`, the exact
/// precedent `switch_branch`/`worktree_status` already set, rather than moving `run_create`'s
/// publish logic into this crate to avoid the re-export (the ruling's own call: that would be a
/// refactor with no behaviour attached).
pub struct PendingCurrentBranchMaterialization {
    current_branch: String,
    old_files: BTreeMap<String, (ObjectId, NodeKind)>,
}

/// Steps 1 and 2 of the R3 ruling (0.56.0 step W3c-1: no longer bundled with the marker -- see
/// [`mark_current_branch_switch_pending`] for that): refuse unless `current_branch`'s worktree is
/// clean against its record (step 1); otherwise capture the files its own live tip currently holds
/// (step 2), the only two things either step needs. Must be called under the producer's own
/// active-WAL lock, **immediately after its own R4 check and before it builds or writes anything of
/// its own** -- this is what makes a refusal here leave nothing written: there is nothing to undo
/// yet.
pub fn require_worktree_clean_and_capture(
    layout: &RepositoryLayout,
    current_branch: &str,
    active_replay: &WalReplay,
    verb: &str,
) -> Result<PendingCurrentBranchMaterialization> {
    require_worktree_clean_for_current_branch(layout, current_branch, active_replay, verb)?;
    let old_files = baseline_files_against_record_or_live(layout, current_branch, active_replay)?;
    Ok(PendingCurrentBranchMaterialization {
        current_branch: current_branch.to_string(),
        old_files,
    })
}

/// The new pre-write check (0.56.0 step W3c-3): runs [`plan_materialization`]'s own planner --
/// the exact function [`materialize_after_publish`] already uses as its own backstop, no second
/// copy -- against `target_manifest` and `pending`'s own captured `old_files`, and refuses when
/// anything is in the way, before the producer's own first write. Must be called after
/// [`require_worktree_clean_and_capture`] (step 1-2) and before the producer's own publish (step
/// 3) -- the same placement [`mark_current_branch_switch_pending`] (step 2b) has, and indeed
/// before it too, so a refusal here never sets the dirty marker at all.
///
/// `pub(crate)`: `target_manifest`'s own type (`ReplayManifest`) stays `pub(crate)` -- a producer
/// whose publish logic lives outside this crate (`branch create --from`) cannot name it, so it
/// goes through the narrower, `pub` [`require_nothing_in_the_way_for_published_ref`] instead,
/// which never requires naming that type across the crate boundary. `execute_merge` (inside this
/// crate, 0.56.0 step W3c-3b) calls this directly: its own target manifest comes from
/// `derive_candidate`'s `entries` plus `text_cache`, converted the way `patch_replay::extract`'s
/// claim path builds its own file list -- see that module's own W3c-3b notes.
///
/// `already_written`: `None` means exactly what the message used to say unconditionally --
/// "(nothing was written)" -- true for every caller except `archive import --adopt`'s own
/// stage-3 backstop (0.56.0 step W3c-5b, defect 1): by the time that one call runs, stage 1 has
/// already admitted the archive's own objects unconditionally and the import journal already
/// names every container it touched, so the plain "(nothing was written)" claim would be false
/// there. `Some(text)` names what is actually true instead, verbatim, in place of that clause.
pub(crate) fn require_nothing_in_the_way(
    layout: &RepositoryLayout,
    pending: &PendingCurrentBranchMaterialization,
    target_manifest: &ReplayManifest,
    action: &str,
    already_written: Option<&str>,
) -> Result<()> {
    let root = layout.worktree_mutation_root();
    let plan = plan_materialization(root, target_manifest, &pending.old_files)?;
    if plan.in_the_way.is_empty() {
        return Ok(());
    }
    Err(PrikkError::Precondition(format!(
        "{} file(s) are in the way of {}'s files: {}; move them aside before {action} ({})",
        plan.in_the_way.len(),
        pending.current_branch,
        plan.in_the_way.join(", "),
        already_written.unwrap_or("nothing was written"),
    )))
}

/// `require_nothing_in_the_way` (above; not linked by name -- it is `pub(crate)`, and
/// `rustdoc::private_intra_doc_links` denies a public item's doc linking to one), for a producer
/// whose target is an **already-published ref** that is not `pending`'s own `current_branch` --
/// `branch create --from`'s own shape (0.56.0 step W3c-3): the target is the `--from` ref's tip,
/// since `current_branch` itself is not yet published at this point in that producer's own call
/// (that is exactly what this create would resolve). Re-derives the target's manifest through
/// `replay_for_verified_worktree_write` (also `pub(crate)`, same reason, not linked), the identical
/// verified-anchor replay [`materialize_after_publish`] already uses on `current_branch` once it is
/// published -- safe to use on `target_ref` here instead, because `target_ref` must already be
/// published to be nameable by `--from` at all (confirmed at the one call site:
/// `resolve_published_target` already refuses otherwise, before this is ever reached). `pub`,
/// re-exported narrowly from `lib.rs` -- the same precedent [`PendingCurrentBranchMaterialization`]
/// itself already set.
pub fn require_nothing_in_the_way_for_published_ref(
    layout: &RepositoryLayout,
    pending: &PendingCurrentBranchMaterialization,
    target_ref: &str,
    action: &str,
) -> Result<()> {
    let (target_replay, _anchor_fallback) = replay_for_verified_worktree_write(layout, target_ref)?;
    require_nothing_in_the_way(layout, pending, &target_replay.manifest, action, None)
}

/// As [`require_nothing_in_the_way_for_published_ref`], but for a target addressed by a bare
/// block id rather than an already-published ref name (0.56.0 step W3c-5): `archive import
/// --adopt`'s own shape. The chain of `RefUpdate`s this adoption is about to publish names its own
/// final tip as a real local `Block` (stage 1 already wrote every carried block unconditionally),
/// but nothing publishes it under any ref name -- least of all `pending`'s own `current_branch` --
/// until the caller's own publish loop runs, moments after this call returns. `reader` is the
/// caller's own already-open [`ObjectReader`] (an `ObjectWriteSession`, typically), not a fresh
/// [`ObjectReadSnapshot`], for the same reason
/// [`replay_for_verified_worktree_write_by_block_id`]'s own doc comment gives. `pub(crate)`, the
/// same reason [`require_nothing_in_the_way`] itself is: this producer's own publish logic lives
/// inside `prikk-store` (`archive::import`), unlike `branch create --from`'s, so no cross-crate
/// re-export is needed.
///
/// `already_written`: as [`require_nothing_in_the_way`]'s own parameter of the same name --
/// `archive import --adopt`'s two call sites (0.56.0 step W3c-5b) pass different values: the
/// early check, before the journal, passes `None` (truthfully nothing has been written yet); the
/// unchanged stage-3 backstop passes `Some` (stage 1's objects and the journal already exist by
/// then, every time it runs, not only in the rare race it now exists to catch).
pub(crate) fn require_nothing_in_the_way_for_block(
    layout: &RepositoryLayout,
    pending: &PendingCurrentBranchMaterialization,
    reader: &impl ObjectReader,
    point_name: &str,
    target_block_id: ObjectId,
    action: &str,
    already_written: Option<&str>,
) -> Result<()> {
    let (target_replay, _anchor_fallback) = replay_for_verified_worktree_write_by_block_id(
        layout,
        reader,
        point_name,
        target_block_id,
    )?;
    require_nothing_in_the_way(
        layout,
        pending,
        &target_replay.manifest,
        action,
        already_written,
    )
}

/// Step 2b of the R3 ruling (0.56.0 step W3c-1): mark the worktree dirty as a `Switch` to the
/// pending capture's own `current_branch`, using the value [`require_worktree_clean_and_capture`]
/// already produced. Called **immediately before the producer's own publish (step 3)** -- not
/// bundled with the clean check any more -- so the marker is set as late as it can be while still
/// bracketing the publish: a crash anywhere from this call through [`materialize_after_publish`]
/// leaves exactly the state `branch switch <current_branch>` (the marker's own named route)
/// resolves.
pub fn mark_current_branch_switch_pending(
    layout: &RepositoryLayout,
    pending: &PendingCurrentBranchMaterialization,
) -> Result<()> {
    mark_worktree_dirty(
        layout,
        &MarkedTarget::Switch(pending.current_branch.clone()),
    )
}

/// What [`materialize_after_publish`] did.
///
/// 0.56.0 step W3c-2: `pub`, with `pub` fields, for the same reason as
/// [`PendingCurrentBranchMaterialization`] -- `branch create --from`'s own CLI output
/// (`crates/prikk-cli/src/branch.rs::run_create`) reads these fields directly, the way `execute_merge`
/// reads them to fill `MergeExecutionReport`'s own five materialization fields.
#[non_exhaustive]
pub struct CurrentBranchMaterializeReport {
    /// Files created or replaced in the worktree.
    pub written_files: usize,
    /// Worktree files already present with identical bytes, found while materializing.
    pub unchanged_files: usize,
    /// Files the pre-publish worktree held that the newly published tip lacks, deleted while
    /// materializing.
    pub deleted_files: usize,
    /// The replay-verified snapshot the materialization could not anchor at, if any (RFC 136
    /// increment 2b).
    pub anchor_fallback: Option<SnapshotAnchorFallback>,
}

/// Step 4 of the R3 ruling: materialize `current_branch`'s now-published tip through the ordinary
/// verified path -- `replay_for_verified_worktree_write`, the identical function
/// `branch_switch.rs`'s own ordinary (non-self-switch-exempted) path already uses, now reading the tip
/// the producer's own step 3 just published, so this is not stale the way it would have been before
/// that publish. Writes, deletes what [`require_worktree_clean_and_capture`]'s captured old
/// baseline held but the new tip does not, records the new baseline, then clears the marker -- the
/// same order `branch_switch.rs` uses. Must be called only after the producer's own publish has
/// already durably returned; a crash at any point up to and including the `in_the_way` refusal below
/// leaves the marker set and the record stale, both of which `branch switch <current_branch>` (run
/// again) resolves by re-running this exact function.
pub fn materialize_after_publish(
    layout: &RepositoryLayout,
    pending: PendingCurrentBranchMaterialization,
) -> Result<CurrentBranchMaterializeReport> {
    let (target_replay, anchor_fallback) =
        replay_for_verified_worktree_write(layout, &pending.current_branch)?;
    let root = layout.worktree_mutation_root();
    let plan = plan_materialization(root, &target_replay.manifest, &pending.old_files)?;
    if !plan.in_the_way.is_empty() {
        return Err(PrikkError::Precondition(format!(
            "{} was published, but {} file(s) are in the way of materializing it into the worktree: \
             {}; move them aside, then run `prikk branch switch {}` to finish (nothing was written \
             just now)",
            pending.current_branch,
            plan.in_the_way.len(),
            plan.in_the_way.join(", "),
            pending.current_branch,
        )));
    }
    apply_materialization_plan(root, &plan)?;
    record_worktree_baseline(
        layout,
        &pending.current_branch,
        Some(target_replay.target_block_id),
        0,
        &[],
    )?;
    clear_worktree_dirty(layout)?;
    Ok(CurrentBranchMaterializeReport {
        written_files: plan.writes.len(),
        unchanged_files: plan.unchanged.len(),
        deleted_files: plan.deletes.len(),
        anchor_fallback,
    })
}

/// 0.56.0 step W3c-4b: when the batch that captured `pending` published nothing at all (every
/// claim either refused outright or was `AlreadySealed` -- see `prikk-cli`'s own `run_seal`, whose
/// `published_count` this answers for), there is no new tip for [`materialize_after_publish`] to
/// replay and nothing to write: the worktree, already confirmed clean against the pre-batch tip by
/// [`require_worktree_clean_and_capture`], is still exactly as correct as it was. Consumes
/// `pending` by value -- so it can never be passed to [`materialize_after_publish`], or anything
/// else, afterward -- and clears the dirty marker the producer's own
/// [`mark_current_branch_switch_pending`] set, the one write this still needs to undo. Does not
/// widen `worktree_marker`'s own module boundary: `clear_worktree_dirty` itself stays
/// `pub(crate)` there; this is simply the one `pub` caller outside that module allowed to ask for
/// it, in the same module [`PendingCurrentBranchMaterialization`] itself already lives in. Must be
/// called under the same held lock the producer's own batch holds, once the loop has confirmed it
/// published zero claims -- the repository is byte-identical before and after, by construction
/// (nothing is read here beyond the marker file itself, and nothing is written but its removal).
pub fn abandon_unpublished_materialization(
    layout: &RepositoryLayout,
    pending: PendingCurrentBranchMaterialization,
) -> Result<()> {
    drop(pending);
    clear_worktree_dirty(layout)
}

// Repository mutation is implemented on Linux, macOS and Windows (`platform-support.md`); tests that
// need a Linux-only, Unix-only or directory-sync mechanism carry their own narrower gate.
#[cfg(test)]
mod tests;
