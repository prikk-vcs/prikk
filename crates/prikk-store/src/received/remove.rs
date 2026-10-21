//! `prikk received remove <remotes/…>` (0.54.0 step 3 Part 5, B2a): removes one received ref's own
//! pointer from the received-index container, on purpose -- the first operation on this container
//! that discards information a read path (`verify`'s `ReceivedRefs` stage, `reachability.rs`'s own
//! roots list, `cat`/`tree`/`checkout` at a bare `remotes/…` point) currently depends on.
//!
//! **Modeled directly on `compact::compact_received_index`'s own write sequence**
//! (`compact.rs:405-495`, read in full this round): acquire the container lock, resolve the live
//! slot (deduced or recorded), replay and reduce (last entry per `ref_name_key` survives), truncate
//! the target slot, append the reduced records, append a new generation record. **Two differences
//! from ordinary compaction, both load-bearing:**
//! 1. **The reduction also drops the target `ref_name`** -- an `O(1)` added filter over
//!    `reduce_received_index_entries`, never a new reduction algorithm.
//! 2. **The recovery-log save is unconditional**, not gated on `deduced` the way ordinary
//!    compaction's own save is (`compact.rs`'s own `read_before_bytes_if_deduced`). Ordinary
//!    compaction's own invariant ("discards no information any read path depends on") means the
//!    sound-generation-log path never needs an undo: the retired slot it overwrites was already
//!    dead. This operation breaks that invariant on purpose, so every run -- deduced or not -- must
//!    be restorable.
//!
//! **Write safety already covers every real concurrent writer** by reusing the identical
//! `LockableContainer::ReceivedIndex` lock `compact`, `bundle import`, `archive import`, `archive
//! export`, and `doctor`'s own recovery-log restore already take -- confirmed from source
//! (`0.54.0-step-2-B-design-round-2-report.md`'s own question 2), not assumed.

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{
    append_file_required, read_file_if_exists, truncate_file_empty_required,
};
use crate::foundation::generation::{self, GenerationRecord};
use crate::foundation::layout::ref_name_key_bytes;
use crate::foundation::layout::{ContainerSlot, LockableContainer, RepositoryLayout};
use crate::lock::acquire_container_locks;
use crate::received::received_index::{
    decode_received_index_entries_for_resolver, encode_received_index_record,
    fold_one_received_index_entry, lookup_received_index_entry, reduce_received_index_entries,
    replay_received_index,
};
use crate::received::validate_received_ref;

/// Whether a run actually removes the pointer, or only reports what removing it would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoveMode {
    Execute,
    PlanOnly,
}

/// What a `received remove` run did, or would do: the target's own prior pointer (so a caller can
/// report what was removed), and the live record counts before/after -- the same shape
/// `compact::CompactionReport` already reports for a compaction, since this is one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReceivedRemoveReport {
    /// The ref name named, verbatim.
    pub ref_name: String,
    /// The target's own pointer before removal, if one existed at all.
    pub removed: Option<crate::received::ReceivedPointer>,
    /// Live record count before reduction (the resolved slot's own entry count, corruption-checked).
    pub entries_before: usize,
    /// Live record count after reduction (what was, or would be, written to the newly-published
    /// slot) -- always `entries_before - 1` when `removed` is `Some`, `entries_before` when `None`.
    pub entries_after: usize,
    /// The recovery-log run id this removal saved under, naming the way back -- `None` for a
    /// `--plan-only` run, which saves nothing.
    pub run_id: Option<String>,
}

fn run_received_remove(
    layout: &RepositoryLayout,
    ref_name: &str,
    mode: RemoveMode,
) -> Result<ReceivedRemoveReport> {
    layout.require_current_format()?;
    validate_received_ref(ref_name)?;
    let key = ref_name_key_bytes(ref_name);

    let _lock = acquire_container_locks(layout, &[LockableContainer::ReceivedIndex])?;

    // No such pointer: refuse before any write, the same shape a `--plan-only` run already reports
    // without a lock held at all (checked again, under the lock, for the real run, so a concurrent
    // import landing between an unlocked pre-check and the lock cannot create a stale "nothing to
    // remove" answer).
    let existing = lookup_received_index_entry(layout, key)?;
    let removed = existing.clone().map(crate::received::ReceivedPointer::from);
    if existing.is_none() {
        return Err(PrikkError::Precondition(format!(
            "no received pointer named {ref_name} -- nothing to remove"
        )));
    }

    let generation_log_path = layout.received_index_generation_log_path();
    let (live_slot, generation_trailing_partial_bytes, generation_tail_offset, _live_slot_deduced) =
        generation::resolve_or_deduce(
            layout,
            &generation_log_path,
            &layout.received_index_slot_path(ContainerSlot::A),
            &layout.received_index_slot_path(ContainerSlot::B),
            "received-ref index has a damaged entry; no repair exists -- preserve the repository; \
             the way out is a copy of this repository's own `.prikk/` directory from a backup \
             taken before the damage",
            "the received index's generation log is lost, and its two slots fit two different \
             histories; run `prikk compact --received-index --keep-slot a|b --plan-only` to see \
             both slots and choose, or restore the repository's whole `.prikk/` from a backup \
             taken before the log was lost",
            &decode_received_index_entries_for_resolver,
            &fold_one_received_index_entry,
        )?;

    let replay = replay_received_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; received remove refuses to run on a corrupt \
             container -- run doctor first"
                .to_string(),
        ));
    }
    let entries_before = replay.entries.len();
    let (replay_trailing_partial_bytes, replay_tail_offset) =
        (replay.trailing_partial_bytes, replay.tail_offset);
    let reduced = reduce_received_index_entries(replay.entries);
    // B2a's own one added step over ordinary compaction's reduction: drop the target's own
    // surviving entry, after the identical last-entry-per-key reduction every reader already
    // performs.
    let compacted: Vec<_> = reduced
        .into_iter()
        .filter(|entry| entry.ref_name_key != key)
        .collect();
    let entries_after = compacted.len();
    let mut run_id = None;

    if mode == RemoveMode::Execute {
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the received index's generation log",
            generation_trailing_partial_bytes,
            generation_tail_offset,
            "run `prikk doctor --repair-tails`, then retry",
        )?;
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the received index",
            replay_trailing_partial_bytes,
            replay_tail_offset,
            "run `prikk doctor --repair-tails`, then retry",
        )?;
        let target_slot = live_slot.other();
        let target_relative =
            layout.repository_relative(&layout.received_index_slot_path(target_slot))?;
        let generation_log_relative = layout.repository_relative(&generation_log_path)?;
        let root = layout.repository_mutation_root();
        let mut buffer = Vec::new();
        for entry in &compacted {
            buffer.extend_from_slice(&encode_received_index_record(entry)?);
        }

        // Unconditional, unlike ordinary compaction's own `deduced.then(...)` gate (module doc,
        // above): this operation discards a pointer a read path depends on, every run, so every
        // run must be restorable, not only the one where the live slot had to be deduced. The
        // slot is saved first, while the generation log still holds its own "before" bytes (its
        // own meaning file, `recovery_log::meaning_paths_for`) -- the identical ordering
        // `compact.rs`'s own deduced-path comment already requires, now unconditional.
        let _run = crate::recovery_log::begin_run();
        #[cfg(test)]
        let _whole_read_scope =
            crate::foundation::fsutil::whole_read_guard::declare("received-remove-recovery-save");
        let target_before = read_file_if_exists(root, &target_relative)?.unwrap_or_default();
        crate::recovery_log::save_replace(
            layout,
            &target_relative.to_string_lossy().replace('\\', "/"),
            &target_before,
            &buffer,
            "received remove",
        )?;
        let generation_log_before =
            read_file_if_exists(root, &generation_log_relative)?.unwrap_or_default();
        let mut generation_log_after = generation_log_before.clone();
        generation_log_after.extend_from_slice(&generation::encode_generation_record(
            &GenerationRecord {
                live_slot: target_slot,
            },
        ));
        crate::recovery_log::save_replace(
            layout,
            &generation_log_relative.to_string_lossy().replace('\\', "/"),
            &generation_log_before,
            &generation_log_after,
            "received remove",
        )?;
        run_id = crate::recovery_log::current_run_id_hex();

        truncate_file_empty_required(root, &target_relative)?;
        append_file_required(root, &target_relative, &buffer)?;
        generation::append_generation_record(
            layout,
            &generation_log_path,
            &GenerationRecord {
                live_slot: target_slot,
            },
        )?;
    }

    Ok(ReceivedRemoveReport {
        ref_name: ref_name.to_string(),
        removed,
        entries_before,
        entries_after,
        run_id,
    })
}

/// Remove `ref_name`'s own received pointer. The objects it reached are not removed -- only the
/// pointer -- so a stored `Block`/`RefState` nothing else reaches becomes an **unreached object**,
/// 0.54.0 step 3 Part 1's own warning (B1), the next time `prikk verify` runs.
///
/// # Errors
///
/// `ref_name` is not a `remotes/…` name; no pointer is named `ref_name`; the received index is
/// damaged; a generation-log or received-index tail is unclean; or a concurrent writer already
/// holds the received-index lock.
pub fn remove_received_pointer(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<ReceivedRemoveReport> {
    run_received_remove(layout, ref_name, RemoveMode::Execute)
}

/// Report what [`remove_received_pointer`] would do, without writing anything. Still takes the
/// container lock for the duration of the read, the same reason `compact::plan_compact_*` already
/// does (module doc there: "numbers read without it could go stale before they reach the
/// terminal, and an operator acts on what a preview reports").
///
/// # Errors
///
/// `ref_name` is not a `remotes/…` name; no pointer is named `ref_name`; or the received index is
/// damaged.
pub fn plan_remove_received_pointer(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<ReceivedRemoveReport> {
    run_received_remove(layout, ref_name, RemoveMode::PlanOnly)
}

#[cfg(test)]
mod tests;
