//! RFC 165 R5: `prikk doctor --rebuild-pointer-index` -- re-deriving the ref-pointer index from the
//! ref log, the way out for a pointer-index record that is complete damage (RFC 164 §9.2) or
//! whatever else leaves it untrustworthy, as `ref complete` (R4) is the way out for the ref log
//! leading the pointer by one sound transition.
//!
//! **Structural, never trust-filtered, no signing.** Every ref's rebuilt state is its own newest
//! record in the ref log, in file order -- "last entry wins," the exact reduction `verify`'s own
//! `read_pointers` and `compact_ref_pointer_index` already perform and persist for the pointer index
//! itself, applied here to the ref log instead. No signature is re-verified while building this base:
//! every record durably in the log already passed `publish_locked`'s own trust check at write time,
//! so re-checking it here would be redundant work, not a new gate.
//!
//! **A current pointer that disagrees with the log is not automatically a lead.** It is a lead only
//! when its own `update_seq` is *ahead* of the log's own newest confirmed sequence for that ref --
//! trust re-enters only then, evaluated against RFC 165 R4's own rule
//! ([`crate::ref_completion::evaluate_known_lead`], the same table `ref complete` uses, deliberately
//! *not* `plan_ref_completion` itself, whose own pointer-index-damage gate would refuse every ref the
//! moment *any* ref's pointer record is damaged -- exactly the shape this rebuild exists to recover
//! from). A completable lead blocks the whole rebuild outright (completing it is the correct fix, and
//! overwriting it would drop an authorized transition); a lead that fails the rule is **dropped** --
//! named in the plan ([`DroppedLead`]), and simply absent from the rebuilt index, since the log never
//! confirmed it in the first place.
//!
//! **A disagreeing pointer whose own sequence is *behind* the log is stale, never a lead** (U4 review
//! v1's own finding): the common cause is the pointer index's own newest record for that ref being
//! damaged, so `current_pointer_tolerating_damage_elsewhere` resolves to an older, already-log-
//! confirmed entry instead. Nothing is dropped; the ref is simply **restored** ([`RestoredRef`]) to
//! what the log already soundly confirms. Conflating the two in the plan's own report would claim an
//! authorized transition is being discarded when none is -- K1's "the plan says exactly what will
//! happen" is precisely what this distinction protects.
//!
//! **Never a new way to decide "is this ref's current pointer a lead."** `current_pointer_tolerating_damage_elsewhere`
//! reads the pointer index directly (not `RefStore::read_current_ref_state_id`, which refuses the
//! moment *any* record in the container is damaged) -- the one place this module's own read departs
//! from `ref_completion`'s, and the reason the whole module exists.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RefStatePayload};

use crate::foundation::fsutil::{append_file_required, truncate_file_empty_required};
use crate::foundation::generation::{self, GenerationRecord};
use crate::foundation::layout::{LockableContainer, RepositoryLayout};
use crate::foundation::tail_guard::require_no_unclean_tail;
use crate::lock::acquire_container_locks;
use crate::object_store::{FileObjectStore, ObjectReader};
use crate::ref_completion::{CompletionRefusal, evaluate_known_lead};
use crate::refs::{
    PointerIndexEntry, decode_ref_log_for_rebuild, encode_pointer_index_record,
    replay_pointer_index,
};

/// One ref's own before/after state in a rebuild plan. `before` is the ref's current pointer, read
/// tolerating damage elsewhere in the container (`None` when this ref has no pointer at all, *or*
/// when this ref's own pointer record is itself the damaged one). `after` is what the rebuilt index
/// would hold for it -- the ref log's own newest record, `None` when the ref never appears in the log
/// at all (an uncompleted, never-logged first publish, now dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RefRebuildEntry {
    /// This entry's own ref.
    pub ref_name: String,
    /// The ref's current pointer, before a rebuild -- `None` if it has no readable pointer at all.
    pub before: Option<ObjectId>,
    /// What the rebuilt index would hold for this ref -- `None` if the ref never appears in the log.
    pub after: Option<ObjectId>,
}

/// A lead the rebuild found and refused to carry forward, because it failed RFC 165 R4's own rule --
/// named, per R5 item 4, not silently absent. **Only a pointer genuinely ahead of the log** (its own
/// `update_seq` at or past the log's own newest confirmed one for this ref) is ever reported here --
/// see [`RestoredRef`] for the other shape a stale pointer can take, which this type must never be
/// used for (U4 review v1's own finding: a damaged newest pointer record falls back to an *older*,
/// already-log-confirmed entry, which is behind the log, not ahead of it, and dropping nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DroppedLead {
    /// The ref whose lead was dropped.
    pub ref_name: String,
    /// The leading `RefState` id that was dropped.
    pub lead_ref_state_id: ObjectId,
    /// Which of RFC 165 R4's own conditions it failed.
    pub reason: CompletionRefusal,
}

/// A ref whose *current pointer* read as something other than the log's own newest state, but turned
/// out to be stale -- **behind** the log, not a lead at all. The usual cause: the pointer index's own
/// newest record for this ref is damaged, so `current_pointer_tolerating_damage_elsewhere` resolves
/// to an older, already-log-confirmed entry instead. Nothing authorized is dropped here; the rebuilt
/// index simply restores the ref to what the log already soundly confirms, which is exactly what a
/// correct read of the pointer would have shown had it not been damaged.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoredRef {
    /// The ref whose pointer was stale.
    pub ref_name: String,
    /// The stale `RefState` id the pointer read as, behind the log's own newest confirmed state.
    pub stale_ref_state_id: ObjectId,
}

/// K1: everything `prikk doctor --rebuild-pointer-index [--plan-only]` prints, and what a real run
/// writes from. `--plan-only` and a real run share this one computation (mirroring
/// `compact_ref_pointer_index`/`plan_compact_ref_pointer_index`'s own `CompactionMode` split): there
/// is no second computation that could disagree with the first.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebuildPlan {
    /// Every ref's own before/after state.
    pub per_ref: Vec<RefRebuildEntry>,
    /// Every lead the rebuild found and refused to carry forward.
    pub dropped_leads: Vec<DroppedLead>,
    /// Every ref whose pointer read as stale (behind the log), restored rather than dropped.
    pub restored: Vec<RestoredRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebuildMode {
    Execute,
    PlanOnly,
}

/// The pointer index's own current entries, tolerating damage anywhere else in the container --
/// `replay_pointer_index`'s own `entries` already contains only what decoded (damage is tracked
/// separately, in `record_outcomes`); reading it directly here, without `has_item_failure()`'s gate,
/// is what lets a damaged record for one ref "just not count" instead of refusing every ref.
fn current_pointer_tolerating_damage_elsewhere(
    entries: &[PointerIndexEntry],
    ref_name: &str,
) -> Option<ObjectId> {
    entries
        .iter()
        .rev()
        .find(|entry| entry.ref_name == ref_name)
        .map(|entry| entry.ref_state_id)
}

fn run_pointer_index_rebuild(layout: &RepositoryLayout, mode: RebuildMode) -> Result<RebuildPlan> {
    layout.require_current_format()?;
    let _lock = acquire_container_locks(
        layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
        ],
    )?;

    // Refusal (1 of 2): the ref log has damage or a tail -- repair first. This is the rebuild's own
    // source of truth; its bytes must be trustworthy before anything is derived from them.
    let discovery = decode_ref_log_for_rebuild(layout)?;
    if let Some(message) = discovery.interior_damage {
        return Err(PrikkError::Integrity(format!(
            "the ref log has a damaged record: {message}; run `prikk doctor --repair-tails` only \
             after the damage is resolved -- a complete damaged record is not a tail"
        )));
    }
    require_no_unclean_tail(
        "the ref log",
        discovery.trailing_partial_bytes,
        0,
        "run `prikk doctor --repair-tails`, then retry",
    )?;

    // The ref log's own newest record per ref, in file order -- "last entry wins," the same
    // reduction `verify`'s `read_pointers` and `compact_ref_pointer_index` already persist for the
    // pointer index itself. `log_tip_seq` is what tells a stale pointer (behind the log) apart from a
    // genuine lead (ahead of it): the ref log's own chain invariant (enforced at write time, and
    // itself just confirmed damage-free above) means at most one `RefState` can ever exist at a given
    // `update_seq` for one ref, so comparing sequences is equivalent to -- and cheaper than -- asking
    // "does this exact id appear anywhere in this ref's own log history."
    let mut log_derived: BTreeMap<String, ObjectId> = BTreeMap::new();
    let mut log_tip_seq: BTreeMap<String, u64> = BTreeMap::new();
    for record in &discovery.records {
        log_derived.insert(record.ref_name.clone(), record.new_ref_state_id);
        log_tip_seq.insert(record.ref_name.clone(), record.update_seq);
    }

    let pointer_replay = replay_pointer_index(layout)?;
    let mut ref_names: BTreeSet<String> = log_derived.keys().cloned().collect();
    ref_names.extend(
        pointer_replay
            .entries
            .iter()
            .map(|entry| entry.ref_name.clone()),
    );

    // Refusal (2 of 2): any lead is completable -- complete it first; a rebuild would drop an
    // authorized transition. Collected across every ref before refusing, so one refusal names all of
    // them, not only the first found.
    let mut completable_leads: Vec<String> = Vec::new();
    let mut dropped_leads: Vec<DroppedLead> = Vec::new();
    let mut restored: Vec<RestoredRef> = Vec::new();
    let mut per_ref: Vec<RefRebuildEntry> = Vec::new();
    let objects = FileObjectStore::new(layout.clone());

    for ref_name in &ref_names {
        let before = current_pointer_tolerating_damage_elsewhere(&pointer_replay.entries, ref_name);
        let after = log_derived.get(ref_name).copied();

        if let Some(leading_id) = before {
            if Some(leading_id) != after {
                let leading_state_envelope = objects
                    .read_typed(leading_id, ObjectType::RefState)?
                    .ok_or_else(|| {
                        PrikkError::Integrity(format!("missing RefState object: {leading_id}"))
                    })?;
                let leading_update_seq = RefStatePayload::decode_canonical(
                    &leading_state_envelope.canonical_payload,
                    leading_state_envelope.schema_version,
                )?
                .update_seq;
                let behind_the_log = log_tip_seq
                    .get(ref_name)
                    .is_some_and(|&tip_seq| leading_update_seq <= tip_seq);

                if behind_the_log {
                    // U4 review v1's own finding: a damaged newest pointer record resolves to an
                    // older, already-log-confirmed entry -- behind the log, not a lead, and nothing
                    // authorized is dropped. The rebuild simply restores the correct, sound value.
                    restored.push(RestoredRef {
                        ref_name: ref_name.clone(),
                        stale_ref_state_id: leading_id,
                    });
                } else {
                    // A genuine lead: ahead of (or with no history in) the log. Evaluate it against
                    // R4's own rule, tolerating pointer-index damage elsewhere -- `evaluate_known_
                    // lead`, not `plan_ref_completion`.
                    match evaluate_known_lead(layout, ref_name, leading_id)? {
                        Ok(_plan) => completable_leads.push(ref_name.clone()),
                        Err(refusal) => dropped_leads.push(DroppedLead {
                            ref_name: ref_name.clone(),
                            lead_ref_state_id: leading_id,
                            reason: refusal,
                        }),
                    }
                }
            }
        }

        per_ref.push(RefRebuildEntry {
            ref_name: ref_name.clone(),
            before,
            after,
        });
    }

    if !completable_leads.is_empty() {
        return Err(PrikkError::Precondition(format!(
            "{} ref(s) have a completable lead; run `prikk ref complete <ref>` first -- a rebuild \
             would drop an authorized transition: {}",
            completable_leads.len(),
            completable_leads.join(", ")
        )));
    }

    if mode == RebuildMode::Execute {
        let generation_log_path = layout.ref_pointer_index_generation_log_path();
        let (live_slot, generation_trailing_partial_bytes, generation_tail_offset) =
            generation::resolve_live_slot_with_tail(layout, &generation_log_path)?;
        require_no_unclean_tail(
            "the ref pointer index's generation log",
            generation_trailing_partial_bytes,
            generation_tail_offset,
            "back it up, truncate it to the named offset, then run `prikk verify`",
        )?;
        let target_slot = live_slot.other();
        let target_relative =
            layout.repository_relative(&layout.ref_pointer_index_slot_path(target_slot))?;
        truncate_file_empty_required(layout.repository_mutation_root(), &target_relative)?;
        let mut buffer = Vec::new();
        for record in &discovery.records {
            let entry = PointerIndexEntry {
                ref_name_key: crate::foundation::layout::ref_name_key_bytes(&record.ref_name),
                ref_name: record.ref_name.clone(),
                ref_state_id: record.new_ref_state_id,
            };
            buffer.extend_from_slice(&encode_pointer_index_record(&entry)?);
        }
        append_file_required(layout.repository_mutation_root(), &target_relative, &buffer)?;
        generation::append_generation_record(
            layout,
            &generation_log_path,
            &GenerationRecord {
                live_slot: target_slot,
            },
        )?;
    }

    Ok(RebuildPlan {
        per_ref,
        dropped_leads,
        restored,
    })
}

/// Report what [`rebuild_pointer_index`] would do, without writing anything.
pub fn plan_pointer_index_rebuild(layout: &RepositoryLayout) -> Result<RebuildPlan> {
    run_pointer_index_rebuild(layout, RebuildMode::PlanOnly)
}

/// RFC 165 R5: rebuild the ref-pointer index from the ref log, into the other slot, switching the
/// generation log atomically -- as compaction does. Refuses and writes nothing over ref-log damage
/// or a tail, or any completable lead.
pub fn rebuild_pointer_index(layout: &RepositoryLayout) -> Result<RebuildPlan> {
    run_pointer_index_rebuild(layout, RebuildMode::Execute)
}

#[cfg(test)]
mod tests;
