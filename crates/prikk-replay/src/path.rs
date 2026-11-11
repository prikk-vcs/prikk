//! Repository-relative lexical path validation.
//!
//! This module owns the canonical repository-relative path value used by replay/lifecycle semantic
//! state. Filesystem layout, materialization policy, and worktree ownership remain in `prikk-store`.
//! Since repository format 8 (0.57.0) a path may be non-ASCII; what it may not contain is the decode table of
//! `prikk-object`, and collisions are judged by the canonical caseless key.
//!
//! The lexical grammar itself (`validate_repo_path`) moved to `prikk-object` (DC-54): object
//! envelope encoders need to call it without creating a `prikk-object -> prikk-replay` dependency
//! cycle. Re-exported here so every existing `prikk_replay::validate_repo_path` caller keeps
//! compiling unchanged.

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};

use prikk_object::canonical_caseless_key;
pub use prikk_object::validate_repo_path;

/// A validated repository-relative path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath(String);

impl RepoPath {
    /// Validate and construct a repository-relative path.
    ///
    /// # Examples
    ///
    /// A validated `RepoPath` carries its own string back out unchanged -- construction does not
    /// normalize case or separators, only refuse what is unsafe. Two paths differing only by case
    /// both construct successfully as distinct values; [`validate_no_path_collisions`] is the
    /// separate check that catches them being used together.
    ///
    /// ```
    /// use prikk_replay::RepoPath;
    ///
    /// let path = RepoPath::parse("src/lib.rs").unwrap();
    /// assert_eq!(path.as_str(), "src/lib.rs");
    ///
    /// assert!(RepoPath::parse("../outside").is_err());
    /// ```
    pub fn parse(value: &str) -> Result<Self> {
        validate_repo_path(value)?;
        Ok(Self(value.to_string()))
    }

    /// Return the canonical slash-separated representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reject duplicate paths and collisions: two paths whose **canonical caseless key** is the same
/// (`NFD(fold(NFD(path)))`, full Unicode case folding), so they are one name to a case-insensitive or normalizing file
/// system. That is a **deliberate over-refusal**: `straße` and `strasse` collide, as do `É` and `é`, `ẞ` and `ss`, an NFC and
/// an NFD spelling, and `K` and the Kelvin sign. A collision check protects the disk, so refusing more is the safe direction,
/// and it **may tighten across Unicode versions** (the crates' tables only gain mappings).
///
/// # Examples
///
/// `README.md` and `readme.md` are two different, individually valid paths -- each passes
/// [`RepoPath::parse`] on its own. Committing both side by side is silently fine on a
/// case-sensitive filesystem (Linux) and silently *not the same two files* on a case-insensitive
/// one (macOS's and Windows' defaults) -- exactly the hazard this check exists to refuse before it
/// becomes a materialization surprise on a different platform than the one that authored it.
///
/// ```
/// use prikk_replay::{RepoPath, validate_no_path_collisions};
///
/// let distinct = [RepoPath::parse("a.txt").unwrap(), RepoPath::parse("b.txt").unwrap()];
/// assert!(validate_no_path_collisions(&distinct).is_ok());
///
/// let colliding = [RepoPath::parse("README.md").unwrap(), RepoPath::parse("readme.md").unwrap()];
/// assert!(
///     validate_no_path_collisions(&colliding).is_err(),
///     "paths differing only by case must be refused together"
/// );
/// ```
pub fn validate_no_path_collisions(paths: &[RepoPath]) -> Result<()> {
    let mut exact = BTreeSet::<&str>::new();
    let mut folded = std::collections::BTreeMap::<String, &str>::new();
    for path in paths {
        let exact_value = path.as_str();
        if !exact.insert(exact_value) {
            return Err(PrikkError::InvalidName(format!(
                "duplicate repository path: {exact_value}"
            )));
        }
        if let Some(earlier) = folded.insert(canonical_caseless_key(exact_value), exact_value) {
            return Err(PrikkError::InvalidName(format!(
                "case-insensitive path collision involving: {} and {}; the two are one name once Unicode case folding and \
                 normalization are applied, so a case-insensitive or normalizing file system would make them one file",
                prikk_error::escape_non_ascii(earlier),
                prikk_error::escape_non_ascii(exact_value)
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
