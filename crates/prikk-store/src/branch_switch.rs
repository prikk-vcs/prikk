//! RFC 151 §2.3: `prikk branch switch heads/<name>` -- move the worktree and the current-branch
//! pointer from one branch to another, losing nothing.
//!
//! **Refusals, in order, each `Precondition` naming the way out:**
//! 1. the target is not an existing local branch, or is closed -- except a self-switch whose own
//!    comparison already reads `Behind` against an unpublished target (0.56.0 step W3-0): RFC 147
//!    §2i's own "an unpublished current branch is never 'does not exist'" extends here too, and
//!    the switch materializes the target's empty (Genesis) state instead;
//! 2. the active WAL holds unsealed work owned by a ref other than the target;
//! 3. the worktree is not clean against the branch being left (`worktree_status`'s own report --
//!    untracked files are not a difference this refuses on; they are never touched);
//! 4. a file the target needs is in the way: an untracked file, or a non-file, at a path the
//!    target writes. Checked for every path before the first write, so a refusal writes nothing.
//!
//! **The transition.** Create or replace every file of the target's replay baseline (the one
//! `checkout --patch-plan` derives), then delete each file of the old baseline the target lacks --
//! **only when its bytes still equal the old baseline's**, which after refusal 3 is every one of
//! them. Untracked files are never read for deletion. The pointer is written **last**.
//!
//! **Crash posture.** Writes are atomic per file, and the whole transition -- writes, deletions and
//! the pointer -- sits inside the worktree-dirty marker, which is cleared **after** the pointer is
//! written. So an interrupted switch leaves no torn file and the marker set (which makes `commit`
//! refuse until the worktree is re-verified), and:
//! - interrupted in the write phase: the pointer names the old branch and every old file the target
//!   lacks is still present;
//! - interrupted in the delete phase or at the pointer write: the pointer names the old branch and
//!   every target file is already in place;
//! - interrupted at the marker clear: the pointer already names the target.
//!
//! **Running the same switch again completes it.** While the marker is set, refusal 3 also accepts a
//! path already holding the target's bytes and a path already absent that the target lacks -- exactly
//! the states an interrupted switch leaves, and states in which switching loses nothing, since those
//! paths end where the target puts them -- and "already on the target" is not a no-op until the
//! marker has been cleared.
//!
//! **An unresolvable pointer** (malformed, or naming a missing branch) passes `from = None`: there is
//! no old baseline, so refusal 3 cannot be asked, no file counts as replaceable and nothing is
//! deleted -- the target's files are written only where absent or already identical. That keeps
//! `doctor`'s route ("run `prikk branch switch`") usable without risking a byte.

use std::collections::BTreeMap;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeKind, ObjectId, ObjectType, RefStatePayload};

use crate::blob_access::ensure_blob_matches_node_kind;
use crate::foundation::fsutil::{
    EntryKind, ensure_directory_required, inspect_entry, read_file_required,
    remove_worktree_file_required, rewrite_in_place_or_create_required,
    set_regular_file_mode_required, stat_file_state_if_exists, write_worktree_file_atomically,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::{ReplayManifest, ReplayManifestEntry, SnapshotAnchorFallback};
use crate::refs::{RefStore, validate_local_branch_ref};
use crate::wal::Wal;
use crate::worktree_marker::{
    MarkedTarget, clear_worktree_dirty, mark_worktree_dirty, worktree_is_dirty,
};
use crate::worktree_status::{WorktreeChangeKind, worktree_status};
use crate::{ActiveRefOwnership, active_ref_ownership};

/// What a completed `branch switch` did.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSwitchReport {
    /// The branch left, when the pointer could be resolved; `None` otherwise.
    pub from: Option<String>,
    /// The branch the pointer now names.
    pub to: String,
    /// Files created or replaced with the target's bytes.
    pub written_files: usize,
    /// Target files already present with identical bytes.
    pub unchanged_files: usize,
    /// Files of the old baseline the target lacks, removed.
    pub deleted_files: usize,
    /// `true` when `to` was already the current branch and nothing was done.
    pub already_current: bool,
    /// 0.57.0 step 1c: `true` when `to` was already the current branch, the worktree's recorded
    /// baseline was absent or ignored, and the worktree is clean against the branch -- so the switch
    /// wrote the record (and nothing else). `false` otherwise.
    pub worktree_record_restored: bool,
    /// The replay-verified snapshot the switch could not anchor at, if any (RFC 136 increment 2b). The
    /// switch replayed from genesis instead; the worktree is the same either way.
    pub anchor_fallback: Option<SnapshotAnchorFallback>,
}

/// Switch the worktree and the current branch from `from` to `target`. See the module doc for the
/// refusals, the order of writes and the crash posture.
pub fn switch_branch(
    layout: &RepositoryLayout,
    from: Option<&str>,
    target: &str,
) -> Result<BranchSwitchReport> {
    layout.require_current_format()?;
    let target = validate_local_branch_ref(target)?;
    let _lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    // RFC 136 §10.3b.3: the derivation gate, before any write.
    crate::worktree_marker::ensure_worktree_replay_verified(layout)?;

    // RFC 147 §2j: naming the branch already current is a no-op whether or not it has ever been
    // sealed -- checked *before* refusal 1, which is about a target that names something else. The
    // unpublished current branch is never "does not exist" here (RFC 147 §2i's own ruling), so this
    // must not funnel through that refusal even by accident.
    //
    // 0.56.0 step W2 (R2): amended -- naming the current branch is a no-op only when the record also
    // shows this worktree is not behind it. When it is (some other command moved the ref without
    // touching the worktree), falling through to the ordinary materialization below is the resync:
    // this is what dissolves §2.7's deadlock ("switch to yourself" used to be an unconditional
    // no-op, so there was no command that could ever bring a stale worktree back in sync). A cheap,
    // separate WAL read here (not the one `active_replay` below becomes) -- its own soundness is
    // refusal 2's job, just below, not this check's.
    if from == Some(target.as_str()) && !worktree_is_dirty(layout)? {
        let probe_replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
        let comparison =
            crate::worktree_baseline::compare_worktree_baseline(layout, &target, &probe_replay)?;
        let current = matches!(
            comparison,
            crate::worktree_baseline::WorktreeBaselineComparison::NoRecord
                | crate::worktree_baseline::WorktreeBaselineComparison::Current
        );
        if current {
            // 0.57.0 step 1c: an absent or ignored record is restored here, but only when the
            // worktree is clean against the branch -- the one condition under which "last written
            // from this tip" is true. Otherwise nothing is written, as before.
            let worktree_record_restored = if matches!(
                comparison,
                crate::worktree_baseline::WorktreeBaselineComparison::NoRecord
            ) {
                match plan_worktree_record_restore(layout, &target, &probe_replay)? {
                    WorktreeRecordRestore::Restorable(identity) => {
                        crate::worktree_baseline::write_worktree_baseline_identity(
                            &identity, layout,
                        )?;
                        true
                    }
                    WorktreeRecordRestore::NotClean => false,
                }
            } else {
                false
            };
            return Ok(BranchSwitchReport {
                from: Some(target.clone()),
                to: target,
                written_files: 0,
                unchanged_files: 0,
                deleted_files: 0,
                already_current: true,
                worktree_record_restored,
                anchor_fallback: None,
            });
        }
    }

    // Refusal 1: an existing, open local branch -- except a self-switch whose own comparison
    // already reads `Behind` against a target with no published `RefState` at all (0.56.0 step
    // W3-0). RFC 147 §2i's own reasoning ("an unpublished current branch is never 'does not
    // exist'") already governs the no-op path just above; it has no equivalent here, so this
    // self-switch refused unconditionally even though the no-op path would treat the same target
    // as existing were the comparison `Current`/`NoRecord` instead of `Behind` -- exactly the
    // `worktree-patch.md` walkthrough's own stuck shape (`commit --ref heads/topic` on a
    // genuinely unborn ref, then `seal`, leaves `heads/main` unpublished and the record `Behind`
    // it). A cheap, separate WAL read here, mirroring the no-op check's own `probe_replay` just
    // above -- its own soundness is refusal 2's job, just below, not this check's. Scoped as
    // narrowly as the shape itself: any other target lacking a published `RefState` (one never
    // named by `commit --ref`, or a genuinely nonexistent branch) still refuses exactly as before,
    // since `from == Some(target.as_str())` is false for it.
    let self_switch_unpublished_behind = from == Some(target.as_str())
        && RefStore::new(layout.clone())
            .read_current_ref_state_id(&target)?
            .is_none()
        && matches!(
            crate::worktree_baseline::compare_worktree_baseline(
                layout,
                &target,
                &Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?,
            )?,
            crate::worktree_baseline::WorktreeBaselineComparison::Behind { .. }
        );
    if !self_switch_unpublished_behind {
        require_open_branch(layout, &target)?;
    }

    // Refusal 2: unsealed work belongs to the target or to nobody.
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let active_replay = wal.replay()?;
    if active_replay.trailing_partial_bytes != 0 || active_replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "the active WAL is damaged or has a partial tail; run `prikk doctor` before switching"
                .to_string(),
        ));
    }
    if !active_replay.records.is_empty() {
        if let ActiveRefOwnership::OwnedByOther(owner) = active_ref_ownership(layout, &target)? {
            return Err(PrikkError::Precondition(format!(
                "the active WAL holds unsealed work for {owner}; seal it (`prikk seal \
                 --allow-no-audit --ref {owner}`) before switching to {target}"
            )));
        }
    }
    // 0.56.0 step W2d-2: a **self-switch** (`from == Some(target)`) whose active WAL is non-empty
    // and whose queued fold has moved (`compare_worktree_baseline` reads `Behind`) now refuses here,
    // before any write, rather than materializing. W2c's own fold-materializing branch (removed by
    // this round -- see the deleted `patch_replay::folded_materialize` module and this file's own
    // git history) built the sealed portion of what it wrote through
    // `resolve_folded_worktree_baseline`, the lifecycle cache -- never through
    // `replay_for_verified_worktree_write`'s own RFC 136 verified-anchor gate, the way every other
    // worktree-overwriting path in this function does. A resync that bypasses that gate is a
    // destructive write on an unverified derivation (the review's own words). Refusal 2, just
    // above, has already confirmed a non-empty active WAL here is owned by `target` itself, so this
    // check only ever fires for the ref actually being switched to -- never another ref's work.
    // The way out this names (`seal`, then `branch switch` again) drains the queue into a new
    // sealed tip, after which the active WAL is empty, this condition no longer applies, and the
    // ordinary materialization below (always `replay_for_verified_worktree_write`) handles it
    // through the verified path, same as any other switch. The fold's own content-identity digest
    // (`WorktreeBaselineIdentity::queued_fold`, compared by `compare_worktree_baseline`) is kept --
    // it is what *detects* this case; only the materialization branch that tried to *act* on the
    // detection by writing unverified bytes is removed.
    if from == Some(target.as_str()) && !active_replay.records.is_empty() {
        if let crate::worktree_baseline::WorktreeBaselineComparison::Behind { .. } =
            crate::worktree_baseline::compare_worktree_baseline(layout, &target, &active_replay)?
        {
            return Err(PrikkError::Precondition(format!(
                "the active WAL holds a queued fold for {target} that has moved without its \
                 sealed block moving; a self-switch cannot resync it without bypassing the \
                 verified-anchor write gate. Run `prikk seal --allow-no-audit --ref {target}` to \
                 drain the queue, then `prikk branch switch {target}` again (nothing was written)"
            )));
        }
    }

    // RFC 136 increment 2b: the target's tree may start only at a replay-verified snapshot --
    // except the self-switch-to-unpublished-`Behind` shape just exempted from refusal 1 above
    // (0.56.0 step W3-0), whose target has no published tip to anchor replay at in the first
    // place. Its true "sealed tip" is RFC 147 §2i's own Genesis: an empty baseline, the same shape
    // `resolve_worktree_baseline`'s own `Genesis` branch (`patch_replay.rs`) already gives an
    // unpublished ref elsewhere in this codebase (`commit --ref <unborn ref>`'s own authoring
    // path). `replay_for_verified_worktree_write` itself errors ("ref ... is not published") for
    // exactly this ref, so it is not called for this one case -- the empty manifest is built by
    // hand instead, with no baseline block and no anchor to have fallen back from.
    let (target_manifest, target_baseline_block_id, anchor_fallback): (
        ReplayManifest,
        Option<ObjectId>,
        Option<SnapshotAnchorFallback>,
    ) = if self_switch_unpublished_behind {
        (ReplayManifest { files: Vec::new() }, None, None)
    } else {
        let (target_replay, anchor_fallback) =
            crate::patch_replay::replay_for_verified_worktree_write(layout, &target)?;
        (
            target_replay.manifest,
            Some(target_replay.target_block_id),
            anchor_fallback,
        )
    };
    // 0.56.0 step W2d-2: every switch -- including a self-switch -- now always writes exactly the
    // sealed-tip-only, verified-anchor manifest. `recorded_queued_ids`/`recorded_wal_sequence` are
    // therefore always empty/`0` here, as they were before W2c's now-removed branch: the one case
    // that would have recorded a non-empty fold (self-switch, non-empty queue, `Behind`) is refused
    // above instead of reaching this point.
    let recorded_queued_ids: Vec<ObjectId> = Vec::new();
    let recorded_wal_sequence: u64 = 0;
    // 0.57.0 U3: the same collision rule every other materializer asks, before any decision below and any write.
    crate::worktree::refuse_colliding_manifest_paths(&target_manifest)?;
    let target_files: BTreeMap<&str, &ReplayManifestEntry> = target_manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let from_files = match from {
        Some(from) => crate::worktree_materialize::baseline_files_against_record_or_live(
            layout,
            from,
            &active_replay,
        )?,
        None => BTreeMap::new(),
    };

    // Refusal 3: clean against the branch being left.
    if let Some(from) = from {
        require_clean(layout, from, &target_files, &active_replay)?;
    }

    // Refusal 4, and the plan: every decision made before the first write.
    let root = layout.worktree_mutation_root();
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
                } else if from_files
                    .get(path)
                    .is_some_and(|(blob, kind)| matches_blob(&current, *blob, *kind))
                {
                    writes.push(entry);
                } else {
                    in_the_way.push(format!(
                        "{path} (a file that is not part of the branch being left)"
                    ));
                }
            }
            Some(_) => in_the_way.push(format!("{path} (not a regular file)")),
        }
    }
    for (path, (blob, kind)) in &from_files {
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
                    in_the_way.push(format!("{path} (changed since the branch being left)"));
                }
            }
            Some(_) => in_the_way.push(format!("{path} (not a regular file)")),
        }
    }
    if !in_the_way.is_empty() {
        return Err(PrikkError::Precondition(format!(
            "refusing to switch to {target}: {} in the way of its files: {}; move them aside and \
             run the switch again (nothing was written)",
            in_the_way.len(),
            in_the_way.join(", ")
        )));
    }

    // The transition: writes, then deletions, then the pointer, and only then the marker -- so an
    // interruption anywhere, the pointer write included, leaves a switch that running it again
    // completes (module doc).
    mark_worktree_dirty(layout, &MarkedTarget::Switch(target.to_string()))?;
    for entry in &writes {
        let relative = Path::new(entry.path.as_str());
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        ensure_directory_required(root, parent)?;
        write_worktree_file_atomically(root, relative, &entry.bytes)?;
        set_regular_file_mode_required(root, relative, entry.mode)?;
    }
    for entry in &unchanged {
        let relative = Path::new(entry.path.as_str());
        let mode = stat_file_state_if_exists(root, relative)?
            .and_then(|stat| stat.mode)
            .map(|mode| mode & 0o7777);
        if mode != Some(entry.mode & 0o7777) {
            set_regular_file_mode_required(root, relative, entry.mode)?;
        }
    }
    for path in &deletes {
        remove_worktree_file_required(root, Path::new(path))?;
    }
    write_current_branch(layout, &target)?;
    clear_worktree_dirty(layout)?;
    // 0.56.0 step W2 (R1), amended step W2c: written last, inside this same dirty-marker
    // choreography, after the marker clears -- the worktree now matches `target`'s sealed tip, plus
    // (for a self-switch whose active WAL was folded in above) that queue's own patch ids, in order.
    // Every ordinary switch still records `wal_sequence: 0` and an empty fold, exactly as before
    // this round: `recorded_queued_ids` is empty for every case but the one just fixed.
    crate::worktree_baseline::record_worktree_baseline(
        layout,
        &target,
        target_baseline_block_id,
        recorded_wal_sequence,
        &recorded_queued_ids,
    )?;

    Ok(BranchSwitchReport {
        from: from.map(str::to_string),
        to: target,
        written_files: writes.len(),
        unchanged_files: unchanged.len(),
        deleted_files: deletes.len(),
        already_current: false,
        worktree_record_restored: false,
        anchor_fallback,
    })
}

fn require_open_branch(layout: &RepositoryLayout, target: &str) -> Result<()> {
    let Some(ref_state_id) = RefStore::new(layout.clone()).read_current_ref_state_id(target)?
    else {
        return Err(PrikkError::Precondition(format!(
            "{target} does not exist; run `prikk branch create {target}` first"
        )));
    };
    let object_store = ObjectReadSnapshot::open(layout)?;
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "missing RefState object for {target}: {ref_state_id}"
            ))
        })?;
    let state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if state.closed {
        return Err(PrikkError::Precondition(format!(
            "{target} is closed; switch to an open branch (`prikk branch list` shows them)"
        )));
    }
    Ok(())
}

/// Refusal 3. While the worktree-dirty marker is set, a path already holding the target's bytes, or
/// already absent where the target has no file, is where an interrupted switch left it -- accepted,
/// so running the switch again completes it.
///
/// 0.56.0 step W2 (R2), generalized step W2f: whenever a record exists and is stale against `from`'s
/// live tip, the comparison baseline is the *recorded* state, not the live one -- otherwise this read
/// against `from`'s live tip would see every file the ref gained since as "missing" and refuse,
/// exactly §2.7's deadlock. **Used whenever `compare_worktree_baseline` reads `Behind` at all,
/// regardless of which ref `recorded.ref_name` names** -- see
/// [`crate::worktree_materialize::baseline_files_against_record_or_live`]'s own doc for why a
/// `recorded.ref_name == from` gate made this unreachable for the one case it most needed to cover (a
/// worktree stuck after `commit --ref <other>`). Unaffected (today's live comparison) only under
/// `NoRecord`/`Current`.
///
/// 0.56.0 step W3b: the dirty-entry classification itself is
/// [`crate::worktree_materialize::dirty_worktree_entries`], shared with the W3b producers' own clean
/// check, which has no resumption case of its own and always passes a closure that answers `false`.
fn require_clean(
    layout: &RepositoryLayout,
    from: &str,
    target_files: &BTreeMap<&str, &ReplayManifestEntry>,
    active_replay: &crate::wal::WalReplay,
) -> Result<()> {
    let comparison =
        crate::worktree_baseline::compare_worktree_baseline(layout, from, active_replay)?;
    let report = match &comparison {
        crate::worktree_baseline::WorktreeBaselineComparison::Behind { recorded, .. } => {
            crate::worktree_status::worktree_status_against_recorded_baseline(
                layout,
                from,
                recorded,
                active_replay,
            )?
        }
        _ => worktree_status(layout, from)?,
    };
    let resuming = worktree_is_dirty(layout)?;
    let dirty = crate::worktree_materialize::dirty_worktree_entries(&report, |change| {
        let target_entry = target_files.get(change.path.as_str());
        resuming
            && match change.kind {
                WorktreeChangeKind::Modified => target_entry.is_some_and(|entry| {
                    std::fs::read(layout.root().join(&change.path))
                        .is_ok_and(|bytes| bytes == entry.bytes)
                }),
                WorktreeChangeKind::Missing => target_entry.is_none(),
                WorktreeChangeKind::Untracked | WorktreeChangeKind::UnsupportedPath => false,
            }
    });
    if dirty.is_empty() {
        return Ok(());
    }
    Err(PrikkError::Precondition(format!(
        "the worktree is not clean against {from}: {}; commit it, or restore those files, before \
         switching (`prikk worktree-status` lists them)",
        dirty.join(", ")
    )))
}

fn matches_blob(bytes: &[u8], blob: ObjectId, kind: NodeKind) -> bool {
    ensure_blob_matches_node_kind(bytes, blob, kind).is_ok()
}

/// The one writer of the pointer besides `init`: `heads/<name>\n`, in place (RFC 168 §3.3: truncate, then append), after every
/// worktree write has completed. The switch marker covers the write: a torn pointer is refused, and the refusal names the switch.
fn write_current_branch(layout: &RepositoryLayout, branch: &str) -> Result<()> {
    let relative = layout.repository_relative(&layout.current_branch_path())?;
    rewrite_in_place_or_create_required(
        layout.repository_mutation_root(),
        &relative,
        format!("{branch}\n").as_bytes(),
    )
}

/// What `branch switch <the current branch>` can do about a worktree record that is absent or ignored
/// (0.57.0 step 1c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeRecordRestore {
    /// The worktree is clean against the branch, so "last written from this tip" is true: the switch
    /// writes this identity (the branch's live one) and changes no file.
    Restorable(crate::worktree_baseline::WorktreeBaselineIdentity),
    /// The worktree differs from the branch (a modified, missing or unsupported path, or a live rename
    /// declaration): the switch writes nothing, and `commit`'s own refusals cover the guard's state.
    NotClean,
}

/// The one predicate behind both the restoring switch and `doctor`'s notice, so the notice appears
/// exactly when the command it names would restore the record: the caller has found the record absent
/// or ignored; this scans the worktree (the same report and the same dirty classification the switch's
/// own refusal 3 uses) and answers whether the live identity may be written.
pub(crate) fn plan_worktree_record_restore(
    layout: &RepositoryLayout,
    branch: &str,
    active_replay: &crate::wal::WalReplay,
) -> Result<WorktreeRecordRestore> {
    if worktree_is_dirty(layout)? {
        return Ok(WorktreeRecordRestore::NotClean);
    }
    let report =
        crate::worktree_status::worktree_status_over_sound_replay(layout, branch, active_replay)?;
    if !crate::worktree_materialize::dirty_worktree_entries(&report, |_| false).is_empty() {
        return Ok(WorktreeRecordRestore::NotClean);
    }
    Ok(WorktreeRecordRestore::Restorable(
        crate::worktree_baseline::live_baseline_identity(layout, branch, active_replay)?,
    ))
}

/// `doctor`'s question: is the current branch's worktree record absent or ignored **and** would
/// `prikk branch switch <it>` restore it? `Ok(false)` for a present record, for a worktree that is not
/// clean (no notice: a warning with nothing to type is noise), and for a WAL that is not sound (that is
/// reported elsewhere).
pub(crate) fn worktree_record_restorable_by_switch(
    layout: &RepositoryLayout,
    branch: &str,
) -> Result<bool> {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
    if replay.trailing_partial_bytes != 0 || replay.has_item_failure() {
        return Ok(false);
    }
    if !matches!(
        crate::worktree_baseline::compare_worktree_baseline(layout, branch, &replay)?,
        crate::worktree_baseline::WorktreeBaselineComparison::NoRecord
    ) {
        return Ok(false);
    }
    Ok(matches!(
        plan_worktree_record_restore(layout, branch, &replay)?,
        WorktreeRecordRestore::Restorable(_)
    ))
}

// Repository mutation is implemented on Linux, macOS and Windows (`platform-support.md`); tests that
// need a Linux-only, Unix-only or directory-sync mechanism carry their own narrower gate.
#[cfg(test)]
mod tests;
