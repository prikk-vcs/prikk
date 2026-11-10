//! `prikk uncommit <patch-id> [--plan-only]` (0.57.0 step 3, S5b): take back your own newest unsealed commit.
//!
//! Nothing is wrong when this runs. It is the inverse of `commit` for work that has not been sealed: the newest queued patch
//! leaves the active WAL, the files in the worktree are not touched, and sealed history is never touched. It is **not** a
//! `doctor` repair (it acts on a healthy queue, and says so), and it shares none of `--discard-damaged-commits`' words.
//!
//! **Newest only, healthy queue only.** A middle patch is refused, naming the patches queued after it (re-committing them gives
//! them new patch ids: a patch id covers its parent). Every damaged or lost-commit shape of RFC 166 refuses with that row's own
//! sentence and its own `doctor` verb, from [`write_refusal_reason`]; a sealed id, an unknown id and an id that is not a patch are
//! refused by what the id is.
//!
//! **The write, in order** -- each recovery-log entry saved just before the write it undoes (so the restore's checks, which read
//! each file as the earlier steps left it, hold), a crash between two steps leaving a state the next run reads honestly:
//! 1. the removed patch's renames are merged back into the rename declarations (so the next `commit` authors a rename again,
//!    not a delete and a create with a new identity). First, while the patch is still in the WAL to be read: once the WAL is cut,
//!    a re-run could no longer know them;
//! 2. the WAL is cut at the patch's frame (the bytes saved first);
//! 3. the commit witness is rewritten over the remaining queue, or cleared when none remains -- the state `seal` leaves;
//! 4. when no patch remains, `ref-name` is cleared, as `seal` clears it;
//! 5. the worktree's record, last (it is written last everywhere in this codebase), only when it equals the queue as it stood.
//!
//! **A re-run finishes it.** What an interrupted run leaves is recognised from the WAL, the witness and the worktree's record --
//! never from the recovery log, which is not authority and only names the run: after step 2 the witness still names the removed
//! patch as the last acknowledged one with nothing after it (RFC 166 row 5, which `commit` and `verify` rightly call a lost
//! commit unless the person asked for exactly that patch); after step 3 or 4 the record still equals the queue **with** the patch.
//!
//! **K1**: `--plan-only` and a real run are one computation, [`run`], under one lock.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType};

use crate::commit_boundary::active::{ActiveRefMetadata, read_active_ref_metadata};
use crate::commit_boundary::classification::{
    RestoreRefusalContext, Verdict, classify, write_refusal_reason,
};
use crate::commit_boundary::witness::{WitnessState, read_witness, rebuild_witness_over_sound_wal};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::decode::{DecodedOperationKind, decode_patch_operations};
use crate::rename_declaration::{
    DeclarationMerge, RenameDeclaration, overwrite_declaration_bytes, plan_declaration_merge,
};
use crate::wal::{Wal, WalReplay};
use crate::worktree_baseline::{RecordRewrite, encode_record, plan_record_rewrite};
use crate::worktree_status::{
    QueuedPathResolution, WorktreeChangeKind, enumerate_queued_patches,
    worktree_status_over_sound_replay,
};

/// What the next `commit` does about one operation of the removed patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationFate {
    /// The worktree still holds the change, so the next `commit` authors it again.
    Reauthored,
    /// A rename, put back into the rename declarations: the next `commit` authors a rename again, keeping the file's identity.
    RenameDeclared,
    /// The worktree no longer holds this change (or never did: a `rollback-draft` patch is not authored from the worktree), so the
    /// next `commit` does not author it.
    NotInWorktree,
    /// `commit` does not author this kind from the worktree at all (a symlink: it refuses them).
    NotAuthoredByCommit,
    /// The path could not be resolved (a node-addressed operation whose node is not live in the folded state): no claim is made.
    Unresolved,
}

/// One operation of the removed patch and its fate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UncommitOperation {
    /// The operation's stable kind label (`create-file`, `rename-path`, ...).
    pub kind: &'static str,
    /// The path(s) it affects: two for a rename (old, new).
    pub paths: Vec<String>,
    /// What the next `commit` does about it.
    pub fate: OperationFate,
}

/// What happens to the worktree's record of what the worktree was made to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordOutcome {
    /// Rewritten to the queue as it stands after the removal.
    Rewritten,
    /// Left as it was: it was not this queue's (already behind the ref, or naming another ref).
    LeftAsItWas,
    /// There is no record (a repository from before 0.56.0).
    NoRecord,
}

/// The plan a real run writes from, and everything `--plan-only` prints (K1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UncommitPlan {
    /// The patch taken out of the queue.
    pub patch_id: ObjectId,
    /// Its WAL sequence. `None` when an earlier run already cut it out and this run only finishes.
    pub sequence: Option<u64>,
    /// The ref the queue belongs to. `None` only when an earlier run already cleared the owner.
    pub ref_name: Option<String>,
    /// The patch's message, as `log` decodes it. `None` when it carries none, or when it is already out of the queue.
    pub message: Option<String>,
    /// An earlier, interrupted `prikk uncommit` of this patch had cut it out of the WAL; this run finishes what is left.
    pub finishing: bool,
    /// The patch ids that stay queued, in order.
    pub queue_after: Vec<ObjectId>,
    /// Bytes of the patch's frame cut out of the WAL (saved first to the recovery log). Zero when finishing.
    pub removed_bytes: usize,
    /// The patch's operations and what the next `commit` does about each. Empty when finishing.
    pub operations: Vec<UncommitOperation>,
    /// Blobs the patch wrote that stay in the object store.
    pub blobs_left: usize,
    /// The renames put back into the rename declarations.
    pub declarations_restored: Vec<RenameDeclaration>,
    /// No patch remains, so the queue's owner (`ref-name`) is cleared too, as `seal` clears it.
    pub clears_owner: bool,
    /// What happens to the worktree's record.
    pub record: RecordOutcome,
    /// The recovery run that holds what was removed. `None` in a plan; set by a real run that wrote.
    pub recovery_run: Option<String>,
    /// When finishing: the earlier `prikk uncommit` run, found in the recovery log, that cut this patch out. `None` when the log
    /// has none (it was cleared, or the patch was a lost acknowledged commit and not an interrupted uncommit). **Chooses only the
    /// wording**: no decision reads it.
    pub earlier_run: Option<String>,
}

enum Mode {
    PlanOnly,
    Execute,
}

/// Report what [`uncommit`] would do, without writing anything.
pub fn plan_uncommit(layout: &RepositoryLayout, patch_id: ObjectId) -> Result<UncommitPlan> {
    run(layout, patch_id, Mode::PlanOnly)
}

/// Take `patch_id`, the newest queued patch, out of the queue (see the module doc).
pub fn uncommit(layout: &RepositoryLayout, patch_id: ObjectId) -> Result<UncommitPlan> {
    run(layout, patch_id, Mode::Execute)
}

/// Where the state `run` found stands with respect to the patch the person named.
enum Found {
    /// The newest record of a healthy queue: the ordinary case.
    Newest,
    /// An earlier run cut it out of the WAL and stopped before the witness was rewritten (row 5, recognised).
    CutOnly,
    /// An earlier run got past the witness; the owner and/or the worktree's record are still to bring up to date.
    PastWitness,
}

fn run(layout: &RepositoryLayout, patch_id: ObjectId, mode: Mode) -> Result<UncommitPlan> {
    let executing = matches!(mode, Mode::Execute);
    // A plan opens no run: only a run that writes has an id to name.
    let _run = executing
        .then(|| crate::recovery_log::begin_run_as(crate::recovery_log::UNCOMMIT_RUN_KIND));
    // The lock `commit` takes: the two cannot interleave, so the plan re-derived here is the plan applied.
    let _active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    if executing {
        layout.ensure_write_state()?;
    }
    layout.require_current_format()?;
    crate::refs::ensure_no_incomplete_publication(layout)?;
    if crate::worktree_marker::worktree_is_dirty(layout)? {
        return Err(PrikkError::Precondition(format!(
            "worktree materialization was interrupted, so the worktree is not verified against its baseline and nothing can be \
             uncommitted; {}",
            crate::worktree_marker::dirty_marker_route(layout)?
        )));
    }

    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
    let owner = read_active_ref_metadata(layout)?;
    let witness = read_witness(layout, DEFAULT_ACTIVE_NAME)?;
    let verdict = classify(layout, &replay, &owner, &witness)?;
    let remaining_now: Vec<ObjectId> = replay
        .records
        .iter()
        .map(|record| record.envelope.object_id())
        .collect();

    let found = locate(
        layout,
        patch_id,
        &replay,
        &owner,
        &witness,
        &verdict,
        &remaining_now,
    )?;

    match found {
        Found::Newest => newest(layout, patch_id, &wal, &replay, &owner, executing),
        Found::CutOnly | Found::PastWitness => finish(
            layout,
            patch_id,
            &replay,
            &owner,
            &remaining_now,
            &found,
            executing,
        ),
    }
}

/// Decide where `patch_id` stands, or refuse by what is true of the queue and of the id.
fn locate(
    layout: &RepositoryLayout,
    patch_id: ObjectId,
    replay: &WalReplay,
    owner: &ActiveRefMetadata,
    witness: &WitnessState,
    verdict: &Verdict,
    remaining_now: &[ObjectId],
) -> Result<Found> {
    // Rows 4-7 and 9 refuse with their own sentence and their own verb -- except row 5 when the person named exactly the patch
    // the witness holds and the WAL stands one short of it: that is an interrupted run of this command, and finishing it is
    // what they asked.
    if let Some(reason) = write_refusal_reason(
        verdict,
        Some(RestoreRefusalContext {
            layout,
            witness,
            queued_count: replay.records.len(),
        }),
    ) {
        if let (Verdict::AcknowledgedLoss { witnessed_seq }, WitnessState::Valid(record)) =
            (verdict, witness)
        {
            let wal_last = replay.records.last().map_or(0, |record| record.seq);
            let owner_matches = matches!(owner, ActiveRefMetadata::Valid(name) if *name == record.ref_name)
                || (replay.records.is_empty() && !matches!(owner, ActiveRefMetadata::Invalid(_)));
            if record.patch_id == patch_id
                && *witnessed_seq == wal_last + 1
                && !replay.has_item_failure()
                && replay.trailing_partial_bytes == 0
                && owner_matches
            {
                return Ok(Found::CutOnly);
            }
        }
        return Err(PrikkError::Precondition(reason));
    }
    match verdict {
        Verdict::Healthy { .. } | Verdict::Pending { .. } => {}
        Verdict::NoWitness {
            wal_otherwise_sound: true,
            ..
        } => {}
        Verdict::CrashTail { .. } | Verdict::WitnessDamaged | Verdict::NoWitness { .. } => {
            return Err(PrikkError::Precondition(
                "the queue is not in a state this command acts on: the commit record or the end of the queue is damaged, \
                 and `prikk doctor --repair-tails --plan-only` says what repairs it (this command takes back a commit from a \
                 healthy queue)"
                    .to_string(),
            ));
        }
        // Every other verdict returned a refusal above.
        _ => {}
    }
    if replay.has_item_failure() || replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Precondition(
            "the queue has a damaged record or an unfinished end: `prikk doctor --plan-only` names the repair (this command takes \
             back a commit from a healthy queue)"
                .to_string(),
        ));
    }
    match remaining_now.iter().position(|id| *id == patch_id) {
        Some(index) if index + 1 == remaining_now.len() => Ok(Found::Newest),
        Some(index) => Err(PrikkError::Precondition(middle_refusal(
            patch_id,
            remaining_now.get(index + 1..).unwrap_or_default(),
        ))),
        None => {
            // Not queued. Past the witness, an earlier run of this command left the worktree's record one commit ahead of the queue:
            // the record equals the queue with this patch on the end.
            let queue_ref = match owner {
                ActiveRefMetadata::Valid(name) => Some(name.as_str()),
                _ => None,
            };
            let mut before = remaining_now.to_vec();
            before.push(patch_id);
            let after_last_seq = replay.records.last().map_or(0, |record| record.seq);
            if matches!(
                plan_record_rewrite(layout, queue_ref, &before, remaining_now, after_last_seq)?,
                RecordRewrite::Rewrite(_)
            ) {
                return Ok(Found::PastWitness);
            }
            Err(PrikkError::Precondition(not_queued_sentence(
                layout, patch_id,
            )?))
        }
    }
}

fn middle_refusal(patch_id: ObjectId, later: &[ObjectId]) -> String {
    let count = later.len();
    let (patches, are) = if count == 1 {
        ("patch", "is")
    } else {
        ("patches", "are")
    };
    let names: Vec<String> = later.iter().map(ToString::to_string).collect();
    format!(
        "{patch_id} is not the newest queued commit: {count} {patches} {are} queued after it ({}). `prikk uncommit` takes back \
         the newest only; take those back first, newest first. Committing them again gives them new patch ids, because a \
         patch id covers the patch before it",
        names.join(", ")
    )
}

/// Why `patch_id` is not a queued commit, by what the id is.
fn not_queued_sentence(layout: &RepositoryLayout, patch_id: ObjectId) -> Result<String> {
    let store = ObjectReadSnapshot::open(layout)?;
    let Some(envelope) = store.read_object(patch_id)? else {
        let earlier = last_run_that_removed(layout, patch_id)?;
        return Ok(match earlier {
            Some(run) => format!(
                "{patch_id} is not queued: `prikk uncommit` already took it out (recovery run {run}; `prikk doctor \
                 --recovery-restore {run} --plan-only` says whether it can be put back)"
            ),
            None => format!(
                "{patch_id} is not queued, and no object in this repository has that id; `prikk status --format json` lists the \
                 queued commits and their ids"
            ),
        });
    };
    Ok(match envelope.object_type {
        ObjectType::Patch => {
            if crate::patch_ids_named_by_any_held_block(layout)?.contains(&patch_id) {
                format!(
                    "{patch_id} is a patch sealed into a block, not a queued commit. Sealed history is never touched; \
                     `prikk rollback-draft` queues a change that undoes it"
                )
            } else {
                format!(
                    "{patch_id} is a patch this repository holds, but it is not in the queue (it was accepted from another \
                     repository, or left by an import). `prikk uncommit` takes back your own queued commits only"
                )
            }
        }
        other => format!(
            "{patch_id} is {other}, not a queued commit; `prikk status --format json` lists the queued commits and their ids"
        ),
    })
}

/// The recovery run whose WAL cut removed `patch_id`, when the log still holds it. **Only a name for the message**: the log is
/// never authority, and no decision reads this.
fn last_run_that_removed(layout: &RepositoryLayout, patch_id: ObjectId) -> Result<Option<String>> {
    let wal_source = layout
        .repository_relative(&layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME))?
        .to_string_lossy()
        .replace('\\', "/");
    let Ok(listing) = crate::recovery_log::list(layout.repository_mutation_root()) else {
        return Ok(None);
    };
    for listed in listing.entries.iter().rev() {
        if listed.entry.source != wal_source
            || listed.entry.label != crate::recovery_log::UNCOMMIT_RUN_KIND
        {
            continue;
        }
        let Ok(cut) = crate::wal::decode_records(&listed.entry.removed) else {
            continue;
        };
        if cut
            .records
            .iter()
            .any(|record| record.envelope.object_id() == patch_id)
        {
            return Ok(Some(listed.id.clone()));
        }
    }
    Ok(None)
}

/// The ordinary case: the newest queued patch, in a healthy queue.
fn newest(
    layout: &RepositoryLayout,
    patch_id: ObjectId,
    wal: &Wal,
    replay: &WalReplay,
    owner: &ActiveRefMetadata,
    executing: bool,
) -> Result<UncommitPlan> {
    let ActiveRefMetadata::Valid(ref_name) = owner else {
        return Err(PrikkError::Integrity(
            "the queue has commits but no valid owner (`prikk doctor --restore-queue-target` gives it back)".to_string(),
        ));
    };
    let preview = wal.preview_remove_newest_record(patch_id)?;
    let (record, record_outcome) = {
        let before: Vec<ObjectId> = replay
            .records
            .iter()
            .map(|record| record.envelope.object_id())
            .collect();
        let after_last_seq = replay
            .records
            .iter()
            .rev()
            .nth(1)
            .map_or(0, |record| record.seq);
        match plan_record_rewrite(
            layout,
            Some(ref_name),
            &before,
            &preview.remaining_patch_ids,
            after_last_seq,
        )? {
            RecordRewrite::Rewrite(identity) => (Some(identity), RecordOutcome::Rewritten),
            RecordRewrite::NotThisQueue => (None, RecordOutcome::LeftAsItWas),
            RecordRewrite::NoRecord => (None, RecordOutcome::NoRecord),
        }
    };

    // What the next commit does about each operation: the worktree against the queue as it will stand.
    let remaining_replay = WalReplay {
        records: replay
            .records
            .get(..replay.records.len().saturating_sub(1))
            .unwrap_or_default()
            .to_vec(),
        trailing_partial_bytes: 0,
        record_outcomes: Vec::new(),
    };
    let status = worktree_status_over_sound_replay(layout, ref_name, &remaining_replay).ok();
    let queued = enumerate_queued_patches(layout)?;
    let entry = queued
        .iter()
        .find(|entry| entry.patch_id == patch_id)
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "queued patch {patch_id} is not in the queue enumeration"
            ))
        })?;
    // A mode-only change is invisible to the worktree status (it compares bytes), but `commit` authors it from the file's mode, so
    // the status cannot answer for it: read the mode the patch set, in the operation's own order, and compare the file.
    let new_modes: Vec<Option<u32>> = replay
        .records
        .iter()
        .find(|record| record.envelope.object_id() == patch_id)
        .map(|record| {
            decode_patch_operations(
                &record.envelope.canonical_payload,
                record.envelope.schema_version,
            )
        })
        .transpose()?
        .unwrap_or_default()
        .iter()
        .map(|operation| match operation.kind {
            DecodedOperationKind::ChangePerm { new_mode, .. } => Some(new_mode),
            _ => None,
        })
        .collect();
    let mut operations = Vec::with_capacity(entry.operations.len());
    let mut declarations = Vec::new();
    for (index, operation) in entry.operations.iter().enumerate() {
        let paths: Vec<String> = operation
            .paths
            .iter()
            .map(|path| match path {
                QueuedPathResolution::Path(path) => path.clone(),
                QueuedPathResolution::Unresolved { node_id } => format!("(node {node_id})"),
            })
            .collect();
        let unresolved = operation
            .paths
            .iter()
            .any(|path| matches!(path, QueuedPathResolution::Unresolved { .. }));
        let holds = |path: &str, kind: WorktreeChangeKind| {
            status.as_ref().is_some_and(|status| {
                status
                    .changes
                    .iter()
                    .any(|change| change.path == path && change.kind == kind)
            })
        };
        let fate = if unresolved || status.is_none() {
            OperationFate::Unresolved
        } else {
            match (operation.kind, paths.as_slice()) {
                ("create-file", [path]) if holds(path, WorktreeChangeKind::Untracked) => {
                    OperationFate::Reauthored
                }
                ("delete-node", [path]) if holds(path, WorktreeChangeKind::Missing) => {
                    OperationFate::Reauthored
                }
                ("edit-text" | "replace-binary", [path])
                    if holds(path, WorktreeChangeKind::Modified) =>
                {
                    OperationFate::Reauthored
                }
                ("change-perm", [path])
                    if new_modes
                        .get(index)
                        .copied()
                        .flatten()
                        .is_some_and(|mode| worktree_has_mode(layout, path, mode)) =>
                {
                    OperationFate::Reauthored
                }
                ("rename-path", [old, new])
                    if holds(old, WorktreeChangeKind::Missing)
                        && holds(new, WorktreeChangeKind::Untracked) =>
                {
                    declarations.push(RenameDeclaration {
                        old_path: old.clone(),
                        new_path: new.clone(),
                    });
                    OperationFate::RenameDeclared
                }
                ("create-symlink", _) => OperationFate::NotAuthoredByCommit,
                _ => OperationFate::NotInWorktree,
            }
        };
        operations.push(UncommitOperation {
            kind: operation.kind,
            paths,
            fate,
        });
    }

    // An edit, a binary replace or a mode change of a node renamed in the same patch is authored together with the rename (the
    // declaration resolves to a rename "with content also changing"): it is re-authored through the declaration, not by itself.
    for operation in &mut operations {
        if matches!(
            operation.kind,
            "edit-text" | "replace-binary" | "change-perm"
        ) && operation.fate == OperationFate::NotInWorktree
            && operation.paths.first().is_some_and(|path| {
                declarations
                    .iter()
                    .any(|declaration| declaration.new_path == *path)
            })
        {
            operation.fate = OperationFate::Reauthored;
        }
    }
    let merge = plan_declaration_merge(layout, &declarations)?;
    if let DeclarationMerge::Conflict(conflict) = &merge {
        return Err(PrikkError::Precondition(format!(
            "{conflict}. Taking this commit back would change what the rename means, so nothing was changed. Commit what is \
             declared now (`prikk worktree-status` lists the declarations), or `prikk mv` the file back, then take this commit \
             back"
        )));
    }
    let blobs_left = blob_count(replay, patch_id)?;
    let clears_owner = preview.remaining_patch_ids.is_empty();
    let mut plan = UncommitPlan {
        patch_id,
        sequence: Some(preview.sequence),
        ref_name: Some(ref_name.clone()),
        message: entry.message.clone(),
        finishing: false,
        queue_after: preview.remaining_patch_ids.clone(),
        removed_bytes: preview.removed_bytes,
        operations,
        blobs_left,
        declarations_restored: declarations,
        clears_owner,
        record: record_outcome,
        recovery_run: None,
        earlier_run: None,
    };
    if !executing {
        return Ok(plan);
    }

    // 1. The renames, back into the declarations (the patch is still in the WAL to be read).
    if let DeclarationMerge::Merged(bytes) = &merge {
        save_declarations(layout, bytes)?;
        overwrite_declaration_bytes(layout, bytes)?;
    }
    // 2. The WAL, cut at the patch's frame; the bytes saved first.
    wal.remove_newest_record(patch_id)?;
    // 3.-5. The witness, the owner, and the worktree's record.
    let sound_after = wal.replay()?;
    finish_queue_state(layout, ref_name, &sound_after, record.as_ref())?;
    plan.recovery_run = crate::recovery_log::current_run_id_hex();
    Ok(plan)
}

/// Steps 3 to 5, which a re-run may have to do alone: the witness over the queue as it stands, the owner when nothing remains, and
/// the worktree's record when `record` says it is to be rewritten.
fn finish_queue_state(
    layout: &RepositoryLayout,
    ref_name: &str,
    sound_after: &WalReplay,
    record: Option<&crate::worktree_baseline::WorktreeBaselineIdentity>,
) -> Result<()> {
    rebuild_witness_over_sound_wal(
        layout,
        DEFAULT_ACTIVE_NAME,
        ref_name,
        sound_after,
        |written| crate::recovery_log::save_witness_replace(layout, written),
    )?;
    clear_owner_when_empty(layout, sound_after)?;
    if let Some(identity) = record {
        save_worktree_record(layout, &encode_record(identity))?;
        crate::worktree_baseline::write_worktree_baseline_identity(identity, layout)?;
    }
    Ok(())
}

/// Save the worktree record's current bytes before it is rewritten. Its meaning is the queue (the recovery log's table).
fn save_worktree_record(layout: &RepositoryLayout, written: &[u8]) -> Result<()> {
    crate::recovery_log::save_current_replace_relative(
        layout,
        std::path::Path::new(crate::recovery_log::WORKTREE_RECORD_SOURCE),
        written,
        "worktree record",
    )
}

/// Save the rename declarations' current bytes before the removed patch's renames are merged back.
fn save_declarations(layout: &RepositoryLayout, written: &[u8]) -> Result<()> {
    let path = layout.default_declarations_path();
    crate::recovery_log::save_current_replace(layout, &path, written, "rename declarations")
}

/// Clear `ref-name` when the queue is empty -- the state `seal` leaves -- saving its bytes first.
fn clear_owner_when_empty(layout: &RepositoryLayout, sound_after: &WalReplay) -> Result<()> {
    if !sound_after.records.is_empty() {
        return Ok(());
    }
    if matches!(
        read_active_ref_metadata(layout)?,
        ActiveRefMetadata::Missing
    ) {
        return Ok(());
    }
    crate::recovery_log::save_ref_name_replace(layout, &[])?;
    crate::commit_boundary::active::remove_active_ref_metadata(layout)?;
    Ok(())
}

/// Whether the file at `path` in the worktree has the executable bit `mode` records (`0o755` set, `0o644` clear). Off Unix there
/// is no such bit to read, and `commit` authors no mode change there, so the answer is no.
fn worktree_has_mode(layout: &RepositoryLayout, path: &str, mode: u32) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(metadata) = std::fs::symlink_metadata(layout.root().join(path)) else {
            return false;
        };
        metadata.is_file() && ((metadata.permissions().mode() & 0o111 != 0) == (mode & 0o111 != 0))
    }
    #[cfg(not(unix))]
    {
        let _ = (layout, path, mode);
        false
    }
}

/// The distinct blobs a patch's operations wrote (a create's and a binary replace's new blob).
fn blob_count(replay: &WalReplay, patch_id: ObjectId) -> Result<usize> {
    let Some(record) = replay
        .records
        .iter()
        .find(|record| record.envelope.object_id() == patch_id)
    else {
        return Ok(0);
    };
    let mut blobs = std::collections::BTreeSet::new();
    for operation in decode_patch_operations(
        &record.envelope.canonical_payload,
        record.envelope.schema_version,
    )? {
        match operation.kind {
            DecodedOperationKind::CreateFile { blob_id, .. } => {
                blobs.insert(blob_id);
            }
            DecodedOperationKind::ReplaceBinary { new_blob_id, .. } => {
                blobs.insert(new_blob_id);
            }
            _ => {}
        }
    }
    Ok(blobs.len())
}

/// An earlier run of this command cut the patch out of the WAL and stopped: finish the witness, the owner and the record.
fn finish(
    layout: &RepositoryLayout,
    patch_id: ObjectId,
    replay: &WalReplay,
    owner: &ActiveRefMetadata,
    remaining_now: &[ObjectId],
    found: &Found,
    executing: bool,
) -> Result<UncommitPlan> {
    let ref_name = match (owner, read_witness(layout, DEFAULT_ACTIVE_NAME)?) {
        (ActiveRefMetadata::Valid(name), _) => Some(name.clone()),
        (_, WitnessState::Valid(record)) => Some(record.ref_name),
        _ => None,
    };
    let after_last_seq = replay.records.last().map_or(0, |record| record.seq);
    let mut before = remaining_now.to_vec();
    before.push(patch_id);
    let (record, record_outcome) = match plan_record_rewrite(
        layout,
        match owner {
            ActiveRefMetadata::Valid(name) => Some(name.as_str()),
            _ => None,
        },
        &before,
        remaining_now,
        after_last_seq,
    )? {
        RecordRewrite::Rewrite(identity) => (Some(identity), RecordOutcome::Rewritten),
        RecordRewrite::NotThisQueue => (None, RecordOutcome::LeftAsItWas),
        RecordRewrite::NoRecord => (None, RecordOutcome::NoRecord),
    };
    let clears_owner = remaining_now.is_empty() && !matches!(owner, ActiveRefMetadata::Missing);
    let mut plan = UncommitPlan {
        patch_id,
        sequence: None,
        ref_name: ref_name.clone(),
        message: None,
        finishing: true,
        queue_after: remaining_now.to_vec(),
        removed_bytes: 0,
        operations: Vec::new(),
        blobs_left: 0,
        declarations_restored: Vec::new(),
        clears_owner,
        record: record_outcome,
        recovery_run: None,
        earlier_run: last_run_that_removed(layout, patch_id)?,
    };
    if !executing {
        return Ok(plan);
    }
    match found {
        Found::CutOnly => {
            let name = ref_name.as_deref().ok_or_else(|| {
                PrikkError::Integrity("no ref name to rewrite the commit record for".to_string())
            })?;
            finish_queue_state(layout, name, replay, record.as_ref())?;
        }
        Found::PastWitness | Found::Newest => {
            clear_owner_when_empty(layout, replay)?;
            if let Some(identity) = record.as_ref() {
                save_worktree_record(layout, &encode_record(identity))?;
                crate::worktree_baseline::write_worktree_baseline_identity(identity, layout)?;
            }
        }
    }
    plan.recovery_run = crate::recovery_log::current_run_id_hex();
    Ok(plan)
}

#[cfg(test)]
mod tests;
