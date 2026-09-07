#![allow(clippy::indexing_slicing)]

use std::path::PathBuf;

use prikk_error::Result;

use super::{PidLiveness, clear_lock, find_held_lock, list_held_locks, read_lock_if_present};
use crate::RepositoryLayout;
use crate::foundation::fsutil::{EntryKind, list_directory};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, LockableContainer};
use crate::lock::{ActiveLock, RefLock, acquire_container_locks};
use crate::test_gates::test_support::unique_temp_dir;

#[test]
fn a_fresh_repository_holds_no_locks() -> Result<()> {
    let root = unique_temp_dir("unlock-empty");
    let layout = RepositoryLayout::init(root.clone())?;
    assert!(list_held_locks(&layout)?.is_empty());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Every lock kind is enumerated, and the current process's own PID (definitely alive, it is the
/// process running this test) is correctly reported as `AppearsRunning` -- the one case the advisory
/// check can prove reliably rather than merely claim, on all three platforms as of DC-99 (Linux/macOS
/// via `kill(pid, 0)`, Windows via `OpenProcess`/`WaitForSingleObject`, `prikk_ffi::process_liveness`).
#[test]
fn every_held_lock_kind_is_enumerated_with_its_own_pid_live() -> Result<()> {
    let root = unique_temp_dir("unlock-enumerate-all-kinds");
    let layout = RepositoryLayout::init(root.clone())?;

    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    let ref_lock = RefLock::acquire(&layout, "heads/main")?;
    let container_lock = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy])?;

    let locks = list_held_locks(&layout)?;
    assert_eq!(locks.len(), 3);
    for lock in &locks {
        assert_eq!(lock.recorded_pid, Some(std::process::id()));
        assert_eq!(lock.liveness, PidLiveness::AppearsRunning);
    }
    let kinds: Vec<&str> = locks.iter().map(|lock| lock.kind.as_str()).collect();
    assert!(kinds.contains(&"active"));
    assert!(kinds.contains(&"ref"));
    assert!(kinds.contains(&"container:trust-policy"));

    drop(active);
    drop(ref_lock);
    drop(container_lock);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A lock recording a PID that does not exist on this host is reported `DoesNotAppearRunning` on all
/// three platforms as of DC-99 -- advisory, not a claim the lock is safe to clear (see the module's
/// own doc for why).
#[test]
fn a_lock_recording_a_nonexistent_pid_is_reported_as_not_appearing_to_run() -> Result<()> {
    let root = unique_temp_dir("unlock-dead-pid");
    let layout = RepositoryLayout::init(root.clone())?;
    let path = layout.default_active_lock_path();
    // A PID within the valid range but astronomically unlikely to name a real process in a test
    // environment -- `test_kill_process` returns `ESRCH` for it on Linux/macOS, and `OpenProcess`
    // fails with `ERROR_INVALID_PARAMETER` for it on Windows.
    std::fs::write(&path, "pid=999999\nkind=active\nnote=test\n")?;

    let locks = list_held_locks(&layout)?;
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].recorded_pid, Some(999_999));
    assert_eq!(locks[0].liveness, PidLiveness::DoesNotAppearRunning);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// `clear_lock` removes exactly the path it is given, no confirmation or liveness check of its own --
/// that is the caller's job, per the module's own "enumerate and report, decide elsewhere" split. After
/// clearing, the lock is re-acquirable.
#[test]
fn clear_lock_removes_the_named_lock_and_it_becomes_reacquirable() -> Result<()> {
    let root = unique_temp_dir("unlock-clear");
    let layout = RepositoryLayout::init(root.clone())?;
    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    let path = active.path().to_path_buf();
    // Simulate a crash: the lock file survives, but nothing still holds it in-process.
    std::mem::forget(active);

    assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_err());
    clear_lock(&layout, &path)?;
    assert!(list_held_locks(&layout)?.is_empty());
    assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_ok());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A missing `pid=` line (malformed body) is `Unknown`, not misread as any specific PID -- fails
/// closed on ambiguity rather than guessing.
#[test]
fn a_malformed_lock_body_reports_unknown_liveness() -> Result<()> {
    let root = unique_temp_dir("unlock-malformed-body");
    let layout = RepositoryLayout::init(root.clone())?;
    let path = layout.default_active_lock_path();
    std::fs::write(&path, "not a lock body at all")?;

    let locks = list_held_locks(&layout)?;
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].recorded_pid, None);
    assert_eq!(locks[0].liveness, PidLiveness::Unknown);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-41 crash window 4, end to end: a container lock survives a simulated crash (the in-process
/// guard never runs `Drop`, matching what a real crash leaves behind), wedging both the compactor and
/// its writer -- and this module's own recovery path is what un-wedges it, not a special case for
/// container locks. Ties `unlock.rs` and `compact.rs` together rather than testing either in
/// isolation, since that is the actual recovery story an operator lives through.
#[test]
fn a_wedged_container_lock_blocks_compaction_until_unlock_clears_it() -> Result<()> {
    let root = unique_temp_dir("unlock-wedged-container-lock");
    let layout = RepositoryLayout::init(root.clone())?;

    let held = acquire_container_locks(&layout, &[LockableContainer::RefPointerIndex])?;
    let path = layout.lockable_container_lock_path(LockableContainer::RefPointerIndex);
    // Simulate a crash: the lock file survives, but nothing still holds it in-process -- exactly
    // what a real crash leaves, not a synthetic shortcut.
    std::mem::forget(held);

    assert!(crate::compact_ref_pointer_index(&layout).is_err());
    assert!(crate::plan_compact_ref_pointer_index(&layout).is_err());

    let locks = list_held_locks(&layout)?;
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].path, path);
    assert_eq!(locks[0].kind, "container:ref-pointer-index");

    clear_lock(&layout, &path)?;
    assert!(list_held_locks(&layout)?.is_empty());

    let report = crate::compact_ref_pointer_index(&layout)?;
    assert_eq!(report.entries_before, 0);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The macOS CI defect, reproduced directly rather than only through the subprocess integration tests
/// that first caught it: a real symlink to the repository root, a `--lock`-equivalent path constructed
/// through the symlinked route, and confirmation that `find_held_lock` still matches it against the
/// `HeldLock` whose own path was built from the *resolved* root -- exactly the mismatch a `--lock
/// <path>` argument hits whenever the operator's path and the store's own internal root disagree only
/// on which route to the same file they took.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn find_held_lock_matches_a_lock_reached_through_a_symlinked_route() -> Result<()> {
    let real_root = unique_temp_dir("unlock-symlink-real");
    let layout = RepositoryLayout::init(real_root.clone())?;
    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;

    let symlink_root = unique_temp_dir("unlock-symlink-link");
    std::fs::remove_dir(&symlink_root)?;
    std::os::unix::fs::symlink(&real_root, &symlink_root)?;

    // The path an operator would type after `cd`-ing through the symlinked route, or copying a path
    // from somewhere that never resolved it -- syntactically different from `layout`'s own root, same
    // physical file.
    let symlinked_target = symlink_root.join(".prikk/active/default/active.lock");
    assert_ne!(symlinked_target, active.path());

    let locks = list_held_locks(&layout)?;
    let found = find_held_lock(&locks, &symlinked_target);
    assert!(
        found.is_some(),
        "a lock reached through a symlinked route must still match the canonical `HeldLock` \
         `list_held_locks` reports"
    );

    drop(active);
    let _ = std::fs::remove_file(&symlink_root);
    let _ = std::fs::remove_dir_all(real_root);
    Ok(())
}

/// RFC 108 increment 2 control 1: `list_held_locks` on a single-(default)-active repository must
/// return exactly what `a54a560`'s hardcoded single-active read returned. Reconstructs that prior
/// implementation's own three reads directly here (not by inspecting the diff) and asserts the new
/// enumeration-based version produces the identical sequence -- this is the increment's central
/// control, per its own §3.
#[test]
fn list_held_locks_matches_the_prior_single_active_read_exactly() -> Result<()> {
    let root = unique_temp_dir("unlock-single-active-parity");
    let layout = RepositoryLayout::init(root.clone())?;
    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    let ref_lock = RefLock::acquire(&layout, "heads/main")?;
    let container_lock = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy])?;

    let mut expected = Vec::new();
    expected.extend(read_lock_if_present(
        &layout,
        &layout.default_active_lock_path(),
    )?);
    let ref_locks_dir = layout.refs_dir().join("locks");
    let ref_locks_relative = layout.repository_relative(&ref_locks_dir)?;
    for entry in list_directory(layout.repository_mutation_root(), &ref_locks_relative)? {
        if entry.kind != EntryKind::Regular {
            continue;
        }
        expected.extend(read_lock_if_present(
            &layout,
            &ref_locks_dir.join(&entry.name),
        )?);
    }
    for container in LockableContainer::ALL {
        expected.extend(read_lock_if_present(
            &layout,
            &layout.lockable_container_lock_path(container),
        )?);
    }

    assert_eq!(list_held_locks(&layout)?, expected);

    drop(active);
    drop(ref_lock);
    drop(container_lock);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 2 control 2: nothing in the codebase creates a second active session yet, so
/// this test constructs `active/second/` and its lock file directly, matching `lock.rs::lock_body`'s
/// own on-disk format (`pid=`/`kind=`/`note=` lines). Fails if `list_held_locks` is ever reverted to
/// the old hardcoded single-active read, because the hand-planted second lock would then simply be
/// absent from its output.
#[test]
fn list_held_locks_reports_a_hand_planted_second_active() -> Result<()> {
    let root = unique_temp_dir("unlock-second-active");
    let layout = RepositoryLayout::init(root.clone())?;
    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;

    let second_lock_path = layout.active_lock_path("second");
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    std::fs::write(
        &second_lock_path,
        format!("pid={}\nkind=active\nnote=test\n", std::process::id()),
    )?;

    let locks = list_held_locks(&layout)?;
    assert_eq!(locks.len(), 2);
    let paths: Vec<&std::path::Path> = locks.iter().map(|lock| lock.path.as_path()).collect();
    assert!(paths.contains(&active.path()));
    assert!(paths.contains(&second_lock_path.as_path()));
    for lock in &locks {
        assert_eq!(lock.kind, "active");
    }

    drop(active);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 2 review, permanent regression: `active_session_names` must return the raw
/// `OsString` unchanged, and this function must derive its path from it directly (never through a
/// lossy `to_string_lossy()` round-trip), or a held lock under a non-UTF-8 session name is silently
/// dropped from the report -- the exact defect this increment exists to prevent, reintroduced one
/// layer down. A session name containing an invalid UTF-8 byte is a valid directory name on Linux;
/// probed directly (not assumed) against the pre-fix implementation, which reported zero locks for
/// this exact setup.
///
/// **Gated to `target_os = "linux"`, not `unix`.** `unix` is an OS family, not a filesystem-behaviour
/// class, and this test needs the latter: Linux's VFS treats a filename as an opaque byte string
/// (rejecting only NUL and `/`) regardless of which native filesystem is mounted underneath, but
/// macOS's APFS enforces UTF-8 and rejects this exact byte sequence outright. **Not reasoned about --
/// run and found wrong**: this test under `#[cfg(unix)]` failed `macOS mutation test suite` in CI
/// (run `33037284343`) with `Error: Io("Illegal byte sequence (os error 92)")` (`EILSEQ`) raised by
/// `create_dir_all` while building the fixture, before the assertion ever ran. **The product fact
/// this establishes, not merely the CI failure**: the silent-lock-drop defect this test guards
/// against is reachable on Linux (proven here, and by `stable`/`msrv-1.85.0` CI both passing on
/// `ubuntu-latest`) and on Windows (see the `cfg(windows)` variant below), but on APFS it is
/// unreachable **by this specific mechanism**, because a session directory with this name cannot
/// exist there at all -- not because the enumeration or the fix behaves differently on that
/// platform.
#[cfg(target_os = "linux")]
#[test]
fn list_held_locks_reports_a_lock_under_a_non_utf8_session_name() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let root = unique_temp_dir("unlock-non-utf8-active-name");
    let layout = RepositoryLayout::init(root.clone())?;
    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;

    let bad_name = std::ffi::OsStr::from_bytes(b"bad\xFFname");
    let bad_lock_path = layout.active_lock_path(bad_name);
    std::fs::create_dir_all(layout.active_session_dir(bad_name))?;
    std::fs::write(
        &bad_lock_path,
        format!("pid={}\nkind=active\nnote=test\n", std::process::id()),
    )?;

    let locks = list_held_locks(&layout)?;
    let paths: Vec<&std::path::Path> = locks.iter().map(|lock| lock.path.as_path()).collect();
    assert_eq!(
        locks.len(),
        2,
        "a held lock under a non-UTF-8 session name must not be silently dropped"
    );
    assert!(paths.contains(&active.path()));
    assert!(paths.contains(&bad_lock_path.as_path()));

    drop(active);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Windows analogue of the Linux non-UTF-8 case above: an unpaired UTF-16 surrogate is not valid
/// UTF-16 text (so it cannot round-trip through `to_string_lossy()`), but it is a valid NTFS
/// directory name component -- `OsString` on Windows is WTF-8 precisely so this is representable.
/// Runs for real on CI's `windows-mutation` job, which executes `cargo test` natively on
/// `windows-latest`, not merely cross-compiles it -- confirmed passing there.
///
/// **This is established for NTFS specifically (what `windows-latest` mounts), not for `windows` as
/// an OS family.** The Linux test above exists because gating on the OS family (`unix`) rather than
/// the filesystem property turned out to be wrong for macOS/APFS -- the same gap could exist here for
/// ReFS or a network filesystem (SMB/CIFS), and it has **not** been established either way. Left as
/// `#[cfg(windows)]` because CI only ever exercises NTFS today and no finer built-in `cfg` exists to
/// name the property directly; if a future CI job runs on a different Windows filesystem and this
/// fails there, that is new evidence, not a contradiction of this note.
#[cfg(windows)]
#[test]
fn list_held_locks_reports_a_lock_under_a_non_utf8_session_name() -> Result<()> {
    use std::os::windows::ffi::OsStringExt;

    let root = unique_temp_dir("unlock-non-utf8-active-name");
    let layout = RepositoryLayout::init(root.clone())?;
    let active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;

    let bad_name = std::ffi::OsString::from_wide(&[
        u16::from(b'b'),
        u16::from(b'a'),
        u16::from(b'd'),
        0xD800, // unpaired high surrogate
        u16::from(b'n'),
        u16::from(b'a'),
        u16::from(b'm'),
        u16::from(b'e'),
    ]);
    let bad_lock_path = layout.active_lock_path(&bad_name);
    std::fs::create_dir_all(layout.active_session_dir(&bad_name))?;
    std::fs::write(
        &bad_lock_path,
        format!("pid={}\nkind=active\nnote=test\n", std::process::id()),
    )?;

    let locks = list_held_locks(&layout)?;
    let paths: Vec<&std::path::Path> = locks.iter().map(|lock| lock.path.as_path()).collect();
    assert_eq!(
        locks.len(),
        2,
        "a held lock under a non-UTF-8 session name must not be silently dropped"
    );
    assert!(paths.contains(&active.path()));
    assert!(paths.contains(&bad_lock_path.as_path()));

    drop(active);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 2 control 3: `unlock` is a recovery surface, run precisely when a repository
/// may not be fully valid. Removing `active/` entirely (simulating exactly that kind of damage) must
/// not make `list_held_locks` error -- an empty result is what lets the rest of `unlock`'s recovery
/// flow continue past a directory that only *valid* repositories are guaranteed to have.
#[test]
fn list_held_locks_succeeds_when_the_active_directory_is_entirely_missing() -> Result<()> {
    let root = unique_temp_dir("unlock-missing-active-dir");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::remove_dir_all(layout.active_dir())?;

    assert!(list_held_locks(&layout)?.is_empty());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Recovery-listing-tolerance follow-up, site 1's "after" control: `refs/locks` is a required
/// directory (`layout.rs::required_directories`), and before this fix a missing one made this whole
/// function -- and `prikk unlock` with it -- fail outright (`Err: i/o error: directory is absent:
/// refs/locks`), defeating the one command that exists to clear a wedged lock on a repository that
/// may not be fully valid. The report quotes the reverted-and-reran probe showing that real "before"
/// error; this is the "after". The absence itself is reported by `doctor`, not here -- see
/// `push_missing_required_directory_issues`'s own doc and
/// `doctor::tests::doctor_reports_a_missing_refs_locks_directory_even_though_unlock_tolerates_it`.
#[test]
fn list_held_locks_succeeds_when_refs_locks_directory_is_missing() -> Result<()> {
    let root = unique_temp_dir("unlock-missing-refs-locks-dir");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::remove_dir_all(layout.refs_dir().join("locks"))?;

    assert!(list_held_locks(&layout)?.is_empty());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Recovery-listing-tolerance follow-up, control 3: the per-ref lock listing's sort is load-bearing,
/// not merely present. Verified by temporarily removing only the `sort_by` call inside
/// `fsutil::list_directory_tolerating_absence` and re-running this exact test -- it failed (quoted in
/// the report) -- then restoring it via `Edit`, matching RFC 108 increment 2's own lesson that a sort
/// test which would pass regardless of the sort is not a control.
///
/// **The insertion order matters and was chosen deliberately, not copied from increment 2's own
/// example.** A first attempt planted these same three names in `["zzz", "mmm", "aaa"]` order and
/// passed even with the sort removed: this filesystem's raw `readdir` happens to return entries in
/// *reverse insertion order* (confirmed by direct probe), and the reverse of an already-descending
/// insertion order is ascending -- indistinguishable from the sorted result by accident, exactly the
/// false-positive control the increment-2 report warned about. `["mmm", "zzz", "aaa"]` avoids that:
/// its reverse (`aaa, zzz, mmm`) is not sorted order (`aaa, mmm, zzz`), confirmed by the same probe
/// before writing this assertion.
///
/// Lock file names are hand-planted, not derived through `RefLock::acquire` (which hashes the ref
/// name via `ref_name_storage_key` into an unpredictable-by-design filename) -- this test only needs
/// *some* three names whose sorted order differs from this filesystem's actual raw order, and
/// controlling them directly is simpler than fighting the hash.
#[test]
fn list_held_locks_reports_per_ref_locks_in_sorted_order() -> Result<()> {
    let root = unique_temp_dir("unlock-ref-lock-order");
    let layout = RepositoryLayout::init(root.clone())?;
    let locks_dir = layout.refs_dir().join("locks");
    for name in ["mmm", "zzz", "aaa"] {
        std::fs::write(
            locks_dir.join(format!("{name}.lock")),
            format!("pid={}\nkind=ref\nnote=test\n", std::process::id()),
        )?;
    }

    let locks = list_held_locks(&layout)?;
    let ref_lock_paths: Vec<PathBuf> = locks
        .iter()
        .filter(|lock| lock.kind == "ref")
        .map(|lock| lock.path.clone())
        .collect();
    let expected: Vec<PathBuf> = ["aaa", "mmm", "zzz"]
        .iter()
        .map(|name| locks_dir.join(format!("{name}.lock")))
        .collect();
    assert_eq!(
        ref_lock_paths, expected,
        "per-ref locks must come back sorted by raw name bytes, not directory-listing order"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A target that resolves to nothing real must still report "not found" rather than propagating an
/// I/O error -- the no-match branch is exactly where a target may be bogus, and `find_held_lock`
/// returns `Option`, not `Result`, on purpose.
#[test]
fn find_held_lock_returns_none_for_a_target_that_does_not_exist() -> Result<()> {
    let root = unique_temp_dir("unlock-bogus-target");
    let layout = RepositoryLayout::init(root.clone())?;
    let _active = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;

    let locks = list_held_locks(&layout)?;
    let bogus = root.join("this-path-was-never-created").join("active.lock");
    assert!(find_held_lock(&locks, &bogus).is_none());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
