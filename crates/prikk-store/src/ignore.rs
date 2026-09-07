//! `.prikkignore` — the worktree ignore mechanism (RFC 124).
//!
//! **Binds at the scan layer only** (RFC 124 §3.1 / handoff §4.1): [`IgnoreRules`] is consulted by
//! `commit`'s own worktree walk (`worktree_patch::node_authoring::worktree_files`) and by
//! `worktree-status`'s (`worktree_status::scan_untracked`) — the two independent walks the handoff
//! found, both cited there. Nothing outside those two call sites may construct or consult an
//! `IgnoreRules` value: identity, replay, verification, and materialization must apply a patch
//! regardless of what the receiver's own ignore file says, or two repositories with different ignore
//! files would disagree about the same signed history.
//!
//! **Syntax: literal repo-relative path prefixes, one per line.** No globbing, no negation, no
//! per-directory files, no comments. A rule matches its own path exactly, or any path nested under
//! it (`target` matches `target` and everything under `target/`, never `target2` or `targetfoo`) —
//! a whole-component prefix, not a bare string prefix, so a rule can never accidentally over-match a
//! differently-named sibling. This is a stated limit, not a first step toward gitignore
//! compatibility — an ignore syntax that nearly matches gitignore's own semantics (globbing,
//! negation, precedence) would be worse than one that obviously does not attempt to.
//!
//! **Discovery only, never removal** (RFC 124 §3.4 / handoff §4.4): [`should_skip_discovery`] takes
//! the set of already-tracked paths and refuses to hide any path already tracked, or lying under an
//! already-tracked path — adding a line to `.prikkignore` must never make `commit` read an existing
//! file as missing and author a `DeleteNode` for it. A whole ignored directory is skipped without
//! descending into it (never opened, never stat'd) exactly when no tracked path lives inside it —
//! this is not only a performance optimization: `commit`'s own walk fails closed on any symlink or
//! unsupported entry kind it encounters (`worktree_files.rs`'s `EntryKind::Symlink`/`EntryKind::Other`
//! arms), and a real `node_modules/` is typically full of exactly those, so without directory-level
//! pruning, ignoring it would not actually let a real project commit at all.
//!
//! **`.prikkignore` is itself an ordinary, tracked worktree file.** It is not a config file (there is
//! none — `main.rs`'s `ActivePatchThresholds` doc comment defers a durable configuration mechanism to
//! a future increment) and it is not per-user or per-checkout: it lives in
//! the worktree, `commit` scans and signs it like anything else unless a rule inside it also covers
//! itself, and it travels with every checkout of this history the same way any other tracked file
//! does — including through `bundle`/`sync`, so a received repository's own future commits are
//! shaped by whatever ignore rules the sender happened to have. Harmless for applying history: the
//! rule binds at discovery only (above), never at materialization, so a receiver's `.prikkignore`
//! (inherited or its own) cannot change what an already-existing patch does when it applies. This is
//! the minimal-mechanism choice regardless: no new authoring path, no new persistence, reusing the
//! ordinary commit/materialization machinery that already exists.
//!
//! **A malformed `.prikkignore` fails closed** — refused, not silently treated as empty. An absent
//! file is not malformed: no `.prikkignore` at all means no rules, and every existing repository
//! (which has no such file) behaves exactly as it did before this mechanism existed.
//!
//! **Every repo-path string this module compares must be built through
//! [`crate::path::pathbuf_to_slash_string`], never `Path::to_str()`/`Path::display()`.** The
//! mechanism's first landing (`b9a6fd8`, reverted at `2235af3`) called `Path::to_str()` on a path a
//! worktree walk built with `Path::join`, which inserts the platform separator — on Windows that
//! produced a backslash-joined string that matched no forward-slash rule and no `/`-joined tracked
//! path, and `commit` failed outright on Windows for any nested worktree path. `is_ignored` and
//! `should_skip_discovery` below are pure string comparisons with no separator awareness of their
//! own; correctness depends entirely on every caller normalizing first, which is why both call
//! sites now go through the one shared converter (see its own doc comment).

use std::collections::BTreeSet;
use std::path::Path;

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::RepositoryLayout;
use crate::path::RepoPath;

/// Fixed name and location: the repository root, alongside the worktree it governs.
pub(crate) const IGNORE_FILE_NAME: &str = ".prikkignore";

/// A loaded, validated set of ignore rules — literal repo-relative path prefixes.
#[derive(Debug, Clone, Default)]
pub(crate) struct IgnoreRules {
    prefixes: Vec<String>,
}

impl IgnoreRules {
    /// Load and validate `.prikkignore` from `layout`'s worktree root, through the same anchored
    /// read every other worktree read in this crate uses. Absent file -> an empty rule set (no
    /// existing repository's behavior changes). Present but malformed -> `Err` (fail closed): not
    /// valid UTF-8, or any non-blank line that is not a valid repository-relative path once its
    /// trailing `/` (if any) is stripped.
    pub(crate) fn load(layout: &RepositoryLayout) -> Result<Self> {
        let Some(bytes) =
            read_file_if_exists(layout.worktree_mutation_root(), Path::new(IGNORE_FILE_NAME))?
        else {
            return Ok(Self::default());
        };
        let text = String::from_utf8(bytes).map_err(|_| {
            PrikkError::InvalidName(format!("{IGNORE_FILE_NAME} is not valid UTF-8"))
        })?;
        let mut prefixes = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let rule = trimmed.trim_end_matches('/');
            if rule.is_empty() {
                return Err(PrikkError::InvalidName(format!(
                    "{IGNORE_FILE_NAME} line {}: a bare \"/\" is not a valid ignore rule",
                    index + 1
                )));
            }
            RepoPath::parse(rule).map_err(|err| {
                PrikkError::InvalidName(format!("{IGNORE_FILE_NAME} line {}: {err}", index + 1))
            })?;
            prefixes.push(rule.to_string());
        }
        Ok(Self { prefixes })
    }

    /// Whether `path` (a repository-relative, `/`-joined path -- see this module's own doc comment
    /// on why that precondition matters) is covered by any rule — equal to it, or nested under it as
    /// a whole path component.
    #[must_use]
    pub(crate) fn is_ignored(&self, path: &str) -> bool {
        self.prefixes.iter().any(|prefix| {
            path == prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }
}

/// The one derivation of "should this worktree entry be invisible to discovery?", consulted by both
/// of the crate's independent worktree walks (handoff §3): ignored, **and** not itself tracked,
/// **and** carrying no tracked descendant. `tracked` is every already-tracked path relevant to the
/// caller's own baseline (a `commit` baseline's file *and* symlink paths together; a
/// `worktree-status` baseline's tracked paths) — checked once here so a future third walk gets this
/// right by construction rather than by copying the check correctly by hand. `path` must already be
/// the `/`-joined form (`crate::path::pathbuf_to_slash_string`), matching every entry of `tracked`.
#[must_use]
pub(crate) fn should_skip_discovery(
    rules: &IgnoreRules,
    tracked: &BTreeSet<String>,
    path: &str,
) -> bool {
    rules.is_ignored(path) && !tracked.contains(path) && !has_tracked_descendant(tracked, path)
}

/// Whether any path in `tracked` lies strictly under `dir` (`dir/...`) — an O(log n) range probe,
/// not a linear scan, so directory-level pruning stays cheap even against a large tracked set.
fn has_tracked_descendant(tracked: &BTreeSet<String>, dir: &str) -> bool {
    let prefix = format!("{dir}/");
    tracked
        .range(prefix.clone()..)
        .next()
        .is_some_and(|candidate| candidate.starts_with(&prefix))
}

#[cfg(test)]
mod tests;
