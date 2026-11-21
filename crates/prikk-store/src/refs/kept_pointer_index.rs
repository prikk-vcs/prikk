//! The ref pointer index a hold keeps (0.58.0 S5, the second user of S4's mechanism).
//!
//! `publish_body` and the publication precondition read the whole pointer index nine times per `sync seal` claim (S5a, `probe-groups-250.txt`).
//! While a command holds a lock, [`replay_pointer_index_shared`] keeps the decoded replay on the repository's `MutationRoot` (the hold's
//! slot, S4's, keyed by this module's type) and, at the next use, reads only what was appended. **Outside a hold nothing is kept** and every call is
//! the whole read `replay_pointer_index` has always been.
//!
//! **Never stale** (S4's rule, with the slot the pointer index adds). A use starts from the live slot *as the generation log resolves it*
//! (`live_pointer_index_slot`, the same function `replay_pointer_index` uses): a compaction or rebuild between two uses makes it a different
//! slot, and that is a full re-read. Then one `stat` of the slot's file (device, inode, size): the same file at the known size is used as is;
//! a longer one is read from where the copy ends, together with the copy's last record (the canary), by `kept_file::read_verified_tail`;
//! another inode, a shorter file or a canary that differs is read whole. A tail with an item failure or a trailing partial record is never
//! added: the copy is dropped and the call takes today's path (damage and torn tails behave as a fresh read does).
//!
//! **Who else rewrites it** (S5a §3): `prikk compact` and `doctor --rebuild-pointer-index` switch the live slot and append a generation
//! record, under the pointer-index lock and no active lock -- seen as a different slot; `doctor --repair-pointer-index-tail` and `--repair-tails`,
//! the publication's own partial-tail repair and `archive import`'s cancel truncate in place -- seen by the size, or by the canary if the file
//! grows past the copy again; another command's `tag create` / `sync adopt-tag` appends (a different ref, under the container locks and no
//! active lock) -- seen as a tail.
//!
//! **Residual, stated** (as S4's, `kept_file.rs`): a truncate below the copy's extent and a regrowth that re-creates our last record byte for
//! byte at its offset after rewriting the prefix is not seen; the append protocol does not produce it.
//!
//! **Platform.** Linux and macOS (the identity is `(device, inode)`); on Windows nothing is kept and every call reads the whole slot.

use std::sync::Arc;

use prikk_error::{PrikkError, Result};

use super::RefPointerSummary;
use super::pointer_index::{
    PointerIndexEntry, PointerIndexRecordOutcome, PointerIndexReplay, decode_pointer_index_records,
    empty_pointer_index_replay, live_pointer_index_slot,
};
use crate::foundation::fsutil::{FileIdentity, read_file_if_exists};
use crate::foundation::kept_file::{self, VerifiedBytes};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};

/// What a hold keeps of the pointer index.
struct KeptPointerIndex {
    slot: ContainerSlot,
    replay: Arc<PointerIndexReplay>,
    known_length: u64,
    identity: FileIdentity,
    /// The last record the replay decoded, as the file holds it (empty for an empty file).
    canary: Vec<u8>,
}

/// The pointer index's decoded replay, shared. The same value `replay_pointer_index` returns -- a whole read -- unless a hold is open and has
/// kept a decode of the same slot, which is then brought up to date by its tail. The hot paths of publication (`lookup_ref_pointer`,
/// `list_ref_pointers`, the precondition) read it; every other reader keeps calling `replay_pointer_index`.
pub(in crate::refs) fn replay_pointer_index_shared(
    layout: &RepositoryLayout,
) -> Result<Arc<PointerIndexReplay>> {
    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("pointer-index-replay");
    let (slot, relative) = live_pointer_index_slot(layout)?;
    let root = layout.repository_mutation_root();
    if let Some(shared) = refresh(layout, slot, &relative)? {
        return Ok(shared);
    }
    let before = if root.hold_is_open() {
        Some(kept_file::stat_file(root, &relative)?)
    } else {
        None
    };
    let Some(bytes) = read_file_if_exists(root, &relative)? else {
        return Ok(Arc::new(empty_pointer_index_replay()));
    };
    let replay = Arc::new(decode_pointer_index_records(&bytes)?);
    if let Some(before) = before {
        if is_clean(&replay) && kept_file::stat_file(root, &relative)?.identity == before.identity {
            if let Some(identity) = before.identity {
                keep(
                    layout,
                    slot,
                    &replay,
                    identity,
                    last_record(&bytes, &replay.record_outcomes),
                );
            }
        }
    }
    Ok(replay)
}

/// The last entry per `ref_name_key`: what `list_ref_pointers` reads, and the view `compact_ref_pointer_index` reduces to.
pub(in crate::refs) fn latest_per_key(
    replay: &PointerIndexReplay,
) -> std::collections::BTreeMap<[u8; 32], PointerIndexEntry> {
    replay
        .entries
        .iter()
        .map(|entry| (entry.ref_name_key, entry.clone()))
        .collect()
}

/// A replay a hold may keep: no damaged frame and no trailing partial record.
fn is_clean(replay: &PointerIndexReplay) -> bool {
    !replay.has_item_failure() && replay.trailing_partial_bytes == 0
}

/// The bytes of the last record of `bytes`, given the replay's outcomes (offsets are into `bytes`); empty when there is none.
fn last_record(bytes: &[u8], outcomes: &[PointerIndexRecordOutcome]) -> Vec<u8> {
    outcomes
        .last()
        .and_then(|outcome| bytes.get(outcome.offset..))
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

fn keep(
    layout: &RepositoryLayout,
    slot: ContainerSlot,
    replay: &Arc<PointerIndexReplay>,
    identity: FileIdentity,
    canary: Vec<u8>,
) {
    layout
        .repository_mutation_root()
        .keep_value(Arc::new(KeptPointerIndex {
            slot,
            replay: Arc::clone(replay),
            known_length: replay.tail_offset as u64,
            identity,
            canary,
        }));
}

/// The kept replay of `slot`, brought up to date; `None` (the kept copy is gone) when nothing was kept or it cannot be proven current.
fn refresh(
    layout: &RepositoryLayout,
    slot: ContainerSlot,
    relative: &std::path::Path,
) -> Result<Option<Arc<PointerIndexReplay>>> {
    let root = layout.repository_mutation_root();
    let Some(kept) = root.take_kept_value::<KeptPointerIndex>() else {
        return Ok(None);
    };
    let kept = Arc::try_unwrap(kept).unwrap_or_else(|shared| KeptPointerIndex {
        slot: shared.slot,
        replay: Arc::clone(&shared.replay),
        known_length: shared.known_length,
        identity: shared.identity,
        canary: shared.canary.clone(),
    });
    // A generation change (a compaction or a rebuild switched the live slot) is a full re-read.
    if kept.slot != slot {
        return Ok(None);
    }
    let observed = kept_file::stat_file(root, relative)?;
    if observed.identity != Some(kept.identity) || observed.size < kept.known_length {
        return Ok(None);
    }
    let VerifiedBytes::Tail(tail) =
        kept_file::read_verified_tail(root, relative, kept.known_length, observed, &kept.canary)?
    else {
        return Ok(None);
    };
    let mut replay = kept.replay;
    let mut canary = kept.canary;
    let mut known_length = kept.known_length;
    if !tail.is_empty() {
        let extension = decode_pointer_index_records(&tail)?;
        // Rule 4: never add a damaged or torn tail; the caller's whole read applies the damaged-index rule.
        if !is_clean(&extension) {
            return Ok(None);
        }
        canary = last_record(&tail, &extension.record_outcomes);
        known_length += tail.len() as u64;
        let base = usize::try_from(kept.known_length).unwrap_or(usize::MAX);
        // Copies the replay only if another caller still holds it (`make_mut`).
        let whole = Arc::make_mut(&mut replay);
        whole.entries.extend(extension.entries);
        whole
            .record_outcomes
            .extend(extension.record_outcomes.into_iter().map(|outcome| {
                PointerIndexRecordOutcome {
                    offset: outcome.offset + base,
                    status: outcome.status,
                }
            }));
        whole.tail_offset = usize::try_from(known_length).unwrap_or(usize::MAX);
    }
    layout
        .repository_mutation_root()
        .keep_value(Arc::new(KeptPointerIndex {
            slot,
            replay: Arc::clone(&replay),
            known_length,
            identity: kept.identity,
            canary,
        }));
    Ok(Some(replay))
}

// 0.58.0 S5: Linux and macOS only, like the mechanism (its identity is `(device, inode)`).
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(in crate::refs) mod tests;

/// The newest pointer per ref, by name, from one read of the pointer index (what [`super::RefStore::list_ref_pointers`] returns). `verify` reads the
/// pointer index once (0.59.0 RS2a: after the ref log, before its object snapshots) and every stage takes its refs from that one read.
///
/// # Errors
///
/// The read's own error, or a damaged entry in the index.
pub(crate) fn pointer_summaries_of(
    replay: Result<&PointerIndexReplay>,
) -> Result<Vec<RefPointerSummary>> {
    let replay = replay?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "ref pointer index has a damaged entry; run `prikk doctor --rebuild-pointer-index \
             --plan-only`, then `prikk doctor --rebuild-pointer-index`"
                .to_string(),
        ));
    }
    let latest = latest_per_key(replay);
    let mut summaries: Vec<RefPointerSummary> = latest
        .into_values()
        .map(|entry| RefPointerSummary {
            ref_name: entry.ref_name,
            ref_state_id: entry.ref_state_id,
        })
        .collect();
    summaries.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    Ok(summaries)
}
