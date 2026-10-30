//! Compatibility imports for repository-relative path validation.

use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};
pub use prikk_replay::{RepoPath, validate_no_path_collisions, validate_repo_path};

/// Join a validated repository-relative path to a worktree/repository root.
pub(crate) fn join_repo_path_to_root(path: &RepoPath, root: &Path) -> PathBuf {
    let mut out = root.to_path_buf();
    for component in path.as_str().split('/') {
        out.push(component);
    }
    out
}

/// Convert a filesystem path into its canonical slash-separated repository-relative string form,
/// component by component. **Never use `Path::to_str()`/`Path::display()` directly for a path a
/// worktree walk builds with `Path::join`** — `join` inserts the platform separator, so on Windows
/// that string is backslash-joined, and every repository-path consumer downstream (`RepoPath::parse`,
/// an ignore rule, a tracked-path set built from `/`-joined baseline paths) either rejects it outright
/// or, worse, silently fails to match against something it should. This is the one converter both of
/// the crate's live-worktree walks (`worktree_status.rs`, `worktree_patch/node_authoring/worktree_files.rs`)
/// must go through — a third differently-shaped conversion is exactly the defect this function exists
/// to close off (RFC 124's re-land, after the mechanism's own first landing broke `commit` on Windows
/// by skipping this and calling `Path::to_str()` instead).
pub(crate) fn pathbuf_to_slash_string(path: &Path) -> Result<String> {
    let mut components = Vec::new();
    for component in path.components() {
        // `InvalidName`, not `Integrity` (RFC 124 re-land, round 3 — the same misclassification
        // RFC 122 §4 ruled on in the same words): a worktree file whose name prikk cannot represent
        // is a name outside the supported subset, not evidence the repository is damaged.
        // `integrity error` is reserved for the latter, and a user who sees it for an ordinary
        // unsupported filename would reach for `doctor`/backups/recovery for nothing.
        let text = component.as_os_str().to_str().ok_or_else(|| {
            PrikkError::InvalidName(format!(
                "worktree path is not valid UTF-8: {}",
                path.display()
            ))
        })?;
        components.push(text.to_string());
    }
    if components.is_empty() {
        // Kept as `Integrity`, unlike the arm above: an empty path can only come from
        // `Path::components()` yielding nothing for a path a worktree walk itself constructed (a
        // directory entry always has a non-empty name, and every call site here builds `path` from
        // one) -- so reaching this means the walk's own invariant broke, not that a user chose an
        // unusual file name. That is a fact about repository/walk state, which is exactly what
        // `Integrity` is for.
        return Err(PrikkError::Integrity("empty worktree path".to_string()));
    }
    Ok(components.join("/"))
}

/// Reword a `RepoPath::parse`/`validate_repo_path` refusal for the authoring context: a live
/// worktree file's name, reached while `commit` (or `diff`'s own worktree-side read) scans the
/// worktree -- never stored or delivered history (0.56.0 step 3d2). Used at this crate's own five
/// worktree-scan call sites, the earlier point in the same `commit` call where this refusal is
/// actually first raised, before any `Patch` operation is ever constructed.
///
/// `pub(crate) use`, not a second definition (0.56.0 step 3e T2): this is the identical function
/// `prikk-object`'s own `CreateFile`/`DeleteNode`/`RenamePath`/`CreateSymlink::validate()` already
/// call at encode time -- re-exported here, never duplicated, so the sentence cannot drift between
/// the two crates again.
pub(crate) use prikk_object::authoring_path_error;

#[cfg(test)]
mod tests;
