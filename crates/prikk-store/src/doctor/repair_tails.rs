//! RFC 164 Rule C (RFC 165 R5 extends it to the ref log): `prikk doctor --repair-tails` -- one repair
//! for every tail Rule A (and now the ref log's own §9.2) defines, across every file it covers (the
//! WAL, the pointer index, the seven Rule-A files, and the ref log), truncating each one it finds
//! under that file's own lock, saving what it removes first.
//!
//! **Not the object containers** -- Rule B makes those report only. `--repair-wal-tail` and
//! `--repair-pointer-index-tail` stay, unchanged, as the single-file forms; this reuses their own
//! repair functions rather than a second implementation of either.
//!
//! **The ref log's own tail is never truncated while any ref's pointer leads its log** (RFC 165 R5):
//! a tail *by position* does not know whether the same bytes are actually a completable publication's
//! own interrupted append (RFC 165 R4 completes those, never truncates them) -- truncating first would
//! destroy exactly what a later `ref complete` needs. `ensure_no_incomplete_publication` is checked
//! before this repair touches the ref log at all; a lead anywhere routes through the same all-or-nothing
//! refusal below as any other file's interior damage, not a silent skip.
//!
//! **All or nothing on interior damage.** Every one of the ten files is read once, up front, before
//! anything is touched. If any one of them has interior damage (a sound record follows a damaged
//! one -- RFC 164 Rule A's own "not a tail" case, extended to the ref log by RFC 165 §9.2), or the ref
//! log's own tail cannot yet be confirmed lead-free, this refuses immediately, naming every such file
//! and its offset, and truncates nothing anywhere.
//!
//! **Locks, in the order this project's own convention already uses** (`bundle.rs::import_bundle`:
//! `ActiveLock` first, container locks second): `ActiveLock` covers the WAL, trust keys, and author
//! keys (none of the three has a dedicated `LockableContainer` -- confirmed from source:
//! `trust.rs::add_trusted_maintainer` and `author_key_index.rs::record_author_key_material` both
//! take `&ActiveLock` as their only guard for these two files, the same way `Wal::truncate_trailing_
//! partial`'s own callers already do for the WAL). `acquire_container_locks` with `RefPointerIndex`,
//! `ReceivedIndex`, and `TrustPolicy` covers the pointer index, the received index, and the trust
//! policy container, each alongside its own generation log -- confirmed from `compact.rs`, which
//! locks exactly these three containers to write to exactly these three generation logs.
//! `acquire_container_locks` sorts and dedups its own argument list before acquiring (`lock.rs`), so
//! passing all three in one call is deadlock-safe regardless of the order named here.

use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{
    MutationRoot, ensure_directory_required, len_to_u64, read_file_if_exists,
    truncate_existing_file_required, write_file_atomically,
};
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryLayout};
use crate::lock::{ActiveLock, acquire_container_locks};
use crate::refs::{
    ensure_no_incomplete_publication, replay_pointer_index, truncate_pointer_index_trailing_partial,
};
use crate::verify::check_appended_file_tails;
use crate::wal::Wal;

/// One covered file's own repair outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RepairTailsFileOutcome {
    /// The same label `verify`'s own `AppendedFileTailStatus` uses for this file (or "WAL"/"pointer
    /// index" for the two RFC 162 rule 3 already covered).
    pub label: &'static str,
    /// Trailing bytes truncated. `0` when this file had no tail to begin with.
    pub truncated_bytes: usize,
    /// The recovery file (relative to `.prikk/`) holding exactly the bytes removed, written durably
    /// before the truncation. `None` when nothing was removed.
    pub recovery_file: Option<PathBuf>,
}

/// `prikk doctor --repair-tails`'s own report: one row per covered file, in a fixed order (the WAL,
/// the pointer index, then the eight files `check_appended_file_tails` reports, the ref log now last
/// among them) -- always all ten, whether or not each one had anything to repair, so a clean
/// repository's own report says so per file rather than by omission.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RepairTailsReport {
    /// One outcome per covered file.
    pub files: Vec<RepairTailsFileOutcome>,
}

/// `prikk doctor --repair-tails`. See the module doc for the locking order and the all-or-nothing
/// rule.
pub fn repair_tails(layout: &RepositoryLayout) -> Result<RepairTailsReport> {
    let _active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    let _container_locks = acquire_container_locks(
        layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
            LockableContainer::TrustPolicy,
        ],
    )?;

    // Read every covered file once, before touching any of them.
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let wal_replay = wal.replay()?;
    // RFC 164 Addendum 2 item 5: `replay_pointer_index` resolves the pointer index's own live slot
    // through `resolve_live_slot`, which refuses raw ("generation log has a damaged record; run doctor
    // before reading") the moment that log has interior damage -- caught here, instead of propagated
    // with `?`, so a damaged generation log refuses the same uniform, named way every other covered
    // file's own damage does, rather than leaking that resolver's own bare refusal text.
    let pointer_replay = match replay_pointer_index(layout) {
        Ok(replay) => replay,
        Err(error) => {
            return Err(PrikkError::Integrity(format!(
                "--repair-tails refuses: 1 file(s) have interior damage, not a tail -- nothing was \
                 touched: pointer index generation log: {error}"
            )));
        }
    };
    let appended = check_appended_file_tails(layout)?;
    // RFC 166 §13 item 4: `--repair-tails` also repairs the WAL, so it must also classify before
    // touching anything, the same as `--repair-wal-tail`. Row 8 (a damaged or stale witness over a
    // wholly sound WAL) is deliberately not refused here -- nothing is at risk, and this repair
    // proactively rebuilds it below (§13 item 5), rather than leaving it to the next commit's own
    // fold-from-scratch-when-uncovered path (§13 item 2), which would otherwise be the only thing
    // that ever replaces it.
    let owning_ref =
        crate::commit_boundary::active::read_active_ref_metadata_for(layout, DEFAULT_ACTIVE_NAME)?;
    let witness = crate::commit_boundary::witness::read_witness(layout, DEFAULT_ACTIVE_NAME)?;
    let commit_witness_verdict = crate::commit_boundary::classification::classify(
        layout,
        &wal_replay,
        &owning_ref,
        &witness,
    )?;

    // All or nothing: refuse before touching anything if any file has interior damage.
    let mut damaged: Vec<String> = Vec::new();
    if let Some(reason) =
        crate::commit_boundary::classification::write_refusal_reason(&commit_witness_verdict, None)
    {
        damaged.push(format!("acknowledged commits: {reason}"));
    }
    if wal_replay.has_item_failure() {
        damaged.push(format!(
            "WAL: {}",
            wal_replay
                .damage_summary()
                .unwrap_or_else(|| "a record failed to decode".to_string())
        ));
    }
    if pointer_replay.has_item_failure() {
        damaged.push("pointer index: an entry failed to decode".to_string());
    }
    for status in &appended {
        if let Some(message) = &status.interior_damage {
            damaged.push(format!("{}: {message}", status.label));
        }
    }
    // RFC 165 R5 (§9.2): a tail *by position* in the ref log is not necessarily lead-free --
    // `ref_log_container_tail` has no view of the pointer index, so the same physical bytes a
    // completable publication's own interrupted log append left behind read identically to orphaned
    // garbage. Truncating a lead's own tail would destroy exactly what a future completion verb needs
    // to finish it (RFC 165 R4), so this refuses rather than guesses: a lead anywhere blocks the ref
    // log's own row specifically, the same all-or-nothing refusal every other covered file's damage
    // already gets, not a silent skip.
    let ref_log_has_tail = appended
        .iter()
        .any(|status| status.label == "ref log" && status.trailing_partial_bytes != 0);
    if ref_log_has_tail {
        if let Err(error) = ensure_no_incomplete_publication(layout) {
            damaged.push(format!(
                "ref log: the ref publication precondition does not hold ({error}), so its trailing \
                 bytes cannot be confirmed lead-free -- not a repairable tail until that is resolved"
            ));
        }
    }
    if !damaged.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "--repair-tails refuses: {} file(s) have interior damage, not a tail -- nothing was \
             touched: {}",
            damaged.len(),
            damaged.join("; ")
        )));
    }

    let mut files = Vec::with_capacity(2 + appended.len());

    let wal_repair = wal.truncate_trailing_partial()?;
    files.push(RepairTailsFileOutcome {
        label: "WAL",
        truncated_bytes: wal_repair.truncated_bytes,
        recovery_file: wal_repair.recovery_file,
    });

    let pointer_repair = truncate_pointer_index_trailing_partial(layout)?;
    files.push(RepairTailsFileOutcome {
        label: "pointer index",
        truncated_bytes: pointer_repair.truncated_bytes,
        recovery_file: pointer_repair.recovery_file,
    });

    for status in &appended {
        if status.trailing_partial_bytes == 0 {
            files.push(RepairTailsFileOutcome {
                label: status.label,
                truncated_bytes: 0,
                recovery_file: None,
            });
            continue;
        }
        let relative = appended_file_relative_path(layout, status.label)?;
        let outcome = truncate_one_tail(
            layout.repository_mutation_root(),
            &relative,
            status.label,
            status.tail_offset,
        )?;
        files.push(outcome);
    }

    // RFC 166 §13 item 5: a damaged or stale witness over a WAL `classify` already confirmed is
    // wholly sound (row 8 -- the only `commit_witness_verdict` shape that reaches this line without
    // having refused above) is rebuilt from the WAL's own sound records, never from the damaged
    // witness's own bytes. `owning_ref` must be `Valid` whenever `wal_replay.records` is non-empty --
    // otherwise row 9 (`OwnershipMissing`) would have refused above instead of reaching here; an empty
    // WAL with no valid owner is left alone (nothing to attribute a rebuilt record to, and
    // `rebuild_witness_over_sound_wal`'s own empty-WAL case clears it regardless of ownership, which
    // this skip only withholds when there is no ref to name in the rebuilt record it would otherwise
    // need).
    if matches!(
        commit_witness_verdict,
        crate::commit_boundary::classification::Verdict::WitnessDamaged
    ) {
        if let crate::commit_boundary::active::ActiveRefMetadata::Valid(ref_name) = &owning_ref {
            crate::commit_boundary::witness::rebuild_witness_over_sound_wal(
                layout,
                DEFAULT_ACTIVE_NAME,
                ref_name,
                &wal_replay,
            )?;
        } else if wal_replay.records.is_empty() {
            crate::commit_boundary::witness::clear_witness(layout, DEFAULT_ACTIVE_NAME)?;
        }
    }

    Ok(RepairTailsReport { files })
}

/// Re-derives one Rule-A file's own current relative path -- the same layout getters (and, for the
/// two generation-aware files, the same `resolve_live_slot` call) `check_appended_file_tails` used
/// to read it, called again here under the same held locks, so the slot cannot have changed between
/// the read pass above and this one.
fn appended_file_relative_path(layout: &RepositoryLayout, label: &'static str) -> Result<PathBuf> {
    let path = match label {
        "trust keys" => layout.trust_key_container_path(),
        "trust policy" => {
            let slot = resolve_live_slot(layout, &layout.trust_policy_generation_log_path())?;
            layout.trust_policy_container_slot_path(slot)
        }
        "author keys" => layout.author_key_container_path(),
        "received index" => {
            let slot = resolve_live_slot(layout, &layout.received_index_generation_log_path())?;
            layout.received_index_slot_path(slot)
        }
        "pointer index generation log" => layout.ref_pointer_index_generation_log_path(),
        "received index generation log" => layout.received_index_generation_log_path(),
        "trust policy generation log" => layout.trust_policy_generation_log_path(),
        "ref log" => {
            layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A)
        }
        other => {
            return Err(PrikkError::Integrity(format!(
                "--repair-tails: unrecognized appended-file label {other:?} -- this is a bug \
                 (`check_appended_file_tails` and `appended_file_relative_path` have drifted)"
            )));
        }
    };
    layout.repository_relative(&path)
}

/// Truncate one Rule-A file to `tail_offset`, saving the removed bytes first. Mirrors `refs/pointer_
/// index.rs::truncate_pointer_index_trailing_partial`'s own shape exactly, generic over the file
/// (a label and a relative path) instead of one copy per format -- Rule C's own point ("one
/// implementation... cannot leave a file behind").
fn truncate_one_tail(
    root: &MutationRoot,
    relative: &Path,
    label: &'static str,
    tail_offset: usize,
) -> Result<RepairTailsFileOutcome> {
    // RFC 165 R5: the ref log is now one of this function's own generic callers (`repair_tails`'s
    // own loop below) and is a "store-growing file" the whole-read guard watches; this repair's own
    // whole read of it (to truncate under lock, the same as every other covered file here) is the
    // declared exception, matching `truncate_incomplete_tail`'s own declaration for the same file.
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let Some(bytes) = read_file_if_exists(root, relative)? else {
        return Ok(RepairTailsFileOutcome {
            label,
            truncated_bytes: 0,
            recovery_file: None,
        });
    };
    let repaired_len = len_to_u64(tail_offset)?;
    let removed = bytes.get(tail_offset..).unwrap_or_default();
    if removed.is_empty() {
        return Ok(RepairTailsFileOutcome {
            label,
            truncated_bytes: 0,
            recovery_file: None,
        });
    }
    let recovery_file = save_removed_bytes(root, label, tail_offset, removed)?;
    truncate_existing_file_required(root, relative, repaired_len)?;
    Ok(RepairTailsFileOutcome {
        label,
        truncated_bytes: removed.len(),
        recovery_file: Some(recovery_file),
    })
}

/// Durably write `removed` to `recovery/<slug>-at-<offset>-<hash>.bytes` under `.prikk/`, and return
/// that path (relative to `.prikk/`) -- mirrors `wal.rs`/`pointer_index.rs`'s own `save_removed_
/// bytes` exactly, generic over the file's label instead of a fixed name.
fn save_removed_bytes(
    root: &MutationRoot,
    label: &str,
    offset: usize,
    removed: &[u8],
) -> Result<PathBuf> {
    let digest = prikk_hash::to_hex(&prikk_hash::sha256(removed));
    let short = digest.get(..16).unwrap_or(&digest);
    let slug: String = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let directory = PathBuf::from("recovery");
    let file = directory.join(format!("{slug}-at-{offset}-{short}.bytes"));
    ensure_directory_required(root, &directory)?;
    write_file_atomically(root, &file, removed)?;
    Ok(file)
}

#[cfg(test)]
mod tests;
