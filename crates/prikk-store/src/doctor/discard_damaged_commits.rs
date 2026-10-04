//! `prikk doctor --discard-damaged-commits [--plan-only]` (RFC 166 D5, §13 item 14).
//!
//! The one way out for rows 4 (acknowledged damage), 5 (acknowledged loss) and 7 (unknown, with a
//! damaged witness) of RFC 166 §5: an acknowledged commit the active session's own commit witness
//! names, that the WAL no longer holds soundly. Refuses, writing nothing, over rows 6 and 10 (a
//! substituted record -- not a crash shape, no verb, a copy is the way out), row 9 (no durable
//! owner -- a different condition, `--restore-queue-target`'s own job), and every other row (nothing
//! acknowledged is damaged or lost to discard).
//!
//! **The write, in order** (never reordered, so a crash between any two steps leaves a state the
//! next run reads honestly): the bytes a genuine trailing-partial tail would otherwise be truncated
//! from are saved to a recovery file first, exactly as `--repair-wal-tail` saves them (the same
//! [`crate::wal::Wal::truncate_trailing_partial`] this repair calls, never a second copy of that
//! logic); then the WAL is truncated to the sound prefix (a no-op when there is none to truncate --
//! row 5's own shape, the WAL already as short as it will get); then the witness is rewritten to
//! cover exactly that sound prefix, from scratch, the same [`crate::commit_boundary::witness::
//! rebuild_witness_over_sound_wal`] row 8's own rebuild already uses.
//!
//! **K1**: `--plan-only` and a real run share one computation, [`run`], under one lock -- the plan
//! printed by one is the plan a real run prints first, never a second, lockless peek.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;
use std::path::PathBuf;

use crate::commit_boundary::active::{ActiveRefMetadata, read_active_ref_metadata};
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{WitnessState, read_witness, rebuild_witness_over_sound_wal};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::wal::Wal;
use crate::worktree_status::worktree_status;

/// The plan a real run writes from, and everything `--plan-only` prints (RFC 166 D5 K1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiscardDamagedCommitsPlan {
    /// The acknowledged sequence this run removes or declares lost. `None` only for row 7 (the
    /// witness itself is unreadable, so no specific sequence can be named).
    pub witnessed_seq: Option<u64>,
    /// That sequence's own Patch id, as the witness held it. `None` exactly when `witnessed_seq` is.
    pub patch_id: Option<ObjectId>,
    /// Trailing bytes this run removes from the WAL (rows 4 and 7's own shape). Zero for row 5,
    /// whose WAL is already as short as it will get -- nothing to truncate, only the witness moves.
    pub truncated_bytes: usize,
    /// The recovery file (relative to `.prikk/`) the removed bytes go to, byte for byte, saved
    /// before the truncation. `None` when `truncated_bytes` is zero.
    pub recovery_file: Option<PathBuf>,
    /// Whether the worktree still holds uncommitted changes for the content this removes -- RFC 166
    /// D5's own instruction: say the content "may still be in your working tree" only when this is
    /// true, never unconditionally.
    pub working_tree_may_still_hold_content: bool,
}

enum Mode {
    PlanOnly,
    Execute,
}

/// Report what [`discard_damaged_commits`] would do, without writing anything.
pub fn plan_discard_damaged_commits(
    layout: &RepositoryLayout,
) -> Result<DiscardDamagedCommitsPlan> {
    run(layout, Mode::PlanOnly)
}

/// The real run: removes acknowledged damage or declares an acknowledged loss (RFC 166 D5, §13 item
/// 14), for rows 4, 5 and 7 of RFC 166 §5 only. Refuses, writing nothing, for every other row.
pub fn discard_damaged_commits(layout: &RepositoryLayout) -> Result<DiscardDamagedCommitsPlan> {
    run(layout, Mode::Execute)
}

/// Rows this verb does not act on, and why -- never a verb that does not exist, and never silent
/// about which different condition applies.
fn refusal_reason(verdict: &Verdict) -> Option<&'static str> {
    match verdict {
        Verdict::AcknowledgedDamage { .. }
        | Verdict::AcknowledgedLoss { .. }
        | Verdict::UnknownWithDamagedWitness => None,
        Verdict::SubstitutedRecord { .. } => Some(
            "a substituted record is not a crash shape, and this verb does not act on it; a copy \
             is the way out",
        ),
        Verdict::OwnershipMissing => Some(
            "no durable owner names this session's queue; that is a different condition from \
             acknowledged damage or loss, and this verb does not act on it",
        ),
        _ => Some("nothing acknowledged is damaged or lost in this session's own queue"),
    }
}

fn run(layout: &RepositoryLayout, mode: Mode) -> Result<DiscardDamagedCommitsPlan> {
    let _active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    crate::refs::ensure_no_incomplete_publication(layout)?;
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
    let owning_ref = read_active_ref_metadata(layout)?;
    let witness = read_witness(layout, DEFAULT_ACTIVE_NAME)?;
    // K2: any evaluation failure above (a read error) already propagated via `?`, before this line,
    // so nothing past it ever runs against a session this verb could not fully evaluate.
    let verdict = classify(layout, &replay, &owning_ref, &witness)?;

    if let Some(reason) = refusal_reason(&verdict) {
        return Err(PrikkError::Precondition(reason.to_string()));
    }

    let (witnessed_seq, patch_id) = match (&verdict, &witness) {
        (
            Verdict::AcknowledgedDamage { witnessed_seq }
            | Verdict::AcknowledgedLoss { witnessed_seq },
            WitnessState::Valid(record),
        ) => (Some(*witnessed_seq), Some(record.patch_id)),
        (Verdict::UnknownWithDamagedWitness, _) => (None, None),
        _ => unreachable!("refusal_reason above already returned for every other verdict"),
    };

    // RFC 166 D5: row 9 (no durable owner) already refused above, so ownership is `Valid` here --
    // the same guarantee `classify`'s own row-9 priority gives every other call site in this crate.
    let owning_ref_name = match &owning_ref {
        ActiveRefMetadata::Valid(name) => name.clone(),
        ActiveRefMetadata::Missing | ActiveRefMetadata::Invalid(_) => {
            unreachable!("row 9 (OwnershipMissing) already refused above for a non-Valid owner")
        }
    };

    // RFC 166 D5: "may still be in your working tree" only when it genuinely is -- read from the
    // same worktree-status computation `prikk status` itself uses, never assumed. This is an
    // informational note, not a safety check: `worktree_status` reads the active WAL's own folded
    // baseline and can itself refuse over the very tail/damage this verb exists to act on (it has
    // no way to know the repair is about to resolve that) -- a failure here only means the note is
    // withheld, never that the discard itself is blocked by a check this verb's own classification
    // already decided was actionable.
    let working_tree_may_still_hold_content = worktree_status(layout, &owning_ref_name)
        .ok()
        .is_some_and(|status| !status.is_clean());

    match mode {
        Mode::PlanOnly => {
            let preview = wal.preview_truncate_trailing_partial()?;
            Ok(DiscardDamagedCommitsPlan {
                witnessed_seq,
                patch_id,
                truncated_bytes: preview.truncated_bytes,
                recovery_file: preview.recovery_file,
                working_tree_may_still_hold_content,
            })
        }
        Mode::Execute => {
            let repair = wal.truncate_trailing_partial()?;
            let sound_replay = wal.replay()?;
            rebuild_witness_over_sound_wal(
                layout,
                DEFAULT_ACTIVE_NAME,
                &owning_ref_name,
                &sound_replay,
            )?;
            Ok(DiscardDamagedCommitsPlan {
                witnessed_seq,
                patch_id,
                truncated_bytes: repair.truncated_bytes,
                recovery_file: repair.recovery_file,
                working_tree_may_still_hold_content,
            })
        }
    }
}

#[cfg(test)]
mod tests;
