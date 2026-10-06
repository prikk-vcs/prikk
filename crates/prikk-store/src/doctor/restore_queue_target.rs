//! `prikk doctor --restore-queue-target --ref <ref> [--not-current-branch] [--plan-only]` (RFC 166
//! D5 as amended by §14, 2026-10-05; §13 item 15).
//!
//! The one way out for row 9 (ownership missing) and D6's own ref-name-vs-witness mismatch (which
//! `classify` already folds into the same [`Verdict::OwnershipMissing`] verdict, "classified the
//! same as no owner at all") -- the 0.48.0 strandings of RFC 166 §1.6 are the main case: a crash
//! during a first commit's own `ref-name` write left a non-empty WAL with no durable owner.
//!
//! **§14 replaces D5's original condition.** Round 2's own review found it unmeetable: D5 asked to
//! refuse "when the queue does not validate against `<ref>`'s current tip by the same check `seal`
//! makes" -- but `seal` checks no such thing (a Patch names neither its own ref nor its own base),
//! so that condition refused nothing that mattered, and let a queue built on one branch be silently
//! attached to, and then published onto, a different one. **The rule now (§14 items 1-4):**
//!
//! - **With a witness:** unchanged (C2). `<ref>` must equal the witness's own ref.
//! - **Without a witness** (a queue stranded by 0.20.0-0.48.0, or one whose witness was removed or
//!   is itself unreadable): `<ref>` must be the current branch, resolved by the same resolver
//!   `commit` uses for its own default ([`crate::refs::current_branch`]). A different ref requires
//!   `--not-current-branch`; so does a current branch that cannot be resolved at all (fail closed --
//!   there is nothing to compare `<ref>` against, so the caller must say so explicitly).
//! - **§13 item 10's tip-matching list is dropped entirely** -- it answered a different, narrower
//!   question (does some ref's tip already equal this queue's patches, the interrupted-seal shape)
//!   that `prikk ref complete`/seal's own retry already cover, and answering it gave no protection
//!   against the real risk the review found.
//! - **A restored owner is final for this verb.** A second restore over an owned queue still
//!   refuses (unchanged) -- a wrong restore is undone by hand, not by a second call that would make
//!   this a general "change owner" command.
//!
//! **The write, last:** `ref-name`, by atomic replace -- never the truncate-then-append
//! [`crate::write_active_ref_metadata`] uses, because that sequence is safe only when D1 guarantees
//! the WAL is empty at the time of the write. Here the WAL is, by construction, non-empty: a tear
//! between truncate and append would recreate the exact stranding this verb exists to repair.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectEnvelope, ObjectId, ObjectType, RefStatePayload};

use crate::commit_boundary::active::read_active_ref_metadata;
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{WitnessState, read_witness};
use crate::foundation::fsutil::rewrite_in_place_or_create_required;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::object_store::{FileObjectStore, ObjectReader};
use crate::patch_replay::decode::{
    DecodedOperationKind, decode_patch_message, decode_patch_operations,
};
use crate::refs::{
    RefStore, current_branch, ensure_no_incomplete_publication, validate_local_branch_ref,
};
use crate::wal::{Wal, WalRecord};

/// One commit's own content, for the plan (RFC 166 §14 item 7): never a bare block hash.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitSummary {
    /// The commit's own message, if it carries one.
    pub message: Option<String>,
    /// Every repository-relative path this commit's own operations name directly (a rename shows
    /// as `"old -> new"`). Node-addressed operations with no path of their own (`EditText`,
    /// `ChangePerm`, `ReplaceBinary` -- FDD-03 §9.3) report plainly rather than fabricating one;
    /// see this module's own `commit_summary` function doc for why resolving their path is out of
    /// scope here.
    pub paths: Vec<String>,
}

/// The plan a real run writes from, and everything `--plan-only` prints (RFC 166 D5 K1, as amended
/// by §14).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoreQueueTargetPlan {
    /// The ref this run attaches the queue to -- from the caller (C2), never from the witness.
    pub ref_name: String,
    /// Every queued patch id, in WAL order -- "the queue's records" (RFC 166 K1).
    pub patch_ids: Vec<ObjectId>,
    /// Each queued commit's own content, in the same order as `patch_ids` (§14 item 7).
    pub queued_commits: Vec<CommitSummary>,
    /// `ref_name`'s own latest sealed commit, if it has ever been published -- the commit these
    /// queued ones will go on top of (§14 item 7). `None` when `ref_name` has never been
    /// published, or when any part of this purely informational read fails; never a gate (RFC 166
    /// D5's own review finding: a tip check answered nothing this verb actually needed).
    pub latest_sealed_commit: Option<CommitSummary>,
    /// `true` when `ref_name` was not named by a witness but *assumed* to be the current branch
    /// (no `--not-current-branch` override) -- the one case §14 item 8's uncertainty sentence
    /// applies to. `false` when a witness decided it (C2) or the caller already overrode the
    /// assumption explicitly.
    pub current_branch_assumed: bool,
}

enum Mode {
    PlanOnly,
    Execute,
}

/// Report what [`restore_queue_target`] would do, without writing anything.
pub fn plan_restore_queue_target(
    layout: &RepositoryLayout,
    ref_name: &str,
    not_current_branch: bool,
) -> Result<RestoreQueueTargetPlan> {
    run(layout, ref_name, not_current_branch, Mode::PlanOnly)
}

/// The real run: gives an owned queue its owner back (RFC 166 D5, §13 item 15, as amended by §14),
/// for row 9 or D6's mismatch only. Refuses, writing nothing, for every other row.
pub fn restore_queue_target(
    layout: &RepositoryLayout,
    ref_name: &str,
    not_current_branch: bool,
) -> Result<RestoreQueueTargetPlan> {
    run(layout, ref_name, not_current_branch, Mode::Execute)
}

fn run(
    layout: &RepositoryLayout,
    ref_name: &str,
    not_current_branch: bool,
    mode: Mode,
) -> Result<RestoreQueueTargetPlan> {
    let ref_name = validate_local_branch_ref(ref_name)?;
    let _active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    if matches!(mode, Mode::Execute) {
        // RFC 168 F6: a plan-only run creates nothing; the execute path is a writer (RFC 168 §3.1, §3.3).
        layout.ensure_write_state()?;
    }
    ensure_no_incomplete_publication(layout)?;
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
    let owning_ref = read_active_ref_metadata(layout)?;
    let witness = read_witness(layout, DEFAULT_ACTIVE_NAME)?;
    // K2: any evaluation failure above (a read error) already propagated via `?`, before this line,
    // so nothing past it ever runs against a session this verb could not fully evaluate.
    let verdict = classify(layout, &replay, &owning_ref, &witness)?;
    if !matches!(verdict, Verdict::OwnershipMissing) {
        return Err(PrikkError::Precondition(
            "this session's queue already has a durable, matching owner; \
             `--restore-queue-target` only acts when ownership is missing or disagrees with the \
             commit witness"
                .to_string(),
        ));
    }
    // RFC 166 §14 item 1: with a witness, `<ref>` must equal the witness's own ref (C2, unchanged).
    // Without one -- absent, or itself unreadable -- `<ref>` must be the current branch unless the
    // caller explicitly overrides with `--not-current-branch`; an unresolvable current branch
    // requires the same flag, fail closed, since there is then nothing to compare `<ref>` against.
    // §14 item 8: `current_branch_assumed` records exactly the one case its own uncertainty
    // sentence applies to -- no witness decided it, and the caller did not already override it.
    let current_branch_assumed = match &witness {
        WitnessState::Valid(record) => {
            if record.ref_name != ref_name {
                return Err(PrikkError::Precondition(format!(
                    "this session's own commit witness names {}, not {ref_name}; restore to that \
                     ref instead, or address the witness first",
                    record.ref_name
                )));
            }
            false
        }
        WitnessState::Absent | WitnessState::Damaged(_) => {
            match (current_branch(layout), not_current_branch) {
                (Ok(current), false) if current != ref_name => {
                    return Err(PrikkError::Precondition(format!(
                        "your current branch is {current}, but you asked to restore to \
                         {ref_name}; if these commits were made with `commit --ref {ref_name}` \
                         (or `rollback-draft` on it), pass --not-current-branch to confirm that; \
                         otherwise restore to {current} instead"
                    )));
                }
                (Ok(_), _) => {}
                (Err(_), false) => {
                    return Err(PrikkError::Precondition(format!(
                        "prikk cannot resolve your current branch, so it cannot be compared \
                         against {ref_name}; pass --not-current-branch to restore to {ref_name} \
                         anyway"
                    )));
                }
                (Err(_), true) => {}
            }
            !not_current_branch
        }
    };
    if replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "active WAL has {} trailing partial bytes; run doctor before restoring ownership",
            replay.trailing_partial_bytes
        )));
    }
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "active WAL has a damaged record; run doctor before restoring ownership".to_string(),
        ));
    }
    if replay.records.is_empty() {
        return Err(PrikkError::Precondition(
            "active WAL has no records; there is no queue to restore ownership for".to_string(),
        ));
    }

    let patch_ids = collect_wal_patch_ids(&replay.records)?;
    let queued_commits = replay
        .records
        .iter()
        .map(|record| commit_summary(&record.envelope))
        .collect::<Result<Vec<_>>>()?;
    let latest_sealed_commit = latest_sealed_commit_summary(layout, &ref_name);
    let plan = RestoreQueueTargetPlan {
        ref_name: ref_name.clone(),
        patch_ids,
        queued_commits,
        latest_sealed_commit,
        current_branch_assumed,
    };

    match mode {
        Mode::PlanOnly => Ok(plan),
        Mode::Execute => {
            let relative = layout.repository_relative(&layout.default_active_ref_name_path())?;
            rewrite_in_place_or_create_required(
                layout.repository_mutation_root(),
                &relative,
                ref_name.as_bytes(),
            )?;
            Ok(plan)
        }
    }
}

fn collect_wal_patch_ids(records: &[WalRecord]) -> Result<Vec<ObjectId>> {
    records
        .iter()
        .map(|record| {
            require_patch_record(record)?;
            Ok(record.envelope.object_id())
        })
        .collect()
}

fn require_patch_record(record: &WalRecord) -> Result<()> {
    if record.envelope.object_type == ObjectType::Patch {
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "active WAL record {} is {}, expected patch",
        record.seq, record.envelope.object_type
    )))
}

/// RFC 166 §14 item 7: a Patch envelope's own message and the paths its operations name directly
/// -- never a bare block hash. `CreateFile`/`DeleteNode`/`CreateSymlink` each name one path;
/// `RenamePath` names both (`"old -> new"`). `EditText`, `ChangePerm` and `ReplaceBinary` are
/// node-addressed only (FDD-03 §9.3): resolving the path a `node_id` currently names would need a
/// full lifecycle replay against the worktree baseline
/// ([`crate::patch_replay::resolve_folded_worktree_baseline`]), a much larger computation than this
/// item's own display purpose justifies, so these report plainly instead of fabricating a path or
/// silently dropping the operation from the count.
fn commit_summary(envelope: &ObjectEnvelope) -> Result<CommitSummary> {
    let bytes = &envelope.canonical_payload;
    let schema_version = envelope.schema_version;
    let message = decode_patch_message(bytes, schema_version)?;
    let operations = decode_patch_operations(bytes, schema_version)?;
    let paths = operations
        .iter()
        .map(|operation| match &operation.kind {
            DecodedOperationKind::CreateFile { path, .. }
            | DecodedOperationKind::DeleteNode { path, .. }
            | DecodedOperationKind::CreateSymlink { path, .. } => path.clone(),
            DecodedOperationKind::RenamePath {
                old_path, new_path, ..
            } => format!("{old_path} -> {new_path}"),
            DecodedOperationKind::EditText { .. }
            | DecodedOperationKind::ChangePerm { .. }
            | DecodedOperationKind::ReplaceBinary { .. } => {
                "(a change to an existing file)".to_string()
            }
        })
        .collect();
    Ok(CommitSummary { message, paths })
}

/// RFC 166 §14 item 7: `ref_name`'s own current tip, described for the plan -- purely
/// informational, never a gate (the review's own finding: a tip check answered nothing this verb
/// actually needed). `None` when `ref_name` has never been published, or when any part of this
/// read fails; a failure here only withholds the description, never blocks the restore itself --
/// the same distinction `discard_damaged_commits`'s own "may still be in your working tree" note
/// already draws between a safety check and a best-effort read.
fn latest_sealed_commit_summary(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Option<CommitSummary> {
    let ref_store = RefStore::new(layout.clone());
    let objects = FileObjectStore::new(layout.clone());
    let ref_state_id = ref_store.read_current_ref_state_id(ref_name).ok()??;
    let ref_state_envelope = objects
        .read_typed(ref_state_id, ObjectType::RefState)
        .ok()??;
    let ref_state = RefStatePayload::decode_canonical(
        &ref_state_envelope.canonical_payload,
        ref_state_envelope.schema_version,
    )
    .ok()?;
    let block_envelope = objects
        .read_typed(ref_state.target_object_id, ObjectType::Block)
        .ok()??;
    let block = BlockPayload::decode_canonical(&block_envelope.canonical_payload).ok()?;
    let last_patch_id = *block.patch_ids.last()?;
    let patch_envelope = objects
        .read_typed(last_patch_id, ObjectType::Patch)
        .ok()??;
    commit_summary(&patch_envelope).ok()
}

#[cfg(test)]
mod tests;
