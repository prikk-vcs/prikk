//! Atomic, durable writes to an arbitrary user-supplied output path (DC-44 increment 2,
//! `bundle-export-durability-handoff-v1.md`).
//!
//! **This is not the anchored durability contract** (`prikk_store`'s `fsutil::anchored`, DC-76/
//! DC-82). That contract is deliberately confined by a `MutationRoot`: every path it writes is
//! resolved *inside* a validated repository root, via `openat`-relative primitives, because the
//! whole design exists to keep repository mutation from ever escaping the directory it was opened
//! against. A `bundle export --output <file>` destination is deliberately outside any
//! repository — the caller names an arbitrary path, often on a different filesystem entirely — so
//! reusing the anchored primitive here would mean either forcing it to accept an unconfined root
//! (defeating its own purpose) or copying its shape without its actual guarantee. This module is
//! new, narrower machinery for a genuinely different problem: durability for one arbitrary file,
//! with no repository root to anchor against.
//!
//! **What [`write_new_file_durably`] guarantees**: a temp file is created (exclusively — this
//! fails if a same-named temp file already exists, which does not happen in practice since the
//! name embeds the process id and a nanosecond timestamp) in the *same directory* as the
//! destination, written, and `fsync`'d before the atomic rename into place — so a crash or a
//! failed write before the rename leaves the destination **completely untouched** (the previous
//! file, if any, survives; if there was no previous file, none appears), and a crash after the
//! rename leaves the **complete** new content at the destination, never a partial write. On
//! failure at any point up to the rename, the temp file is removed on a best-effort basis so
//! nothing partial is left behind under either name.
//!
//! **What it does not guarantee, stated rather than implied**: on Windows, the destination
//! directory's own metadata is not additionally synced after the rename — `std`'s only portable
//! way to open a directory as fsync-able is a Unix one (`File::open` on a directory path, then
//! `sync_all`; `std::fs::File::open` refuses a directory path on Windows). Rust's own `std::fs::
//! rename` on Windows uses `MoveFileExW` with `MOVEFILE_WRITE_THROUGH`, which the Windows API
//! documents as not returning success until the move is written through to disk — believed to
//! make this gap narrower there than on Unix, not verified on a real Windows host by this
//! increment; CI's own Windows job is what actually confirms cross-platform behavior, not this
//! comment. On Unix (Linux and macOS), the destination's parent directory *is* fsync'd after the
//! rename, propagating any failure as a hard error — matching the anchored contract's own
//! treatment of directory durability as required, not best-effort, and getting macOS's
//! `F_FULLFSYNC` upgrade automatically, since the directory is opened as an ordinary
//! `std::fs::File` (`sync_all` already special-cases Apple targets internally; only the anchored
//! contract's own `rustix`-based directory type has to do that by hand, because it never
//! constructs a `std::fs::File` for a directory at all).
//!
//! **What the collision check built on [`destination_exists`] does not guarantee**: it is a
//! plain existence check before the write begins, not an atomic no-clobber rename — this crate
//! carries no third-party dependency (confirmed by this crate's own long-standing "no
//! `serde_json` here either" rule, restated for the same reason: no `rustix`, no `libc` crate
//! either, so no `renameat2(..., RENAME_NOREPLACE)` and no portable no-clobber flag for
//! `MoveFileExW`), so a file created at the destination in the narrow window between this check
//! and the eventual rename would still be silently replaced. Acceptable for the case this guards
//! against — a human or a script re-running `bundle export` against a path it already used — not
//! a defense against a concurrent adversary racing the same path.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// True if `destination` already exists (as any kind of filesystem entry, including a broken
/// symlink — `symlink_metadata` does not follow the link, so a dangling symlink still counts as
/// "something is there" rather than reading as absent).
pub(crate) fn destination_exists(destination: &Path) -> bool {
    std::fs::symlink_metadata(destination).is_ok()
}

/// Write `bytes` to `destination`, atomically and durably, never leaving a partial file at
/// `destination` either on success or on failure. See this module's own doc comment for exactly
/// what is and is not guaranteed. Overwrites `destination` if it already exists — callers that
/// need to refuse an existing destination must check [`destination_exists`] themselves first;
/// this function's own job is the write, not the collision policy.
pub(crate) fn write_new_file_durably(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp_path = temporary_sibling_path(destination)?;
    let write_result = write_and_sync_temp_file(&temp_path, bytes);
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result?;

    // RFC 157 §7.2's killed-process control needs a failure at exactly this point -- the temporary file
    // complete on disk, the rename not yet done. The store's own failpoints cannot reach here: they are
    // injected into the *anchored* writer, which by construction only writes inside a repository root, and
    // this module exists precisely because `--output` names a path outside any repository (see the module
    // doc). Test-only, and never compiled into the shipped binary.
    #[cfg(test)]
    if tests::take_failure_before_rename() {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!(
            "failed to move the completed write into place at {}: simulated failure before the rename",
            destination.display()
        ));
    }

    std::fs::rename(&temp_path, destination).map_err(|err| {
        let _ = std::fs::remove_file(&temp_path);
        format!(
            "failed to move the completed write into place at {}: {err}",
            destination.display()
        )
    })?;

    sync_parent_directory(destination)
}

fn write_and_sync_temp_file(temp_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = File::create_new(temp_path).map_err(|err| {
        format!(
            "failed to create a temporary file beside the destination at {}: {err}",
            temp_path.display()
        )
    })?;
    file.write_all(bytes).map_err(|err| {
        format!(
            "failed to write to the temporary file at {}: {err}",
            temp_path.display()
        )
    })?;
    file.sync_all().map_err(|err| {
        format!(
            "failed to sync the temporary file at {} before moving it into place: {err}",
            temp_path.display()
        )
    })
}

/// A same-directory, effectively-unique temp path for `destination` — same directory so the later
/// rename is a same-filesystem, atomic operation, never a cross-filesystem copy. Uniqueness comes
/// from the process id plus a nanosecond timestamp, not a counter: this function is called at most
/// once per `bundle export` invocation, so the DC-83/DC-84 thread-contention hazard that made
/// `process::id()` alone insufficient for the test suite's own unique-path helper does not apply
/// here — there is no second call in the same process racing this one.
fn temporary_sibling_path(destination: &Path) -> Result<PathBuf, String> {
    let file_name = destination.file_name().ok_or_else(|| {
        format!(
            "output path {} has no file name to write",
            destination.display()
        )
    })?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| format!("system clock is before the Unix epoch: {err}"))?
        .as_nanos();
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".tmp-{}-{nanos}", std::process::id()));
    Ok(destination.with_file_name(temp_name))
}

/// Required on Unix (a failure here fails the whole write, matching the anchored contract's own
/// treatment of directory durability) -- see the `#[cfg(not(unix))]` sibling below for why this
/// is a no-op on Windows instead, not merely a weaker version of the same thing.
#[cfg(unix)]
fn sync_parent_directory(destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let directory = File::open(parent).map_err(|err| {
        format!(
            "failed to open the destination directory {} to sync it after the rename: {err}",
            parent.display()
        )
    })?;
    directory.sync_all().map_err(|err| {
        format!(
            "failed to sync the destination directory {} after the rename -- the new file is in \
             place, but its durability across a crash is not confirmed: {err}",
            parent.display()
        )
    })
}

/// Windows has no portable `std`-only way to open a directory for `fsync`
/// (`std::fs::File::open` on a directory path fails there) — see this module's own doc comment
/// for why this is believed to matter less on Windows than it would on Unix, and why it is stated
/// as a real gap rather than silently worked around.
#[cfg(not(unix))]
fn sync_parent_directory(_destination: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests;
