//! Unclean-shutdown worktree marker (RFC 102 Stage 1, closing RFC 101's T12).
//!
//! Worktree materialization always creates new file names, and Windows offers no primitive that
//! makes a new name durable. A crash between writing a file's content and that name becoming durable
//! leaves the name gone. The danger is not the lost file -- it is recoverable from sealed history --
//! it is the inference commit-authoring draws from its absence: a baseline path missing from the
//! worktree is signed as a user deletion (`worktree_patch/node_authoring.rs`'s deletion-inference
//! loop), which a materialization crash can trigger for a file the user never touched.
//!
//! This marker converts that silent signed-data-loss risk into a detected condition. It is a fixed
//! name created once at `init` (`RepositoryLayout::init`), never later, so its own creation carries
//! no new-name-durability risk of its own -- `init` is idempotent and retry-safe. Set dirty by
//! appending a sentinel record before a materialization call's first worktree write; cleared by
//! truncating to empty after the call's last worktree write durably completes. **Never written with
//! `atomic_replace`**: RFC 102 §3's correction established that `atomic_replace` renames a temp file
//! onto the destination unconditionally, even when the destination already exists -- a new-name event
//! whose Windows durability is DC-87 §3.4's still-open question, exactly the gap this marker exists to
//! close. `durable_append`/`durable_truncate_to_empty` update the file's existing bytes in place, with
//! no rename at all.
//!
//! **Ordering is what makes this sound, not the primitive alone.** A crash before the dirty-set means
//! no worktree write happened yet -- nothing to falsely infer. A crash during the clear leaves the
//! marker dirty, which is the safe direction: a spurious refusal on the next commit attempt, never a
//! missed dirty state (append/truncate fails toward "still has bytes," never toward a corrupted
//! intermediate value).
//!
//! A repository initialized before this marker existed has no marker file. Treated as clean (never
//! dirty) rather than an error -- this project's standing direction is that migration for existing
//! repositories is not required, and a missing marker is not evidence of an unclean shutdown.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::DEFAULT_ACTIVE_NAME;
use crate::foundation::fsutil::{
    append_file_required, create_new_file_required, read_file_if_exists,
    truncate_file_empty_required,
};
use crate::foundation::layout::RepositoryLayout;
use crate::lock::ActiveLock;

/// Fixed sentinel appended on each dirty-set. Content is never parsed -- only "the file has any
/// bytes" is meaningful -- but a recognizable magic makes the file self-explanatory to inspection.
const DIRTY_SENTINEL: &[u8] = b"PRIKK-WORKTREE-DIRTY\n";

/// Mark the worktree dirty. Must be called before the first worktree write of a materialization
/// call, and its success must be confirmed before that write begins.
pub(crate) fn mark_worktree_dirty(layout: &RepositoryLayout) -> Result<()> {
    let relative = layout.repository_relative(&layout.worktree_unclean_shutdown_marker_path())?;
    append_file_required(layout.repository_mutation_root(), &relative, DIRTY_SENTINEL)
}

/// Clear the worktree-dirty marker. Must be called only after every worktree write of the
/// materialization call it brackets has durably completed.
pub(crate) fn clear_worktree_dirty(layout: &RepositoryLayout) -> Result<()> {
    let relative = layout.repository_relative(&layout.worktree_unclean_shutdown_marker_path())?;
    truncate_file_empty_required(layout.repository_mutation_root(), &relative)
}

/// The way out of a set dirty marker, for every refusal that names it (checkout-refusal round §2.3).
/// Both routes run a whole materialization of the current branch, which clears the marker; both were
/// measured to. The store never reads the current-branch pointer (RFC 151 §2.1), so the branch is named
/// by `prikk status`, not here.
pub(crate) const DIRTY_MARKER_ROUTE: &str = "run `prikk checkout --patch-materialize --ref <the current \
     branch>` or `prikk branch switch <the current branch>` (`prikk status` names the current branch); \
     either one writes the branch's files again and clears this";

/// Whether a checkout or branch switch stopped part-way: the dirty marker (RFC 102) is set. After the
/// checkout-refusal round only a crash or a change during the checkout sets it. For `status`.
pub fn worktree_materialization_interrupted(layout: &RepositoryLayout) -> Result<bool> {
    worktree_is_dirty(layout)
}

/// Return true when the worktree marker is dirty -- a prior materialization call did not complete,
/// and commit-authoring must not infer deletion from worktree absence until the worktree is
/// re-verified against its baseline. A missing marker file (a repository initialized before this
/// marker existed) reads as clean, not dirty -- see the module doc.
pub(crate) fn worktree_is_dirty(layout: &RepositoryLayout) -> Result<bool> {
    let relative = layout.repository_relative(&layout.worktree_unclean_shutdown_marker_path())?;
    let bytes = read_file_if_exists(layout.repository_mutation_root(), &relative)?;
    Ok(bytes.is_some_and(|bytes| !bytes.is_empty()))
}

// ---- The provisional-worktree marker and the derivation gate (RFC 136 §10.3b.1-3) ---------------------
//
// `checkout --snapshot-materialize` writes a worktree from a snapshot, which proves only that it matches
// its block's signed state root, never that replaying history produces it (RFC 136 §6). Until `prikk
// verify` has replayed the repository, that worktree must not become history: `commit` would sign its
// difference from true replay as the user's own patch. The marker records that state; the gate refuses
// every command that would derive history from it; only `verify` clears it.
//
// **Append-only while set.** Each materialization appends one record and nothing rewrites a record, so a
// crash can never leave the marker half-cleared: the file is empty (clear) or holds at least one whole
// or partial record (set). The last whole record names the most recent materialization.

/// The first field of every marker record.
const PROVISIONAL_RECORD_MAGIC: &str = "PRIKK-PROVISIONAL-WORKTREE-v1";

/// What the provisional-worktree marker names: the ref and the snapshot block the worktree was
/// materialized from. `"<unreadable marker>"` in both fields when the marker is set but its last record
/// does not parse; the marker still counts as set.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionalWorktree {
    /// The ref `checkout --snapshot-materialize` read.
    pub ref_name: String,
    /// The snapshot's block id.
    pub block_id: String,
}

/// What [`clear_provisional_marker_if_unchanged`] did.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionalClearOutcome {
    /// The marker still held the bytes `verify` read before verifying, and is now empty.
    Cleared,
    /// A materialization appended to the marker while `verify` ran; the marker is kept.
    ChangedDuringVerify,
    /// The marker was already clear.
    NotSet,
}

/// Set the provisional marker for a worktree about to be written from `block_id`'s snapshot on
/// `ref_name`. Must be called, and succeed, before the first worktree write, under the active lock.
pub(crate) fn mark_worktree_provisional(
    layout: &RepositoryLayout,
    ref_name: &str,
    block_id: ObjectId,
) -> Result<()> {
    let relative = layout.repository_relative(&layout.provisional_worktree_marker_path())?;
    let root = layout.repository_mutation_root();
    // A repository initialized before this marker existed has no file: create it, empty, first.
    if read_file_if_exists(root, &relative)?.is_none() {
        create_new_file_required(root, &relative, &[])?;
    }
    let record = format!("{PROVISIONAL_RECORD_MAGIC} {ref_name} {block_id}\n");
    append_file_required(root, &relative, record.as_bytes())
}

/// The marker's bytes when it is set, `None` when it is clear or absent. `verify` reads this before
/// verifying, to compare against at the end.
pub fn provisional_marker_bytes(layout: &RepositoryLayout) -> Result<Option<Vec<u8>>> {
    let relative = layout.repository_relative(&layout.provisional_worktree_marker_path())?;
    let bytes = read_file_if_exists(layout.repository_mutation_root(), &relative)?;
    Ok(bytes.filter(|bytes| !bytes.is_empty()))
}

/// What the marker names, when it is set.
pub fn provisional_worktree(layout: &RepositoryLayout) -> Result<Option<ProvisionalWorktree>> {
    let Some(bytes) = provisional_marker_bytes(layout)? else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&bytes);
    let named =
        text.lines().rev().find_map(
            |line| match line.split(' ').collect::<Vec<_>>().as_slice() {
                [PROVISIONAL_RECORD_MAGIC, ref_name, block_id] => {
                    Some(((*ref_name).to_string(), (*block_id).to_string()))
                }
                _ => None,
            },
        );
    let (ref_name, block_id) = named.unwrap_or_else(|| {
        (
            "<unreadable marker>".to_string(),
            "<unreadable marker>".to_string(),
        )
    });
    Ok(Some(ProvisionalWorktree { ref_name, block_id }))
}

/// **The derivation gate** (RFC 136 §10.3b.3): the one check every command that turns worktree
/// content into history calls before any write. `Precondition` while the marker is set, naming the ref,
/// the block and the route.
pub fn ensure_worktree_replay_verified(layout: &RepositoryLayout) -> Result<()> {
    match provisional_worktree(layout)? {
        None => Ok(()),
        Some(provisional) => Err(PrikkError::Precondition(format!(
            "the worktree was materialized from the snapshot of Block {} on {} and is not \
             replay-verified; run `prikk verify` to replay the repository and clear this",
            provisional.block_id, provisional.ref_name
        ))),
    }
}

/// Clear the marker if it still holds `observed`, the bytes `verify` read before verifying (RFC 136
/// §10.3b.1). Compare-and-remove under the active lock, which `checkout --snapshot-materialize` holds
/// while it appends: a materialization that ran during `verify` changed the bytes and keeps its marker,
/// and one cannot append between this comparison and the truncation.
pub fn clear_provisional_marker_if_unchanged(
    layout: &RepositoryLayout,
    observed: &[u8],
) -> Result<ProvisionalClearOutcome> {
    let _lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    let Some(current) = provisional_marker_bytes(layout)? else {
        return Ok(ProvisionalClearOutcome::NotSet);
    };
    if current != observed {
        return Ok(ProvisionalClearOutcome::ChangedDuringVerify);
    }
    let relative = layout.repository_relative(&layout.provisional_worktree_marker_path())?;
    truncate_file_empty_required(layout.repository_mutation_root(), &relative)?;
    Ok(ProvisionalClearOutcome::Cleared)
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
