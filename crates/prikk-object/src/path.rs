//! Repository-relative lexical path validation.
//!
//! Moved here from `prikk-replay` (DC-54): this is pure lexical grammar with no dependency on
//! `RepoPath` or lifecycle state, and object-envelope encoders need to call it without creating a
//! `prikk-object -> prikk-replay` dependency cycle (`prikk-replay` already depends on
//! `prikk-object`). `prikk-replay::RepoPath::parse` calls this function and re-exports it, so
//! every existing caller of `prikk_replay::validate_repo_path` / `prikk_store::validate_repo_path`
//! keeps compiling unchanged.

use prikk_error::{PrikkError, Result};

mod canonical;
mod decode_table;

pub use canonical::{canonical_caseless_key, escape_non_ascii, non_nfc_spelling};
pub use decode_table::{first_refused_character, refusal_reason};

/// The single case-folding definition every case-insensitive-collision check across repository
/// paths, branch ref names, tag ref names, and maintainer trust key ids folds through (DC-72 design
/// ruling: "one folding definition, four call sites" — `rfcs/accepted/DC-72-PATH-SAFETY-CONFORMANCE.md`
/// §3.5). `prikk-object` is the lowest crate every surface's crate already depends on, directly or
/// transitively, so this is the one place a fifth surface's collision check would also reach.
///
/// Deliberately ASCII-only, not Unicode normalization: an NFC-composed and NFD-decomposed spelling
/// of the same visible name are different byte sequences and are not folded together here. Recorded
/// as a known limitation, not an oversight, at `docs/src/reference/path-safety.md`. **Repository paths no longer use this**
/// (0.57.0 U3: they use [`canonical_caseless_key`]); branch and tag ref names and maintainer key ids still do, by ruling.
#[must_use]
pub fn ascii_fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// POSIX `NAME_MAX`: the per-component **byte**-length ceiling (UTF-8 bytes). Windows' own NTFS limit is 255 UTF-16 code
/// units, and a string never has more UTF-16 code units than UTF-8 bytes, so a component under this ceiling is always under
/// NTFS's too: there is no second, UTF-16 limit, because it could never fail (0.57.0 U2, U0 finding 1). A component longer
/// than this cannot exist on any of the three platforms this project mutates on
/// (`docs/src/reference/platform-support.md`), independent of where in the tree it sits or how deep the worktree root is.
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
    // 0.57.0 U2 (repository format 8): non-ASCII paths are admitted. What is refused is exactly the explicit decode table
    // (`decode_table.rs`), frozen at format 8 -- the same list for every binary that reads the history. NFC is not asked
    // here (it is asked only where a name is authored; `non_nfc_spelling`).
    if let Some((character, reason)) = first_refused_character(value) {
        if character.is_ascii() {
            // C0 controls and DEL were refused before format 8, permanently: plain `InvalidName`, as ever.
            return Err(PrikkError::InvalidName(
                "control characters are not allowed in repository paths".to_string(),
            ));
        }
        // 0.56.0 step 3d2: a dedicated variant, not `InvalidName` -- this refusal is genuinely version-relative (a newer
        // `prikk` may admit more characters; see `PrikkError::RefusedPathCharacter`'s own doc comment), so every caller
        // downstream that needs to say so distinctly matches the variant, never rendered text. Its own `Display` is the
        // "reading" sentence; `prikk-object`'s own authoring call sites (`payload/patch/operations.rs`) reword it for
        // `commit`.
        return Err(PrikkError::RefusedPathCharacter {
            path: value.to_string(),
            character,
            reason,
        });
    }
    for (index, component) in value.split('/').enumerate() {
        // 0.57.0 U3: asked of the canonical caseless key, not ASCII case alone. On a case-insensitive, normalizing file system
        // `.pri` + U+212A (Kelvin sign) + `k` is the `.prikk` directory (the Kelvin sign case-folds to `k`). This is the one
        // decode-time check that folds; it is version-stable in the direction that matters (the fold of a character that
        // reaches ASCII letters is fixed, and a later Unicode version can only add such characters, which tightens).
        if index == 0 && canonical_caseless_key(component) == ".prikk" {
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
/// (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, and since 0.57.0 the superscript forms `COM¹ COM² COM³ LPT¹ LPT²
/// LPT³`, which Windows reserves in every directory and which a non-ASCII path can now spell), checked case-insensitively
/// and regardless of host OS. The names are explicit, so the rule is the same in every Unicode version. Exposed for other storage surfaces that build a literal filesystem path
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
        || matches!(
            base.as_str(),
            "COM\u{b9}" | "COM\u{b2}" | "COM\u{b3}" | "LPT\u{b9}" | "LPT\u{b2}" | "LPT\u{b3}"
        )
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod unicode_stability;
