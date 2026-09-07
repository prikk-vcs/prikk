//! Stale-lock recovery (RFC 102 Stage 6 Step 2, design-v1.md §15.7 decision 3, handoff §4):
//! `lock.rs::lock_body`'s own text says it outright -- *"note=PR-007 lock has no stale-lock stealing
//! yet"* -- and a lock file surviving a crash wedges that lock permanently. `doctor.rs:405`'s own
//! repair path acquires `ActiveLock`, so the tool meant to repair the repository was blocked by the
//! very thing needing repair, with no recovery until this module.
//!
//! **Rejected: PID-based auto-stealing.** The tempting design is "if the recorded `pid=` isn't
//! running, the lock is safe to steal automatically." The two failure directions are not symmetric:
//!
//! - **False negative** (stale lock, liveness check says "still running"): no worse than today's
//!   permanent wedge. Annoying, not dangerous.
//! - **False positive** (lock genuinely held, liveness check wrongly says "not running", auto-steals
//!   it): **two writers now believe they hold exclusive access to the same container simultaneously**
//!   -- the exact race Step 2 exists to close, reintroduced by the mechanism meant to keep the
//!   repository usable. PID reuse after a reboot, and PID-namespace isolation across containers (a
//!   process id meaningful inside one container's namespace is not the same number space the host or
//!   a different container sees), both make this a real, not theoretical, failure path for a tool
//!   whose deployment context includes CI containers.
//!
//! An auto-stealing mechanism whose failure mode is silent data corruption, built to fix a failure
//! mode that is merely inconvenient, is the wrong trade.
//!
//! **So this module never removes a lock on its own.** `list_held_locks` only enumerates and reports;
//! `clear_lock` only removes the exact path it is given, once. Prompting for confirmation and deciding
//! whether the `--yes`/`--force` scripting escape applies are `prikk unlock`'s own job (`prikk-cli`),
//! not this module's -- keeping the decision in the caller that can actually see a terminal keeps this
//! module a pure, easily-tested primitive.
//!
//! **The liveness check is advisory, and the asymmetry in how it is trusted is the whole point:** a
//! *positive* result (`AppearsRunning`) is reliable evidence to refuse -- `kill(pid, 0)` succeeding, or
//! failing with `EPERM`, both mean the process genuinely exists. A *negative* result
//! (`DoesNotAppearRunning`) is **not** evidence the lock is safe to clear, for the PID-reuse/namespace
//! reasons above -- it is information for the operator, never authorization for the tool.

use std::path::{Path, PathBuf};

use prikk_error::Result;

use crate::foundation::fsutil::{
    EntryKind, list_directory_tolerating_absence, read_file_if_exists, remove_file_required,
};
use crate::foundation::layout::{LockableContainer, RepositoryLayout};

/// Best-effort, advisory-only liveness of a lock's recorded `pid=`. See the module doc for why a
/// negative or unknown result must never be treated as authorization to clear the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidLiveness {
    /// The recorded PID genuinely exists on this host right now -- reliable: refuse to clear.
    AppearsRunning,
    /// The recorded PID does not appear to exist on this host -- **not proof it is safe to clear**
    /// (PID reuse, container namespace isolation both make "not found here" compatible with "still
    /// running somewhere that matters").
    DoesNotAppearRunning,
    /// The check could not be performed (unparseable `pid=` value, or no liveness primitive on this
    /// platform).
    Unknown,
}

/// One lock file found on disk, parsed from its own body (`lock.rs::lock_body`'s format:
/// `pid=<n>\nkind=<k>\nnote=...\n`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldLock {
    /// The lock file's own path -- pass this to `clear_lock` to remove it.
    pub path: PathBuf,
    /// The `kind=` field recorded in the lock body (`"active"`, `"ref"`, or one of
    /// `lock::container_lock_kind`'s strings).
    pub kind: String,
    /// The `pid=` field, if the body parsed cleanly.
    pub recorded_pid: Option<u32>,
    /// Best-effort, advisory liveness of `recorded_pid` -- see `PidLiveness`'s own doc.
    pub liveness: PidLiveness,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn check_pid_liveness(pid: u32) -> PidLiveness {
    let Ok(raw) = i32::try_from(pid) else {
        return PidLiveness::Unknown;
    };
    let Some(rustix_pid) = rustix::process::Pid::from_raw(raw) else {
        return PidLiveness::Unknown;
    };
    match rustix::process::test_kill_process(rustix_pid) {
        Ok(()) => PidLiveness::AppearsRunning,
        // `EPERM` means the kernel found a process to check permissions against -- it exists, this
        // caller simply cannot signal it. That is still existence, not absence.
        Err(rustix::io::Errno::PERM) => PidLiveness::AppearsRunning,
        Err(rustix::io::Errno::SRCH) => PidLiveness::DoesNotAppearRunning,
        Err(_) => PidLiveness::Unknown,
    }
}

/// DC-99: mirrors the Unix implementation's own reasoning, including its subtlety --
/// `prikk_ffi::ProcessLiveness::Exists` covers both a confirmed-running handle and the
/// access-denied case (`ERROR_ACCESS_DENIED`, the kernel found a process to check permissions
/// against), the same two situations Linux/macOS's `Ok(())`/`EPERM` arms both map to
/// `AppearsRunning`.
#[cfg(target_os = "windows")]
fn check_pid_liveness(pid: u32) -> PidLiveness {
    match prikk_ffi::process_liveness(pid) {
        prikk_ffi::ProcessLiveness::Exists => PidLiveness::AppearsRunning,
        prikk_ffi::ProcessLiveness::DoesNotExist => PidLiveness::DoesNotAppearRunning,
        prikk_ffi::ProcessLiveness::Indeterminate => PidLiveness::Unknown,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn check_pid_liveness(_pid: u32) -> PidLiveness {
    PidLiveness::Unknown
}

fn parse_lock_body(bytes: &[u8]) -> (String, Option<u32>) {
    let body = String::from_utf8_lossy(bytes);
    let mut kind = String::new();
    let mut recorded_pid = None;
    for line in body.lines() {
        if let Some(value) = line.strip_prefix("kind=") {
            kind = value.to_string();
        } else if let Some(value) = line.strip_prefix("pid=") {
            recorded_pid = value.parse::<u32>().ok();
        }
    }
    (kind, recorded_pid)
}

fn read_lock_if_present(layout: &RepositoryLayout, path: &Path) -> Result<Option<HeldLock>> {
    let relative = layout.repository_relative(path)?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(None);
    };
    let (kind, recorded_pid) = parse_lock_body(&bytes);
    let liveness = recorded_pid.map_or(PidLiveness::Unknown, check_pid_liveness);
    Ok(Some(HeldLock {
        path: path.to_path_buf(),
        kind,
        recorded_pid,
        liveness,
    }))
}

/// Enumerate every lock file currently present: every active session's lock, every per-ref lock,
/// and every one of the four container locks. Read-only -- never clears anything, matching the
/// module's own "enumerate and report, never decide" split.
///
/// RFC 108 increment 2: previously read one hardcoded active-session lock. `layout.rs`'s own
/// enumeration is what makes this plural now, so a second active session -- still not creatable by
/// anything today -- is found rather than silently missed the day something does create one.
///
/// **Tolerates a missing `refs/locks` directory rather than erroring** (recovery-listing-tolerance
/// follow-up): this is a recovery surface, run precisely when a repository may not be fully valid,
/// and `refs/locks` being absent used to make this whole function -- and `prikk unlock` with it --
/// fail outright, defeating the one command that exists to clear a wedged lock. The absence itself
/// is reported by `doctor`, not here (`doctor_repository`'s own sweep over
/// `RepositoryLayout::required_directories`), so this function stays a pure listing rather than
/// growing a second, ad hoc way to say "something is missing."
pub fn list_held_locks(layout: &RepositoryLayout) -> Result<Vec<HeldLock>> {
    let mut locks = Vec::new();

    for name in layout.active_session_names()? {
        if let Some(lock) = read_lock_if_present(layout, &layout.active_lock_path(&name))? {
            locks.push(lock);
        }
    }

    let ref_locks_dir = layout.refs_dir().join("locks");
    let ref_locks_relative = layout.repository_relative(&ref_locks_dir)?;
    if let Some(entries) = list_directory_tolerating_absence(
        layout.repository_mutation_root(),
        &ref_locks_dir,
        &ref_locks_relative,
        "the per-ref lock directory",
    )? {
        for entry in entries {
            if entry.kind != EntryKind::Regular {
                continue;
            }
            let path = ref_locks_dir.join(&entry.name);
            if let Some(lock) = read_lock_if_present(layout, &path)? {
                locks.push(lock);
            }
        }
    }

    for container in LockableContainer::ALL {
        if let Some(lock) =
            read_lock_if_present(layout, &layout.lockable_container_lock_path(container))?
        {
            locks.push(lock);
        }
    }

    Ok(locks)
}

/// Find the held lock naming the same file as `target`, resolving both sides through the filesystem
/// before comparing. A path reached through a different-but-equivalent route -- a symlinked temp
/// directory (every macOS `/tmp`/`/var` path), a symlinked home, a symlinked mount -- must still match
/// the lock `list_held_locks` itself reports; exact string equality alone silently misses these,
/// telling an operator with a genuinely wedged repository and a genuinely correct path that "no held
/// lock" exists (the CI run that found this: a real lock, a real matching path, reported absent,
/// because `HeldLock::path` is built from an OS-resolved root while an independently-typed `--lock`
/// argument is not).
///
/// Falls back to plain path equality if either side fails to resolve (`std::fs::canonicalize` errors
/// on a path that does not exist): a target that names nothing real is exactly the "not currently
/// held" case this function must still express as `None`, not an I/O error -- the no-match branch is
/// precisely where the target may be bogus. `lock.path` is resolved defensively here too, even though
/// it is already well-formed by construction today (`list_held_locks` builds it from an
/// OS-resolved root) -- that is an invariant of the current call path, not a guarantee this function
/// should assume holds forever.
///
/// `print_locks` (`prikk-cli`) already emits every `HeldLock::path` in its resolved form, so an
/// operator who copies a path straight from a `prikk unlock` listing always gets a working `--lock`
/// argument -- this defect only ever bit a path an operator (or a test) constructed independently.
#[must_use]
pub fn find_held_lock<'a>(locks: &'a [HeldLock], target: &Path) -> Option<&'a HeldLock> {
    locks
        .iter()
        .find(|lock| paths_name_the_same_file(&lock.path, target))
}

fn paths_name_the_same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Clear one specific lock file by path. The caller (`prikk unlock`) is responsible for obtaining
/// operator confirmation before calling this -- this function performs no confirmation, no liveness
/// check, and no safety gate of its own: by the time it is called, the decision has already been made
/// by a human who read `list_held_locks`'s own advisory. Removing a lock that is still genuinely held
/// lets two writers race the container it names -- that is the whole risk this module exists to keep
/// an operator, not a heuristic, deciding.
pub fn clear_lock(layout: &RepositoryLayout, path: &Path) -> Result<()> {
    let relative = layout.repository_relative(path)?;
    remove_file_required(layout.repository_mutation_root(), &relative)
}

#[cfg(test)]
mod tests;
