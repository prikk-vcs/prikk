//! Safe worktree materialization helpers.
//!
//! PR-017 adds an opt-in snapshot materializer for snapshot-backed blocks. It writes only
//! repository-validated snapshot entries, refuses conflicting existing files, refuses symlinked
//! parents/targets, and never removes files.

use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};

use crate::DEFAULT_ACTIVE_NAME;
use crate::checkout::load_snapshot_checkout;
use crate::foundation::fsutil::{
    EntryKind, ensure_directory_required, inspect_entry, read_file_if_exists, read_file_required,
    set_regular_file_mode_required, stat_file_state_if_exists, sync_directory_required,
    write_worktree_file_atomically,
};
use crate::foundation::layout::RepositoryLayout;
use crate::lock::ActiveLock;
use crate::patch_replay::read::{files_to_replay_manifest, replay_state_from_snapshot};
use crate::patch_replay::{ReplayManifest, ReplayManifestEntry};
use crate::path::join_repo_path_to_root;
use crate::worktree_marker::{
    DIRTY_MARKER_ROUTE, clear_worktree_dirty, mark_worktree_dirty, mark_worktree_provisional,
};

/// Result of an opt-in snapshot worktree materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SnapshotMaterializationReport {
    /// Human-readable ref name.
    pub ref_name: String,
    /// Whether the worktree was marked provisional (RFC 136 §10.3b.2). `false` when the snapshot's Block
    /// is in this repository's replay-verified record (§10.3c ruling 2): its content is what replay
    /// gives, so no marker is written.
    pub provisional: bool,
    /// Number of files described by the snapshot manifest.
    pub planned_files: usize,
    /// Number of files written by this invocation.
    pub written_files: usize,
    /// Number of files already present with identical bytes.
    pub unchanged_files: usize,
    /// Total content bytes represented by the snapshot manifest.
    pub total_content_bytes: u64,
    /// Repository-relative paths in materialization order.
    pub paths: Vec<String>,
}

/// Materialize a snapshot-backed checkout into the repository worktree.
///
/// This operation is intentionally conservative:
///
/// - it accepts only blocks with a validated snapshot manifest;
/// - it refuses to overwrite an existing file with different bytes;
/// - it refuses symlinked parents and symlinked target files;
/// - it never removes extra worktree files;
/// - it relies on `RepoPath` validation to keep writes inside the worktree.
pub fn materialize_snapshot_checkout(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<SnapshotMaterializationReport> {
    layout.require_current_format()?;
    // RFC 136 §10.1a: the snapshot is the target block's own state. Each file is written from its
    // Blob with the mode its state entry records, through the same mode-aware materializer patch
    // checkout uses.
    let (plan, files) = load_snapshot_checkout(layout, ref_name)?;
    let block_id = plan.checkout.block_id.ok_or_else(|| {
        PrikkError::Integrity(format!(
            "snapshot checkout plan for {ref_name} names no block"
        ))
    })?;
    let (files, live_nodes) = replay_state_from_snapshot(files);
    let manifest = files_to_replay_manifest(files, &live_nodes)?;
    // RFC 136 §10.3b.2: the worktree about to be written is the block's signed state, not replay's.
    // The active lock is held across the marker and every write, so `verify`'s compare-and-remove of
    // the marker cannot interleave with this append. The marker is durable before the first write; a
    // crash between the two leaves it set, which fails closed.
    let _lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    // Checkout-refusal round §2.1: every conflict refuses here, before the provisional marker and the
    // dirty marker, so a refused materialization writes nothing at all.
    refuse_manifest_conflicts(layout, &manifest)?;
    let provisional = !crate::verified_blocks::load_verified_blocks(layout).contains(&block_id);
    if provisional {
        mark_worktree_provisional(layout, ref_name, block_id)?;
    }
    // RFC 102 Stage 1: dirty before the first possible worktree write, cleared only after every
    // write in this call has durably completed -- see `worktree_marker`'s own doc for why the
    // ordering, not just the primitive, is what closes T12.
    mark_worktree_dirty(layout)?;
    let write_report = materialize_replay_manifest_entries(layout, &manifest)?;
    clear_worktree_dirty(layout)?;
    Ok(SnapshotMaterializationReport {
        ref_name: ref_name.to_string(),
        provisional,
        planned_files: plan.file_count,
        written_files: write_report.written_files,
        unchanged_files: write_report.unchanged_files,
        total_content_bytes: plan.total_content_bytes,
        paths: plan.paths,
    })
}

/// Result of materializing a validated manifest into a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManifestMaterializationReport {
    /// Number of files written by this invocation.
    pub(crate) written_files: usize,
    /// Number of files already present with identical bytes.
    pub(crate) unchanged_files: usize,
}

/// Materialize a mode-aware replay manifest without deleting extra files (DC-73) -- the one
/// materializer for patch checkout and, since RFC 136 §10.1a, snapshot checkout.
pub(crate) fn materialize_replay_manifest_entries(
    layout: &RepositoryLayout,
    manifest: &ReplayManifest,
) -> Result<ManifestMaterializationReport> {
    #[cfg(test)]
    if let Some(change) = BETWEEN_PLAN_AND_WRITE.with(|slot| slot.borrow_mut().take()) {
        change();
    }
    let mut written_files = 0_usize;
    let mut unchanged_files = 0_usize;
    for entry in &manifest.files {
        match materialize_replay_entry(layout, entry)? {
            EntryWriteOutcome::Written => written_files += 1,
            EntryWriteOutcome::Unchanged => unchanged_files += 1,
        }
    }
    Ok(ManifestMaterializationReport {
        written_files,
        unchanged_files,
    })
}

fn materialize_replay_entry(
    layout: &RepositoryLayout,
    entry: &ReplayManifestEntry,
) -> Result<EntryWriteOutcome> {
    let root = layout.root();
    let target = join_repo_path_to_root(&entry.path, root);
    ensure_target_is_inside_root(root, &target)?;
    ensure_parent_directory(layout, entry.path.as_str())?;
    let relative = Path::new(entry.path.as_str());
    if let Some(current) = read_file_if_exists(layout.worktree_mutation_root(), relative)? {
        // The second guard (checkout-refusal round §2.2). Every caller ran `refuse_manifest_conflicts`
        // first, so a difference here means the file changed during the checkout. The dirty marker
        // stays set, correctly: the worktree is partly written (RFC 102).
        if current != entry.bytes {
            return Err(PrikkError::Precondition(format!(
                "{} changed during the checkout, so the worktree is partly written; move that file \
                 aside, then {DIRTY_MARKER_ROUTE}",
                entry.path.as_str()
            )));
        }
        // `stat.mode` is `None` on a platform with no observable POSIX mode (DC-87 §3.3/§4.3), in
        // which case `current_mode` is `None` here too: the comparison below never matches, the
        // skip-optimization never fires, and `set_regular_file_mode_required` always runs — `entry`'s
        // already-decided mode, not this stat, is what ends up on disk either way.
        let current_mode = stat_file_state_if_exists(layout.worktree_mutation_root(), relative)?
            .and_then(|stat| stat.mode)
            .map(|mode| mode & 0o7777);
        // Re-sync the containing directory on both arms: identical bytes here may be an earlier
        // attempt's write whose directory sync failed, and that attempt also never set the mode --
        // so the mode-fixing arm is exactly where an unrepaired rename would otherwise stay
        // undurable (found moving `sync_matrix`'s retry test onto this materializer, RFC 136 1a).
        sync_directory_required(layout.worktree_mutation_root(), relative)?;
        if current_mode == Some(entry.mode & 0o7777) {
            return Ok(EntryWriteOutcome::Unchanged);
        }
        set_regular_file_mode_required(layout.worktree_mutation_root(), relative, entry.mode)?;
        return Ok(EntryWriteOutcome::Written);
    }
    write_worktree_file_atomically(layout.worktree_mutation_root(), relative, &entry.bytes)?;
    set_regular_file_mode_required(layout.worktree_mutation_root(), relative, entry.mode)?;
    Ok(EntryWriteOutcome::Written)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryWriteOutcome {
    Written,
    Unchanged,
}

fn ensure_parent_directory(layout: &RepositoryLayout, repo_path: &str) -> Result<()> {
    let relative = Path::new(repo_path);
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    ensure_directory_required(layout.worktree_mutation_root(), parent)
}

fn ensure_target_is_inside_root(root: &Path, target: &Path) -> Result<()> {
    if !target.starts_with(root) {
        return Err(PrikkError::Integrity(format!(
            "materialization target escaped repository root: {}",
            target.display()
        )));
    }
    Ok(())
}

/// Checkout-refusal round §2.1, the one planner: a pure pass over every manifest entry, applying the
/// checks `materialize_replay_entry` applies, before any marker and any write. Every conflict is
/// named, not the first. A path escaping the root stays `Integrity`: it is not a user state.
///
/// Callers: `--patch-materialize`, `--patch-materialize-delete` (`patch_checkout.rs`) and
/// `--snapshot-materialize` (above). `branch switch` keeps its own refusal 4: its rule differs, since it
/// may replace a file that still matches the branch being left.
pub(crate) fn refuse_manifest_conflicts(
    layout: &RepositoryLayout,
    manifest: &ReplayManifest,
) -> Result<()> {
    let root = layout.root();
    let mut conflicts = Vec::new();
    for entry in &manifest.files {
        let target = join_repo_path_to_root(&entry.path, root);
        ensure_target_is_inside_root(root, &target)?;
        if let Some(conflict) =
            entry_conflict(layout, Path::new(entry.path.as_str()), &entry.bytes)?
        {
            conflicts.push(format!("{} ({conflict})", entry.path.as_str()));
        }
    }
    if conflicts.is_empty() {
        return Ok(());
    }
    Err(PrikkError::Precondition(format!(
        "refusing to materialize: {} path(s) in the way: {}; move them aside, or commit them on \
         their own branch, and run the checkout again (nothing was written)",
        conflicts.len(),
        conflicts.join(", ")
    )))
}

/// Why writing `bytes` at `relative` would refuse, if it would: a parent that is a symlink or not a
/// directory, walked from the top so no read follows a link, or a target that is not a regular file or
/// holds other bytes.
fn entry_conflict(
    layout: &RepositoryLayout,
    relative: &Path,
    bytes: &[u8],
) -> Result<Option<&'static str>> {
    let root = layout.worktree_mutation_root();
    let mut ancestor = PathBuf::new();
    for component in relative.parent().into_iter().flat_map(Path::components) {
        ancestor.push(component);
        match inspect_entry(root, &ancestor)? {
            // Nothing below an absent directory exists; the write creates it.
            None => return Ok(None),
            Some(EntryKind::Directory) => {}
            Some(EntryKind::Symlink) => return Ok(Some("a parent directory is a symlink")),
            Some(_) => return Ok(Some("a parent path is not a directory")),
        }
    }
    match inspect_entry(root, relative)? {
        None => Ok(None),
        Some(EntryKind::Regular) => Ok((read_file_required(root, relative)? != bytes)
            .then_some("an existing file with different content")),
        Some(EntryKind::Symlink) => Ok(Some("a symlink")),
        Some(_) => Ok(Some("not a regular file")),
    }
}

#[cfg(test)]
thread_local! {
    static BETWEEN_PLAN_AND_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam, unreachable from production (checkout-refusal round §3): run `change` once, after the
/// plan and before the first write of the next `materialize_replay_manifest_entries` on this thread.
#[cfg(test)]
pub(crate) fn between_plan_and_write_for_test(change: impl FnOnce() + 'static) {
    BETWEEN_PLAN_AND_WRITE.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}
