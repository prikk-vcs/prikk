//! Where prikk looks for a signing seed (RFC 148 §3).
//!
//! **Two places per role, in order, and no third:**
//!
//! 1. `PRIKK_<ROLE>_SEED_FILE`, if set — a path, an override, the escape hatch;
//! 2. otherwise `<default key directory>/<role>.seed`.
//!
//! **There is no environment channel for a seed.** `PRIKK_AUTHOR_SEED` and
//! `PRIKK_MAINTAINER_SEED` are not read; they are *detected and refused*, for one release, so that
//! nobody's automation silently starts signing with a different key than it thinks it is using. A
//! silent ignore would be the worst available behaviour here, which is why it is forbidden rather
//! than merely avoided.
//!
//! The default directory is resolved by prikk itself, with no dependency, and has exactly one
//! candidate per platform — never a repository, never a parent directory, never a fallback chain
//! beyond the single `XDG`-then-`HOME` step the XDG spec itself defines.

use std::path::{Path, PathBuf};

use crate::commands::CliError;

/// The two signing roles. Each names its own environment variables and its own file, so no call
/// site spells a variable name and none can drift from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Author,
    Maintainer,
}

impl Role {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Role::Author => "author",
            Role::Maintainer => "maintainer",
        }
    }

    /// The default key id when `PRIKK_<ROLE>_KEY_ID` is unset — the same word as the role, which is
    /// what `prikk setup` has always written.
    pub(crate) const fn default_key_id(self) -> &'static str {
        self.label()
    }

    pub(crate) const fn key_id_var(self) -> &'static str {
        match self {
            Role::Author => "PRIKK_AUTHOR_KEY_ID",
            Role::Maintainer => "PRIKK_MAINTAINER_KEY_ID",
        }
    }

    pub(crate) const fn seed_file_var(self) -> &'static str {
        match self {
            Role::Author => "PRIKK_AUTHOR_SEED_FILE",
            Role::Maintainer => "PRIKK_MAINTAINER_SEED_FILE",
        }
    }

    /// The variable that used to carry a raw seed. **Read only to refuse.**
    ///
    /// REMOVE THIS DETECTION IN 0.41.0 — 0.40.0 is the release that stops reading it and refuses,
    /// and the very next release drops the refusal. After that window a stale `PRIKK_AUTHOR_SEED` in
    /// someone's shell profile is simply an unused variable, and this refusal becomes noise.
    pub(crate) const fn retired_seed_var(self) -> &'static str {
        match self {
            Role::Author => "PRIKK_AUTHOR_SEED",
            Role::Maintainer => "PRIKK_MAINTAINER_SEED",
        }
    }

    pub(crate) const fn seed_file_name(self) -> &'static str {
        match self {
            Role::Author => "author.seed",
            Role::Maintainer => "maintainer.seed",
        }
    }
}

/// The one directory prikk keeps keys in, per platform.
///
/// Unix (Linux, macOS, BSD): `$XDG_CONFIG_HOME/prikk`, else `$HOME/.config/prikk`.
/// Windows: `%APPDATA%\prikk`.
///
/// **No second candidate and no search.** A key directory that is sometimes one place and sometimes
/// another is a key directory nobody can reason about: "which key signed this?" must have one
/// answer, and `PRIKK_<ROLE>_SEED_FILE` is the only way to change it.
#[cfg(unix)]
pub(crate) fn default_key_dir() -> std::result::Result<PathBuf, CliError> {
    if let Some(xdg) = non_empty_var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("prikk"));
    }
    let home = non_empty_var("HOME").ok_or_else(|| {
        CliError::Failure(
            "cannot locate prikk's key directory: neither XDG_CONFIG_HOME nor HOME is set. Set \
             one of them, or point PRIKK_AUTHOR_SEED_FILE/PRIKK_MAINTAINER_SEED_FILE at your seed \
             files directly"
                .to_string(),
        )
    })?;
    Ok(PathBuf::from(home).join(".config").join("prikk"))
}

/// Windows: `%APPDATA%` is per-user by platform ACL, which is what this directory relies on for
/// confidentiality — there is no mode bit to set and none is set. Stated here and in
/// `docs/src/guide/first-run.md`, because relying on an inherited ACL silently is exactly the thing
/// `key generate --out` refuses to do for an arbitrary path.
#[cfg(windows)]
pub(crate) fn default_key_dir() -> std::result::Result<PathBuf, CliError> {
    let appdata = non_empty_var("APPDATA").ok_or_else(|| {
        CliError::Failure(
            "cannot locate prikk's key directory: APPDATA is not set. Set it, or point \
             PRIKK_AUTHOR_SEED_FILE/PRIKK_MAINTAINER_SEED_FILE at your seed files directly"
                .to_string(),
        )
    })?;
    Ok(PathBuf::from(appdata).join("prikk"))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn default_key_dir() -> std::result::Result<PathBuf, CliError> {
    Err(CliError::Failure(
        "prikk has no default key directory on this platform -- set \
         PRIKK_AUTHOR_SEED_FILE/PRIKK_MAINTAINER_SEED_FILE to name your seed files directly"
            .to_string(),
    ))
}

fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Where this role's seed is read from: the override if set, otherwise the default directory's file.
///
/// **An override that is set always wins, even when the file is missing.** Falling back to the
/// default file would mean a typo in `PRIKK_AUTHOR_SEED_FILE` signs with a different key than the
/// operator named, silently — the same class of failure the retired environment channel is being
/// removed to prevent.
pub(crate) fn seed_path(role: Role) -> std::result::Result<PathBuf, CliError> {
    if let Some(path) = non_empty_var(role.seed_file_var()) {
        return Ok(PathBuf::from(path));
    }
    Ok(default_key_dir()?.join(role.seed_file_name()))
}

/// Refuse a retired `PRIKK_<ROLE>_SEED`, naming where the key lives now.
///
/// Checked before anything is read, and refused whether or not a seed file also exists: an operator
/// whose environment still carries the old variable must be told, not quietly switched.
fn refuse_retired_env(role: Role) -> std::result::Result<(), CliError> {
    if std::env::var_os(role.retired_seed_var()).is_none() {
        return Ok(());
    }
    let location = match default_key_dir() {
        Ok(dir) => dir.display().to_string(),
        Err(_) => "your key directory".to_string(),
    };
    Err(CliError::Failure(
        prikk_error::PrikkError::Precondition(format!(
            "{} is no longer read; your keys are in {location} (or set {})",
            role.retired_seed_var(),
            role.seed_file_var()
        ))
        .to_string(),
    ))
}

/// Refuse a seed file any other user can read.
///
/// Unix only: there is a mode to check. On Windows the default directory relies on `%APPDATA%`'s
/// per-user ACL (see [`default_key_dir`]) and an arbitrary `--*-seed-out` path is refused outright
/// by `key.rs`, so there is no silent-inherited-permissions case left to check for.
#[cfg(unix)]
fn require_private_mode(path: &Path) -> std::result::Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path)
        .map_err(|err| CliError::Failure(format!("cannot read {}: {err}", path.display())))?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(CliError::Failure(format!(
            "{} is readable by group or other (mode {mode:04o}); run `chmod 600 {}`",
            path.display(),
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private_mode(_path: &Path) -> std::result::Result<(), CliError> {
    Ok(())
}

/// Read this role's seed: refuse a retired variable, resolve the path, refuse a readable file, then
/// decode.
pub(crate) fn read_seed(
    role: Role,
) -> std::result::Result<[u8; prikk_crypto::ED25519_KEY_LEN], CliError> {
    refuse_retired_env(role)?;
    let path = seed_path(role)?;
    if !path.exists() {
        // Both named routes are checked to work from here, which is not automatic: `prikk setup`
        // refuses a directory that already holds a repository (RFC 135), so naming only that would
        // send a user with an existing repository in a circle -- `commit` to `setup` and back. The
        // `key generate` route creates the key directory itself when the path is prikk's own.
        let shown = path.display().to_string();
        return Err(CliError::Failure(format!(
            "{} signing is required: no seed at {shown}. Create one with `prikk key generate --out \
             {shown}`, run `prikk setup` in a new project directory, or set {} to an existing seed \
             file",
            role.label(),
            role.seed_file_var()
        )));
    }
    require_private_mode(&path)?;
    let contents = std::fs::read_to_string(&path)
        .map_err(|err| CliError::Failure(format!("cannot read {}: {err}", path.display())))?;
    crate::decode_seed_hex(contents.trim(), &path.display().to_string()).map_err(CliError::Failure)
}

/// This role's key id: the environment variable if set and non-empty, otherwise the role's own name.
pub(crate) fn key_id(role: Role) -> std::result::Result<String, CliError> {
    match std::env::var(role.key_id_var()) {
        Ok(value) if value.trim().is_empty() => Err(CliError::Usage(format!(
            "{} must not be empty",
            role.key_id_var()
        ))),
        Ok(value) => Ok(value),
        Err(_) => Ok(role.default_key_id().to_string()),
    }
}

/// Create the default key directory, private to this user, and return it.
///
/// `0700` on Unix. On Windows the directory inherits `%APPDATA%`'s per-user ACL, which is the whole
/// of its confidentiality story and is documented as such.
pub(crate) fn ensure_key_dir() -> std::result::Result<PathBuf, CliError> {
    let dir = default_key_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|err| CliError::Failure(format!("failed to create {}: {err}", dir.display())))?;
    set_key_dir_mode(&dir)?;
    Ok(dir)
}

#[cfg(unix)]
fn set_key_dir_mode(dir: &Path) -> std::result::Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|err| {
        CliError::Failure(format!(
            "failed to set mode 0700 on {}: {err}",
            dir.display()
        ))
    })
}

#[cfg(not(unix))]
fn set_key_dir_mode(_dir: &Path) -> std::result::Result<(), CliError> {
    Ok(())
}
