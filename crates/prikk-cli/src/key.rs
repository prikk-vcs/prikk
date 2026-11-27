//! `prikk key` — generate a fresh Ed25519 seed, or derive a public key from one already held
//! (RFC 135 §2). Neither subcommand needs an open repository: a visitor must be able to generate
//! a key *before* `init`.
//!
//! **The seed is never accepted on argv (RFC 135 §9.3, a ruling, not a preference).**
//! `/proc/<pid>/cmdline` is world-readable on Linux and shells record argv in history; a
//! `--seed <hex>` flag would leak key material to every process on the machine. `key public` reads
//! the seed from a *named* environment variable instead — the name is not the secret.

use std::path::{Path, PathBuf};

use crate::arg_scan::{SetOnce, flag_value, unknown_argument};
use crate::commands::CliError;
use crate::key_material::Role;
use crate::stdout::println;
use prikk_crypto::Ed25519KeyPair;

/// Dispatch `prikk key [generate|public]`.
pub fn run_key(args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("generate") => run_generate(iter.collect()),
        Some("public") => run_public(iter.collect()),
        Some("status") => crate::key_status::run_key_status(iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown key subcommand: {other} (expected generate, public or status)"
        ))),
        None => Err(CliError::Usage(
            "usage: prikk key generate [--out <path>]\n       \
             prikk key public [--seed-file <path>] [--role author|maintainer]\n       \
             prikk key status [path] [--role author|maintainer] [--format json]"
                .to_string(),
        )),
    }
}

fn run_generate(args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut out = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out" => {
                let value = flag_value(&mut iter, "key generate --out")?;
                out.set_once("--out", PathBuf::from(value))?;
            }
            other => return Err(unknown_argument("key generate", other)),
        }
    }

    let seed = Ed25519KeyPair::generate_seed().map_err(|err| err.to_string())?;
    let public_key = Ed25519KeyPair::from_seed(&seed).public_key_bytes();
    let public_key_hex = prikk_hash::to_hex(&public_key);

    match out {
        Some(path) => {
            let key_id = crate::key_material::derived_key_id(&seed);
            let key_id_file = crate::key_material::write_new_key(
                &seed,
                &path,
                || {
                    prepare_seed_path(&path)?;
                    Ok(())
                },
                || write_seed_to_path(&seed, &path),
            )?;
            println!("wrote seed to {} (mode 0600)", path.display());
            println!("key id: {key_id} (in {})", key_id_file.display());
            println!("public key: {public_key_hex}");
            println!();
            println!("next steps:");
            // RFC 148 §4: inside the key directory the *file name* is the role, so the next steps
            // are the ones that actually apply to this file. Printing the maintainer trust step
            // beside a file called `author.seed` and then telling the reader to ignore it would be
            // worse than not printing it: advice you have to un-read is not advice.
            match key_directory_role(&path) {
                Some(crate::key_material::Role::Author) => {
                    println!(
                        "  nothing -- this is your AUTHOR key, prikk finds it automatically, and \
                         an AUTHOR key needs no trust step"
                    );
                }
                Some(crate::key_material::Role::Maintainer) => {
                    println!(
                        "  prikk trust maintainer add --key-id {key_id} --public-key {public_key_hex}"
                    );
                    println!(
                        "note: prikk finds this seed automatically; the step above adopts it in one \
                         repository"
                    );
                }
                None => {
                    println!(
                        "  prikk trust maintainer add --key-id {key_id} --public-key {public_key_hex}"
                    );
                    println!("  export PRIKK_MAINTAINER_SEED_FILE=\"{}\"", path.display());
                    println!(
                        "note: the same seed works as an AUTHOR key instead -- set \
                         PRIKK_AUTHOR_SEED_FILE to it and skip the trust step"
                    );
                }
            }
        }
        None => {
            let seed_hex = prikk_hash::to_hex(&seed);
            let key_id = crate::key_material::derived_key_id(&seed);
            println!("seed: {seed_hex}");
            println!("note: this seed is now in your terminal scrollback -- treat it as a secret");
            println!("key id: {key_id}");
            println!("public key: {public_key_hex}");
            println!();
            println!("next steps:");
            // Distinct default key ids, addendum 2 (F3): `--out` writes the seed and its key-id file
            // together, so it is the route to recommend. Saving by hand needs both files; a seed saved
            // alone signs under the legacy id every installation made before 0.45.0 shares.
            println!(
                "  recommended: re-run with --out <path>, which writes the seed and its key-id file"
            );
            println!(
                "  prikk trust maintainer add --key-id {key_id} --public-key {public_key_hex}"
            );
            match crate::key_material::default_key_dir() {
                Ok(dir) => {
                    println!("  or save by hand -- both files, mode 0600:");
                    println!(
                        "    {}  (the seed above and a newline)",
                        dir.join(Role::Maintainer.seed_file_name()).display()
                    );
                    println!(
                        "    {}  (exactly {key_id} and a newline)",
                        dir.join(Role::Maintainer.key_id_file_name()).display()
                    );
                }
                Err(_) => {
                    println!(
                        "  or save by hand -- both files, mode 0600: the seed in a file, and beside \
                         it <that file>.key-id holding exactly {key_id} and a newline"
                    );
                }
            }
            println!(
                "note: a seed saved without its key-id file signs under the shared legacy id \
                 `maintainer`, which other installations may also use"
            );
            println!(
                "note: the same seed works as an AUTHOR key instead -- name the files author.seed and \
                 author.key-id and skip the trust step"
            );
        }
    }
    Ok(())
}

/// `prikk key public [--seed-file <path>] [--role author|maintainer]`.
///
/// RFC 148 replaces `--seed-env <NAME>` with `--seed-file <path>`: a seed no longer travels through
/// the environment at all, so naming a *variable* on argv has nothing to name. With no `--seed-file`
/// the role's own file in the default key directory is read, which is the common case — "what is the
/// public half of the key I already have?" should not require knowing where it lives.
fn run_public(args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut seed_file = None;
    let mut role = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--seed-file" => {
                let value = flag_value(&mut iter, "key public --seed-file")?;
                seed_file.set_once("--seed-file", value)?;
            }
            "--role" => {
                let value = flag_value(&mut iter, "key public --role")?;
                let parsed = match value.as_str() {
                    "author" => Role::Author,
                    "maintainer" => Role::Maintainer,
                    other => {
                        return Err(CliError::Usage(format!(
                            "key public --role does not support {other:?} (expected author or \
                             maintainer)"
                        )));
                    }
                };
                role.set_once("--role", parsed)?;
            }
            other => return Err(unknown_argument("key public", other)),
        }
    }
    // RFC 150 §3: with no `--seed-file`, the missing-file answer comes from the same query
    // `key status` and the signing path use, so "why can I not derive this?" has one answer
    // everywhere. An explicit `--seed-file` still reads that exact path and says so itself.
    let seed = match seed_file {
        Some(path) => crate::read_seed_file(&std::path::PathBuf::from(path))?,
        None => crate::key_material::read_seed(role.unwrap_or(Role::Author))?,
    };
    let public_key = Ed25519KeyPair::from_seed(&seed).public_key_bytes();
    println!("public key: {}", prikk_hash::to_hex(&public_key));
    Ok(())
}

/// Write `seed` to `path`: mode `0600`, refuses to overwrite an existing file, refuses any path
/// with a `.prikk` component (RFC 135 §9.2 -- prikk never invents a secret's location and never
/// manages its lifecycle, so it must not write one where it might later mistake the file for its
/// own). Shared with `prikk setup`, which writes seeds the same way.
///
/// **Windows default ruling (RFC 135 §2.1): refused.** `std::os::unix::fs::PermissionsExt` is
/// Unix-only, and an ACL-based equivalent needs Win32 FFI -- `#![forbid(unsafe_code)]` (this
/// crate) plus DC-90 (unsafe is a reviewed exception, not an import) make that its own decision.
/// Writing a secret at inherited permissions and saying nothing is not acceptable; refusing
/// outright and pointing at the print-and-place path is.
pub(crate) fn write_seed_to_path(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    prepare_seed_path(path)?;
    write_seed_to_path_platform(seed, path)
}

/// The directory a seed named by the user is written into must **exist and be a directory** (0.59.0 H2c). prikk never creates it: where a secret lives, and with
/// what mode, is the user's choice (the one directory it does create is its own key directory, [`prepare_seed_path`]). Checked **before anything is written**
/// (`setup` before it initialises the repository, `key generate` before the key-id file), so a refusal leaves nothing behind. A path inside prikk's own key
/// directory is not refused here: the key directory is created when the seed is written.
///
/// **On unix the directory must also be writable and searchable** (0.59.0 H2d: `access(dir, W_OK | X_OK)`): a directory that exists but cannot be written gave the
/// same half-run as a missing one (`initialized …`, then `Permission denied` on the key-id file). `access` judges by the **real** user id, which is the one
/// prikk runs as (prikk is never setuid). **Root passes it whatever the mode.** Windows is unchanged: it refuses a seed-out path outright.
pub(crate) fn require_seed_directory(path: &Path) -> std::result::Result<(), CliError> {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if let Ok(key_dir) = crate::key_material::default_key_dir() {
        if directory == key_dir.as_path() {
            return Ok(());
        }
    }
    match std::fs::metadata(directory) {
        Ok(metadata) if metadata.is_dir() => directory_is_writable(path, directory),
        Ok(_) => Err(CliError::Failure(format!(
            "cannot write the seed to {}: {} is not a directory; name a seed path inside a directory",
            path.display(),
            directory.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(CliError::Failure(format!(
                "cannot write the seed to {}: the directory {} does not exist; create it first (`mkdir -p {}`), \
                 because prikk does not choose where a secret lives",
                path.display(),
                directory.display(),
                directory.display()
            )))
        }
        Err(error) => Err(CliError::Failure(format!(
            "cannot write the seed to {}: {}: {error}",
            path.display(),
            directory.display()
        ))),
    }
}

/// `access(directory, W_OK | X_OK)`: refuse, with the operating system's words, a directory the seed cannot be created in.
#[cfg(unix)]
fn directory_is_writable(path: &Path, directory: &Path) -> std::result::Result<(), CliError> {
    use rustix::fs::{Access, access};
    access(directory, Access::WRITE_OK | Access::EXEC_OK).map_err(|errno| {
        CliError::Failure(format!(
            "cannot write the seed to {}: the directory {} is not writable: {}; fix its permissions (`chmod u+wx {}`) or name another directory",
            path.display(),
            directory.display(),
            std::io::Error::from(errno),
            directory.display()
        ))
    })
}

#[cfg(not(unix))]
fn directory_is_writable(_path: &Path, _directory: &Path) -> std::result::Result<(), CliError> {
    Ok(())
}

/// Every refusal [`write_seed_to_path`] can make before it writes: a `.prikk` component, the seed's directory ([`require_seed_directory`]), and on Windows the
/// outright refusal. Also creates the key directory when `path` is inside it. Run first by
/// `key_material::write_new_key`, so that a refused seed never leaves its key-id file behind.
pub(crate) fn prepare_seed_path(path: &Path) -> std::result::Result<(), CliError> {
    if path.components().any(|c| c.as_os_str() == ".prikk") {
        return Err(CliError::Usage(
            "the seed output path must not be inside .prikk/ -- prikk never manages a secret's \
             lifecycle"
                .to_string(),
        ));
    }
    // RFC 148: when the target *is* prikk's own key directory, create it (mode 0700) first. Without
    // this the route the missing-seed refusal names -- `prikk key generate --out <that path>` --
    // fails on a machine that has never run `prikk setup`, which is precisely the machine the
    // refusal is speaking to. prikk still invents no location: the user named this path, and it is
    // the one location prikk already knows about.
    if let Ok(key_dir) = crate::key_material::default_key_dir() {
        if path.parent() == Some(key_dir.as_path()) {
            crate::key_material::ensure_key_dir()?;
        }
    }
    require_seed_directory(path)?;
    windows_seed_path_refusal()
}

#[cfg(windows)]
fn windows_seed_path_refusal() -> std::result::Result<(), CliError> {
    Err(windows_seed_file_refusal())
}

#[cfg(windows)]
fn windows_seed_file_refusal() -> CliError {
    CliError::Failure(
        "writing a seed to a file is not yet supported on Windows -- Unix file permissions \
         (mode 0600) have no portable equivalent here without unsafe code or a new dependency, \
         and this project refuses to write a secret at inherited permissions silently. Run \
         `prikk key generate` without --out, then save the printed seed yourself."
            .to_string(),
    )
}

#[cfg(not(windows))]
fn windows_seed_path_refusal() -> std::result::Result<(), CliError> {
    Ok(())
}

/// Which role a path names, when it is inside the key directory — `author.seed` or
/// `maintainer.seed` — and `None` for any other location.
///
/// RFC 148 §4: inside that directory the file name *is* the role, so `key generate --out` can tell a
/// reader which of its two next steps actually applies to the file they just created rather than
/// printing both and leaving them to work it out.
fn key_directory_role(path: &Path) -> Option<crate::key_material::Role> {
    let key_dir = crate::key_material::default_key_dir().ok()?;
    if path.parent() != Some(key_dir.as_path()) {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    [
        crate::key_material::Role::Author,
        crate::key_material::Role::Maintainer,
    ]
    .into_iter()
    .find(|role| role.seed_file_name() == name)
}

/// Write a seed into prikk's own key directory (RFC 148).
///
/// Distinct from [`write_seed_to_path`] in exactly one way that matters: **it is not refused on
/// Windows.** `write_seed_to_path` refuses an arbitrary Windows path because prikk cannot set an ACL
/// without unsafe code, and writing a secret at whatever permissions the location happens to inherit
/// — silently — is not acceptable. The default key directory is the one location where the inherited
/// permissions are the *right* ones and are stated rather than assumed: `%APPDATA%` is per-user by
/// platform ACL, `key_material::default_key_dir` says so, and `first-run.md` says so to the reader.
///
/// On every platform the file is created whole or not at all and never replaces one (0.59.0 K6,
/// `key_material::publish_new_file`), so a second `prikk setup` cannot quietly replace a key you are still using, and a kill
/// cannot leave a torn seed in the key directory.
pub(crate) fn write_seed_to_key_dir(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    write_seed_into_key_dir_platform(seed, path)
}

/// The key directory's own seed file: created whole or not at all and never replacing (`key_material::publish_new_file`), on every platform.
fn write_seed_into_key_dir_platform(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    crate::key_material::publish_new_file(path, &seed_file_bytes(seed))
}

/// What a seed file holds: 64 hex characters and a newline.
fn seed_file_bytes(seed: &[u8; prikk_crypto::ED25519_KEY_LEN]) -> Vec<u8> {
    let mut bytes = prikk_hash::to_hex(seed).into_bytes();
    bytes.push(b'\n');
    bytes
}

#[cfg(windows)]
fn write_seed_to_path_platform(
    _seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    _path: &Path,
) -> std::result::Result<(), CliError> {
    Err(windows_seed_file_refusal())
}

#[cfg(unix)]
fn write_seed_to_path_platform(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    crate::key_material::publish_new_file(path, &seed_file_bytes(seed))
}

#[cfg(not(any(unix, windows)))]
fn write_seed_to_path_platform(
    _seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    _path: &Path,
) -> std::result::Result<(), CliError> {
    Err(CliError::Failure(
        "writing a seed to a file is not supported on this platform -- run `prikk key generate` \
         without --out, then save the printed seed yourself."
            .to_string(),
    ))
}
