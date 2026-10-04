//! `prikk doctor --restore-queue-target --ref <ref> [--plan-only]` (RFC 166 D5, §13 item 15).
//!
//! The one way out for row 9 (ownership missing) and D6's own ref-name-vs-witness mismatch (which
//! `classify` already folds into the same [`Verdict::OwnershipMissing`] verdict, "classified the
//! same as no owner at all") -- the 0.48.0 strandings of RFC 166 §1.6 are the main case: a crash
//! during a first commit's own `ref-name` write left a non-empty WAL with no durable owner.
//!
//! **C2: the ref comes from the user, never from the witness.** A witness is unsigned local
//! metadata (RFC 166 C2); it may only ever add a refusal, never supply data to a write. When a
//! witness exists and names a *different* ref than the one requested, this refuses -- the witness,
//! when present and valid, is a stronger source of truth about this queue's own intended owner than
//! the caller's guess, so disagreement is grounds to stop and ask, not to overrule it silently.
//!
//! **The tip check (D5): "the same check `seal` makes, run without writing."** [`current_tip_block`]
//! mirrors `prikk-cli`'s own `seal/support.rs::current_ref_state` exactly -- same refusals, same
//! `None`-is-genesis contract -- reading only, never writing. Any damage it finds is exactly what
//! would stop `seal` from publishing onto that same ref. It is reimplemented here rather than shared
//! across the crate boundary because `prikk-store` already carries this precedent in two other
//! places (`commit_boundary::classification::patch_is_sealed_and_reachable_since`,
//! `seal_from_accepted::read_current_tip`): each caller mirrors the contract it needs rather than
//! forcing one function to serve writers in both crates.
//!
//! **K1, the plan (§13 item 10):** every ref (sorted) whose own current tip's patch ids equal this
//! queue's, exactly -- when more than one ref validates, the plan lists every one, so a restore
//! never attaches a queue to the wrong branch without the caller being told first.
//!
//! **The write, last:** `ref-name`, by atomic replace -- never the truncate-then-append
//! [`crate::write_active_ref_metadata`] uses, because that sequence is safe only when D1 guarantees
//! the WAL is empty at the time of the write. Here the WAL is, by construction, non-empty: a tear
//! between truncate and append would recreate the exact stranding this verb exists to repair.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectId, ObjectType, RefStatePayload};

use crate::commit_boundary::active::read_active_ref_metadata;
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{WitnessState, read_witness};
use crate::foundation::fsutil::write_file_atomically;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryFormat, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::object_store::{FileObjectStore, ObjectReader};
use crate::refs::{RefStore, ensure_no_incomplete_publication, validate_local_branch_ref};
use crate::wal::{Wal, WalRecord};

/// The plan a real run writes from, and everything `--plan-only` prints (RFC 166 D5 K1, item 10).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoreQueueTargetPlan {
    /// The ref this run attaches the queue to -- from the caller (C2), never from the witness.
    pub ref_name: String,
    /// Every queued patch id, in WAL order -- "the queue's records" (RFC 166 §13 item 10, K1).
    pub patch_ids: Vec<ObjectId>,
    /// `ref_name`'s own current tip block, if it has ever been published. `None` is genesis, not
    /// damage: attaching an orphaned, never-sealed queue to a ref that has no history yet is the
    /// ordinary case this verb exists for.
    pub current_tip_block_id: Option<ObjectId>,
    /// Every published ref (by name, sorted) whose own current tip has exactly these patch ids --
    /// §13 item 10's own ambiguity disclosure. Includes `ref_name` itself when it is one of them.
    pub tip_matches: Vec<String>,
}

enum Mode {
    PlanOnly,
    Execute,
}

/// Report what [`restore_queue_target`] would do, without writing anything.
pub fn plan_restore_queue_target(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<RestoreQueueTargetPlan> {
    run(layout, ref_name, Mode::PlanOnly)
}

/// The real run: gives an owned queue its owner back (RFC 166 D5, §13 item 15), for row 9 or D6's
/// mismatch only. Refuses, writing nothing, for every other row.
pub fn restore_queue_target(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<RestoreQueueTargetPlan> {
    run(layout, ref_name, Mode::Execute)
}

fn run(layout: &RepositoryLayout, ref_name: &str, mode: Mode) -> Result<RestoreQueueTargetPlan> {
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
    // C2 / D5: the ref comes from the caller; a witness naming a *different* ref refuses.
    if let WitnessState::Valid(record) = &witness {
        if record.ref_name != ref_name {
            return Err(PrikkError::Precondition(format!(
                "this session's own commit witness names {}, not {ref_name}; restore to that ref \
                 instead, or address the witness first",
                record.ref_name
            )));
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

    let ref_store = RefStore::new(layout.clone());
    let objects = FileObjectStore::new(layout.clone());
    let patch_ids = collect_wal_patch_ids(&replay.records)?;

    // D5: "the same check `seal` makes, run without writing" -- see the module doc.
    let current_tip_block_id = current_tip_block(layout, &objects, &ref_store, &ref_name)?;

    // §13 item 10: every ref whose own tip validates against this queue, not only the requested
    // one -- said before writing, so attaching to the wrong branch is never silent.
    let mut tip_matches = Vec::new();
    for summary in ref_store.list_ref_pointers()? {
        if let Some(block_id) = current_tip_block(layout, &objects, &ref_store, &summary.ref_name)?
        {
            if block_patch_ids_match(&objects, block_id, &patch_ids)? {
                tip_matches.push(summary.ref_name);
            }
        }
    }

    let plan = RestoreQueueTargetPlan {
        ref_name: ref_name.clone(),
        patch_ids,
        current_tip_block_id,
        tip_matches,
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

/// Mirrors `prikk-cli`'s own `seal/support.rs::current_ref_state` (RFC 166 D5's "same check" seal
/// makes): same refusals, same `None`-is-genesis contract, read-only. Returns the current tip
/// Block's id, not the full `RefState` -- nothing here needs the rest of it.
fn current_tip_block(
    layout: &RepositoryLayout,
    objects: &impl ObjectReader,
    ref_store: &RefStore,
    ref_name: &str,
) -> Result<Option<ObjectId>> {
    let Some(ref_state_id) = ref_store.read_current_ref_state_id(ref_name)? else {
        let log = ref_store.replay_log(ref_name)?;
        // RFC 102 Stage 2: a damaged sole record would otherwise read as `log.records.is_empty()`
        // below, misclassifying a ref with corrupted history as one with none at all (genesis).
        if log.has_item_failure() {
            return Err(PrikkError::Integrity(format!(
                "ref {ref_name} pointer is missing and its log has a damaged record; run doctor \
                 before restoring ownership"
            )));
        }
        if log.trailing_partial_bytes != 0 {
            return Err(PrikkError::Integrity(format!(
                "ref {ref_name} pointer is missing and its log has trailing partial bytes; run \
                 doctor before restoring ownership"
            )));
        }
        if !log.records.is_empty()
            && (matches!(
                layout.format(),
                RepositoryFormat::CurrentV6 | RepositoryFormat::V7
            ) || log.records.len() > 1)
        {
            return Err(PrikkError::Integrity(format!(
                "ref {ref_name} pointer/log state does not match the expected publication \
                 transition; run doctor before restoring ownership"
            )));
        }
        return Ok(None);
    };
    let envelope = objects
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "current ref {ref_name} points to missing RefState {ref_state_id}"
            ))
        })?;
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if payload.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "current RefState name mismatch: expected {ref_name}, got {}",
            payload.ref_name
        )));
    }
    if objects
        .read_typed(payload.target_object_id, ObjectType::Block)?
        .is_none()
    {
        return Err(PrikkError::Integrity(format!(
            "current RefState {ref_state_id} targets missing block {}",
            payload.target_object_id
        )));
    }
    Ok(Some(payload.target_object_id))
}

fn block_patch_ids_match(
    objects: &impl ObjectReader,
    block_id: ObjectId,
    patch_ids: &[ObjectId],
) -> Result<bool> {
    let envelope = objects
        .read_typed(block_id, ObjectType::Block)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!("current ref targets missing block {block_id}"))
        })?;
    let block = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
    Ok(block.patch_ids == patch_ids)
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
