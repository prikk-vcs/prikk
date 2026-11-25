//! The ref log a hold keeps (0.58.0 S5, the third user of S4's mechanism; the pointer index is `kept_pointer_index.rs`).
//!
//! `publish_body` and the publication precondition decode the whole ref log three times per `sync seal` claim (S5a, `probe-groups-250.txt`):
//! the precondition twice, `classify_state`'s `replay_log` once. While a command holds a lock, [`load_ref_log_view`] keeps what the log says on the
//! repository's `MutationRoot` (the hold's slot, S4's) and, at the next use, reads only what was appended. **Outside a hold nothing is kept** and
//! every call is the whole read and decode it has always been.
//!
//! **Never stale** (S4's rule). One `stat` of the log (device, inode, size): the same file at the known size is used as is; a longer one is read
//! from where the copy ends, together with the copy's last record (the canary), by `kept_file::read_verified_tail`; another inode, a shorter file
//! or a canary that differs is read whole. A tail with a damaged frame or a trailing partial record is never added: the copy is dropped and the
//! call takes today's path, with today's refusals and recovery (every `PointerLeading`/`Complete`/partial-tail case of a publication runs on a
//! whole read).
//!
//! **The ref log has no slots.** `log-b.container` exists (empty, made by `init`) but nothing opens it, and there is no generation log for it
//! (`ref_log_container_slot_path` is spelled with `B` only in the recovery lookup and in tests): the file is `log-a`, identified like the
//! object index. If the log ever switches slots, its resolver joins the key as the pointer index's does.
//!
//! **Who else rewrites it** (S5a §3): the publication's own partial-tail repair (`truncate_incomplete_tail`), `doctor --repair-tails` and
//! `archive import`'s cancel truncate in place -- seen by the size, or by the canary if the file grows past the copy again; `doctor
//! --rebuild-pointer-index` reads it and writes the pointer index; another command's `tag create` / `sync adopt-tag` appends (a different ref,
//! under the container locks and no active lock) -- seen as a tail.
//!
//! **Residual, stated** (as S4's, `kept_file.rs`): a truncate below the copy's extent and a regrowth that re-creates our last record byte for
//! byte at its offset after rewriting the prefix is not seen; the append protocol does not produce it.
//!
//! **What is kept (0.59.0 P5a2).** Not the decoded replay (about nine times the file in memory, and walked per use) but a per-ref **summary**
//! (`ref_log_summary.rs`): for every ref the newest record's ids and envelope, the record count and whether the chain is sound to the tip, and
//! whether any record carried `created_at != 0`. It is built from a whole decode of a clean log and extended by the appended tail only, so a use
//! under a hold reads the canary and what was appended, and decodes only that. A use that needs the records themselves, or meets any doubt (see
//! `ref_log_summary::Fallback`, each counted), takes the whole read it has always taken.
//!
//! **Platform.** Linux and macOS (the identity is `(device, inode)`); on Windows nothing is kept and every call reads the whole log.

use std::sync::Arc;

use prikk_error::Result;

use super::container::{
    RefContainerRecordOutcome, RefContainerRecordStatus, RefContainerReplay,
    decode_ref_container_records,
};
use super::ref_log_summary::{Fallback, RefLogSummary, count_fallback, enabled};
use crate::foundation::fsutil::{FileIdentity, read_file_if_exists};
use crate::foundation::kept_file::{self, VerifiedBytes};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};

/// What a hold keeps of the ref log.
struct KeptRefLog {
    summary: Arc<RefLogSummary>,
    known_length: u64,
    identity: FileIdentity,
    /// The last record the summary covers, as the file holds it (empty for an empty file).
    canary: Vec<u8>,
}

/// The ref log read whole: its decode and its bytes.
pub(in crate::refs) struct LoadedRefLog {
    pub(in crate::refs) replay: RefContainerReplay,
    pub(in crate::refs) bytes: Vec<u8>,
}

/// The ref log as one use sees it: the kept summary, brought up to date, or the whole log read now.
pub(in crate::refs) enum RefLogView {
    /// A hold's kept summary of a **clean** log (every frame evaluated, no trailing partial record), current as of this call.
    Summary(Arc<RefLogSummary>),
    /// The whole log, read and decoded now (and, under a hold, a clean one has been summarised and kept for the next use).
    Whole(LoadedRefLog),
}

/// The ref log container, read and decoded in full, never kept. `None` when the file does not exist.
pub(in crate::refs) fn load_ref_log_whole(
    layout: &RepositoryLayout,
) -> Result<Option<LoadedRefLog>> {
    // RFC 165 R2: the `ref-log-replay` open finding (`whole_read_guard.rs`'s `SCOPES` table) -- one read per precondition check or
    // classification, not one per ref.
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let relative =
        layout.repository_relative(&layout.ref_log_container_slot_path(ContainerSlot::A))?;
    let root = layout.repository_mutation_root();
    let Some(bytes) = read_file_if_exists(root, &relative)? else {
        return Ok(None);
    };
    let replay = decode_ref_container_records(&bytes)?;
    Ok(Some(LoadedRefLog { replay, bytes }))
}

/// The ref log as the next use needs it: under a hold that kept a summary of the same file, that summary brought up to date by the appended
/// tail; otherwise the whole read (summarised and kept when a hold is open and the log is clean). `None` when the file does not exist.
pub(in crate::refs) fn load_ref_log_view(layout: &RepositoryLayout) -> Result<Option<RefLogView>> {
    let relative =
        layout.repository_relative(&layout.ref_log_container_slot_path(ContainerSlot::A))?;
    let root = layout.repository_mutation_root();
    if enabled() {
        if let Some(summary) = refresh(layout, &relative)? {
            return Ok(Some(RefLogView::Summary(summary)));
        }
    }
    let before = if root.hold_is_open() {
        Some(kept_file::stat_file(root, &relative)?)
    } else {
        None
    };
    let Some(loaded) = load_ref_log_whole(layout)? else {
        return Ok(None);
    };
    if let Some(before) = before {
        if enabled()
            && is_clean(&loaded.replay)
            && kept_file::stat_file(root, &relative)?.identity == before.identity
        {
            if let Some(identity) = before.identity {
                let mut summary = RefLogSummary::default();
                if summary.extend(&loaded.replay.records).is_ok() {
                    keep(
                        layout,
                        Arc::new(summary),
                        identity,
                        loaded.bytes.len() as u64,
                        last_record(&loaded.bytes, &loaded.replay.record_outcomes),
                    );
                } else {
                    count_fallback(Fallback::PayloadUndecodable);
                }
            }
        }
    }
    Ok(Some(RefLogView::Whole(loaded)))
}

/// A replay a hold may keep a summary of: every frame evaluated and no trailing partial record.
fn is_clean(replay: &RefContainerReplay) -> bool {
    replay.trailing_partial_bytes == 0
        && replay
            .record_outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, RefContainerRecordStatus::Evaluated))
}

/// The bytes of the last record of `bytes`, given the replay's outcomes (offsets are into `bytes`); empty when there is none.
fn last_record(bytes: &[u8], outcomes: &[RefContainerRecordOutcome]) -> Vec<u8> {
    outcomes
        .last()
        .and_then(|outcome| bytes.get(outcome.offset..))
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

fn keep(
    layout: &RepositoryLayout,
    summary: Arc<RefLogSummary>,
    identity: FileIdentity,
    known_length: u64,
    canary: Vec<u8>,
) {
    layout
        .repository_mutation_root()
        .keep_value(Arc::new(KeptRefLog {
            summary,
            known_length,
            identity,
            canary,
        }));
}

/// The kept summary, brought up to date; `None` (the kept copy is gone) when nothing was kept or it cannot be proven current. **Every `None` is
/// a doubt, counted by its reason** ([`Fallback`]): the caller reads the whole log, whose refusals are the ones every caller has always printed.
fn refresh(
    layout: &RepositoryLayout,
    relative: &std::path::Path,
) -> Result<Option<Arc<RefLogSummary>>> {
    let root = layout.repository_mutation_root();
    let Some(kept) = root.take_kept_value::<KeptRefLog>() else {
        count_fallback(Fallback::NothingKept);
        return Ok(None);
    };
    let kept = Arc::try_unwrap(kept).unwrap_or_else(|shared| KeptRefLog {
        summary: Arc::clone(&shared.summary),
        known_length: shared.known_length,
        identity: shared.identity,
        canary: shared.canary.clone(),
    });
    let observed = kept_file::stat_file(root, relative)?;
    if observed.identity != Some(kept.identity) || observed.size < kept.known_length {
        count_fallback(Fallback::FileChanged);
        return Ok(None);
    }
    let VerifiedBytes::Tail(tail) =
        kept_file::read_verified_tail(root, relative, kept.known_length, observed, &kept.canary)?
    else {
        count_fallback(Fallback::CanaryDiffers);
        return Ok(None);
    };
    let mut summary = kept.summary;
    let mut canary = kept.canary;
    let mut known_length = kept.known_length;
    if !tail.is_empty() {
        let extension = decode_ref_container_records(&tail)?;
        // Never add a damaged or torn tail; the caller's whole read applies the damaged-log rules.
        if !is_clean(&extension) {
            count_fallback(Fallback::TailNotClean);
            return Ok(None);
        }
        canary = last_record(&tail, &extension.record_outcomes);
        known_length += tail.len() as u64;
        // Copies the summary only if another caller still holds it (`make_mut`).
        if Arc::make_mut(&mut summary)
            .extend(&extension.records)
            .is_err()
        {
            count_fallback(Fallback::PayloadUndecodable);
            return Ok(None);
        }
    }
    keep(
        layout,
        Arc::clone(&summary),
        kept.identity,
        known_length,
        canary,
    );
    Ok(Some(summary))
}

// 0.58.0 S5: Linux and macOS only, like the mechanism (its identity is `(device, inode)`).
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;
