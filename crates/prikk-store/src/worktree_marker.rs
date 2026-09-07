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

use prikk_error::Result;

use crate::foundation::fsutil::{
    append_file_required, read_file_if_exists, truncate_file_empty_required,
};
use crate::foundation::layout::RepositoryLayout;

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

/// Return true when the worktree marker is dirty -- a prior materialization call did not complete,
/// and commit-authoring must not infer deletion from worktree absence until the worktree is
/// re-verified against its baseline. A missing marker file (a repository initialized before this
/// marker existed) reads as clean, not dirty -- see the module doc.
pub(crate) fn worktree_is_dirty(layout: &RepositoryLayout) -> Result<bool> {
    let relative = layout.repository_relative(&layout.worktree_unclean_shutdown_marker_path())?;
    let bytes = read_file_if_exists(layout.repository_mutation_root(), &relative)?;
    Ok(bytes.is_some_and(|bytes| !bytes.is_empty()))
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
