//! `prikk doctor --rebuild-commit-index` (0.57.0 step 0c): rebuild the commit-index from the worktree.
//!
//! The index is a cache and holds no history, so this needs no recovery-log entry. It **rebuilds in
//! fact**: every file `commit` would scan is read once, hashed under the kind `commit` would compare
//! it in (the baseline node's kind for a tracked path, [`classify_new`] for a new one), and the
//! resulting index replaces the old one -- including every entry that was wrong. It is built from the
//! same parts `commit` and `prikk diff` use (the folded baseline, the worktree walk with its ignore
//! rules and tracked exception, the existing-content rule), so it indexes exactly the paths `commit`
//! looks up.
//!
//! **A writer, so it takes the lock `commit` takes** (the default active-session lock) before it reads
//! a byte, and a `commit` that runs meanwhile loses cleanly (`lock conflict`) instead of racing it.
//! The same lock's creation stamp is the racy-entry horizon ([`CommitIndex::set_racy_horizon`]): a file
//! written in the same timestamp tick is read here but not entered. A file that changes between its
//! stat and its read is not entered either. `--plan-only` takes no lock, reads nothing and writes nothing:
//! it counts what a real run would read.

use std::collections::BTreeSet;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{BlobKind, NodeKind};

use crate::commit_index::{
    self, CommitIndex, CommitIndexEntry, CommitIndexRebuildReport, file_system_horizon,
};
use crate::foundation::fsutil::stat_file_state_if_exists;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::object_store::ObjectReadSnapshot;
use crate::patch_replay::resolve_folded_worktree_baseline_without_cache_write;
use crate::wal::Wal;

use super::worktree_files::enumerate_worktree_files_naming_refusals;
use super::{
    REGULAR_FILE_MODE, TextCache, classify_new, existing_content_refusal, read_worktree_file_bytes,
    split_baseline,
};

/// Rebuild (or, with `plan_only`, count) the commit-index against `ref_name`'s folded baseline.
pub(crate) fn rebuild_commit_index_for(
    layout: &RepositoryLayout,
    ref_name: &str,
    plan_only: bool,
) -> Result<CommitIndexRebuildReport> {
    // Before anything is read: the lock `commit` takes. A plan takes none and writes nothing.
    let lock = if plan_only {
        None
    } else {
        Some(ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?)
    };
    let entries_before = CommitIndex::load(layout)?.entries().len();

    let object_store = ObjectReadSnapshot::open(layout)?;
    let active_replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
    if active_replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "active WAL has {} trailing partial bytes; run `prikk doctor --repair-wal-tail` before \
             rebuilding the commit-index",
            active_replay.trailing_partial_bytes
        )));
    }
    if active_replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "active WAL has a damaged record; run doctor before rebuilding the commit-index"
                .to_string(),
        ));
    }
    let mut text_cache = TextCache::new();
    let resolved = resolve_folded_worktree_baseline_without_cache_write(
        layout,
        &object_store,
        ref_name,
        &active_replay,
        &mut text_cache,
    )?;
    let (baseline_files, baseline_symlinks) = split_baseline(&resolved.state);
    let tracked_paths: BTreeSet<String> = baseline_files
        .keys()
        .chain(baseline_symlinks.keys())
        .cloned()
        .collect();
    let (worktree, _refused) = enumerate_worktree_files_naming_refusals(layout, &tracked_paths)?;
    let paths_read = worktree.len();
    let Some(lock) = lock else {
        return Ok(CommitIndexRebuildReport {
            entries_before,
            paths_read,
            entries_written: 0,
            left_out: 0,
            written: false,
        });
    };

    let mut index = CommitIndex::default();
    index.set_racy_horizon(file_system_horizon(lock.path()));
    for (path, meta) in &worktree {
        if baseline_symlinks.contains_key(path) {
            continue; // a symlink node has no content to index; `commit` refuses to touch it
        }
        let bytes = read_worktree_file_bytes(layout, path)?;
        let (blob_kind, mode_default) = match baseline_files.get(path) {
            Some(base) => (
                match base.kind {
                    NodeKind::TextFile => BlobKind::Text,
                    NodeKind::BinaryFile => BlobKind::Binary,
                    NodeKind::Symlink => continue,
                },
                base.mode,
            ),
            None => (classify_new(&bytes).0, REGULAR_FILE_MODE),
        };
        if existing_content_refusal(path, blob_kind, &bytes).is_some() {
            continue; // `commit` refuses this content; there is nothing to cache for it
        }
        // The stat the entry will record must still describe the bytes just read.
        let unchanged_since_walk =
            stat_file_state_if_exists(layout.worktree_mutation_root(), Path::new(path))?
                .is_some_and(|now| {
                    now.size == meta.size
                        && now.mtime_secs == meta.mtime_secs
                        && now.mtime_nanos == meta.mtime_nanos
                });
        if !unchanged_since_walk {
            continue;
        }
        index.record(
            path.clone(),
            CommitIndexEntry {
                size: meta.size,
                mtime_secs: meta.mtime_secs,
                mtime_nanos: meta.mtime_nanos,
                mode: meta.mode.unwrap_or(mode_default),
                kind: blob_kind,
                content_hash: commit_index::content_hash(blob_kind, &bytes)?,
            },
        );
    }
    index.save(layout)?;
    drop(lock);
    let entries_written = index.entries().len();
    Ok(CommitIndexRebuildReport {
        entries_before,
        paths_read,
        entries_written,
        left_out: paths_read.saturating_sub(entries_written),
        written: true,
    })
}
