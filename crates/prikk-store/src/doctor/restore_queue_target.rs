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
use prikk_object::{ObjectId, ObjectType};

use crate::commit_boundary::active::read_active_ref_metadata;
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{WitnessState, read_witness};
use crate::foundation::fsutil::write_file_atomically;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::refs::{current_branch, ensure_no_incomplete_publication, validate_local_branch_ref};
use crate::wal::{Wal, WalRecord};

/// The plan a real run writes from, and everything `--plan-only` prints (RFC 166 D5 K1, as amended
/// by §14).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoreQueueTargetPlan {
    /// The ref this run attaches the queue to -- from the caller (C2), never from the witness.
    pub ref_name: String,
    /// Every queued patch id, in WAL order -- "the queue's records" (RFC 166 K1).
    pub patch_ids: Vec<ObjectId>,
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
    match &witness {
        WitnessState::Valid(record) => {
            if record.ref_name != ref_name {
                return Err(PrikkError::Precondition(format!(
                    "this session's own commit witness names {}, not {ref_name}; restore to that \
                     ref instead, or address the witness first",
                    record.ref_name
                )));
            }
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
        }
    }
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
    let plan = RestoreQueueTargetPlan {
        ref_name: ref_name.clone(),
        patch_ids,
    };

    match mode {
        Mode::PlanOnly => Ok(plan),
        Mode::Execute => {
            let relative = layout.repository_relative(&layout.default_active_ref_name_path())?;
            write_file_atomically(
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

#[cfg(test)]
mod tests;
