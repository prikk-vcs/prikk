//! A **read-only** view of what `commit` would author, for `prikk diff` (RFC 153 §6.2, §7.2).
//!
//! `diff`'s worktree side must show exactly what `commit` would author, so this module is built from the same
//! parts `author_inner` is, and none of them is written twice: the folded baseline
//! ([`resolve_folded_worktree_baseline_without_cache_write`], the same derivation that never refreshes the cache), the worktree walk with its ignore rules and its tracked exception
//! ([`super::worktree_files`], one walk with a switch for naming a refusal instead of failing on it), the
//! declaration classifier ([`crate::declaration_resolution::resolve_declarations`]), the baseline split, the
//! kind rule for a new file ([`classify_new`]), the existing-content rule
//! ([`existing_content_refusal`]) and the text materialization ([`current_text_for_node`]). What it does **not**
//! do is what makes `commit` a writer: it takes no lock, writes no blob, updates no commit index, refreshes no
//! baseline cache, mints no node id and appends nothing -- and it does not consult the dirty or provisional markers, which gate *history*, so
//! a worktree that `commit` refuses to sign can still be looked at.
//!
//! **What it returns** is two lists of manifest entries the comparison in `diff.rs` already knows how to read:
//! the worktree's side, and (when asked) the baseline's side **for the paths that differ**, so that a bare
//! `diff` of a large tree materializes the text of the files that changed and not of every file -- the same
//! economy `commit` has, and the reason the baseline side is not simply "everything".

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};
use prikk_object::{BlobKind, NodeId, NodeKind, ObjectId};

use crate::commit_index;
use crate::declaration_resolution::{DeclarationResolution, DeclaredSource};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::ignore::IgnoreRules;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::{
    ReplayManifestEntry, resolve_folded_worktree_baseline_without_cache_write,
};
use crate::path::RepoPath;
use crate::rename_declaration::read_rename_declarations;
use crate::wal::Wal;

use super::worktree_files::enumerate_worktree_files_naming_refusals;
use super::{
    BaselineFile, REGULAR_FILE_MODE, TextCache, classify_new, current_text_for_node,
    existing_content_refusal, read_file_blob_bytes_if_present, read_worktree_file_bytes,
    split_baseline,
};

/// Which baseline entries to hand back next to the worktree's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Retain {
    /// The worktree's side is **every** authorable file, and no baseline entries are returned: the caller
    /// compares against a point it replayed itself (`diff --from <point>`).
    Everything,
    /// Only the paths whose content or mode differ from the baseline, on **both** sides: an unchanged file is
    /// read, compared by its content hash and dropped, so its bytes are not kept and its baseline text is never
    /// materialized (a bare `diff`).
    ChangedFromBaseline,
}

/// A worktree entry `commit` refuses, named with the refusal `commit` prints and no content (RFC 147 §3f).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsupportedWorktreePath {
    pub(crate) path: String,
    pub(crate) refusal: String,
}

/// What [`read_worktree_for_diff`] found.
#[non_exhaustive]
pub(crate) struct WorktreeRead {
    /// The worktree's side, sorted by path. A new file carries no node id (`commit` has not minted one); a
    /// tracked file carries its baseline node's, and a declared rename's destination carries its source's.
    pub(crate) worktree: Vec<ReplayManifestEntry>,
    /// The baseline's side for the retained paths, sorted by path (empty for [`Retain::Everything`]).
    pub(crate) baseline: Vec<ReplayManifestEntry>,
    /// Entries `commit` would refuse, sorted by path.
    pub(crate) unsupported: Vec<UnsupportedWorktreePath>,
    /// How many already-queued (unsealed) patches were folded into the baseline: `commit`'s baseline is the
    /// sealed tip **plus** its queue, and a report that hid that would contradict the commit it describes.
    pub(crate) queued_patches: usize,
    /// The sealed tip the baseline starts from, or `None` for an unpublished branch (an empty baseline).
    pub(crate) tip_block_id: Option<ObjectId>,
}

fn blob_kind_of(kind: NodeKind) -> Option<BlobKind> {
    match kind {
        NodeKind::TextFile => Some(BlobKind::Text),
        NodeKind::BinaryFile => Some(BlobKind::Binary),
        NodeKind::Symlink => None,
    }
}

fn unsupported_symlink(path: &str) -> PrikkError {
    PrikkError::UnsupportedObjectType(format!(
        "{path} is a symlink node in the baseline; `prikk diff` does not support symlinks (RFC 153 §7.4)"
    ))
}

/// One manifest entry. `blob_id` is the content hash of a **binary** file's bytes -- the id `commit` would
/// give the blob -- and `None` for text, as `entry_of` expects.
fn manifest_entry(
    node_id: Option<NodeId>,
    path: &str,
    bytes: Vec<u8>,
    mode: u32,
    kind: NodeKind,
) -> Result<ReplayManifestEntry> {
    let blob_id = match kind {
        NodeKind::BinaryFile => Some(commit_index::content_hash(BlobKind::Binary, &bytes)?),
        NodeKind::TextFile | NodeKind::Symlink => None,
    };
    Ok(ReplayManifestEntry {
        node_id,
        path: RepoPath::parse(path)?,
        bytes,
        mode,
        kind: Some(kind),
        blob_id,
    })
}

/// A baseline file as a manifest entry, its content materialized the way `commit` materializes it. A blob that
/// is missing fails the whole call (RFC 153 §6a A), as it does for `tree` and `cat`.
fn baseline_entry(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    base: &BaselineFile,
    path: &str,
    lineage: (Option<ObjectId>, Option<ObjectId>),
    text_cache: &TextCache,
) -> Result<ReplayManifestEntry> {
    let bytes = match base.kind {
        NodeKind::TextFile => current_text_for_node(
            layout,
            object_store,
            base,
            path,
            lineage.0,
            lineage.1,
            text_cache,
        )?,
        NodeKind::BinaryFile => read_file_blob_bytes_if_present(object_store, base.blob_id)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "{path}: the baseline's binary blob {} is missing from the object store",
                    base.blob_id
                ))
            })?,
        NodeKind::Symlink => return Err(unsupported_symlink(path)),
    };
    let blob_id = match base.kind {
        NodeKind::BinaryFile => Some(base.blob_id),
        NodeKind::TextFile | NodeKind::Symlink => None,
    };
    Ok(ReplayManifestEntry {
        node_id: Some(base.node_id),
        path: RepoPath::parse(path)?,
        bytes,
        mode: base.mode,
        kind: Some(base.kind),
        blob_id,
    })
}

/// Read the worktree the way `commit` would author it, against `ref_name`'s folded baseline. Read-only.
pub(crate) fn read_worktree_for_diff(
    layout: &RepositoryLayout,
    ref_name: &str,
    retain: Retain,
) -> Result<WorktreeRead> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let active_replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
    if active_replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "active WAL has {} trailing partial bytes; run `prikk doctor --repair-wal-tail` before diffing \
             the worktree",
            active_replay.trailing_partial_bytes
        )));
    }
    if active_replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "active WAL has a damaged record; run doctor before diffing the worktree".to_string(),
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
    let queued_patches = if resolved.queued_on_other_ref.is_none() {
        active_replay.records.len()
    } else {
        0
    };
    let lineage = match resolved.lineage {
        Some((block, horizon)) => (Some(block), Some(horizon)),
        None => (None, None),
    };
    let tip_block_id = lineage.0;

    let (baseline_files, baseline_symlinks) = split_baseline(&resolved.state);
    // A symlink node in the baseline is an operation replay does not apply (RFC 153 §7.4): the call fails.
    if let Some(path) = baseline_symlinks.keys().next() {
        return Err(unsupported_symlink(path));
    }
    let tracked_paths: BTreeSet<String> = baseline_files.keys().cloned().collect();

    let (worktree, refused) = enumerate_worktree_files_naming_refusals(layout, &tracked_paths)?;
    let mut unsupported: Vec<UnsupportedWorktreePath> = refused
        .into_iter()
        .map(|entry| UnsupportedWorktreePath {
            path: entry.path,
            refusal: entry.refusal,
        })
        .collect();
    let refused_paths: BTreeSet<String> = unsupported.iter().map(|u| u.path.clone()).collect();

    // Declarations: the one classifier `commit` obeys. A refused declaration refuses the commit whole, so it
    // fails this call with the same words -- a diff of a commit that cannot happen has no honest answer.
    let rules = IgnoreRules::load(layout)?;
    let declarations = read_rename_declarations(layout)?;
    let outcomes = crate::declaration_resolution::resolve_declarations(
        layout,
        &declarations,
        |path| {
            baseline_files.get(path).map(|base| DeclaredSource {
                node_id: base.node_id,
                kind: base.kind,
                blob_id: base.blob_id,
                mode: base.mode,
            })
        },
        &rules,
        &tracked_paths,
    )?;
    let mut renamed_away: BTreeSet<String> = BTreeSet::new();
    let mut confirmed_renames: Vec<(String, String)> = Vec::new();
    for outcome in &outcomes {
        match &outcome.resolution {
            DeclarationResolution::Refused(message) => {
                return Err(PrikkError::Precondition(message.clone()));
            }
            DeclarationResolution::Rename { .. } => {
                renamed_away.insert(outcome.old_path.clone());
                confirmed_renames.push((outcome.old_path.clone(), outcome.new_path.clone()));
            }
            // Never tracked, or destination deleted / a directory / ignored: `commit` authors no rename, so
            // this shows the paths as they are.
            _ => {}
        }
    }
    let rename_targets: BTreeSet<String> = confirmed_renames
        .iter()
        .map(|(_, new_path)| new_path.clone())
        .collect();

    let keep_baseline = retain == Retain::ChangedFromBaseline;
    let mut worktree_side: Vec<ReplayManifestEntry> = Vec::new();
    let mut baseline_side: Vec<ReplayManifestEntry> = Vec::new();
    let materialize = |base: &BaselineFile, path: &str| {
        baseline_entry(layout, &object_store, base, path, lineage, &text_cache)
    };

    for (path, meta) in &worktree {
        if rename_targets.contains(path) {
            continue; // handled with its source below
        }
        let bytes = read_worktree_file_bytes(layout, path)?;
        match baseline_files.get(path) {
            Some(base) => {
                let Some(blob_kind) = blob_kind_of(base.kind) else {
                    return Err(unsupported_symlink(path));
                };
                if let Some(refusal) = existing_content_refusal(path, blob_kind, &bytes) {
                    unsupported.push(UnsupportedWorktreePath {
                        path: path.clone(),
                        refusal: PrikkError::from(refusal).to_string(),
                    });
                    continue;
                }
                // The mode `commit` would record: an observed one, else the baseline's carried forward.
                let mode = meta.mode.unwrap_or(base.mode);
                let unchanged = commit_index::content_hash(blob_kind, &bytes)? == base.blob_id
                    && mode == base.mode;
                if unchanged && keep_baseline {
                    continue;
                }
                worktree_side.push(manifest_entry(
                    Some(base.node_id),
                    path,
                    bytes,
                    mode,
                    base.kind,
                )?);
                if keep_baseline {
                    baseline_side.push(materialize(base, path)?);
                }
            }
            None => {
                let (_, kind) = classify_new(&bytes);
                let mode = meta.mode.unwrap_or(REGULAR_FILE_MODE);
                worktree_side.push(manifest_entry(None, path, bytes, mode, kind)?);
            }
        }
    }

    // A declared rename: the destination is the source's node, with whatever content and mode it now has.
    for (old_path, new_path) in &confirmed_renames {
        let (Some(base), Some(meta)) = (baseline_files.get(old_path), worktree.get(new_path))
        else {
            return Err(PrikkError::Integrity(format!(
                "{old_path} -> {new_path}: a confirmed rename lost its source or destination between \
                 resolution and reading"
            )));
        };
        if keep_baseline {
            baseline_side.push(materialize(base, old_path)?);
        }
        let Some(blob_kind) = blob_kind_of(base.kind) else {
            return Err(unsupported_symlink(old_path));
        };
        let bytes = read_worktree_file_bytes(layout, new_path)?;
        if let Some(refusal) = existing_content_refusal(new_path, blob_kind, &bytes) {
            unsupported.push(UnsupportedWorktreePath {
                path: new_path.clone(),
                refusal: PrikkError::from(refusal).to_string(),
            });
            continue;
        }
        let mode = meta.mode.unwrap_or(base.mode);
        worktree_side.push(manifest_entry(
            Some(base.node_id),
            new_path,
            bytes,
            mode,
            base.kind,
        )?);
    }

    // Deletions: baseline files the worktree no longer has. A path `commit` refuses (a tracked file replaced by
    // a symlink, say) is named once, as unsupported, and is not also a deletion.
    if keep_baseline {
        for (path, base) in &baseline_files {
            if worktree.contains_key(path)
                || renamed_away.contains(path)
                || refused_paths.contains(path)
            {
                continue;
            }
            baseline_side.push(materialize(base, path)?);
        }
    }

    worktree_side.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    baseline_side.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    unsupported.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(WorktreeRead {
        worktree: worktree_side,
        baseline: baseline_side,
        unsupported,
        queued_patches,
        tip_block_id,
    })
}
