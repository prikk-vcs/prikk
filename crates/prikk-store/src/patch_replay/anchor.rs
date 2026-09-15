//! Anchored replay for read-only reports (RFC 136 increment 2a: §10.3c ruling 1, §10.3a ruling 5,
//! §10.3b.4).
//!
//! A read-only report may start at the nearest block whose snapshot passes the loader, seeding files
//! and live nodes from it and replaying only the blocks after it. The snapshot supplies **state only**.
//! Every field that is a function of history -- the patch and operation counts, the kinds applied, and
//! the files history deleted -- is computed over the **whole** chain by decoding the anchor and every
//! block before it, never by applying them and never from the snapshot.
//!
//! **Who may anchor.** [`Anchoring`] is an explicit argument to [`replay_chain`], with no default. A
//! worktree write (`checkout --patch-materialize`, `--patch-materialize-delete`, `branch switch`) and
//! rollback preview pass [`Anchoring::Never`] through `replay_supported_patch_chain`; only the
//! read-only reports pass [`Anchoring::ReadOnlyReport`], and a source-scan test pins which files may
//! name it (`every_read_only_anchor_caller_is_a_read_only_report`). State from a snapshot must not
//! reach the worktree before increment 2b's replay-verified anchor exists (§10.3c ruling 2).
//!
//! **A snapshot that fails the loader** is never trusted and never silently skipped: the replay falls
//! back to genesis and returns the finding, which the caller prints on stderr naming `prikk verify`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use prikk_error::Result;
use prikk_object::{NodeId, ObjectId};

use super::apply::ReplayLiveNode;
use super::decode::{
    DecodedDeletePreimage, DecodedOperationKind, applied_operation_kind_label,
    decode_patch_operations, ensure_apply_supported,
};
use super::read::{read_block, read_patch, replay_state_from_snapshot};
use super::{PatchReplayDeletedFile, apply_operation_sequence};
use crate::ObjectReader;
use crate::path::RepoPath;
use crate::snapshot::{SnapshotFile, load_block_snapshot};

/// Whether a replay may start at a snapshot. There is no default: every caller states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchoring<'a> {
    /// Replay every block from genesis. Rollback preview.
    Never,
    /// A read-only report: may start at the nearest snapshot that passes the loader.
    ReadOnlyReport,
    /// A worktree write (RFC 136 §10.3c ruling 2, increment 2b): may start only at the nearest snapshot
    /// whose Block is in this repository's replay-verified record **and** that passes the loader.
    /// A snapshot on an unrecorded Block is skipped without being read.
    VerifiedWorktreeWrite(&'a BTreeSet<ObjectId>),
}

/// A snapshot a read-only report could not anchor at because it failed validation (RFC 136 §10.3b.4).
/// The report replayed from genesis instead, so its output is unchanged.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAnchorFallback {
    /// The block whose snapshot failed the loader.
    pub block_id: ObjectId,
    /// The loader's finding.
    pub finding: String,
}

impl fmt::Display for SnapshotAnchorFallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "warning: the snapshot of Block {} failed validation ({}); this report replayed the whole \
             history instead -- run `prikk verify`",
            self.block_id, self.finding
        )
    }
}

/// The replayed chain: state, the whole chain's history fields, and any fallback.
pub(crate) struct ChainReplay {
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub(crate) live_nodes: BTreeMap<NodeId, ReplayLiveNode>,
    pub(crate) deleted_files: BTreeMap<String, PatchReplayDeletedFile>,
    pub(crate) patch_count: usize,
    pub(crate) applied_operation_count: usize,
    pub(crate) applied_operation_kinds: BTreeSet<&'static str>,
    pub(crate) fallback: Option<SnapshotAnchorFallback>,
}

/// Replay `block_ids` (oldest first, as `single_parent_chain` returns them).
pub(crate) fn replay_chain(
    reader: &impl ObjectReader,
    block_ids: &[ObjectId],
    anchoring: Anchoring<'_>,
) -> Result<ChainReplay> {
    let mut chain = ChainReplay {
        files: BTreeMap::new(),
        live_nodes: BTreeMap::new(),
        deleted_files: BTreeMap::new(),
        patch_count: 0,
        applied_operation_count: 0,
        applied_operation_kinds: BTreeSet::new(),
        fallback: None,
    };
    let (anchor, fallback) = match anchoring {
        Anchoring::Never => (None, None),
        Anchoring::ReadOnlyReport => find_anchor(reader, block_ids, None)?,
        Anchoring::VerifiedWorktreeWrite(verified) => {
            find_anchor(reader, block_ids, Some(verified))?
        }
    };
    chain.fallback = fallback;
    let replay_from = match anchor {
        Some((index, snapshot)) => {
            let history = block_ids.get(..=index).unwrap_or_default();
            decode_history(reader, history, &mut chain)?;
            let (files, live_nodes) = replay_state_from_snapshot(snapshot);
            chain.files = files;
            chain.live_nodes = live_nodes;
            index + 1
        }
        None => 0,
    };
    for block_id in block_ids.get(replay_from..).unwrap_or_default() {
        let block = read_block(reader, *block_id)?;
        for patch_id in block.patch_ids {
            let patch = read_patch(reader, patch_id)?;
            let operations =
                decode_patch_operations(&patch.canonical_payload, patch.schema_version)?;
            let (count, kinds) = apply_operation_sequence(
                reader,
                &mut chain.files,
                &mut chain.live_nodes,
                &mut chain.deleted_files,
                operations,
            )?;
            chain.applied_operation_count += count;
            chain.applied_operation_kinds.extend(kinds);
            chain.patch_count += 1;
        }
    }
    Ok(chain)
}

/// The nearest snapshot on the chain that passes the loader, with its index; or, when the nearest
/// snapshot fails the loader, no anchor and the finding.
#[allow(clippy::type_complexity)]
fn find_anchor(
    reader: &impl ObjectReader,
    block_ids: &[ObjectId],
    verified: Option<&BTreeSet<ObjectId>>,
) -> Result<(
    Option<(usize, Vec<SnapshotFile>)>,
    Option<SnapshotAnchorFallback>,
)> {
    #[cfg(test)]
    if ANCHORING_SUPPRESSED.with(std::cell::Cell::get) {
        return Ok((None, None));
    }
    for (index, block_id) in block_ids.iter().enumerate().rev() {
        let block = read_block(reader, *block_id)?;
        if block.snapshot_blob_ref.is_none() {
            continue;
        }
        // A worktree write anchors only at a Block this repository replay-verified (§10.3c ruling 2).
        if verified.is_some_and(|recorded| !recorded.contains(block_id)) {
            continue;
        }
        #[cfg(test)]
        SNAPSHOT_ANCHOR_LOADS.with(|loads| loads.set(loads.get() + 1));
        match load_block_snapshot(reader, *block_id, &block) {
            Ok(Some(snapshot)) => return Ok((Some((index, snapshot)), None)),
            Ok(None) => continue,
            Err(err) => {
                return Ok((
                    None,
                    Some(SnapshotAnchorFallback {
                        block_id: *block_id,
                        finding: err.to_string(),
                    }),
                ));
            }
        }
    }
    Ok((None, None))
}

/// The history fields of `blocks` -- the anchor and everything before it -- by decoding, never by
/// applying: the same counts, kinds and `deleted_files` rules `apply_operation_sequence` follows. A
/// `DeleteNode` of a file records the deleted path; a later `CreateFile` at that path removes it
/// (`apply.rs`); a rename touches neither. An operation replay refuses is refused here too.
fn decode_history(
    reader: &impl ObjectReader,
    blocks: &[ObjectId],
    chain: &mut ChainReplay,
) -> Result<()> {
    for block_id in blocks {
        let block = read_block(reader, *block_id)?;
        for patch_id in block.patch_ids {
            let patch = read_patch(reader, patch_id)?;
            for operation in
                decode_patch_operations(&patch.canonical_payload, patch.schema_version)?
            {
                ensure_apply_supported(&operation)?;
                chain
                    .applied_operation_kinds
                    .insert(applied_operation_kind_label(&operation.kind));
                chain.applied_operation_count += 1;
                match operation.kind {
                    DecodedOperationKind::DeleteNode {
                        path,
                        preimage:
                            DecodedDeletePreimage::File {
                                old_node_kind,
                                old_blob_id,
                                ..
                            },
                        ..
                    } => {
                        let deleted = PatchReplayDeletedFile {
                            path: RepoPath::parse(&path)?,
                            old_blob_id,
                            old_node_kind,
                        };
                        chain.deleted_files.insert(path, deleted);
                    }
                    DecodedOperationKind::CreateFile { path, .. } => {
                        chain.deleted_files.remove(&path);
                    }
                    _ => {}
                }
            }
            chain.patch_count += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static ANCHORING_SUPPRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SNAPSHOT_ANCHOR_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Test-only: run `body` with anchoring suppressed on this thread, so a test can compare every
/// anchored report with the same report replayed from genesis. `cfg(test)`, like
/// `without_checkpoints_for_test`.
#[cfg(test)]
pub(crate) fn without_anchoring_for_test<T>(body: impl FnOnce() -> T) -> T {
    ANCHORING_SUPPRESSED.with(|flag| flag.set(true));
    let result = body();
    ANCHORING_SUPPRESSED.with(|flag| flag.set(false));
    result
}

/// Test-only: how many snapshots this thread's anchor searches have loaded.
#[cfg(test)]
pub(crate) fn snapshot_anchor_loads_for_test() -> usize {
    SNAPSHOT_ANCHOR_LOADS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests;
