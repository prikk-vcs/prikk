//! Repository-relative lexical path validation.
//!
//! Moved here from `prikk-replay` (DC-54): this is pure lexical grammar with no dependency on
//! `RepoPath` or lifecycle state, and object-envelope encoders need to call it without creating a
//! `prikk-object -> prikk-replay` dependency cycle (`prikk-replay` already depends on
//! `prikk-object`). `prikk-replay::RepoPath::parse` calls this function and re-exports it, so
//! every existing caller of `prikk_replay::validate_repo_path` / `prikk_store::validate_repo_path`
//! keeps compiling unchanged.

use prikk_error::{PrikkError, Result};

/// The single case-folding definition every case-insensitive-collision check across repository
/// paths, branch ref names, tag ref names, and maintainer trust key ids folds through (DC-72 design
/// ruling: "one folding definition, four call sites" — `rfcs/accepted/DC-72-PATH-SAFETY-CONFORMANCE.md`
/// §3.5). `prikk-object` is the lowest crate every surface's crate already depends on, directly or
/// transitively, so this is the one place a fifth surface's collision check would also reach.
///
/// Deliberately ASCII-only, not Unicode normalization: an NFC-composed and NFD-decomposed spelling
/// of the same visible name are different byte sequences and are not folded together here. Recorded
/// as a known limitation, not an oversight, at `docs/src/reference/path-safety.md`.
#[must_use]
pub fn ascii_fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// POSIX `NAME_MAX`: the per-component byte-length ceiling shared by Linux, macOS, and Windows'
/// own NTFS component limit (255 UTF-16 code units — the same count as bytes here, since repository
/// paths are ASCII-only). A component longer than this cannot exist on any of the three platforms
/// this project mutates on (`docs/src/reference/platform-support.md`), independent of where in the
/// tree it sits or how deep the worktree root is.
const MAX_COMPONENT_LEN: usize = 255;

/// macOS's `PATH_MAX` (`sys/syslimits.h`), the strictest of the three mutation platforms' hard
/// total-path limits: Linux's is 4096, and Windows' legacy default (`MAX_PATH`, 260) is a separate,
/// narrower, already-documented gap that depends on the worktree root's own depth too and cannot be
/// bounded from a repository-relative length alone (`docs/src/reference/platform-support.md`).
/// Chosen as the floor so a path this validator accepts is materializable on Linux and macOS
/// unconditionally, regardless of worktree root depth.
const MAX_TOTAL_LEN: usize = 1024;

/// Validate that a path is safe as a repository-relative path.
///
/// # Examples
///
/// A caller who never inspects the `Result` and just writes wherever the string points would not
/// notice a traversal attempt until it escaped the repository -- checking the return value here is
/// what turns a silent misuse into an explicit refusal.
///
/// ```
/// use prikk_object::validate_repo_path;
///
/// assert!(validate_repo_path("src/lib.rs").is_ok());
///
/// let escape = validate_repo_path("../outside-the-repo");
/// assert!(escape.is_err(), "a `..` component must never validate");
/// ```
pub fn validate_repo_path(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(PrikkError::InvalidName(
            "repository path must not be empty".to_string(),
        ));
    }
    if value.len() > MAX_TOTAL_LEN {
        return Err(PrikkError::InvalidName(format!(
            "repository path exceeds {MAX_TOTAL_LEN} bytes ({} bytes)",
            value.len()
        )));
    }
    if value.starts_with('/') {
        return Err(PrikkError::InvalidName(
            "absolute paths are not allowed".to_string(),
        ));
    }
    if value.contains('\\') {
        return Err(PrikkError::InvalidName(
            "backslashes are not allowed in repository paths".to_string(),
        ));
    }
    if value.contains(':') {
        return Err(PrikkError::InvalidName(
            "colon characters are not allowed in repository paths".to_string(),
        ));
    }
    if !value.is_ascii() {
        // 0.56.0 step 3d2: a dedicated variant, not `InvalidName` -- this is the one
        // `validate_repo_path` refusal that is genuinely version-relative (a future `prikk` may
        // accept non-ASCII paths; see `PrikkError::NonAsciiRepoPath`'s own doc comment), so every
        // caller downstream that needs to say so distinctly matches the variant, never rendered
        // text. Its own `Display` is the "reading" sentence; `prikk-object`'s own authoring call
        // sites (`payload/patch/operations.rs`) reword it for `commit`.
        return Err(PrikkError::NonAsciiRepoPath {
            path: value.to_string(),
        });
    }
    if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(PrikkError::InvalidName(
            "control characters are not allowed in repository paths".to_string(),
        ));
    }
    for (index, component) in value.split('/').enumerate() {
        if index == 0 && component.eq_ignore_ascii_case(".prikk") {
            return Err(PrikkError::InvalidName(
                "repository paths must not target the .prikk metadata directory".to_string(),
            ));
        }
        validate_component(component)?;
    }
    Ok(())
}

fn validate_component(component: &str) -> Result<()> {
    if component.is_empty() {
        return Err(PrikkError::InvalidName(
            "empty path components are not allowed".to_string(),
        ));
    }
    if component.len() > MAX_COMPONENT_LEN {
        return Err(PrikkError::InvalidName(format!(
            "path component exceeds {MAX_COMPONENT_LEN} bytes ({} bytes)",
            component.len()
        )));
    }
    if component == "." || component == ".." {
        return Err(PrikkError::InvalidName(
            "dot path components are not allowed".to_string(),
        ));
    }
    if component.ends_with(' ') || component.ends_with('.') {
        return Err(PrikkError::InvalidName(
            "path components must not end with space or dot".to_string(),
        ));
    }
    if is_windows_reserved_name(component) {
        return Err(PrikkError::InvalidName(format!(
            "Windows reserved path component is not allowed: {component}"
        )));
    }
    Ok(())
}

/// Whether a path component's basename (before the first `.`) is a Windows-reserved device name
/// (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`), checked case-insensitively and
/// regardless of host OS. Exposed for other storage surfaces that build a literal filesystem path
/// component from user-supplied text outside the `RepoPath` grammar (DC-72).
#[must_use]
/// # Examples
///
/// `con.txt` is an ordinary-looking filename that creates and reads back fine on Linux and macOS --
/// the reservation is invisible there, which is exactly why silently skipping this check would be a
/// defect nobody notices until the same repository is checked out on Windows. Matching is by the
/// component's name before its first `.`, case-insensitively, so an extension does not hide it.
///
/// ```
/// use prikk_object::is_windows_reserved_name;
///
/// assert!(is_windows_reserved_name("con.txt"));
/// assert!(is_windows_reserved_name("NUL"));
/// assert!(!is_windows_reserved_name("console.txt"));
/// ```
pub fn is_windows_reserved_name(component: &str) -> bool {
    let base = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches!(
            base.as_str(),
            "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
        )
        || matches!(
            base.as_str(),
            "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
        )
}

#[cfg(test)]
mod tests;
