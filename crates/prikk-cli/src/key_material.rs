//! Where prikk looks for a signing seed (RFC 148 §3).
//!
//! **Two places per role, in order, and no third:**
//!
//! 1. `PRIKK_<ROLE>_SEED_FILE`, if set — a path, an override, the escape hatch;
//! 2. otherwise `<default key directory>/<role>.seed`.
//!
//! **There is no environment channel for a seed.** `PRIKK_AUTHOR_SEED` and `PRIKK_MAINTAINER_SEED`
//! were that channel until 0.40.0, which stopped reading them and *refused* rather than ignoring
//! them, so that nobody's automation could silently start signing with a different key than it
//! thought it was using. That window was deliberately one release wide and closed in 0.41.0: the
//! variables are now simply unread, like any other name prikk knows nothing about. What replaced the
//! refusal is `prikk key status`, which answers "which key will actually sign here" directly instead
//! of waiting for a signing attempt to object.
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
/// operator named, silently — the same class of failure the retired environment channel was removed
/// to prevent.
pub(crate) fn seed_path(role: Role) -> std::result::Result<PathBuf, CliError> {
    if let Some(path) = non_empty_var(role.seed_file_var()) {
        return Ok(PathBuf::from(path));
    }
    Ok(default_key_dir()?.join(role.seed_file_name()))
}

/// Where a seed was looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeedSource {
    /// `PRIKK_<ROLE>_SEED_FILE` was set and names this path.
    Override,
    /// No override; the key directory's own file for this role.
    KeyDirectory,
}

impl SeedSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            SeedSource::Override => "seed-file-override",
            SeedSource::KeyDirectory => "key-directory",
        }
    }
}

/// Why a seed is not usable. RFC 150 §2's `reason` vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unusable {
    /// The key directory's file is not there.
    Missing,
    /// `PRIKK_<ROLE>_SEED_FILE` was set and that file is not there. Distinct from `Missing` on
    /// purpose: the operator named a path, and the answer must be about *that* path.
    OverrideMissing,
    /// Unix mode rule: group or other can read it.
    ReadableByOthers { mode: u32 },
    /// Present and private, but not 64 hex characters.
    Undecodable { detail: String },
}

impl Unusable {
    /// The machine-readable reason, RFC 150 §2's own vocabulary.
    pub(crate) fn code(&self) -> String {
        match self {
            Unusable::Missing => "missing".to_string(),
            Unusable::OverrideMissing => "override-missing".to_string(),
            Unusable::ReadableByOthers { mode } => format!("readable-by-others (mode {mode:04o})"),
            Unusable::Undecodable { .. } => "undecodable".to_string(),
        }
    }
}

/// One role's key material, as a **question answered** rather than an operation attempted.
///
/// RFC 150 §1: `commit`/`seal` and `key status` must not drift, so there is one computation and two
/// readers. This is it. The signing path calls [`read_seed`], which is a thin turn of a not-usable
/// status into the refusal message it has always printed; `key status` renders the same status
/// without signing anything. A change to the rule changes both, or neither.
pub(crate) struct KeyStatus {
    pub(crate) role: Role,
    pub(crate) source: SeedSource,
    pub(crate) path: PathBuf,
    pub(crate) key_id: String,
    /// `true` when `PRIKK_<ROLE>_KEY_ID` supplied it, `false` when it defaulted to the role's name.
    pub(crate) key_id_from_environment: bool,
    /// `Ok` with the seed when usable; `Err` with the reason when not.
    pub(crate) seed: std::result::Result<[u8; prikk_crypto::ED25519_KEY_LEN], Unusable>,
}

impl KeyStatus {
    pub(crate) fn usable(&self) -> bool {
        self.seed.is_ok()
    }

    pub(crate) fn public_key_hex(&self) -> Option<String> {
        self.seed.as_ref().ok().map(|seed| {
            prikk_hash::to_hex(&prikk_crypto::Ed25519KeyPair::from_seed(seed).public_key_bytes())
        })
    }
}

/// Answer "what key material does this role have, and can it sign?" — reading, never refusing.
///
/// Every check the signing path applies lives here: the override-versus-default resolution, the Unix
/// mode rule, the decode. The only thing it does *not* do is turn a negative answer into an error,
/// because the whole point of RFC 150 is a command that can answer in exactly the states where
/// signing cannot.
pub(crate) fn status(role: Role) -> std::result::Result<KeyStatus, CliError> {
    let override_path = non_empty_var(role.seed_file_var());
    let (source, path) = match override_path {
        Some(path) => (SeedSource::Override, PathBuf::from(path)),
        None => (
            SeedSource::KeyDirectory,
            default_key_dir()?.join(role.seed_file_name()),
        ),
    };
    let (key_id, key_id_from_environment) = match std::env::var(role.key_id_var()) {
        Ok(value) if value.trim().is_empty() => {
            return Err(CliError::Usage(format!(
                "{} must not be empty",
                role.key_id_var()
            )));
        }
        Ok(value) => (value, true),
        Err(_) => (role.default_key_id().to_string(), false),
    };

    let seed = read_seed_at(&path, source);
    Ok(KeyStatus {
        role,
        source,
        path,
        key_id,
        key_id_from_environment,
        seed,
    })
}

fn read_seed_at(
    path: &Path,
    source: SeedSource,
) -> std::result::Result<[u8; prikk_crypto::ED25519_KEY_LEN], Unusable> {
    if !path.exists() {
        return Err(match source {
            SeedSource::Override => Unusable::OverrideMissing,
            SeedSource::KeyDirectory => Unusable::Missing,
        });
    }
    if let Some(mode) = group_or_other_readable_mode(path) {
        return Err(Unusable::ReadableByOthers { mode });
    }
    let contents = std::fs::read_to_string(path).map_err(|err| Unusable::Undecodable {
        detail: format!("cannot read {}: {err}", path.display()),
    })?;
    crate::decode_seed_hex(contents.trim(), &path.display().to_string())
        .map_err(|detail| Unusable::Undecodable { detail })
}

/// The mode, when group or other can read this file. Unix only: there is a mode to check.
///
/// On Windows the key directory relies on `%APPDATA%`'s per-user ACL (see [`default_key_dir`]) and an
/// arbitrary `--*-seed-out` path is refused outright by `key.rs`, so there is no
/// silent-inherited-permissions case left to check for.
#[cfg(unix)]
fn group_or_other_readable_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
fn group_or_other_readable_mode(_path: &Path) -> Option<u32> {
    None
}

/// Read this role's seed for **signing**: turn [`status`]'s answer into the refusal the signing path
/// has always printed.
///
/// RFC 150 §1: this is the thin half. Every rule it enforces is computed in [`status`], so
/// `key status` cannot say "usable" where `commit` refuses, or the reverse —
/// `commit_and_key_status_agree_on_every_state` perturbs the shared query to prove it.
pub(crate) fn read_seed(
    role: Role,
) -> std::result::Result<[u8; prikk_crypto::ED25519_KEY_LEN], CliError> {
    let status = status(role)?;
    match &status.seed {
        Ok(seed) => Ok(*seed),
        Err(reason) => Err(refusal_for(&status, reason)),
    }
}

/// The message the signing path prints for a not-usable status. Unchanged wording from RFC 148.
fn refusal_for(status: &KeyStatus, reason: &Unusable) -> CliError {
    let shown = status.path.display().to_string();
    match reason {
        Unusable::Missing | Unusable::OverrideMissing => CliError::Failure(format!(
            "{} signing is required: no seed at {shown}. Create one with `prikk key generate --out \
             {shown}`, run `prikk setup` in a new project directory, or set {} to an existing seed \
             file",
            status.role.label(),
            status.role.seed_file_var()
        )),
        Unusable::ReadableByOthers { mode } => CliError::Failure(format!(
            "{shown} is readable by group or other (mode {mode:04o}); run `chmod 600 {shown}`"
        )),
        Unusable::Undecodable { detail } => CliError::Failure(detail.clone()),
    }
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
