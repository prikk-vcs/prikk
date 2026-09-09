//! Read-only worktree status against the replay baseline.
//!
//! RFC 122 (`replay-baseline-handoff-v1.md`) rewired this off the pre-node snapshot-blob baseline
//! `commit` left behind at the patch-replay migration (`patch_replay.rs`): every real repository
//! this CLI can create is authored against node-addressed replay state, never a stored snapshot
//! Blob, so requiring one here (as this module did before) refused on every such repository. This
//! module now shares `commit`'s own baseline derivation
//! (`patch_replay::resolve_folded_worktree_baseline`) rather than reconstructing it a second way —
//! see that function's own doc comment for why a second implementation that merely agrees today is
//! itself the defect class being fixed.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, ObjectId};

use crate::blob_access::ensure_blob_matches_node_kind;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::ignore::{IgnoreRules, should_skip_discovery};
use crate::lifecycle_cache::replay::TextCache;
use crate::node::node_lifecycle::{NodeContent, NodeLifecycleState};
use crate::object_store::ObjectReadSnapshot;
use crate::patch_replay::decode::{
    DecodedOperationKind, DecodedPatchOperation, decode_patch_operations,
};
use crate::patch_replay::resolve_folded_worktree_baseline;
use crate::path::{RepoPath, join_repo_path_to_root};
use crate::rename_declaration::read_rename_declarations;
use crate::wal::Wal;
use crate::{ActiveRefMetadata, RenameDeclaration, read_active_ref_metadata};

/// Read-only worktree status report against the replay baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStatusReport {
    /// Human-readable ref name used as the baseline.
    pub ref_name: String,
    /// Number of tracked files in the baseline.
    pub tracked_files: usize,
    /// Number of tracked files that match the baseline bytes.
    pub unchanged_files: usize,
    /// Worktree changes detected against the baseline.
    pub changes: Vec<WorktreeChange>,
    /// `Some(other_ref)` when the active WAL is non-empty but owned by a ref other than
    /// `ref_name` — real, committed-but-unsealed work, not part of this ref's baseline (RFC 122
    /// `replay-baseline-handoff-v2-amendment.md` §4). Any `Untracked` changes above are reported
    /// relative to `ref_name`'s own baseline regardless — this field adds context, it does not
    /// reclassify them: a queued file for a *different* ref is not part of that other ref's
    /// baseline either, so it correctly still shows as untracked here.
    pub queued_elsewhere: Option<String>,
    /// Live rename declarations (RFC 144 §4o.3): `prikk mv`'s durable intent, not yet consumed by a
    /// commit. A declaration made and forgotten becomes permanent history at the next commit, so it
    /// must be visible in the command whose job is *what will this commit do*.
    pub declarations: Vec<RenameDeclaration>,
}

impl WorktreeStatusReport {
    /// Return true when the worktree has no detected changes.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.changes.is_empty()
    }

    /// Count changes by kind.
    #[must_use]
    pub fn count_kind(&self, kind: WorktreeChangeKind) -> usize {
        self.changes
            .iter()
            .filter(|change| change.kind == kind)
            .count()
    }
}

/// A single worktree change detected by the read-only status scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeChange {
    /// Repository-relative path, when it could be represented safely.
    pub path: String,
    /// Change kind.
    pub kind: WorktreeChangeKind,
    /// Short explanation intended for CLI display.
    pub detail: String,
}

/// Worktree change kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeChangeKind {
    /// A tracked file is missing from the worktree.
    Missing,
    /// A tracked file exists but differs from the baseline bytes.
    Modified,
    /// A worktree file is not present in the baseline.
    Untracked,
    /// A worktree path could not be safely represented as a Prikk repo path.
    UnsupportedPath,
}

impl WorktreeChangeKind {
    /// Stable CLI label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Modified => "modified",
            Self::Untracked => "untracked",
            Self::UnsupportedPath => "unsupported-path",
        }
    }
}

/// Compute read-only worktree status against the replay baseline for `ref_name` — the same
/// baseline `commit` would author against, already-queued (unsealed) patches folded on top
/// (`resolve_folded_worktree_baseline`'s own doc comment explains why folding is the right choice
/// here: it answers "what would the next commit author?", which is the question this command's own
/// caller is actually asking, and it agrees with `commit` by construction rather than by
/// coincidence).
pub fn worktree_status(layout: &RepositoryLayout, ref_name: &str) -> Result<WorktreeStatusReport> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let active_replay = wal.replay()?;
    if active_replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "active WAL has {} trailing partial bytes; run `prikk doctor --repair-wal-tail` \
             before checking worktree status",
            active_replay.trailing_partial_bytes
        )));
    }
    if active_replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "active WAL has a damaged record; run doctor before checking worktree status"
                .to_string(),
        ));
    }
    let mut text_cache = TextCache::new();
    let resolved = resolve_folded_worktree_baseline(
        layout,
        &object_store,
        ref_name,
        &active_replay,
        &mut text_cache,
    )?;

    let mut baseline_paths = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    let mut changes = Vec::new();
    let mut unchanged_files = 0_usize;
    let mut tracked_files = 0_usize;

    for (_, node) in resolved.state.live_nodes() {
        // Symlink nodes carry no file-content blob to compare (`ensure_blob_matches_node_kind`
        // refuses one outright) — no current authoring path creates one anyway (module doc: "symlink
        // authoring fails closed"), and the snapshot-manifest baseline this replaced never carried
        // symlinks either, so this is the same, pre-existing blind spot, not a new one.
        let NodeContent::File { blob_id, .. } = &node.content else {
            continue;
        };
        tracked_files += 1;
        let path_text = node.path.as_str().to_string();
        baseline_paths.insert(path_text.clone());
        seen_paths.insert(path_text.clone());
        let target = join_repo_path_to_root(&node.path, layout.root());
        if !target.exists() {
            changes.push(WorktreeChange {
                path: path_text,
                kind: WorktreeChangeKind::Missing,
                detail: "tracked file is absent from the worktree".to_string(),
            });
            continue;
        }
        let metadata = fs::symlink_metadata(&target)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            changes.push(WorktreeChange {
                path: path_text,
                kind: WorktreeChangeKind::Modified,
                detail: "tracked path is not a regular file".to_string(),
            });
            continue;
        }
        let bytes = fs::read(&target)?;
        if ensure_blob_matches_node_kind(&bytes, *blob_id, node.kind).is_ok() {
            unchanged_files += 1;
        } else {
            changes.push(WorktreeChange {
                path: path_text,
                kind: WorktreeChangeKind::Modified,
                detail: "tracked file bytes differ from the baseline".to_string(),
            });
        }
    }

    // RFC 124: the same derivation `commit`'s own worktree walk consults (`ignore.rs`'s own module
    // doc explains why one shared function, not two independently-written checks). Loaded once per
    // call, from the same `.prikkignore` `commit` reads -- both commands otherwise agree, so
    // disagreeing about which paths are ignored would recreate exactly the defect shape RFC 122
    // fixed for the baseline itself.
    let rules = IgnoreRules::load(layout)?;
    scan_untracked(
        layout.root(),
        layout.root(),
        &baseline_paths,
        &seen_paths,
        &rules,
        &mut changes,
    )?;
    changes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.kind.as_str().cmp(right.kind.as_str()))
    });

    let declarations = read_rename_declarations(layout)?;

    Ok(WorktreeStatusReport {
        ref_name: ref_name.to_string(),
        tracked_files,
        unchanged_files,
        changes,
        queued_elsewhere: resolved.queued_on_other_ref,
        declarations,
    })
}

fn scan_untracked(
    root: &Path,
    current: &Path,
    baseline_paths: &BTreeSet<String>,
    seen_paths: &BTreeSet<String>,
    rules: &IgnoreRules,
    changes: &mut Vec<WorktreeChange>,
) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(err) => {
            return Err(PrikkError::Io {
                kind: None,
                context: err.to_string(),
            });
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if is_prikk_metadata_path(root, &path) {
            continue;
        }
        // RFC 124: an ignored path with no tracked descendant is invisible to this scan -- checked
        // before dispatching on directory/file, so an ignored directory is never descended into at
        // all, the same pruning `commit`'s own walk applies for the same reason (its own module doc
        // in `ignore.rs`). Built through the shared, separator-safe `path_to_repo_string` -- never
        // `path.display()`/`path.to_str()` directly (RFC 124's own re-land: that is exactly the bug
        // this mechanism's first landing had).
        if let Ok(rel) = path_to_repo_string(root, &path) {
            if should_skip_discovery(rules, baseline_paths, &rel) {
                continue;
            }
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            scan_untracked(root, &path, baseline_paths, seen_paths, rules, changes)?;
            continue;
        }
        let repo_path = path_to_repo_string(root, &path);
        match repo_path.and_then(|text| RepoPath::parse(&text).map(|_| text)) {
            Ok(text) => {
                if !baseline_paths.contains(&text) && !seen_paths.contains(&text) {
                    changes.push(WorktreeChange {
                        path: text,
                        kind: WorktreeChangeKind::Untracked,
                        detail: "worktree file is not in the baseline".to_string(),
                    });
                }
            }
            Err(err) => {
                changes.push(WorktreeChange {
                    path: path.display().to_string(),
                    kind: WorktreeChangeKind::UnsupportedPath,
                    detail: format!(
                        "worktree path is not representable as a safe Prikk path: {err}"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn is_prikk_metadata_path(root: &Path, path: &Path) -> bool {
    match path.strip_prefix(root) {
        Ok(relative) => first_component_is_prikk(relative),
        Err(_) => false,
    }
}

fn first_component_is_prikk(relative: &Path) -> bool {
    let Some(first) = relative.components().next() else {
        return false;
    };
    first.as_os_str().to_str() == Some(".prikk")
}

fn path_to_repo_string(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        PrikkError::Integrity(format!(
            "worktree path escaped repository root: {}",
            path.display()
        ))
    })?;
    crate::path::pathbuf_to_slash_string(relative)
}

/// One path a queued operation affects (RFC 140 §3/§4). `CreateFile`, `DeleteNode`, `RenamePath`,
/// and `CreateSymlink` carry their own path(s) directly in the payload and are always `Path` --
/// there is nothing to resolve or fail to resolve. `EditText`, `ChangePerm`, and `ReplaceBinary`
/// are node-addressed (`EditText`'s own doc: "node-addressed, not path-addressed") and are
/// resolved against the folded worktree baseline the queue would author against; `Unresolved` is a
/// real answer for one of those, not a defect -- RFC 140 §4 requires it be reported, never turned
/// into a refusal of the whole enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedPathResolution {
    /// A repository-relative path, read from the payload or resolved against the baseline.
    Path(String),
    /// A node-addressed operation whose node id is not live in the folded baseline -- hex-encoded,
    /// matching the format this project already uses for key material (`prikk_hash::to_hex`).
    Unresolved {
        /// Hex-encoded node id that failed to resolve.
        node_id: String,
    },
}

/// One operation inside a queued patch (RFC 140): a stable, lowercase-hyphenated kind label (the
/// same idiom as [`WorktreeChangeKind::as_str`]) plus the path(s) it affects, in payload order.
/// Every kind reports exactly one path except `rename-path`, which reports two:
/// `[old_path, new_path]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedOperationEntry {
    /// Stable kind label: `create-file`, `delete-node`, `edit-text`, `rename-path`,
    /// `change-perm`, `create-symlink`, or `replace-binary`.
    pub kind: &'static str,
    /// The path(s) this operation affects, in payload order.
    pub paths: Vec<QueuedPathResolution>,
}

/// One queued patch, in queue order (RFC 140).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedPatchEntry {
    /// Signed Patch object id.
    pub patch_id: ObjectId,
    /// This patch's own operations, in their own canonical (`op_seq`) order.
    pub operations: Vec<QueuedOperationEntry>,
}

/// Enumerate the active WAL's queued patches, in queue order, with node-addressed operations
/// resolved against the same folded-baseline derivation `commit` and `worktree-status` already use
/// (`resolve_folded_worktree_baseline`, RFC 122 §3) -- not a second derivation (RFC 140 §3).
///
/// **Called only when enumeration is actually requested** (`status --format json`): an empty
/// active WAL returns an empty list without calling that derivation at all, so the ordinary,
/// argument-less `status` path -- which never reaches this function -- keeps paying nothing new
/// (RFC 140 §5).
///
/// **A queue whose owning ref cannot be determined still enumerates.** If the active-ref metadata
/// is missing or malformed, every node-addressed operation in the queue is reported `Unresolved`
/// rather than failing the whole read -- the same principle §4 states for one bad node id inside
/// an otherwise-good baseline, extended to a queue that has no good baseline to resolve against at
/// all. Path-addressed operations (which need no resolution) are reported normally regardless.
pub fn enumerate_queued_patches(layout: &RepositoryLayout) -> Result<Vec<QueuedPatchEntry>> {
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let active_replay = wal.replay()?;
    if active_replay.records.is_empty() {
        return Ok(Vec::new());
    }

    let resolved_state = match read_active_ref_metadata(layout)? {
        ActiveRefMetadata::Valid(owner) => {
            let object_store = ObjectReadSnapshot::open(layout)?;
            let mut text_cache = TextCache::new();
            let folded = resolve_folded_worktree_baseline(
                layout,
                &object_store,
                &owner,
                &active_replay,
                &mut text_cache,
            )?;
            Some(folded.state)
        }
        ActiveRefMetadata::Missing | ActiveRefMetadata::Invalid(_) => None,
    };

    let mut entries = Vec::with_capacity(active_replay.records.len());
    for record in &active_replay.records {
        let operations = decode_patch_operations(
            &record.envelope.canonical_payload,
            record.envelope.schema_version,
        )?;
        entries.push(QueuedPatchEntry {
            patch_id: record.envelope.object_id(),
            operations: operations
                .iter()
                .map(|operation| queued_operation_entry(operation, resolved_state.as_ref()))
                .collect(),
        });
    }
    Ok(entries)
}

fn queued_operation_entry(
    operation: &DecodedPatchOperation,
    resolved_state: Option<&NodeLifecycleState>,
) -> QueuedOperationEntry {
    match &operation.kind {
        DecodedOperationKind::CreateFile { path, .. } => single_path("create-file", path.clone()),
        DecodedOperationKind::DeleteNode { path, .. } => single_path("delete-node", path.clone()),
        DecodedOperationKind::CreateSymlink { path, .. } => {
            single_path("create-symlink", path.clone())
        }
        DecodedOperationKind::RenamePath {
            old_path, new_path, ..
        } => QueuedOperationEntry {
            kind: "rename-path",
            paths: vec![
                QueuedPathResolution::Path(old_path.clone()),
                QueuedPathResolution::Path(new_path.clone()),
            ],
        },
        DecodedOperationKind::EditText { node_id, .. } => {
            resolved_node_path("edit-text", *node_id, resolved_state)
        }
        DecodedOperationKind::ChangePerm { node_id, .. } => {
            resolved_node_path("change-perm", *node_id, resolved_state)
        }
        DecodedOperationKind::ReplaceBinary { node_id, .. } => {
            resolved_node_path("replace-binary", *node_id, resolved_state)
        }
    }
}

fn single_path(kind: &'static str, path: String) -> QueuedOperationEntry {
    QueuedOperationEntry {
        kind,
        paths: vec![QueuedPathResolution::Path(path)],
    }
}

fn resolved_node_path(
    kind: &'static str,
    node_id: NodeId,
    resolved_state: Option<&NodeLifecycleState>,
) -> QueuedOperationEntry {
    let resolution = resolved_state
        .and_then(|state| state.live_node(&node_id))
        .map(|node| QueuedPathResolution::Path(node.path.as_str().to_string()))
        .unwrap_or_else(|| QueuedPathResolution::Unresolved {
            node_id: prikk_hash::to_hex(node_id.as_bytes()),
        });
    QueuedOperationEntry {
        kind,
        paths: vec![resolution],
    }
}

#[cfg(test)]
mod tests;
