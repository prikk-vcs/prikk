//! What `commit` will do with each live rename declaration — computed once, for both `commit` and
//! `worktree-status` (RFC 147 §2f, from stikk's letter 013).
//!
//! **Why this is shared and not restated.** `worktree-status` answers "what will this commit do?", and
//! a declaration is part of that answer: `prikk mv` records durable intent, and the next `commit`
//! either authors it as a rename, records it as a deletion, drops it, or refuses the whole commit.
//! Before this, status listed declarations without saying which, so a front-end could show "rename"
//! just before a commit refused. The resolution must therefore come from the same function commit
//! obeys, never from a second reading of the same rules.
//!
//! **The batch matters.** Every check below is evaluated against the whole set of live declarations,
//! not one in isolation: a two-node swap lands both sources back in the worktree, and each is the
//! other's destination. Checking a declaration alone would refuse a real swap as a contradiction.

use std::collections::BTreeSet;

use prikk_error::Result;
use prikk_object::{NodeId, NodeKind, ObjectId};

use crate::foundation::fsutil::{EntryKind, inspect_entry, read_file_if_exists};
use crate::foundation::layout::RepositoryLayout;
use crate::ignore::{IgnoreRules, should_skip_discovery};
use crate::path::{RepoPath, join_repo_path_to_root};
use crate::rename_declaration::RenameDeclaration;

/// What the next `commit` will do with one declaration.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationResolution {
    /// Authored as a `RenamePath`. `content_changed`/`mode_changed` say whether the destination also
    /// differs from the source node's baseline, which commit authors as an accompanying `EditText`
    /// (or `ReplaceBinary`) and `ChangePerm`.
    Rename {
        /// The destination's bytes differ from the baseline node's content. `None` when the destination
        /// is not a regular file (a symlink, FIFO or socket): it is never opened, so there is no
        /// difference to report (RFC 147 §2h), and `commit` refuses over that path anyway.
        content_changed: Option<bool>,
        /// The destination's mode differs from the baseline node's mode. `None` exactly when
        /// `content_changed` is.
        mode_changed: Option<bool>,
    },
    /// The destination is gone, so the source is authored as a deletion instead.
    Deletion,
    /// A directory stands at the destination, so the source is authored as a deletion. Reported as
    /// `deletion`, like [`Self::Deletion`] (RFC 147 §2g ruling 4: no new report value in a patch
    /// release); distinguished here only so `commit` can name the real cause.
    DeletionDirectory,
    /// The destination exists on disk but `.prikkignore` excludes it, so the source is authored as a
    /// deletion. Distinguished from [`Self::Deletion`] because the cause, and the fix, differ.
    DeletionIgnored,
    /// The source was never a tracked node, so the declaration is vacuous and dropped.
    NeverTracked,
    /// `commit` refuses the whole commit, with this message.
    Refused(String),
}

impl DeclarationResolution {
    /// The stable name a report uses.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "rename",
            Self::Deletion | Self::DeletionDirectory => "deletion",
            Self::DeletionIgnored => "deletion-ignored",
            Self::NeverTracked => "never-tracked",
            Self::Refused(_) => "refused",
        }
    }

    /// The refusal message, when this resolution is one.
    #[must_use]
    pub fn refusal(&self) -> Option<&str> {
        match self {
            Self::Refused(message) => Some(message.as_str()),
            _ => None,
        }
    }
}

/// One declaration and what commit will do with it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationOutcome {
    /// The declaration's source path.
    pub old_path: String,
    /// The declaration's destination path.
    pub new_path: String,
    /// What the next `commit` will do with it.
    pub resolution: DeclarationResolution,
}

/// The baseline node a declaration's source names, as each caller already holds it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeclaredSource {
    pub(crate) node_id: NodeId,
    pub(crate) kind: NodeKind,
    pub(crate) blob_id: ObjectId,
    pub(crate) mode: u32,
}

/// Resolve every live declaration, in store order.
///
/// `baseline` answers what tracked node a path holds; `rules` and `tracked` are the ignore rules and
/// tracked paths commit's own worktree walk skips by. **Presence is decided here, by one predicate**
/// ([`entry_at`]), never by a caller: RFC 147 §2f let each caller pass its own view, and they disagreed
/// about a directory (§2g).
pub(crate) fn resolve_declarations(
    layout: &RepositoryLayout,
    declarations: &[RenameDeclaration],
    baseline: impl Fn(&str) -> Option<DeclaredSource>,
    rules: &IgnoreRules,
    tracked: &BTreeSet<String>,
) -> Result<Vec<DeclarationOutcome>> {
    let declared_old_paths: BTreeSet<&str> = declarations
        .iter()
        .map(|declaration| declaration.old_path.as_str())
        .collect();
    let declared_new_paths: BTreeSet<&str> = declarations
        .iter()
        .map(|declaration| declaration.new_path.as_str())
        .collect();

    let mut outcomes = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        let old_path = declaration.old_path.as_str();
        let new_path = declaration.new_path.as_str();
        let resolution = resolve_one(
            layout,
            old_path,
            new_path,
            &baseline,
            &|path| entry_at(layout, rules, tracked, path),
            &declared_old_paths,
            &declared_new_paths,
        )?;
        outcomes.push(DeclarationOutcome {
            old_path: declaration.old_path.clone(),
            new_path: declaration.new_path.clone(),
            resolution,
        });
    }
    Ok(outcomes)
}

/// What stands at a declared path, in the terms commit's worktree walk
/// (`node_authoring/worktree_files.rs::walk_dir`) sorts every entry into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclaredEntry {
    /// Excluded by `.prikkignore`, exactly as the walk skips it (`should_skip_discovery`).
    Ignored,
    /// Nothing there.
    Absent,
    /// A directory: the walk descends into it, so it is never a file at this path.
    Directory,
    /// A regular file: the walk records it.
    RegularFile,
    /// A symlink, FIFO, socket or anything else: the walk refuses the whole commit over this path.
    NotAFile,
}

impl DeclaredEntry {
    /// Something the walk sees as an entry at this path — recorded, or refused over.
    fn is_present(self) -> bool {
        matches!(self, Self::RegularFile | Self::NotAFile)
    }
}

/// The one presence predicate (RFC 147 §2g ruling 1). The ignore rules first, as the walk checks them
/// before dispatching on kind; then one **non-following** stat for the kind. Nothing is opened here.
fn entry_at(
    layout: &RepositoryLayout,
    rules: &IgnoreRules,
    tracked: &BTreeSet<String>,
    path: &str,
) -> Result<DeclaredEntry> {
    if should_skip_discovery(rules, tracked, path) {
        return Ok(DeclaredEntry::Ignored);
    }
    Ok(
        match inspect_entry(layout.worktree_mutation_root(), std::path::Path::new(path))? {
            None => DeclaredEntry::Absent,
            Some(EntryKind::Directory) => DeclaredEntry::Directory,
            Some(EntryKind::Regular) => DeclaredEntry::RegularFile,
            Some(EntryKind::Symlink | EntryKind::Other) => DeclaredEntry::NotAFile,
        },
    )
}

fn resolve_one(
    layout: &RepositoryLayout,
    old_path: &str,
    new_path: &str,
    baseline: &impl Fn(&str) -> Option<DeclaredSource>,
    entry: &impl Fn(&str) -> Result<DeclaredEntry>,
    declared_old_paths: &BTreeSet<&str>,
    declared_new_paths: &BTreeSet<&str>,
) -> Result<DeclarationResolution> {
    // A declaration whose source was never a tracked node asserts nothing about history.
    let Some(source) = baseline(old_path) else {
        return Ok(DeclarationResolution::NeverTracked);
    };

    let destination = entry(new_path)?;

    // The worktree contradicts the declaration: the source is back, and no other live declaration
    // claims to have landed there (which is what a two-node swap looks like).
    if entry(old_path)?.is_present() && !declared_new_paths.contains(old_path) {
        return Ok(DeclarationResolution::Refused(contradicted_message(
            old_path,
            new_path,
            destination.is_present(),
        )));
    }

    match destination {
        // §2g ruling 3: decided from the ignore rules, never inferred from "on disk but not seen".
        DeclaredEntry::Ignored => return Ok(DeclarationResolution::DeletionIgnored),
        DeclaredEntry::Absent => return Ok(DeclarationResolution::Deletion),
        // §2g ruling 2: a directory is not a file at this path; commit records the deletion.
        DeclaredEntry::Directory => return Ok(DeclarationResolution::DeletionDirectory),
        DeclaredEntry::RegularFile | DeclaredEntry::NotAFile => {}
    }

    // The destination is another tracked node this commit does not also move: authoring the rename
    // would emit what seal-time validation refuses.
    if let Some(occupant) = baseline(new_path) {
        if occupant.node_id != source.node_id && !declared_old_paths.contains(new_path) {
            return Ok(DeclarationResolution::Refused(occupied_message(
                old_path, new_path,
            )));
        }
    }

    // §2h: a destination that is not a regular file is never opened — a FIFO would block the read
    // forever — and reports no difference, because there is none to measure.
    let (content_changed, mode_changed) = if destination == DeclaredEntry::RegularFile {
        let (content, mode) = destination_differences(layout, new_path, &source)?;
        (Some(content), Some(mode))
    } else {
        (None, None)
    };
    Ok(DeclarationResolution::Rename {
        content_changed,
        mode_changed,
    })
}

/// Whether the destination's content and mode differ from the source node's baseline — the same
/// comparison commit makes to decide whether to author an `EditText`/`ReplaceBinary` beside the
/// rename, and a `ChangePerm`: content by hashing the destination's bytes as the node's own kind
/// (`blob_access`), mode by the recorded bits.
fn destination_differences(
    layout: &RepositoryLayout,
    new_path: &str,
    source: &DeclaredSource,
) -> Result<(bool, bool)> {
    let repo_path = RepoPath::parse(new_path)?;
    let on_disk = join_repo_path_to_root(&repo_path, layout.root());
    // Commit's own anchored read: opened non-blocking and without following a link, and refused unless
    // it is still a regular file — so a destination swapped for a FIFO between the classification above
    // and this read cannot block it either.
    let Some(bytes) = read_file_if_exists(
        layout.worktree_mutation_root(),
        std::path::Path::new(new_path),
    )?
    else {
        // Gone since it was classified: nothing to compare.
        return Ok((false, false));
    };
    let content_changed = match source.kind {
        NodeKind::TextFile | NodeKind::BinaryFile => {
            !crate::blob_access::bytes_match_blob_id(&bytes, source.blob_id, source.kind)?
        }
        // A symlink node has no file-content blob; commit refuses symlink authoring outright.
        NodeKind::Symlink => false,
    };
    let mode_changed = observed_mode(&on_disk).is_some_and(|mode| mode != source.mode & 0o7777);
    Ok((content_changed, mode_changed))
}

#[cfg(unix)]
fn observed_mode(path: &std::path::Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(std::fs::symlink_metadata(path).ok()?.permissions().mode() & 0o7777)
}

/// No observable POSIX mode: report no mode change rather than a synthetic one (DC-87 §3.3/§4.3).
#[cfg(not(unix))]
fn observed_mode(_path: &std::path::Path) -> Option<u32> {
    None
}

/// The source is back in the worktree. Both commands named here were measured to work in this state
/// (RFC 147 §2f ruling 2); a shell move back is not one of them, and is why the old advice looped.
fn contradicted_message(old_path: &str, new_path: &str, destination_present: bool) -> String {
    if destination_present {
        format!(
            "{old_path} -> {new_path}: both paths exist in the worktree, so which one is the tracked \
             node is not prikk's to guess. Set one copy aside first -- `prikk mv` refuses while both \
             are there: delete {old_path} and commit to author the rename, or delete {new_path} and \
             run `prikk mv {new_path} {old_path}` to drop the declaration"
        )
    } else {
        format!(
            "{old_path} -> {new_path}: the source is present in the worktree again, so the declared \
             move is not what the worktree holds. Run `prikk mv {new_path} {old_path}` to drop the \
             declaration, or `prikk mv {old_path} {new_path}` to make the move again"
        )
    }
}

/// The destination is another tracked node. Measured: `prikk mv <new> <old>` drops the declaration and
/// lets the commit through; restoring the occupied node's file does not.
fn occupied_message(old_path: &str, new_path: &str) -> String {
    format!(
        "{old_path} -> {new_path}: the destination is already occupied by a different tracked node \
         that this commit does not also move or delete. Run `prikk mv {new_path} {old_path}` to drop \
         the declaration -- {old_path} keeps its content, and {new_path} stays deleted in the \
         worktree, so the commit authors that deletion"
    )
}
