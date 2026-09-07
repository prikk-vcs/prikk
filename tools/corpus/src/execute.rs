//! RFC 139 increment 2's executor (handoff §1, §7): drives a real `prikk` binary to turn an
//! [`crate::plan::ActionManifest`] into a real, throwaway repository -- the same CLI surface a user
//! would drive (`init`, `commit`, `trust maintainer add`, `seal`), never `prikk-store` directly, per
//! RFC 139 §7's own requirement.
//!
//! **Takes the binary path explicitly** rather than searching `PATH` or guessing
//! `target/<profile>/prikk` (handoff §2.1): a corpus built by one binary is not comparable to one
//! built by another, and RFC 139 §4's provenance discipline requires recording which binary built a
//! repository -- only possible from outside the crate that declares the binary if the path is passed
//! in, since `env!("CARGO_BIN_EXE_prikk")` is defined only for `crates/prikk-cli`'s own test/bench
//! targets (this crate is not one).
//!
//! Deliberately low-level: this module exposes one primitive per CLI invocation
//! ([`init_repository`], [`run_commit`], [`trust_maintainer`], [`run_seal`]) plus
//! [`materialize_commit`] for the pure filesystem side, rather than one all-in-one "build everything"
//! function. RFC 139 §6's build-cost curve needs to time each step separately; a monolithic function
//! would have to grow a timing-callback parameter to support that, which is more machinery than
//! exposing the steps directly. [`build`] is a thin, untimed convenience over all of them for callers
//! (the correctness controls) that only care about the resulting repository, not the timing.

use std::fmt;
use std::path::Path;
use std::process::{Command, Output};

use prikk_store::MaintainerSigner;

use crate::plan::{ActionManifest, PlannedAction, PlannedCommit};
use crate::profile::Profile;
use crate::rng::generate_bytes;

/// The ref every planned commit and seal targets. RFC 139's builder models one linear history, not
/// branching -- nothing in the RFC or its increments asks for more, and a single ref is what keeps
/// "sealed block count" and "commit count" the same number throughout this crate.
pub const REF_NAME: &str = "heads/main";

/// A `prikk` binary's identity, recorded once per build (handoff §2.1, RFC 139 §4's provenance
/// discipline): a corpus built by one binary is not comparable to one built by another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryIdentity {
    /// The path the caller supplied, as given.
    pub path: String,
    /// `prikk --version`'s stdout, trimmed.
    pub version_output: String,
    /// Hex SHA-256 of the binary file's own bytes -- costs one read, and is the strongest identity
    /// this can record (handoff §2.1: "prefer its SHA-256, which costs one read").
    pub sha256: String,
}

/// Why driving the CLI or the filesystem failed.
#[derive(Debug)]
pub enum ExecuteError {
    /// An I/O operation failed.
    Io(String),
    /// A `prikk` invocation exited non-zero.
    CommandFailed {
        /// What was being attempted.
        what: String,
        /// The process's exit code, if any.
        status: Option<i32>,
        /// Captured stderr.
        stderr: String,
    },
    /// Deriving the fixed maintainer signer from the profile's seed failed.
    Signing(String),
    /// A profile's hex-encoded field is not well-formed hex of the expected length.
    HexDecode {
        /// Which field.
        field: &'static str,
    },
    /// Regenerated content did not hash to the value the manifest recorded for it -- an internal
    /// consistency check, not something a correct planner/executor pair should ever trigger.
    ContentMismatch {
        /// The path whose content mismatched.
        path: String,
    },
}

impl fmt::Display for ExecuteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "io error: {message}"),
            Self::CommandFailed {
                what,
                status,
                stderr,
            } => write!(f, "{what} failed (status {status:?}): {stderr}"),
            Self::Signing(message) => write!(f, "signing error: {message}"),
            Self::HexDecode { field } => write!(f, "{field}: not well-formed 64-char hex"),
            Self::ContentMismatch { path } => {
                write!(
                    f,
                    "{path}: regenerated content did not match its recorded hash"
                )
            }
        }
    }
}

impl std::error::Error for ExecuteError {}

impl From<std::io::Error> for ExecuteError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

fn require_success(output: &Output, what: &str) -> Result<(), ExecuteError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(ExecuteError::CommandFailed {
            what: what.to_owned(),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Record `binary_path`'s identity: its reported version and the SHA-256 of its own file bytes.
pub fn binary_identity(binary_path: &Path) -> Result<BinaryIdentity, ExecuteError> {
    let output = Command::new(binary_path).arg("--version").output()?;
    require_success(&output, "--version")?;
    let version_output = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let bytes = std::fs::read(binary_path)?;
    let sha256 = prikk_hash::to_hex(&prikk_hash::sha256(&bytes));
    Ok(BinaryIdentity {
        path: binary_path.display().to_string(),
        version_output,
        sha256,
    })
}

/// `prikk init` a fresh repository at `repo_root`, creating the directory first.
pub fn init_repository(binary_path: &Path, repo_root: &Path) -> Result<(), ExecuteError> {
    std::fs::create_dir_all(repo_root)?;
    let output = Command::new(binary_path)
        .current_dir(repo_root)
        .arg("init")
        .output()?;
    require_success(&output, "init")
}

/// Apply one planned commit's filesystem actions to `repo_root`'s worktree -- writes, appends, and
/// deletes, in order. Does not invoke `prikk` itself; call [`run_commit`] afterward.
pub fn materialize_commit(repo_root: &Path, commit: &PlannedCommit) -> Result<(), ExecuteError> {
    for action in &commit.actions {
        match action {
            PlannedAction::CreateFile {
                path,
                size_bytes,
                content_seed,
                content_sha256,
            } => {
                let bytes = generate_bytes(*content_seed, *size_bytes);
                verify_content(content_sha256, &bytes, path)?;
                let full_path = repo_root.join(path);
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&full_path, &bytes)?;
            }
            PlannedAction::EditText {
                path,
                append_bytes,
                content_seed,
                content_sha256,
            } => {
                let appended = generate_bytes(*content_seed, *append_bytes);
                verify_content(content_sha256, &appended, path)?;
                let full_path = repo_root.join(path);
                let mut existing = std::fs::read(&full_path)?;
                existing.extend_from_slice(&appended);
                std::fs::write(&full_path, &existing)?;
            }
            PlannedAction::DeleteNode { path } => {
                std::fs::remove_file(repo_root.join(path))?;
            }
        }
    }
    Ok(())
}

fn verify_content(expected_hex: &str, bytes: &[u8], path: &str) -> Result<(), ExecuteError> {
    let actual_hex = prikk_hash::to_hex(&prikk_hash::sha256(bytes));
    if actual_hex == expected_hex {
        Ok(())
    } else {
        Err(ExecuteError::ContentMismatch {
            path: path.to_owned(),
        })
    }
}

/// Build (without running) a `prikk commit --ref <ref_name> -m <message>` command, authoring with
/// the profile's fixed author key. Exposed so callers that need non-`.output()` execution (RFC 139
/// §6's peak-memory pass, which must `.spawn()` and poll) can build the exact same command
/// [`run_commit`] would run, rather than duplicating its env/arg construction.
pub fn commit_command(
    binary_path: &Path,
    repo_root: &Path,
    profile: &Profile,
    ref_name: &str,
    message: &str,
) -> Command {
    let mut command = Command::new(binary_path);
    command
        .current_dir(repo_root)
        .env("PRIKK_AUTHOR_KEY_ID", &profile.builder_inputs.author_key_id)
        .env("PRIKK_AUTHOR_SEED", &profile.builder_inputs.author_seed_hex)
        .args(["commit", "--ref", ref_name, "-m", message]);
    command
}

/// Run `prikk commit --ref <ref_name> -m <message>`, authoring with the profile's fixed author key.
pub fn run_commit(
    binary_path: &Path,
    repo_root: &Path,
    profile: &Profile,
    ref_name: &str,
    message: &str,
) -> Result<Output, ExecuteError> {
    let output = commit_command(binary_path, repo_root, profile, ref_name, message).output()?;
    require_success(&output, "commit")?;
    Ok(output)
}

/// Trust the profile's fixed maintainer key. Idempotent-enough for repeated calls within one build,
/// matching `tests/support/mod.rs`'s own precedent -- only the seal itself must succeed.
pub fn trust_maintainer(
    binary_path: &Path,
    repo_root: &Path,
    profile: &Profile,
) -> Result<(), ExecuteError> {
    let seed = decode_hex_32(
        &profile.builder_inputs.maintainer_seed_hex,
        "maintainer_seed_hex",
    )?;
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed(
        profile.builder_inputs.maintainer_key_id.clone(),
        &seed,
    )
    .map_err(|err| ExecuteError::Signing(err.to_string()))?;
    let public_key_hex = prikk_hash::to_hex(&signer.public_key_bytes());
    let output = Command::new(binary_path)
        .current_dir(repo_root)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            &profile.builder_inputs.maintainer_key_id,
            "--public-key",
            &public_key_hex,
        ])
        .output()?;
    let _ = require_success(&output, "trust maintainer add");
    Ok(())
}

/// Run `prikk seal --allow-no-audit --ref <ref_name>`, using the profile's fixed maintainer key.
pub fn run_seal(
    binary_path: &Path,
    repo_root: &Path,
    profile: &Profile,
    ref_name: &str,
) -> Result<Output, ExecuteError> {
    let output = Command::new(binary_path)
        .current_dir(repo_root)
        .env(
            "PRIKK_MAINTAINER_KEY_ID",
            &profile.builder_inputs.maintainer_key_id,
        )
        .env(
            "PRIKK_MAINTAINER_SEED",
            &profile.builder_inputs.maintainer_seed_hex,
        )
        .args(["seal", "--allow-no-audit", "--ref", ref_name])
        .output()?;
    require_success(&output, "seal")?;
    Ok(output)
}

/// Publish a new local branch ref `name` targeting `from_ref`'s **current** block (RFC 139
/// increment 3, handoff §2.3: the corpus builder gains this so a measurement can produce a real
/// divergence -- `branch create --from` points the new branch at a block that already exists,
/// unlike an ordinary `commit`+`seal` on a fresh ref name, which would instead mint an unrelated
/// genesis). Requires the profile's fixed maintainer key, like [`run_seal`]; trusts it first, same
/// idempotent-enough precedent as [`trust_maintainer`].
pub fn branch_create(
    binary_path: &Path,
    repo_root: &Path,
    profile: &Profile,
    name: &str,
    from_ref: &str,
) -> Result<(), ExecuteError> {
    trust_maintainer(binary_path, repo_root, profile)?;
    let output = Command::new(binary_path)
        .current_dir(repo_root)
        .env(
            "PRIKK_MAINTAINER_KEY_ID",
            &profile.builder_inputs.maintainer_key_id,
        )
        .env(
            "PRIKK_MAINTAINER_SEED",
            &profile.builder_inputs.maintainer_seed_hex,
        )
        .args(["branch", "create", name, "--from", from_ref])
        .output()?;
    require_success(&output, "branch create")
}

/// Run `prikk checkout --patch-plan --ref <ref_name>` against `repo_root` (a directory that
/// already contains a `.prikk`, and nothing else -- planning does not need a worktree present).
/// RFC 139 increment 3 §2.2: read-only, so safe to run repeatedly against the same directory.
pub fn checkout_patch_plan(
    binary_path: &Path,
    repo_root: &Path,
    ref_name: &str,
) -> Result<Output, ExecuteError> {
    let output = Command::new(binary_path)
        .current_dir(repo_root)
        .args(["checkout", "--patch-plan", "--ref", ref_name])
        .output()?;
    require_success(&output, "checkout --patch-plan")?;
    Ok(output)
}

/// Run `prikk checkout --patch-materialize --ref <ref_name>` against `repo_root`. Writes the
/// reconstructed worktree into `repo_root` itself, alongside its `.prikk` -- callers measuring
/// repeatedly must supply a fresh `repo_root` each time (this is what RFC 136 §9 item 1 actually
/// asks about: the cost of materializing a worktree from sealed history alone, the way a fresh
/// clone would).
pub fn checkout_patch_materialize(
    binary_path: &Path,
    repo_root: &Path,
    ref_name: &str,
) -> Result<Output, ExecuteError> {
    let output = Command::new(binary_path)
        .current_dir(repo_root)
        .args(["checkout", "--patch-materialize", "--ref", ref_name])
        .output()?;
    require_success(&output, "checkout --patch-materialize")?;
    Ok(output)
}

fn decode_hex_32(hex: &str, field: &'static str) -> Result<[u8; 32], ExecuteError> {
    if hex.len() != 64 {
        return Err(ExecuteError::HexDecode { field });
    }
    let mut bytes = Vec::with_capacity(32);
    for chunk in hex.as_bytes().chunks(2) {
        let pair = std::str::from_utf8(chunk).map_err(|_| ExecuteError::HexDecode { field })?;
        let byte = u8::from_str_radix(pair, 16).map_err(|_| ExecuteError::HexDecode { field })?;
        bytes.push(byte);
    }
    bytes
        .try_into()
        .map_err(|_| ExecuteError::HexDecode { field })
}

/// Untimed convenience: `init`, then for every planned commit, materialize + `commit` + (once)
/// `trust maintainer add` + `seal`. For callers that only need the resulting repository, not
/// per-step timing -- RFC 139 §6's cost curve calls the primitives above directly instead.
pub fn build(
    binary_path: &Path,
    repo_root: &Path,
    profile: &Profile,
    manifest: &ActionManifest,
) -> Result<(), ExecuteError> {
    init_repository(binary_path, repo_root)?;
    let mut trusted = false;
    for (index, commit) in manifest.commits.iter().enumerate() {
        materialize_commit(repo_root, commit)?;
        run_commit(
            binary_path,
            repo_root,
            profile,
            REF_NAME,
            &format!("corpus commit {index}"),
        )?;
        if !trusted {
            trust_maintainer(binary_path, repo_root, profile)?;
            trusted = true;
        }
        run_seal(binary_path, repo_root, profile, REF_NAME)?;
    }
    Ok(())
}
