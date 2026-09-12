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
        Some(other) => Err(CliError::Usage(format!(
            "unknown key subcommand: {other} (expected generate or public)"
        ))),
        None => Err(CliError::Usage(
            "usage: prikk key generate [--out <path>]\n       \
             prikk key public [--seed-file <path>] [--role author|maintainer]"
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
            write_seed_to_path(&seed, &path)?;
            println!("wrote seed to {} (mode 0600)", path.display());
            println!("public key: {public_key_hex}");
            println!();
            println!("next steps:");
            println!(
                "  prikk trust maintainer add --key-id maintainer --public-key {public_key_hex}"
            );
            // RFC 148: a seed reaches prikk through a file. If this one landed in the key
            // directory it is already found automatically; anywhere else needs the `_FILE` line,
            // and saying which of the two is the case is more useful than printing a line the
            // reader may not need.
            match crate::key_material::default_key_dir() {
                Ok(dir) if path.parent() == Some(dir.as_path()) => {
                    println!("this seed is in your key directory -- prikk finds it automatically");
                }
                _ => {
                    println!("  export PRIKK_MAINTAINER_SEED_FILE=\"{}\"", path.display());
                }
            }
            println!(
                "note: the same seed works as an AUTHOR key instead -- name it author.seed (or set \
                 PRIKK_AUTHOR_SEED_FILE) and skip the trust step"
            );
        }
        None => {
            let seed_hex = prikk_hash::to_hex(&seed);
            println!("seed: {seed_hex}");
            println!("note: this seed is now in your terminal scrollback -- treat it as a secret");
            println!("public key: {public_key_hex}");
            println!();
            println!("next steps:");
            println!(
                "  prikk trust maintainer add --key-id maintainer --public-key {public_key_hex}"
            );
            // RFC 148: there is no longer a variable to paste this into. Save it to a file -- the
            // key directory's own name if you want prikk to find it without being told.
            match crate::key_material::default_key_dir() {
                Ok(dir) => println!(
                    "  save this seed as {} (mode 0600), or re-run with --out <path>",
                    dir.join("maintainer.seed").display()
                ),
                Err(_) => println!("  save this seed to a file (mode 0600), or re-run with --out"),
            }
            println!(
                "note: the same seed works as an AUTHOR key instead -- name it author.seed and \
                 skip the trust step"
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
    let path = match seed_file {
        Some(path) => std::path::PathBuf::from(path),
        None => crate::key_material::seed_path(role.unwrap_or(Role::Author))?,
    };
    let seed = crate::read_seed_file(&path)?;
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
    write_seed_to_path_platform(seed, path)
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
/// On Unix this is `write_seed_to_path`'s own `0600` create-new, unchanged — including its refusal
/// to overwrite, so a second `prikk setup` cannot quietly replace a key you are still using.
pub(crate) fn write_seed_to_key_dir(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    write_seed_into_key_dir_platform(seed, path)
}

#[cfg(unix)]
fn write_seed_into_key_dir_platform(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    write_seed_to_path_platform(seed, path)
}

#[cfg(not(unix))]
fn write_seed_into_key_dir_platform(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                CliError::Failure(format!(
                    "refusing to overwrite an existing file: {}",
                    path.display()
                ))
            } else {
                CliError::Failure(format!("failed to create {}: {err}", path.display()))
            }
        })?;
    let seed_hex = prikk_hash::to_hex(seed);
    file.write_all(seed_hex.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|err| CliError::Failure(format!("failed to write {}: {err}", path.display())))?;
    Ok(())
}

#[cfg(windows)]
fn write_seed_to_path_platform(
    _seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    _path: &Path,
) -> std::result::Result<(), CliError> {
    Err(CliError::Failure(
        "writing a seed to a file is not yet supported on Windows -- Unix file permissions \
         (mode 0600) have no portable equivalent here without unsafe code or a new dependency, \
         and this project refuses to write a secret at inherited permissions silently. Run \
         `prikk key generate` without --out, then save the printed seed yourself."
            .to_string(),
    ))
}

#[cfg(unix)]
fn write_seed_to_path_platform(
    seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    path: &Path,
) -> std::result::Result<(), CliError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                CliError::Failure(format!(
                    "refusing to overwrite an existing file: {}",
                    path.display()
                ))
            } else {
                CliError::Failure(format!("failed to create {}: {err}", path.display()))
            }
        })?;
    let seed_hex = prikk_hash::to_hex(seed);
    file.write_all(seed_hex.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|err| CliError::Failure(format!("failed to write {}: {err}", path.display())))?;
    Ok(())
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
