//! Nonblocking final-entry opens and same-handle validation.

use std::path::Path;

use prikk_error::{PrikkError, Result};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::{failpoints, io_error};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fd::{AsFd, OwnedFd};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fs::{self, FileType, Mode, OFlags};

/// Platform-neutral path decomposition, shared by every backend that resolves a relative mutation
/// path one component at a time (Unix's fd-anchored walk, Windows' path-anchored walk).
pub(super) fn required_parent(path: &Path) -> Result<&Path> {
    path.parent().ok_or_else(|| PrikkError::Io {
        kind: None,
        context: "relative mutation path has no parent".to_string(),
    })
}

/// See `required_parent`.
pub(super) fn required_file_name(path: &Path) -> Result<&std::ffi::OsStr> {
    path.file_name().ok_or_else(|| PrikkError::Io {
        kind: None,
        context: "relative mutation path has no file name".to_string(),
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_new_regular(
    directory: impl AsFd,
    name: &std::ffi::OsStr,
) -> rustix::io::Result<OwnedFd> {
    failpoints::required_open().map_err(|_| rustix::io::Errno::IO)?;
    fs::openat(
        directory,
        name,
        OFlags::WRONLY
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::NONBLOCK
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_existing_regular(
    directory: impl AsFd,
    name: &std::ffi::OsStr,
    flags: OFlags,
) -> Result<OwnedFd> {
    failpoints::required_open()?;
    let fd = fs::openat(
        directory,
        name,
        flags | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io_error)?;
    validate_regular(&fd)?;
    Ok(fd)
}

/// RFC 102 Stage 5, design-v1.md §14.3/§14.5: requires an existing file rather than falling back to
/// creation on `EEXIST`, matching `open_existing_regular`'s own use by `durable_truncate`
/// (`anchored/linux.rs:63-64`). A create-on-append is a new-directory-entry event on every append --
/// harmless on Linux (a wasted `EEXIST` syscall) but the exact gap this RFC exists to close on
/// Windows, and it silently repaired an interrupted `init` (RFC's own §14.2 finding) into an
/// undetectable one. Safe with zero behavior change on every path that exists today: every production
/// `append_file_required` caller targets a name `RepositoryLayout::init` already created via
/// `create_empty_file_once`, which routes through `create_new_file_required` -- a separate primitive
/// that never calls this function -- so nothing here has ever depended on the fallback for
/// correctness.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_append_regular(directory: impl AsFd, name: &std::ffi::OsStr) -> Result<OwnedFd> {
    open_existing_regular(directory, name, OFlags::WRONLY | OFlags::APPEND)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn validate_regular(fd: impl AsFd) -> Result<()> {
    let stat = fs::fstat(fd).map_err(io_error)?;
    if FileType::from_raw_mode(stat.st_mode).is_file() {
        return Ok(());
    }
    Err(PrikkError::Integrity(
        "mutation target is not a regular file".to_string(),
    ))
}
