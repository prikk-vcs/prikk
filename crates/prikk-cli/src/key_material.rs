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

    /// The **legacy** default key id: the role word, used when `PRIKK_<ROLE>_KEY_ID` is unset and no
    /// key-id file sits beside the seed — a seed made before 0.45.0. Every such installation shares it.
    pub(crate) const fn legacy_key_id(self) -> &'static str {
        self.label()
    }

    pub(crate) const fn key_id_file_name(self) -> &'static str {
        match self {
            Role::Author => "author.key-id",
            Role::Maintainer => "maintainer.key-id",
        }
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
    /// The key-id file beside the seed does not hold the id derived from this seed: a replaced seed, a
    /// copied file, or hand-edited content. A custom id is `PRIKK_<ROLE>_KEY_ID`'s job, never the file's.
    KeyIdFileMismatch {
        path: PathBuf,
        file_id: String,
        derived_id: String,
    },
}

impl Unusable {
    /// The machine-readable reason, RFC 150 §2's own vocabulary.
    pub(crate) fn code(&self) -> String {
        match self {
            Unusable::Missing => "missing".to_string(),
            Unusable::OverrideMissing => "override-missing".to_string(),
            Unusable::ReadableByOthers { mode } => format!("readable-by-others (mode {mode:04o})"),
            Unusable::Undecodable { .. } => "undecodable".to_string(),
            Unusable::KeyIdFileMismatch { .. } => "key-id-file-mismatch".to_string(),
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
    /// Where `key_id` came from.
    pub(crate) key_id_source: KeyIdSource,
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

/// Where a key id came from (RFC 135 addendum, distinct default key ids, rule 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyIdSource {
    /// `PRIKK_<ROLE>_KEY_ID`.
    Environment,
    /// The key-id file beside the seed, at this path.
    KeyFile(PathBuf),
    /// No variable and no file: the legacy role word.
    LegacyDefault,
}

impl KeyIdSource {
    /// `key-status-v1`'s `key_id_source` value.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            KeyIdSource::Environment => "environment",
            KeyIdSource::KeyFile(_) => "key-file",
            KeyIdSource::LegacyDefault => "default",
        }
    }
}

/// The default id of a new key: `ed25519-` and the first 16 lowercase hex characters of its public key.
/// Role-neutral: one seed has one id, whichever role uses it. A name, not a security property — binding
/// one id to one public key is what enforces identity.
pub(crate) fn derived_key_id(seed: &[u8; prikk_crypto::ED25519_KEY_LEN]) -> String {
    let public_key = prikk_crypto::Ed25519KeyPair::from_seed(seed).public_key_bytes();
    let hex = prikk_hash::to_hex(&public_key);
    format!("ed25519-{}", hex.get(..16).unwrap_or(&hex))
}

/// The key-id file that belongs to the seed at `seed_path`: `<role>.key-id` for the key directory's own
/// `<role>.seed`, otherwise `<seed path>.key-id`.
pub(crate) fn key_id_path(seed_path: &Path) -> PathBuf {
    if let (Ok(key_dir), Some(name)) = (default_key_dir(), seed_path.file_name()) {
        if seed_path.parent() == Some(key_dir.as_path()) {
            for role in [Role::Author, Role::Maintainer] {
                if name == role.seed_file_name() {
                    return key_dir.join(role.key_id_file_name());
                }
            }
        }
    }
    let mut name = seed_path.as_os_str().to_owned();
    name.push(".key-id");
    PathBuf::from(name)
}

/// **The one key id resolution** (rule 3), used by every signer, by `setup` and by `key status`:
/// `PRIKK_<ROLE>_KEY_ID` if set; otherwise the key-id file beside `seed_path`; otherwise the legacy role
/// word. Returns the id, its source, and — when the file exists and the seed decodes — a mismatch if the
/// file does not hold the id derived from that seed (rule 4).
pub(crate) fn resolve_key_id(
    role: Role,
    seed_path: &Path,
    seed: &std::result::Result<[u8; prikk_crypto::ED25519_KEY_LEN], Unusable>,
) -> std::result::Result<(String, KeyIdSource, Option<Unusable>), CliError> {
    match std::env::var(role.key_id_var()) {
        Ok(value) if value.trim().is_empty() => {
            return Err(CliError::Usage(format!(
                "{} must not be empty",
                role.key_id_var()
            )));
        }
        Ok(value) => return Ok((value, KeyIdSource::Environment, None)),
        Err(_) => {}
    }
    let file = key_id_path(seed_path);
    match std::fs::read_to_string(&file) {
        Ok(contents) => {
            let file_id = contents.strip_suffix('\n').unwrap_or(&contents).to_string();
            let mismatch = match seed {
                Ok(seed) => {
                    let derived_id = derived_key_id(seed);
                    (file_id != derived_id).then(|| Unusable::KeyIdFileMismatch {
                        path: file.clone(),
                        file_id: file_id.clone(),
                        derived_id,
                    })
                }
                Err(_) => None,
            };
            Ok((file_id, KeyIdSource::KeyFile(file), mismatch))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok((
            role.legacy_key_id().to_string(),
            KeyIdSource::LegacyDefault,
            None,
        )),
        Err(err) => Err(CliError::Failure(format!(
            "cannot read {}: {err}",
            file.display()
        ))),
    }
}

/// Refuse to create a seed at `seed_path` if it, or the key-id file that would belong to it, already
/// exists — naming both paths, and before either is written. A leftover key-id file beside a new seed
/// would otherwise be refused at the first signature.
pub(crate) fn require_new_key_paths(seed_path: &Path) -> std::result::Result<(), CliError> {
    let key_id_file = key_id_path(seed_path);
    if seed_path.exists() || key_id_file.exists() {
        return Err(CliError::Failure(format!(
            "refusing to overwrite an existing file: a new seed needs both {} and {} to be absent",
            seed_path.display(),
            key_id_file.display()
        )));
    }
    Ok(())
}

/// Write the key-id file for a seed just created: the id and one trailing newline, `0600` on Unix (the
/// key directory's ACL on Windows), never overwriting.
pub(crate) fn write_key_id_file(
    seed_path: &Path,
    key_id: &str,
) -> std::result::Result<PathBuf, CliError> {
    use std::io::Write;

    let path = key_id_path(seed_path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|err| CliError::Failure(format!("failed to create {}: {err}", path.display())))?;
    file.write_all(key_id.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|err| CliError::Failure(format!("failed to write {}: {err}", path.display())))?;
    Ok(path)
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
    let seed = read_seed_at(&path, source);
    let (key_id, key_id_source, mismatch) = resolve_key_id(role, &path, &seed)?;
    let seed = match mismatch {
        Some(mismatch) => Err(mismatch),
        None => seed,
    };
    Ok(KeyStatus {
        role,
        source,
        path,
        key_id,
        key_id_source,
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
        Unusable::KeyIdFileMismatch {
            path,
            file_id,
            derived_id,
        } => CliError::Failure(format!(
            "{} signing refused: {} holds key id {file_id}, but the seed at {shown} derives \
             {derived_id} -- the file does not belong to this seed. Restore the seed it belongs to, \
             or remove the file; a custom key id is set with {}, never by editing the file",
            status.role.label(),
            path.display(),
            status.role.key_id_var()
        )),
    }
}

/// This role's key id and seed for **signing**, from one [`status`] answer — so the id a signature
/// carries is the id `key status` reports.
pub(crate) fn signing_key(
    role: Role,
) -> std::result::Result<(String, [u8; prikk_crypto::ED25519_KEY_LEN]), CliError> {
    let status = status(role)?;
    match &status.seed {
        Ok(seed) => Ok((status.key_id.clone(), *seed)),
        Err(reason) => Err(refusal_for(&status, reason)),
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
