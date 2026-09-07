//! RFC 102 Stage 6 Step 2's compactor (design-v1.md §15, §15.6-§15.9): reclaims dead records from the
//! three genuine compaction targets by writing their live, reduced record set to the currently-retired
//! slot, then durably switching the generation log to name it live -- acceptance criterion 1's
//! ordering: the new slot's bytes are durable before the generation record that makes them
//! authoritative is appended (`generation::append_generation_record`).
//!
//! **Refuses on any known-corrupt record (§15.3, non-negotiable).** A naive compactor that read via
//! the resync reader and wrote back only what it yields would silently drop a corrupt record from the
//! new slot while abandoning the old one -- corruption becomes permanent deletion, through the exact
//! mechanism built to survive it, and the operation reports success. Every function below fails closed
//! on *any* damaged entry, not just the latest -- stricter than the read path's own "damaged latest
//! entry" contract, because compaction is destructive to the retired slot in a way an ordinary read
//! never is.
//!
//! **The container lock is held for the whole operation** -- resolve, read, reduce, and (for a real
//! run) truncate, write, switch -- so a concurrent writer for this container is excluded throughout,
//! not just during the final switch. This is what lets the writers that share the same lock
//! (`publish`, `add_trusted_maintainer`/`remove_trusted_maintainer`, `import_bundle`) stay ignorant of
//! compaction: they cannot observe a torn state, because they cannot run at all while this holds the
//! lock. `plan_compact_*` holds the same lock for the same reason, even though it never writes --
//! numbers read without it could go stale before they reach the terminal, and an operator acts on what
//! a preview reports.
//!
//! **Never touches the ref-log or trust-key containers.** Neither is a compaction target -- the ref
//! log is DC-38/DC-69's audit trail, and the trust key container is TOFU history (`trust.rs:77`) --
//! and this module has no function for either, which is the enforcement, not a rule stated in prose.
//!
//! **Compaction discards no information any read path depends on -- not merely "nothing reads it."**
//! `refs/verify/scan.rs`'s own read path (`read_pointers`) iterates every ref-pointer-index record in
//! append order and overwrites per `ref_name_key`, keeping only the last -- the exact reduction
//! `compact_ref_pointer_index` performs and persists. `verify`'s output is unchanged by construction,
//! not merely by inspection: compaction persists the reduction the reader already computes at read
//! time, it does not invent a new one.
//!
//! **No confirmation prompt, unlike `unlock`.** `prikk unlock`'s prompt exists because the tool is
//! asking the operator to supply a fact it cannot check itself -- whether a process is truly gone.
//! Compaction has no equivalent unknown: the container lock excludes concurrent writers, the
//! corruption refusal checks every record before touching anything, and the reduction persists exactly
//! what every reader already resolves. A prompt here would gate on nothing -- the same "a check that
//! cannot fail is worse than no check" shape §15.9 already named for a different case in this stage.

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{append_file_required, truncate_file_empty_required};
use crate::foundation::generation::{self, GenerationRecord};
use crate::foundation::layout::{LockableContainer, RepositoryLayout};
use crate::lock::acquire_container_locks;
use crate::received::received_index::{
    ReceivedIndexEntry, encode_received_index_record, replay_received_index,
};
use crate::refs::{PointerIndexEntry, encode_pointer_index_record, replay_pointer_index};
use crate::trust_index::{encode_trust_policy_record, replay_trust_policy};

/// Outcome of one compaction run: how many live records existed before and after reduction. This is
/// the deduplication compaction performs on index/pointer *records*, not object deletion -- nothing
/// in this module ever deletes an object; `entries_before - entries_after` counts stale pointer/
/// snapshot records reclaimed, never data. `plan_compact_*` returns the same shape without writing
/// anything -- what a real run *would* report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionReport {
    /// Which container this run (or preview) targets.
    pub container: LockableContainer,
    /// Live record count before reduction (the resolved slot's own entry count, corruption-checked).
    pub entries_before: usize,
    /// Live record count after reduction (what was, or would be, written to the newly-published
    /// slot).
    pub entries_after: usize,
}

/// Whether a compaction run publishes its reduction or only reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionMode {
    /// Truncate the retired slot, write the reduced set, and switch the generation record.
    Execute,
    /// Compute the same reduction and report the same counts, but touch nothing on disk.
    PlanOnly,
}

fn run_ref_pointer_index_compaction(
    layout: &RepositoryLayout,
    mode: CompactionMode,
) -> Result<CompactionReport> {
    layout.require_current_format()?;
    let _lock = acquire_container_locks(layout, &[LockableContainer::RefPointerIndex])?;
    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let live_slot = generation::resolve_live_slot(layout, &generation_log_path)?;

    let replay = replay_pointer_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "ref pointer index has a damaged entry; compaction refuses to run on a corrupt \
             container -- run doctor first"
                .to_string(),
        ));
    }
    let entries_before = replay.entries.len();
    let mut compacted: Vec<PointerIndexEntry> = Vec::new();
    for entry in replay.entries {
        compacted
            .retain(|existing: &PointerIndexEntry| existing.ref_name_key != entry.ref_name_key);
        compacted.push(entry);
    }
    let entries_after = compacted.len();

    if mode == CompactionMode::Execute {
        let target_slot = live_slot.other();
        let target_relative =
            layout.repository_relative(&layout.ref_pointer_index_slot_path(target_slot))?;
        truncate_file_empty_required(layout.repository_mutation_root(), &target_relative)?;
        let mut buffer = Vec::new();
        for entry in &compacted {
            buffer.extend_from_slice(&encode_pointer_index_record(entry)?);
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

    Ok(CompactionReport {
        container: LockableContainer::RefPointerIndex,
        entries_before,
        entries_after,
    })
}

/// Compact the ref-pointer-index container: last entry per `ref_name_key` survives, matching
/// `lookup_ref_pointer`'s own reverse-scan resolution exactly -- compaction changes which bytes are on
/// disk, never which pointer a lookup resolves to.
pub fn compact_ref_pointer_index(layout: &RepositoryLayout) -> Result<CompactionReport> {
    run_ref_pointer_index_compaction(layout, CompactionMode::Execute)
}

/// Report what `compact_ref_pointer_index` would reclaim, without writing anything.
pub fn plan_compact_ref_pointer_index(layout: &RepositoryLayout) -> Result<CompactionReport> {
    run_ref_pointer_index_compaction(layout, CompactionMode::PlanOnly)
}

fn run_received_index_compaction(
    layout: &RepositoryLayout,
    mode: CompactionMode,
) -> Result<CompactionReport> {
    layout.require_current_format()?;
    let _lock = acquire_container_locks(layout, &[LockableContainer::ReceivedIndex])?;
    let generation_log_path = layout.received_index_generation_log_path();
    let live_slot = generation::resolve_live_slot(layout, &generation_log_path)?;

    let replay = replay_received_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; compaction refuses to run on a corrupt \
             container -- run doctor first"
                .to_string(),
        ));
    }
    let entries_before = replay.entries.len();
    let mut compacted: Vec<ReceivedIndexEntry> = Vec::new();
    for entry in replay.entries {
        compacted
            .retain(|existing: &ReceivedIndexEntry| existing.ref_name_key != entry.ref_name_key);
        compacted.push(entry);
    }
    let entries_after = compacted.len();

    if mode == CompactionMode::Execute {
        let target_slot = live_slot.other();
        let target_relative =
            layout.repository_relative(&layout.received_index_slot_path(target_slot))?;
        truncate_file_empty_required(layout.repository_mutation_root(), &target_relative)?;
        let mut buffer = Vec::new();
        for entry in &compacted {
            buffer.extend_from_slice(&encode_received_index_record(entry)?);
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

    Ok(CompactionReport {
        container: LockableContainer::ReceivedIndex,
        entries_before,
        entries_after,
    })
}

/// Compact the received-index container: last entry per `ref_name_key` survives, matching
/// `lookup_received_index_entry`'s own resolution exactly.
pub fn compact_received_index(layout: &RepositoryLayout) -> Result<CompactionReport> {
    run_received_index_compaction(layout, CompactionMode::Execute)
}

/// Report what `compact_received_index` would reclaim, without writing anything.
pub fn plan_compact_received_index(layout: &RepositoryLayout) -> Result<CompactionReport> {
    run_received_index_compaction(layout, CompactionMode::PlanOnly)
}

fn run_trust_policy_compaction(
    layout: &RepositoryLayout,
    mode: CompactionMode,
) -> Result<CompactionReport> {
    layout.require_current_format()?;
    let _lock = acquire_container_locks(layout, &[LockableContainer::TrustPolicy])?;
    let generation_log_path = layout.trust_policy_generation_log_path();
    let live_slot = generation::resolve_live_slot(layout, &generation_log_path)?;

    let replay = replay_trust_policy(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "trust policy container has a damaged snapshot; compaction refuses to run on a \
             corrupt container -- run doctor first"
                .to_string(),
        ));
    }
    let entries_before = replay.entries.len();
    let last_snapshot = replay.entries.into_iter().next_back();
    let entries_after = usize::from(last_snapshot.is_some());

    if mode == CompactionMode::Execute {
        let target_slot = live_slot.other();
        let target_relative =
            layout.repository_relative(&layout.trust_policy_container_slot_path(target_slot))?;
        truncate_file_empty_required(layout.repository_mutation_root(), &target_relative)?;
        if let Some(entry) = &last_snapshot {
            let record = encode_trust_policy_record(entry)?;
            append_file_required(layout.repository_mutation_root(), &target_relative, &record)?;
        }
        generation::append_generation_record(
            layout,
            &generation_log_path,
            &GenerationRecord {
                live_slot: target_slot,
            },
        )?;
    }

    Ok(CompactionReport {
        container: LockableContainer::TrustPolicy,
        entries_before,
        entries_after,
    })
}

/// Compact the trust-policy container: only the last complete snapshot survives -- not a per-key
/// reduction like the other two, because this container is snapshots, not an append log of individual
/// adoptions (`trust_index.rs`'s own module doc). Every earlier snapshot is, by definition, entirely
/// superseded.
pub fn compact_trust_policy(layout: &RepositoryLayout) -> Result<CompactionReport> {
    run_trust_policy_compaction(layout, CompactionMode::Execute)
}

/// Report what `compact_trust_policy` would reclaim, without writing anything.
pub fn plan_compact_trust_policy(layout: &RepositoryLayout) -> Result<CompactionReport> {
    run_trust_policy_compaction(layout, CompactionMode::PlanOnly)
}

#[cfg(test)]
mod tests;
