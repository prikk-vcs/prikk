//! RFC 158 Stage A handoff §1: one shared function reads an outside artifact under a bound, used
//! by every one of the six CLI entry points that reads a file arriving from outside the repository
//! (`bundle import`/`preview`/`verify`, `sync compare`/`build`/`accept`). Before this module, each
//! site called `std::fs::read` first and let the store compare `bytes.len()` afterwards -- so the
//! refusal came after the allocation it existed to prevent (measured on 0.46.0: a sparse 1 GiB file
//! given to `bundle verify` peaked at 1,051,060 KB resident, then refused).
//!
//! **The contract** (handoff §1): open the file, then check its size from the open handle's own
//! metadata -- not from the path, so the file checked is the file read; then read through
//! `Read::take(bound + 1)` and refuse if more than `bound` bytes arrive, since metadata can lie (a
//! FIFO reports a length of 0, and a file can grow between the check and the read); initial
//! capacity is `min(declared length, bound)`, never the declared length alone. A non-regular file
//! is not refused for being non-regular -- reading a FIFO is a legitimate way to feed a bundle --
//! it simply gets no fast path, since its reported length never exceeds the bound on its own.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use prikk_error::PrikkError;

use crate::commands::CliError;

/// A byte bound paired with a human-readable description of where it came from and how to change
/// it (RFC 158 Stage A handoff §2: every size refusal must name both, and `prikk-store` has no
/// notion of an environment variable, a CLI flag, or a config file to name -- only this layer does).
pub(crate) struct SizeBound {
    pub(crate) bytes: u64,
    pub(crate) description: String,
}

impl SizeBound {
    /// DC-86's existing shape, restated for a bound whose refusal must now say where it came from:
    /// absent means the documented default; present but non-numeric or zero is a hard error, never
    /// a silent fall back.
    pub(crate) fn from_env(name: &str, default: usize) -> Result<Self, String> {
        let Ok(raw) = std::env::var(name) else {
            return Ok(Self {
                bytes: default as u64,
                description: format!(
                    "the default limit of {default} bytes ({}) -- set ${name} to raise or lower it",
                    mib(default as u64)
                ),
            });
        };
        let trimmed = raw.trim();
        let value: usize = trimmed
            .parse()
            .map_err(|_| format!("{name} must be a positive integer, got {raw:?}"))?;
        if value == 0 {
            return Err(format!("{name} must be greater than zero, got 0"));
        }
        Ok(Self {
            bytes: value as u64,
            description: format!(
                "the limit of {value} bytes ({}), set by ${name} -- change that variable to raise \
                 or lower it",
                mib(value as u64)
            ),
        })
    }

    /// A bound with no override mechanism at all -- the have-list bound, "a constant, no variable"
    /// in the handoff's own words.
    pub(crate) fn fixed(bytes: usize, what: &str) -> Self {
        Self {
            bytes: bytes as u64,
            description: format!(
                "the fixed limit of {bytes} bytes ({}) for {what} -- this bound cannot be changed",
                mib(bytes as u64)
            ),
        }
    }

    /// `--max-object-bytes` on the command itself -- highest precedence (RFC 158 Stage A §3).
    pub(crate) fn from_flag(bytes: u64) -> Self {
        Self {
            bytes,
            description: format!(
                "the limit of {bytes} bytes ({}), set by --max-object-bytes",
                mib(bytes)
            ),
        }
    }

    /// `incoming.max-object-bytes` from `.prikk/config` -- second precedence, only when a
    /// repository is open (handoff §3: `bundle verify` has none, and its refusal must not suggest
    /// `prikk config`).
    pub(crate) fn from_config(bytes: u64) -> Self {
        Self {
            bytes,
            description: format!(
                "the limit of {bytes} bytes ({}), set by incoming.max-object-bytes in `prikk config`",
                mib(bytes)
            ),
        }
    }

    /// The per-object default, named as such -- `mentions_config` is false only for `bundle
    /// verify`, which has no repository to point at.
    pub(crate) fn default_object_bound(bytes: usize, mentions_config: bool) -> Self {
        let change_it = if mentions_config {
            "pass --max-object-bytes, or set incoming.max-object-bytes in `prikk config`, to \
             change it"
        } else {
            "pass --max-object-bytes to change it"
        };
        Self {
            bytes: bytes as u64,
            description: format!(
                "the default limit of {bytes} bytes ({}) -- {change_it}",
                mib(bytes as u64)
            ),
        }
    }
}

/// `--max-object-bytes <N>`'s value, shared by every one of its four flag-parsing call sites
/// (`bundle import`/`preview`/`verify`, `sync accept`): a positive integer, never a silent floor.
pub(crate) fn parse_max_object_bytes_value(value: &str) -> Result<u64, CliError> {
    let parsed: u64 = value.parse().map_err(|_| {
        CliError::Usage(format!(
            "--max-object-bytes must be a positive integer, got {value:?}"
        ))
    })?;
    if parsed == 0 {
        return Err(CliError::Usage(
            "--max-object-bytes must be greater than zero, got 0".to_string(),
        ));
    }
    Ok(parsed)
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// The generic form of the bounded read (handoff §1's own three steps), taking an already-open
/// source and its already-known declared length so the file-opening/`metadata()` step can be
/// tested separately from the read-and-enforce step (handoff §6 control 2: "unit-tested over an
/// in-memory `Read` that counts bytes").
pub(crate) fn read_bounded<R: Read>(
    label: &str,
    display_path: &str,
    declared_len: u64,
    source: &mut R,
    bound: &SizeBound,
) -> Result<Vec<u8>, CliError> {
    if declared_len > bound.bytes {
        return Err(oversized_declared(label, display_path, declared_len, bound));
    }
    let capacity = usize::try_from(declared_len.min(bound.bytes)).unwrap_or(0);
    let mut buf = Vec::with_capacity(capacity);
    // Metadata can lie (a FIFO reports 0; a file can grow after the check above), so this is
    // enforced again here, independently -- `take(bound + 1)` never reads more than one byte past
    // the bound no matter what `source` is prepared to yield.
    source
        .take(bound.bytes + 1)
        .read_to_end(&mut buf)
        .map_err(|err| format!("failed to read {label} at {display_path}: {err}"))?;
    if buf.len() as u64 > bound.bytes {
        return Err(oversized_while_streaming(label, display_path, bound));
    }
    Ok(buf)
}

/// Open `path`, read its metadata from the open handle (not from the path itself), and read it
/// bounded by `bound` -- the file-backed instance of [`read_bounded`], and the one every one of
/// the six entry points calls.
pub(crate) fn read_bounded_file(
    label: &str,
    path: &Path,
    bound: &SizeBound,
) -> Result<Vec<u8>, CliError> {
    let mut file = File::open(path)
        .map_err(|err| format!("failed to open {label} at {}: {err}", path.display()))?;
    let declared_len = file
        .metadata()
        .map_err(|err| {
            format!(
                "failed to read metadata for {label} at {}: {err}",
                path.display()
            )
        })?
        .len();
    read_bounded(
        label,
        &path.display().to_string(),
        declared_len,
        &mut file,
        bound,
    )
}

fn oversized_declared(
    label: &str,
    display_path: &str,
    declared_len: u64,
    bound: &SizeBound,
) -> CliError {
    CliError::Failure(format!(
        "{label} at {display_path} is {declared_len} bytes, over {}",
        bound.description
    ))
}

fn oversized_while_streaming(label: &str, display_path: &str, bound: &SizeBound) -> CliError {
    CliError::Failure(format!(
        "{label} at {display_path} is over {} -- more bytes arrived while reading it than its \
         declared size said to expect",
        bound.description
    ))
}

/// RFC 158 Stage A §2: re-render a [`PrikkError::ObjectOverBound`] with `object_bound`'s own
/// description of where the bound came from and how to change it -- `prikk-store` has no notion of
/// a flag or a config file to name, so its own `Display` deliberately does not try. Every other
/// error passes through unchanged, exactly as `.to_string()` already rendered it. Shared by
/// `bundle.rs` (import/preview/verify) and `sync.rs` (accept) -- the four callers of
/// [`crate::config::resolve_max_object_bytes`].
pub(crate) fn render_incoming_error(err: PrikkError, object_bound: &SizeBound) -> CliError {
    if let PrikkError::ObjectOverBound { declared_bytes, .. } = err {
        return CliError::Failure(format!(
            "an object in this input declares {declared_bytes} bytes, over {}",
            object_bound.description
        ));
    }
    CliError::Failure(err.to_string())
}

#[cfg(test)]
mod tests;
