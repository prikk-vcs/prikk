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

use crate::foundation::fsutil::{
    append_file_required, read_file_if_exists, truncate_file_empty_required,
};
use crate::foundation::generation::{self, GenerationRecord};
use crate::foundation::layout::{ContainerSlot, LockableContainer, RepositoryLayout};
use crate::lock::acquire_container_locks;
use crate::received::received_index::{
    decode_received_index_entries_for_resolver, encode_received_index_record,
    fold_one_received_index_entry, reduce_received_index_entries, replay_received_index,
};
use crate::refs::{
    decode_pointer_index_entries_for_resolver, encode_pointer_index_record,
    fold_one_pointer_index_entry, reduce_pointer_index_entries, replay_pointer_index,
};
use crate::trust_index::{
    decode_trust_policy_entries_for_resolver, encode_trust_policy_record,
    fold_one_trust_policy_entry, reduce_trust_policy_entries, replay_trust_policy,
};

/// Handoff 165 Q2: when the live slot resolving this compaction was deduced rather than recorded
/// (the generation log named none), this compaction is the first durable record of which slot is
/// live -- and the target slot it is about to overwrite, plus the generation log, are the only
/// copies of the *other* history the deduction considered. Reads the "before" bytes of a file this
/// compaction is about to touch, but only when there is a deduction to protect against -- an
/// ordinary compaction (the log already recorded) saves nothing, as before this round.
fn read_before_bytes_if_deduced(
    layout: &RepositoryLayout,
    relative: &std::path::Path,
    deduced: bool,
) -> Result<Vec<u8>> {
    if deduced {
        #[cfg(test)]
        let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare(
            "compaction-over-deduced-live-slot-recovery-save",
        );
        Ok(read_file_if_exists(layout.repository_mutation_root(), relative)?.unwrap_or_default())
    } else {
        Ok(Vec::new())
    }
}

/// Saves the target slot's own before/after bytes to the recovery log, under the run the caller
/// already began -- the same `Kind::Replace` shape `pointer_rebuild.rs`'s own F2 save already uses.
/// **Must be called before the generation log is touched**: for the trust policy and received-
/// index containers, a slot's own meaning file (`meaning_paths_for`) *is* its generation log, and
/// `save_replace` reads that meaning file's *current* bytes -- calling this after the generation
/// log already changed would record the slot's meaning against the *new* generation, which a later
/// restore (undoing the generation-log step first, then this one) would never see again, so the
/// run could never actually restore. Called only when the live slot was deduced.
fn save_deduced_target_slot_recovery(
    layout: &RepositoryLayout,
    target_relative: &std::path::Path,
    target_before: &[u8],
    target_after: &[u8],
    label: &str,
) -> Result<()> {
    let target_source = target_relative.to_string_lossy().replace('\\', "/");
    crate::recovery_log::save_replace(layout, &target_source, target_before, target_after, label)?;
    Ok(())
}

/// Saves the generation log's own before/after bytes to the recovery log, under the same run
/// [`save_deduced_target_slot_recovery`] already saved the target slot to. Called after the
/// generation log has actually been switched (its own meaning, per `meaning_paths_for`, is always
/// empty, so call order relative to the slot's own write does not matter for this one).
fn save_deduced_generation_log_recovery(
    layout: &RepositoryLayout,
    generation_log_relative: &std::path::Path,
    generation_log_before: &[u8],
    generation_log_after: &[u8],
    label: &str,
) -> Result<()> {
    let generation_log_source = generation_log_relative.to_string_lossy().replace('\\', "/");
    crate::recovery_log::save_replace(
        layout,
        &generation_log_source,
        generation_log_before,
        generation_log_after,
        label,
    )?;
    Ok(())
}

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

/// RFC 164 round 2 Addendum 1, item 1: read-only, unlocked checks of one subsystem's own guarded
/// files (its live slot and generation log), for `compact --all` (or any multi-target `compact`) to
/// call for *every* target it will touch, before compacting *any* of them. Without this, a multi-
/// target run could compact the pointer index durably, then only discover the received index's own
/// torn tail -- the review's own finding: Rule D's "a refusal writes nothing at all" held per
/// subsystem, not across the whole command. Each function here is strictly earlier than, and
/// redundant with, its own `run_*_compaction`'s authoritative check under the container lock -- a
/// genuine race between this check and the real one is caught there, not here, the same two-call-site
/// shape Rule D's other guarded writers already use. The target slot (about to be truncated and
/// overwritten were compaction to proceed) is exempt by construction: only the live slot is ever
/// replayed by any of these three.
pub fn precheck_ref_pointer_index_before_compaction(layout: &RepositoryLayout) -> Result<()> {
    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let (_, generation_trailing_partial_bytes, generation_tail_offset) =
        generation::resolve_live_slot_with_tail(
            layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry; run `prikk doctor --rebuild-pointer-index \
             --plan-only`, then `prikk doctor --rebuild-pointer-index`",
            "ref pointer index's generation log is lost, and its two slots fit two different \
             histories; run `prikk doctor --rebuild-pointer-index --plan-only`, then `prikk \
             doctor --rebuild-pointer-index` -- the ref log decides, not either slot",
            decode_pointer_index_entries_for_resolver,
            fold_one_pointer_index_entry,
        )?;
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the ref pointer index's generation log",
        generation_trailing_partial_bytes,
        generation_tail_offset,
        "run `prikk doctor --repair-tails`, then retry",
    )?;
    let replay = replay_pointer_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "ref pointer index has a damaged entry; compaction refuses to run on a corrupt \
             container -- run doctor first"
                .to_string(),
        ));
    }
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the ref pointer index",
        replay.trailing_partial_bytes,
        replay.tail_offset,
        "run `prikk doctor --repair-tails`, then retry",
    )
}

/// See [`precheck_ref_pointer_index_before_compaction`]'s own doc.
pub fn precheck_received_index_before_compaction(layout: &RepositoryLayout) -> Result<()> {
    let generation_log_path = layout.received_index_generation_log_path();
    let (_, generation_trailing_partial_bytes, generation_tail_offset) =
        generation::resolve_live_slot_with_tail(
            layout,
            &generation_log_path,
            &layout.received_index_slot_path(ContainerSlot::A),
            &layout.received_index_slot_path(ContainerSlot::B),
            "received-ref index has a damaged entry; no repair exists -- preserve the repository; \
             the way out is a copy of this repository's own `.prikk/` directory from a backup \
             taken before the damage",
            "the received index's generation log is lost, and its two slots fit two different \
             histories; prikk will not guess. Restore the repository's whole `.prikk/` from a \
             backup taken before the log was lost",
            decode_received_index_entries_for_resolver,
            fold_one_received_index_entry,
        )?;
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the received index's generation log",
        generation_trailing_partial_bytes,
        generation_tail_offset,
        "run `prikk doctor --repair-tails`, then retry",
    )?;
    let replay = replay_received_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; compaction refuses to run on a corrupt \
             container -- run doctor first"
                .to_string(),
        ));
    }
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the received index",
        replay.trailing_partial_bytes,
        replay.tail_offset,
        "run `prikk doctor --repair-tails`, then retry",
    )
}

/// See [`precheck_ref_pointer_index_before_compaction`]'s own doc.
pub fn precheck_trust_policy_before_compaction(layout: &RepositoryLayout) -> Result<()> {
    let generation_log_path = layout.trust_policy_generation_log_path();
    let (_, generation_trailing_partial_bytes, generation_tail_offset) =
        generation::resolve_live_slot_with_tail(
            layout,
            &generation_log_path,
            &layout.trust_policy_container_slot_path(ContainerSlot::A),
            &layout.trust_policy_container_slot_path(ContainerSlot::B),
            "trust policy container has a damaged snapshot; no repair exists -- preserve the \
             repository; the way out is a copy of this repository's own `.prikk/` directory from \
             a backup taken before the damage, then re-apply every trust change made since that \
             backup",
            "the trust policy's generation log is lost, and its two slots fit two different \
             histories (one trusts a key the other has revoked); prikk will not guess. Restore \
             the repository's whole `.prikk/` from a backup taken before the log was lost, then \
             re-apply every trust change made since that backup",
            decode_trust_policy_entries_for_resolver,
            fold_one_trust_policy_entry,
        )?;
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the trust policy container's generation log",
        generation_trailing_partial_bytes,
        generation_tail_offset,
        "run `prikk doctor --repair-tails`, then retry",
    )?;
    let replay = replay_trust_policy(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "trust policy container has a damaged snapshot; compaction refuses to run on a \
             corrupt container -- run doctor first"
                .to_string(),
        ));
    }
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the trust policy container",
        replay.trailing_partial_bytes,
        replay.tail_offset,
        "run `prikk doctor --repair-tails`, then retry",
    )
}

fn run_ref_pointer_index_compaction(
    layout: &RepositoryLayout,
    mode: CompactionMode,
) -> Result<CompactionReport> {
    layout.require_current_format()?;
    let _lock = acquire_container_locks(layout, &[LockableContainer::RefPointerIndex])?;
    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let (live_slot, generation_trailing_partial_bytes, generation_tail_offset, live_slot_deduced) =
        generation::resolve_or_deduce(
            layout,
            &generation_log_path,
            &layout.ref_pointer_index_slot_path(ContainerSlot::A),
            &layout.ref_pointer_index_slot_path(ContainerSlot::B),
            "ref pointer index has a damaged entry; run `prikk doctor --rebuild-pointer-index \
         --plan-only`, then `prikk doctor --rebuild-pointer-index`",
            "ref pointer index's generation log is lost, and its two slots fit two different \
         histories; run `prikk doctor --rebuild-pointer-index --plan-only`, then `prikk \
         doctor --rebuild-pointer-index` -- the ref log decides, not either slot",
            &decode_pointer_index_entries_for_resolver,
            &fold_one_pointer_index_entry,
        )?;

    if live_slot_deduced.is_some() {
        // Q2 review (H1 ruling item 2): unlike the received index and the trust policy container,
        // the pointer index has a real way out of the deduced state that does not need a save at
        // all -- the rebuild re-derives the index from the ref log directly, reading neither slot
        // as live, so it is unaffected by which one a deduction would have picked. Refusing here
        // removes the meaning-lookup hazard H1 found (this container's own slot meaning is the ref
        // log, not the generation log, but there is still only one way out worth naming) and keeps
        // a single answer rather than two.
        return Err(PrikkError::Integrity(
            "ref pointer index's generation log names no live slot; compaction refuses to act on \
             a deduced slot -- run `prikk doctor --rebuild-pointer-index --plan-only`, then \
             `prikk doctor --rebuild-pointer-index` instead, which re-derives the index from the \
             ref log directly, never reading either slot as live"
                .to_string(),
        ));
    }
    let replay = replay_pointer_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "ref pointer index has a damaged entry; compaction refuses to run on a corrupt \
             container -- run doctor first"
                .to_string(),
        ));
    }
    let entries_before = replay.entries.len();
    let (replay_trailing_partial_bytes, replay_tail_offset) =
        (replay.trailing_partial_bytes, replay.tail_offset);
    let compacted = reduce_pointer_index_entries(replay.entries);
    let entries_after = compacted.len();

    if mode == CompactionMode::Execute {
        // RFC 163 §9: the generation log's own write-side tail guard, checked only here -- a
        // `--plan-only` run writes nothing and must not refuse over a tail it will never write behind
        // (Addendum 1 item 2's own "refuse only when the operation will append" rule, applied here).
        // Fires before this compaction's first byte, not only before the generation record.
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the ref pointer index's generation log",
            generation_trailing_partial_bytes,
            generation_tail_offset,
            "run `prikk doctor --repair-tails`, then retry",
        )?;
        // RFC 164 round 2 Addendum 1, item 1's "quieter shape": the LIVE slot's own tail, not only
        // the generation log's, else a torn or zeroed tail on the live slot is silently compacted
        // away -- the tail bytes are never read into `entries` above, so they are simply absent from
        // the newly-written target slot, with no recovery file and no line saying so. The target slot
        // (about to be truncated and overwritten below) is exempt by construction: only the live slot
        // is ever replayed here, never the retired one.
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the ref pointer index",
            replay_trailing_partial_bytes,
            replay_tail_offset,
            "run `prikk doctor --repair-tails`, then retry",
        )?;
        let target_slot = live_slot.other();
        let target_relative =
            layout.repository_relative(&layout.ref_pointer_index_slot_path(target_slot))?;
        let root = layout.repository_mutation_root();
        truncate_file_empty_required(root, &target_relative)?;
        let mut buffer = Vec::new();
        for entry in &compacted {
            buffer.extend_from_slice(&encode_pointer_index_record(entry)?);
        }
        append_file_required(root, &target_relative, &buffer)?;
        generation::append_generation_record(
            layout,
            &generation_log_path,
            &GenerationRecord {
                live_slot: target_slot,
            },
        )?;
        // Q2 review (H1 ruling item 2): the deduced state is refused above, before this point is
        // ever reached -- `live_slot_deduced` is always `None` here, so there is nothing to save.
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
    let (live_slot, generation_trailing_partial_bytes, generation_tail_offset, live_slot_deduced) =
        generation::resolve_or_deduce(
            layout,
            &generation_log_path,
            &layout.received_index_slot_path(ContainerSlot::A),
            &layout.received_index_slot_path(ContainerSlot::B),
            "received-ref index has a damaged entry; no repair exists -- preserve the repository; \
             the way out is a copy of this repository's own `.prikk/` directory from a backup \
             taken before the damage",
            "the received index's generation log is lost, and its two slots fit two different \
             histories; prikk will not guess. Restore the repository's whole `.prikk/` from a \
             backup taken before the log was lost",
            &decode_received_index_entries_for_resolver,
            &fold_one_received_index_entry,
        )?;

    let replay = replay_received_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; compaction refuses to run on a corrupt \
             container -- run doctor first"
                .to_string(),
        ));
    }
    let entries_before = replay.entries.len();
    let (replay_trailing_partial_bytes, replay_tail_offset) =
        (replay.trailing_partial_bytes, replay.tail_offset);
    let compacted = reduce_received_index_entries(replay.entries);
    let entries_after = compacted.len();

    if mode == CompactionMode::Execute {
        // RFC 163 §9: see the identical guard in `run_ref_pointer_index_compaction` above.
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the received index's generation log",
            generation_trailing_partial_bytes,
            generation_tail_offset,
            "run `prikk doctor --repair-tails`, then retry",
        )?;
        // RFC 164 round 2 Addendum 1, item 1: the live slot's own tail -- see the identical guard
        // (and its own comment) in `run_ref_pointer_index_compaction` above.
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
        let deduced = live_slot_deduced.is_some();
        let root = layout.repository_mutation_root();
        let mut buffer = Vec::new();
        for entry in &compacted {
            buffer.extend_from_slice(&encode_received_index_record(entry)?);
        }
        // Q2 review (H1 ruling item 1): both saves happen *before* either write -- `save_replace`'s
        // own contract. The slot first, while the generation log still holds its before bytes (its
        // own meaning file); the generation log's own save computes the bytes the append below
        // will produce without performing it yet, so a crash between the saves and the real writes
        // leaves the recovery run already durable and nothing yet overwritten.
        let _run = deduced.then(crate::recovery_log::begin_run);
        if deduced {
            let target_before_bytes = read_before_bytes_if_deduced(layout, &target_relative, true)?;
            save_deduced_target_slot_recovery(
                layout,
                &target_relative,
                &target_before_bytes,
                &buffer,
                "received index compaction over a deduced live slot",
            )?;
            let generation_log_before =
                read_before_bytes_if_deduced(layout, &generation_log_relative, true)?;
            let mut generation_log_after = generation_log_before.clone();
            generation_log_after.extend_from_slice(&generation::encode_generation_record(
                &GenerationRecord {
                    live_slot: target_slot,
                },
            ));
            save_deduced_generation_log_recovery(
                layout,
                &generation_log_relative,
                &generation_log_before,
                &generation_log_after,
                "received index compaction over a deduced live slot",
            )?;
        }
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
    let (live_slot, generation_trailing_partial_bytes, generation_tail_offset, live_slot_deduced) =
        generation::resolve_or_deduce(
            layout,
            &generation_log_path,
            &layout.trust_policy_container_slot_path(ContainerSlot::A),
            &layout.trust_policy_container_slot_path(ContainerSlot::B),
            "trust policy container has a damaged snapshot; no repair exists -- preserve the \
             repository; the way out is a copy of this repository's own `.prikk/` directory from \
             a backup taken before the damage, then re-apply every trust change made since that \
             backup",
            "the trust policy's generation log is lost, and its two slots fit two different \
             histories (one trusts a key the other has revoked); prikk will not guess. Restore \
             the repository's whole `.prikk/` from a backup taken before the log was lost, then \
             re-apply every trust change made since that backup",
            &decode_trust_policy_entries_for_resolver,
            &fold_one_trust_policy_entry,
        )?;

    let replay = replay_trust_policy(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "trust policy container has a damaged snapshot; compaction refuses to run on a \
             corrupt container -- run doctor first"
                .to_string(),
        ));
    }
    let entries_before = replay.entries.len();
    let (replay_trailing_partial_bytes, replay_tail_offset) =
        (replay.trailing_partial_bytes, replay.tail_offset);
    let compacted = reduce_trust_policy_entries(replay.entries);
    let entries_after = compacted.len();

    if mode == CompactionMode::Execute {
        // RFC 163 §9: see the identical guard in `run_ref_pointer_index_compaction` above.
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the trust policy container's generation log",
            generation_trailing_partial_bytes,
            generation_tail_offset,
            "run `prikk doctor --repair-tails`, then retry",
        )?;
        // RFC 164 round 2 Addendum 1, item 1: the live slot's own tail -- see the identical guard
        // (and its own comment) in `run_ref_pointer_index_compaction` above.
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the trust policy container",
            replay_trailing_partial_bytes,
            replay_tail_offset,
            "run `prikk doctor --repair-tails`, then retry",
        )?;
        let target_slot = live_slot.other();
        let target_relative =
            layout.repository_relative(&layout.trust_policy_container_slot_path(target_slot))?;
        let generation_log_relative = layout.repository_relative(&generation_log_path)?;
        let deduced = live_slot_deduced.is_some();
        let root = layout.repository_mutation_root();
        let buffer = match compacted.first() {
            Some(entry) => encode_trust_policy_record(entry)?,
            None => Vec::new(),
        };
        // Q2 review (H1 ruling item 1): both saves happen *before* either write -- see the
        // identical ordering (and its own comment) in `run_received_index_compaction` above.
        let _run = deduced.then(crate::recovery_log::begin_run);
        if deduced {
            let target_before_bytes = read_before_bytes_if_deduced(layout, &target_relative, true)?;
            save_deduced_target_slot_recovery(
                layout,
                &target_relative,
                &target_before_bytes,
                &buffer,
                "trust policy compaction over a deduced live slot",
            )?;
            let generation_log_before =
                read_before_bytes_if_deduced(layout, &generation_log_relative, true)?;
            let mut generation_log_after = generation_log_before.clone();
            generation_log_after.extend_from_slice(&generation::encode_generation_record(
                &GenerationRecord {
                    live_slot: target_slot,
                },
            ));
            save_deduced_generation_log_recovery(
                layout,
                &generation_log_relative,
                &generation_log_before,
                &generation_log_after,
                "trust policy compaction over a deduced live slot",
            )?;
        }
        truncate_file_empty_required(root, &target_relative)?;
        if !buffer.is_empty() {
            append_file_required(root, &target_relative, &buffer)?;
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
