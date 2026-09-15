//! RFC 151 §2.3: `prikk branch switch heads/<name>` -- move the worktree and the current-branch
//! pointer from one branch to another, losing nothing.
//!
//! **Refusals, in order, each `Precondition` naming the way out:**
//! 1. the target is not an existing local branch, or is closed;
//! 2. the active WAL holds unsealed work owned by a ref other than the target;
//! 3. the worktree is not clean against the branch being left (`worktree_status`'s own report --
//!    untracked files are not a difference this refuses on; they are never touched);
//! 4. a file the target needs is in the way: an untracked file, or a non-file, at a path the
//!    target writes. Checked for every path before the first write, so a refusal writes nothing.
//!
//! **The transition.** Create or replace every file of the target's replay baseline (the one
//! `checkout --patch-plan` derives), then delete each file of the old baseline the target lacks --
//! **only when its bytes still equal the old baseline's**, which after refusal 3 is every one of
//! them. Untracked files are never read for deletion. The pointer is written **last**.
//!
//! **Crash posture.** Writes are atomic per file, and the whole transition -- writes, deletions and
//! the pointer -- sits inside the worktree-dirty marker, which is cleared **after** the pointer is
//! written. So an interrupted switch leaves no torn file and the marker set (which makes `commit`
//! refuse until the worktree is re-verified), and:
//! - interrupted in the write phase: the pointer names the old branch and every old file the target
//!   lacks is still present;
//! - interrupted in the delete phase or at the pointer write: the pointer names the old branch and
//!   every target file is already in place;
//! - interrupted at the marker clear: the pointer already names the target.
//!
//! **Running the same switch again completes it.** While the marker is set, refusal 3 also accepts a
//! path already holding the target's bytes and a path already absent that the target lacks -- exactly
//! the states an interrupted switch leaves, and states in which switching loses nothing, since those
//! paths end where the target puts them -- and "already on the target" is not a no-op until the
//! marker has been cleared.
//!
//! **An unresolvable pointer** (malformed, or naming a missing branch) passes `from = None`: there is
//! no old baseline, so refusal 3 cannot be asked, no file counts as replaceable and nothing is
//! deleted -- the target's files are written only where absent or already identical. That keeps
//! `doctor`'s route ("run `prikk branch switch`") usable without risking a byte.

use std::collections::BTreeMap;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeKind, ObjectId, ObjectType, RefStatePayload};

use crate::blob_access::ensure_blob_matches_node_kind;
use crate::foundation::fsutil::{
    EntryKind, ensure_directory_required, inspect_entry, read_file_required,
    remove_worktree_file_required, set_regular_file_mode_required, stat_file_state_if_exists,
    write_file_atomically, write_worktree_file_atomically,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::node::node_lifecycle::NodeContent;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::{
    ReplayManifestEntry, SnapshotAnchorFallback, resolve_folded_worktree_baseline_with_own_cache,
};
use crate::refs::{RefStore, validate_local_branch_ref};
use crate::wal::Wal;
use crate::worktree_marker::{clear_worktree_dirty, mark_worktree_dirty, worktree_is_dirty};
use crate::worktree_status::{WorktreeChangeKind, worktree_status};
use crate::{ActiveRefOwnership, active_ref_ownership};

/// What a completed `branch switch` did.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSwitchReport {
    /// The branch left, when the pointer could be resolved; `None` otherwise.
    pub from: Option<String>,
    /// The branch the pointer now names.
    pub to: String,
    /// Files created or replaced with the target's bytes.
    pub written_files: usize,
    /// Target files already present with identical bytes.
    pub unchanged_files: usize,
    /// Files of the old baseline the target lacks, removed.
    pub deleted_files: usize,
    /// `true` when `to` was already the current branch and nothing was done.
    pub already_current: bool,
    /// The replay-verified snapshot the switch could not anchor at, if any (RFC 136 increment 2b). The
    /// switch replayed from genesis instead; the worktree is the same either way.
    pub anchor_fallback: Option<SnapshotAnchorFallback>,
}

/// Switch the worktree and the current branch from `from` to `target`. See the module doc for the
/// refusals, the order of writes and the crash posture.
pub fn switch_branch(
    layout: &RepositoryLayout,
    from: Option<&str>,
    target: &str,
) -> Result<BranchSwitchReport> {
    layout.require_current_format()?;
    let target = validate_local_branch_ref(target)?;
    let _lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    // RFC 136 §10.3b.3: the derivation gate, before any write.
    crate::worktree_marker::ensure_worktree_replay_verified(layout)?;

    // Refusal 1: an existing, open local branch.
    require_open_branch(layout, &target)?;
    if from == Some(target.as_str()) && !worktree_is_dirty(layout)? {
        return Ok(BranchSwitchReport {
            from: Some(target.clone()),
            to: target,
            written_files: 0,
            unchanged_files: 0,
            deleted_files: 0,
            already_current: true,
            anchor_fallback: None,
        });
    }

    // Refusal 2: unsealed work belongs to the target or to nobody.
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let active_replay = wal.replay()?;
    if active_replay.trailing_partial_bytes != 0 || active_replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "the active WAL is damaged or has a partial tail; run `prikk doctor` before switching"
                .to_string(),
        ));
    }
    if !active_replay.records.is_empty() {
        if let ActiveRefOwnership::OwnedByOther(owner) = active_ref_ownership(layout, &target)? {
            return Err(PrikkError::Precondition(format!(
                "the active WAL holds unsealed work for {owner}; seal it (`prikk seal \
                 --allow-no-audit --ref {owner}`) before switching to {target}"
            )));
        }
    }

    // RFC 136 increment 2b: the target's tree may start only at a replay-verified snapshot.
    let (target_replay, anchor_fallback) =
        crate::patch_replay::replay_for_verified_worktree_write(layout, &target)?;
    let target_manifest = target_replay.manifest;
    let target_files: BTreeMap<&str, &ReplayManifestEntry> = target_manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let from_files = match from {
        Some(from) => baseline_files(layout, from, &active_replay)?,
        None => BTreeMap::new(),
    };

    // Refusal 3: clean against the branch being left.
    if let Some(from) = from {
        require_clean(layout, from, &target_files)?;
    }

    // Refusal 4, and the plan: every decision made before the first write.
    let root = layout.worktree_mutation_root();
    let mut writes = Vec::new();
    let mut unchanged = Vec::new();
    let mut deletes = Vec::new();
    let mut in_the_way = Vec::new();
    for entry in &target_manifest.files {
        let path = entry.path.as_str();
        match inspect_entry(root, Path::new(path))? {
            None => writes.push(entry),
            Some(EntryKind::Regular) => {
                let current = read_file_required(root, Path::new(path))?;
                if current == entry.bytes {
                    unchanged.push(entry);
                } else if from_files
                    .get(path)
                    .is_some_and(|(blob, kind)| matches_blob(&current, *blob, *kind))
                {
                    writes.push(entry);
                } else {
                    in_the_way.push(format!(
                        "{path} (a file that is not part of the branch being left)"
                    ));
                }
            }
            Some(_) => in_the_way.push(format!("{path} (not a regular file)")),
        }
    }
    for (path, (blob, kind)) in &from_files {
        if target_files.contains_key(path.as_str()) {
            continue;
        }
        match inspect_entry(root, Path::new(path))? {
            None => {}
            Some(EntryKind::Regular) => {
                let current = read_file_required(root, Path::new(path))?;
                if matches_blob(&current, *blob, *kind) {
                    deletes.push(path.clone());
                } else {
                    in_the_way.push(format!("{path} (changed since the branch being left)"));
                }
            }
            Some(_) => in_the_way.push(format!("{path} (not a regular file)")),
        }
    }
    if !in_the_way.is_empty() {
        return Err(PrikkError::Precondition(format!(
            "refusing to switch to {target}: {} in the way of its files: {}; move them aside and \
             run the switch again (nothing was written)",
            in_the_way.len(),
            in_the_way.join(", ")
        )));
    }

    // The transition: writes, then deletions, then the pointer, and only then the marker -- so an
    // interruption anywhere, the pointer write included, leaves a switch that running it again
    // completes (module doc).
    mark_worktree_dirty(layout)?;
    for entry in &writes {
        let relative = Path::new(entry.path.as_str());
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        ensure_directory_required(root, parent)?;
        write_worktree_file_atomically(root, relative, &entry.bytes)?;
        set_regular_file_mode_required(root, relative, entry.mode)?;
    }
    for entry in &unchanged {
        let relative = Path::new(entry.path.as_str());
        let mode = stat_file_state_if_exists(root, relative)?
            .and_then(|stat| stat.mode)
            .map(|mode| mode & 0o7777);
        if mode != Some(entry.mode & 0o7777) {
            set_regular_file_mode_required(root, relative, entry.mode)?;
        }
    }
    for path in &deletes {
        remove_worktree_file_required(root, Path::new(path))?;
    }
    write_current_branch(layout, &target)?;
    clear_worktree_dirty(layout)?;

    Ok(BranchSwitchReport {
        from: from.map(str::to_string),
        to: target,
        written_files: writes.len(),
        unchanged_files: unchanged.len(),
        deleted_files: deletes.len(),
        already_current: false,
        anchor_fallback,
    })
}

fn require_open_branch(layout: &RepositoryLayout, target: &str) -> Result<()> {
    let Some(ref_state_id) = RefStore::new(layout.clone()).read_current_ref_state_id(target)?
    else {
        return Err(PrikkError::Precondition(format!(
            "{target} does not exist; run `prikk branch create {target}` first"
        )));
    };
    let object_store = ObjectReadSnapshot::open(layout)?;
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "missing RefState object for {target}: {ref_state_id}"
            ))
        })?;
    let state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if state.closed {
        return Err(PrikkError::Precondition(format!(
            "{target} is closed; switch to an open branch (`prikk branch list` shows them)"
        )));
    }
    Ok(())
}

/// The old baseline's files: path -> (blob, kind), from the same folded baseline `worktree-status`
/// and `commit` agree on.
fn baseline_files(
    layout: &RepositoryLayout,
    from: &str,
    active_replay: &crate::wal::WalReplay,
) -> Result<BTreeMap<String, (ObjectId, NodeKind)>> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let resolved = resolve_folded_worktree_baseline_with_own_cache(
        layout,
        &object_store,
        from,
        active_replay,
    )?;
    Ok(resolved
        .state
        .live_nodes()
        .filter_map(|(_, node)| match &node.content {
            NodeContent::File { blob_id, .. } => {
                Some((node.path.as_str().to_string(), (*blob_id, node.kind)))
            }
            _ => None,
        })
        .collect())
}

/// Refusal 3. While the worktree-dirty marker is set, a path already holding the target's bytes, or
/// already absent where the target has no file, is where an interrupted switch left it -- accepted,
/// so running the switch again completes it.
fn require_clean(
    layout: &RepositoryLayout,
    from: &str,
    target_files: &BTreeMap<&str, &ReplayManifestEntry>,
) -> Result<()> {
    let report = worktree_status(layout, from)?;
    let resuming = worktree_is_dirty(layout)?;
    let mut dirty = Vec::new();
    for change in &report.changes {
        let target_entry = target_files.get(change.path.as_str());
        let resumed = resuming
            && match change.kind {
                WorktreeChangeKind::Modified => target_entry.is_some_and(|entry| {
                    std::fs::read(layout.root().join(&change.path))
                        .is_ok_and(|bytes| bytes == entry.bytes)
                }),
                WorktreeChangeKind::Missing => target_entry.is_none(),
                WorktreeChangeKind::Untracked | WorktreeChangeKind::UnsupportedPath => false,
            };
        match change.kind {
            WorktreeChangeKind::Untracked => {}
            _ if resumed => {}
            kind => dirty.push(format!("{} ({})", change.path, kind.as_str())),
        }
    }
    for declaration in &report.declarations {
        dirty.push(format!(
            "rename declaration {} -> {}",
            declaration.old_path, declaration.new_path
        ));
    }
    if dirty.is_empty() {
        return Ok(());
    }
    Err(PrikkError::Precondition(format!(
        "the worktree is not clean against {from}: {}; commit it, or restore those files, before \
         switching (`prikk worktree-status` lists them)",
        dirty.join(", ")
    )))
}

fn matches_blob(bytes: &[u8], blob: ObjectId, kind: NodeKind) -> bool {
    ensure_blob_matches_node_kind(bytes, blob, kind).is_ok()
}

/// The one writer of the pointer besides `init`: `heads/<name>\n`, atomically, after every worktree
/// write has completed.
fn write_current_branch(layout: &RepositoryLayout, branch: &str) -> Result<()> {
    let relative = layout.repository_relative(&layout.current_branch_path())?;
    write_file_atomically(
        layout.repository_mutation_root(),
        &relative,
        format!("{branch}\n").as_bytes(),
    )
}

// Every test builds real repositories through `RepositoryLayout::init` and mutates the worktree,
// which is Linux-only here, like `patch_checkout`'s own tests.
#[cfg(all(test, target_os = "linux"))]
mod tests;
