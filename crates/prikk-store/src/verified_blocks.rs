//! The replay-verified block record (RFC 136 increment 2b, §10.3c ruling 2).
//!
//! A monotonic set of Block ids whose `state_merkle_root` this repository confirmed by replay. State
//! read from a snapshot may reach the worktree only through an anchor in this set; any other anchored
//! worktree write replays from genesis.
//!
//! **Rebuildable and never authoritative**, like the DC-64 lifecycle cache. A missing, truncated,
//! mis-checksummed or unreadable file, or one written by any other `prikk-store` version, reads as the
//! empty set, never an error: replay then runs in full. Keying the file to the crate version means a
//! replay fix never inherits a verdict an older replay reached.
//!
//! **Writers** are only the operations that have just confirmed roots by replay: `seal_block` (the
//! parent lineage its derivation verified, plus the new Block, whose root it computed) and `verify`
//! (every Block whose state outcome is `Verified`). Nothing that merely receives or points at blocks
//! writes here.
//!
//! **Concurrency.** Each write is a read-merge-write through an atomic replace. Two writers racing can
//! each read before the other writes; the later replace then drops ids only the earlier one added. That
//! loses entries -- the safe direction -- and never adds one: every id in either writer's set was either
//! read from the record or verified by that writer.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use prikk_object::ObjectId;

use crate::foundation::fsutil::{DirectoryHold, read_file_if_exists, write_file_atomically};
use crate::foundation::kept_file::{self, FileState};
use crate::foundation::layout::RepositoryLayout;

const RECORD_FILE_NAME: &str = "replay-verified-blocks.v1";
pub(crate) const RECORD_MAGIC: &[u8] = b"PRIKK-REPLAY-VERIFIED-BLOCKS-v1\0";
const RECORD_SCHEMA_VERSION: u32 = 1;
/// The replay semantics a recorded verdict was reached under.
const REPLAY_SEMANTICS_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn record_path(layout: &RepositoryLayout) -> PathBuf {
    layout.cache_dir().join(RECORD_FILE_NAME)
}

/// What a hold keeps of the record (0.58.0 S6, the fourth user of S4's mechanism).
///
/// The record is written by atomic replace, so any write -- ours or another process's -- gives the file another identity: `file` is the
/// `(device, inode, size)` the file part of `set` was read at (or written to), and a load that finds another one reads the file again.
/// `set` is the file's ids **and** `pending`, the ids this process confirmed by replay during an open deferral and has not written yet.
struct KeptVerified {
    set: Arc<BTreeSet<ObjectId>>,
    file: FileState,
    pending: BTreeSet<ObjectId>,
    /// Open [`DeferredRecords`] scopes; while any is open, `record_verified_blocks` adds to `set` and `pending` and writes nothing.
    deferring: usize,
}

fn unwrap_kept(kept: Arc<KeptVerified>) -> KeptVerified {
    Arc::try_unwrap(kept).unwrap_or_else(|shared| KeptVerified {
        set: Arc::clone(&shared.set),
        file: shared.file,
        pending: shared.pending.clone(),
        deferring: shared.deferring,
    })
}

fn record_state(layout: &RepositoryLayout) -> Option<FileState> {
    let relative = layout.repository_relative(&record_path(layout)).ok()?;
    kept_file::stat_file(layout.repository_mutation_root(), &relative).ok()
}

/// A file state a kept copy can be compared against: an identity, or an absent file. Where the platform has no identity (Windows) a present
/// file cannot be told from its replacement, so nothing is trusted and every load reads the file.
fn comparable(state: &FileState) -> bool {
    state.identity.is_some() || state.size == 0
}

/// The record as the file holds it: the empty set on any failure.
fn read_record(layout: &RepositoryLayout) -> BTreeSet<ObjectId> {
    load(layout).unwrap_or_default()
}

/// The recorded set, or the empty set on any failure. Under a hold it is the set the hold kept, brought up to date: the file is read again
/// only if its identity or size is not the one the kept set was read at, and the ids this process has confirmed and not yet written (an open
/// [`DeferredRecords`]) are always in it.
pub(crate) fn load_verified_blocks(layout: &RepositoryLayout) -> Arc<BTreeSet<ObjectId>> {
    let root = layout.repository_mutation_root();
    if !root.hold_is_open() {
        return Arc::new(read_record(layout));
    }
    let kept = current(layout, root.take_kept_value::<KeptVerified>());
    let set = Arc::clone(&kept.set);
    root.keep_value(Arc::new(kept));
    set
}

/// The kept state, valid for the file as it is now: the kept one if the file is the one it was read at, else a fresh read joined with the
/// pending ids.
fn current(layout: &RepositoryLayout, kept: Option<Arc<KeptVerified>>) -> KeptVerified {
    let observed = record_state(layout);
    let kept = kept.map(unwrap_kept);
    if let (Some(kept), Some(observed)) = (&kept, &observed) {
        if comparable(observed) && kept.file == *observed {
            return KeptVerified {
                set: Arc::clone(&kept.set),
                file: kept.file,
                pending: kept.pending.clone(),
                deferring: kept.deferring,
            };
        }
    }
    let (pending, deferring) =
        kept.map_or((BTreeSet::new(), 0), |kept| (kept.pending, kept.deferring));
    // The state is taken before the read: a file replaced during it is read again at the next load, never trusted.
    let mut set = read_record(layout);
    set.extend(pending.iter().copied());
    KeptVerified {
        set: Arc::new(set),
        file: observed.unwrap_or(FileState {
            identity: None,
            size: u64::MAX,
        }),
        pending,
        deferring,
    }
}

fn load(layout: &RepositoryLayout) -> Option<BTreeSet<ObjectId>> {
    let relative = layout.repository_relative(&record_path(layout)).ok()?;
    let bytes = read_file_if_exists(layout.repository_mutation_root(), &relative).ok()??;
    decode(&bytes)
}

/// Add `ids`, each of which the caller has just confirmed by replay. Best-effort: a failure to write
/// leaves fewer entries, never an error.
///
/// **Inside a [`DeferredRecords`] scope nothing is written**: the ids join the hold's set (so the batch's later derivations anchor on them) and
/// are written once, when the scope ends. Absence is always safe -- a kill before that leaves the record without these ids, and a later run
/// replays instead of anchoring -- and an id is never added that was not confirmed here.
pub(crate) fn record_verified_blocks(
    layout: &RepositoryLayout,
    ids: impl IntoIterator<Item = ObjectId>,
) {
    let root = layout.repository_mutation_root();
    let ids: Vec<ObjectId> = ids.into_iter().collect();
    if root.hold_is_open() {
        if let Some(kept) = root.take_kept_value::<KeptVerified>() {
            if kept.deferring > 0 {
                let mut kept = unwrap_kept(kept);
                let set = Arc::make_mut(&mut kept.set);
                for id in ids {
                    if set.insert(id) {
                        kept.pending.insert(id);
                    }
                }
                root.keep_value(Arc::new(kept));
                return;
            }
            root.keep_value(kept);
        }
    }
    let mut set: BTreeSet<ObjectId> = (*load_verified_blocks(layout)).clone();
    let before = set.len();
    set.extend(ids);
    if set.len() == before {
        return;
    }
    write_record(layout, set, BTreeSet::new(), 0);
}

/// Write `set` as the record and, under a hold, keep it with the file's new identity (the replace gave it one).
fn write_record(
    layout: &RepositoryLayout,
    set: BTreeSet<ObjectId>,
    pending: BTreeSet<ObjectId>,
    deferring: usize,
) {
    let Ok(relative) = layout.repository_relative(&record_path(layout)) else {
        return;
    };
    let root = layout.repository_mutation_root();
    let written = write_file_atomically(root, &relative, &encode(&set)).is_ok();
    if root.hold_is_open() {
        if let (true, Some(file)) = (written, record_state(layout)) {
            root.keep_value(Arc::new(KeptVerified {
                set: Arc::new(set),
                file,
                pending,
                deferring,
            }));
        }
    }
}

/// While this is alive, `record_verified_blocks` on the same repository writes nothing; its ids are kept in memory (and seen by every
/// `load_verified_blocks`) and written **once**, merged into the record as the file is then, when the last scope ends (0.58.0 S6).
/// `sync seal` opens one for a batch: it used to rewrite the whole record for every sealed Block, reading and writing a file that grows
/// with the claims. A scope also holds a `DirectoryHold`, so the kept set exists for exactly its length. On a platform that keeps nothing
/// (Windows) it does nothing and every record is written when made, as before.
pub(crate) struct DeferredRecords {
    layout: RepositoryLayout,
    hold: Option<DirectoryHold>,
}

pub(crate) fn defer_records(layout: &RepositoryLayout) -> DeferredRecords {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return DeferredRecords {
            layout: layout.clone(),
            hold: None,
        };
    }
    let root = layout.repository_mutation_root();
    let hold = root.hold_directories();
    let mut kept = current(layout, root.take_kept_value::<KeptVerified>());
    kept.deferring += 1;
    root.keep_value(Arc::new(kept));
    DeferredRecords {
        layout: layout.clone(),
        hold: Some(hold),
    }
}

impl Drop for DeferredRecords {
    fn drop(&mut self) {
        if self.hold.is_none() {
            return;
        }
        let root = self.layout.repository_mutation_root();
        let Some(kept) = root.take_kept_value::<KeptVerified>() else {
            return;
        };
        let mut kept = unwrap_kept(kept);
        kept.deferring = kept.deferring.saturating_sub(1);
        if kept.deferring > 0 || kept.pending.is_empty() {
            root.keep_value(Arc::new(kept));
            return;
        }
        // Merged with the record as it is now, not with the set read at the start: the ids another process added since are kept, and the
        // only ids added here are the ones confirmed in this process.
        let mut merged = read_record(&self.layout);
        merged.extend(kept.pending.iter().copied());
        write_record(&self.layout, merged, BTreeSet::new(), 0);
    }
}

pub(crate) fn encode(set: &BTreeSet<ObjectId>) -> Vec<u8> {
    let version = REPLAY_SEMANTICS_VERSION.as_bytes();
    let mut body = Vec::with_capacity(4 + 2 + version.len() + 4 + 32 * set.len());
    body.extend_from_slice(&RECORD_SCHEMA_VERSION.to_be_bytes());
    body.extend_from_slice(
        &u16::try_from(version.len())
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    body.extend_from_slice(version);
    body.extend_from_slice(&u32::try_from(set.len()).unwrap_or(u32::MAX).to_be_bytes());
    for id in set {
        body.extend_from_slice(id.as_bytes());
    }
    let mut out = Vec::with_capacity(RECORD_MAGIC.len() + 32 + body.len());
    out.extend_from_slice(RECORD_MAGIC);
    out.extend_from_slice(&prikk_hash::sha256(&body));
    out.extend_from_slice(&body);
    out
}

pub(crate) fn decode(bytes: &[u8]) -> Option<BTreeSet<ObjectId>> {
    let after_magic = bytes.strip_prefix(RECORD_MAGIC)?;
    let (checksum, body) = after_magic.split_at_checked(32)?;
    if prikk_hash::sha256(body) != checksum {
        return None;
    }
    let (schema, rest) = body.split_at_checked(4)?;
    if u32::from_be_bytes(schema.try_into().ok()?) != RECORD_SCHEMA_VERSION {
        return None;
    }
    let (version_len, rest) = rest.split_at_checked(2)?;
    let version_len = usize::from(u16::from_be_bytes(version_len.try_into().ok()?));
    let (version, rest) = rest.split_at_checked(version_len)?;
    if version != REPLAY_SEMANTICS_VERSION.as_bytes() {
        return None;
    }
    let (count, rest) = rest.split_at_checked(4)?;
    let count = usize::try_from(u32::from_be_bytes(count.try_into().ok()?)).ok()?;
    if rest.len() != count.checked_mul(32)? {
        return None;
    }
    rest.chunks_exact(32)
        .map(|chunk| chunk.try_into().ok().map(ObjectId::from_bytes))
        .collect()
}

#[cfg(test)]
mod tests;
