//! Root-relative worktree enumeration for node authoring.
//!
//! DC-56: this walk is metadata-only — it lists directories and stats regular files (size, mtime,
//! mode) but never opens or reads their content. `author_inner` consults the commit-index cache
//! against this metadata to decide, per path, whether a content read can be skipped entirely.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use prikk_error::PrikkError;

use super::{
    AuthorError, EXECUTABLE_FILE_MODE, REGULAR_FILE_MODE, RepoPath, RepositoryLayout,
    WorktreeEntryShape, authoring_refusal,
};
use crate::foundation::fsutil::{
    EntryKind, RootFileStat, list_directory, stat_file_state_if_exists,
};
use crate::ignore::{IgnoreRules, should_skip_discovery};
use crate::path::pathbuf_to_slash_string;

/// A worktree regular file's metadata, gathered without reading its content.
///
/// `mode` is `None` on a platform with no observable POSIX mode (DC-87 §3.3/§4.3) — the caller
/// decides what that means (see `node_authoring.rs`'s existing-node comparison and creation sites),
/// not this type.
pub(super) struct WorktreeFileMeta {
    pub(super) size: u64,
    pub(super) mtime_secs: i64,
    pub(super) mtime_nanos: u32,
    pub(super) mode: Option<u32>,
}

/// `tracked` is every path the baseline already has a node for (files and symlinks together) —
/// RFC 124 §4.4: an ignore rule must never make an already-tracked path disappear from this map, or
/// `node_authoring.rs`'s own deletion-inference loop (`baseline_files` minus this map) would read a
/// still-present, merely-now-ignored file as deleted and author a `DeleteNode` for it.
pub(super) fn enumerate_worktree_files(
    layout: &RepositoryLayout,
    tracked: &BTreeSet<String>,
) -> std::result::Result<BTreeMap<String, WorktreeFileMeta>, AuthorError> {
    let rules = IgnoreRules::load(layout).map_err(AuthorError::Store)?;
    let mut out = BTreeMap::new();
    walk_dir(layout, Path::new(""), &rules, tracked, &mut out)?;
    Ok(out)
}

fn walk_dir(
    layout: &RepositoryLayout,
    dir: &Path,
    rules: &IgnoreRules,
    tracked: &BTreeSet<String>,
    out: &mut BTreeMap<String, WorktreeFileMeta>,
) -> std::result::Result<(), AuthorError> {
    let entries =
        list_directory(layout.worktree_mutation_root(), dir).map_err(AuthorError::Store)?;
    for entry in entries {
        let file_name = entry.name;
        if file_name == ".prikk" {
            continue;
        }
        let path = join_relative(dir, &file_name);
        // RFC 124: checked once per entry, before dispatching on kind, so an ignored directory is
        // skipped without ever being opened -- essential, not only faster, since a real
        // `node_modules/`-shaped directory is typically full of symlinks and other entry kinds this
        // walk would otherwise fail closed on below.
        //
        // Built through the shared, separator-safe `pathbuf_to_slash_string` -- **never**
        // `Path::to_str()`/`Path::to_string_lossy()` directly on a `Path::join`-built path. `join`
        // inserts the platform separator, so on Windows that string is backslash-joined and neither
        // matches a forward-slash ignore rule nor a `tracked` entry (both always `/`-joined) — the
        // exact defect this mechanism's first landing had (reverted at `2235af3`; see the amendment
        // in `rfcs/handoffs/124-worktree-ignore-mechanism/`). A conversion failure here (non-UTF-8)
        // is not swallowed: it falls through to the ordinary dispatch below, where `insert_regular_file`
        // performs the identical conversion and fails closed on it as it always has.
        if let Ok(rel) = pathbuf_to_slash_string(&path) {
            if should_skip_discovery(rules, tracked, &rel) {
                continue;
            }
        }
        if matches!(entry.kind, EntryKind::Directory) {
            walk_dir(layout, &path, rules, tracked, out)?;
            continue;
        }
        // RFC 147 §2e(a): the refusal comes from the one shared classifier, so `worktree-status`
        // reporting a path as refused and `commit` refusing it are the same decision rendered
        // twice, not two rules that happen to agree today. Baseline kind is `None` here: the walk
        // reports what is *on disk*, and a tracked path that is now a symlink refuses on that fact
        // before authoring ever consults the baseline.
        if let Some(refusal) = authoring_refusal(
            &path.to_string_lossy(),
            None,
            WorktreeEntryShape::from_entry_kind(entry.kind),
        ) {
            return Err(refusal);
        }
        insert_regular_file(layout, &path, out)?;
    }
    Ok(())
}

fn insert_regular_file(
    layout: &RepositoryLayout,
    path: &Path,
    out: &mut BTreeMap<String, WorktreeFileMeta>,
) -> std::result::Result<(), AuthorError> {
    // Same shared, separator-safe conversion as the ignore check above -- this call pre-dates RFC
    // 124 and used `path.to_str()` directly (fine on the separator this crate has always been
    // exercised on, but the same latent defect the ignore mechanism's own bug surfaced); routed
    // through the one converter now, not left as a second, differently-correct copy of the fix.
    let rel = pathbuf_to_slash_string(path).map_err(AuthorError::Store)?;
    let repo_path = RepoPath::parse(&rel).map_err(AuthorError::Store)?;
    let stat: RootFileStat = stat_file_state_if_exists(layout.worktree_mutation_root(), path)
        .map_err(AuthorError::Store)?
        .ok_or_else(|| {
            AuthorError::Store(PrikkError::Io {
                kind: None,
                context: format!("worktree entry disappeared: {}", path.display()),
            })
        })?;
    out.insert(
        repo_path.as_str().to_string(),
        WorktreeFileMeta {
            size: stat.size,
            mtime_secs: stat.mtime_secs,
            mtime_nanos: stat.mtime_nanos,
            mode: normalize_file_mode(stat.mode),
        },
    );
    Ok(())
}

fn normalize_file_mode(mode: Option<u32>) -> Option<u32> {
    mode.map(|mode| {
        if mode & 0o111 != 0 {
            EXECUTABLE_FILE_MODE
        } else {
            REGULAR_FILE_MODE
        }
    })
}

fn join_relative(parent: &Path, name: &std::ffi::OsStr) -> PathBuf {
    if parent.as_os_str().is_empty() {
        PathBuf::from(name)
    } else {
        parent.join(name)
    }
}
