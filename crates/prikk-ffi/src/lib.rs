//! Prikk's sole FFI surface (DC-96 Windows Anchor Identity, widened by 0.57.0 step 2 for lock liveness). Every
//! future FFI need lands here rather than motivating a second entry in `UNSAFE_EXEMPT_CRATES`, which DC-90 forbids
//! anyway -- see that gate's own module doc for why at most one workspace crate may hold this exemption.
//!
//! What it holds, per platform (a platform with no FFI need compiles to nothing, and this crate does not anticipate
//! needs that do not exist yet):
//!
//! - **Windows:** a file's identity (`FileIdentity`, `identity_of`) and current path (`current_path_of`), advisory
//!   process liveness (`ProcessLiveness`, `process_liveness`), a no-replace rename (`rename_no_replace`), a process's
//!   start time (`ProcessStart`, `process_start_time`) and the machine's identity (`machine_guid`).
//! - **macOS** (0.57.0, a lock names its holder): a process's start time (`ProcessStart`, `process_start_time`, from
//!   `proc_pidinfo`) and the machine's identity (`machine_uuid`, from `gethostuuid`).
//! - **Linux and other targets:** nothing. A holder's start time is a plain file read (`/proc/<pid>/stat`), done
//!   by `prikk-store` without FFI.

/// Identity of an open filesystem object on Windows -- the value that confirms an anchor handle
/// still refers to the object it was bound to (DC-96). Meaningful only within one boot on one
/// volume -- never derive an object id, a container path, or any on-disk artifact from it, and
/// never persist it past the process that read it.
///
/// **Two variants, not one uniform 128-bit shape padded from the 64-bit fallback -- deliberately**
/// (DC-99 Stage 2). `microsoft/STL`'s own `_Get_file_id_by_handle` (`stl/src/filesystem.cpp`,
/// backing `std::filesystem::equivalent`) zero-pads the 64-bit index into `FILE_ID_INFO`'s 128-bit
/// shape and compares both forms with one `memcmp`. That is safe for *their* use: two handles
/// opened and compared within one call, so a filesystem lacking `FileIdInfo` produces the same
/// zero-padded shape on both sides. **It is not safe for this crate's use**: `WindowsAuthority`
/// captures an identity once (`bind`) and compares it against one re-derived later, at walk time --
/// a stored, zero-padded 64-bit value could compare byte-equal to some other file's genuine 128-bit
/// id that happens to end in eight zero bytes. Astronomically unlikely, but not structurally
/// prevented by a uniform shape, and "correct for STL's own use case" is not the same claim as
/// "correct for this one" (DC-99 stage-2-investigation-ruling-v1 §1). The enum below makes a
/// cross-form comparison **impossible**, not merely unlikely: a derived `PartialEq` compares the
/// discriminant first, so an `Id128` and an `Id64` never compare equal regardless of what bytes
/// either one holds. If a future edit "simplifies" this back to one padded shape, it is
/// reintroducing exactly the risk this comment -- and the ruling it records -- exists to name.
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileIdentity {
    /// `GetFileInformationByHandleEx(FileIdInfo)` -- NTFS, ReFS.
    Id128 {
        /// `FILE_ID_INFO::VolumeSerialNumber`.
        volume_serial_number: u64,
        /// `FILE_ID_INFO::FileId`'s 16-byte identifier, opaque and directly comparable as bytes --
        /// no reserved or unstable portion (confirmed against `microsoft/STL`'s own `memcmp`-based
        /// comparison of the whole struct).
        file_id: [u8; 16],
    },
    /// `GetFileInformationByHandle`'s 64-bit file index -- the fallback for filesystems that
    /// refuse `FileIdInfo` (FAT/exFAT, some network filesystems). Triggered only by
    /// `ERROR_NOT_SUPPORTED` or `ERROR_INVALID_PARAMETER` from the primary call, the same two
    /// codes `microsoft/STL`'s own fallback branches on -- any other failure is a real error,
    /// propagated rather than silently downgraded to this variant.
    Id64 {
        /// `BY_HANDLE_FILE_INFORMATION::dwVolumeSerialNumber`.
        volume_serial_number: u32,
        /// `BY_HANDLE_FILE_INFORMATION`'s `nFileIndexHigh`/`nFileIndexLow`, combined -- not
        /// guaranteed unique on ReFS (Microsoft's own documentation), which is exactly why the
        /// 128-bit form above is preferred whenever the filesystem supports it.
        file_index: u64,
    },
}

#[cfg(all(test, windows))]
std::thread_local! {
    static FORCE_FALLBACK_ONCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test-only: force the very next `identity_of` call on this thread to take the 64-bit fallback
/// path, as if the primary call had failed with `ERROR_NOT_SUPPORTED` -- without a real
/// unsupported-filesystem volume, which CI does not provision (DC-99 stage-2-investigation-ruling-v1
/// §3). Consumed after one call, matching this workspace's other test-only failure-injection
/// mechanism (`fsutil::anchored::failpoints`, `prikk-store`). **Observable, not merely forced**: the
/// caller must assert the returned `FileIdentity` is the `Id64` variant -- if this override ever
/// stopped working, `identity_of` would take the real primary path and return `Id128`, and that
/// assertion would fail. There is no state in which the override silently forces nothing.
#[cfg(all(test, windows))]
pub fn force_identity_fallback_once() {
    FORCE_FALLBACK_ONCE.with(|flag| flag.set(true));
}

#[cfg(windows)]
fn should_force_identity_fallback() -> bool {
    #[cfg(test)]
    {
        FORCE_FALLBACK_ONCE.with(|flag| flag.replace(false))
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Read `file`'s identity from its already-open handle, preferring the 128-bit form
/// (`GetFileInformationByHandleEx(FileIdInfo)`) and falling back to the 64-bit form
/// (`GetFileInformationByHandle`) only when the OS itself reports the 128-bit form unsupported. The
/// caller owns opening the handle, including share flags and reparse-point policy
/// (`FILE_SHARE_DELETE`, `FILE_FLAG_OPEN_REPARSE_POINT`).
#[cfg(windows)]
pub fn identity_of(file: &std::fs::File) -> std::io::Result<FileIdentity> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };

    let handle: HANDLE = file.as_raw_handle();

    if !should_force_identity_fallback() {
        let mut id128 = FILE_ID_INFO::default();
        // SAFETY: `handle` is a valid, open HANDLE for the duration of this call, borrowed from
        // `file`. `id128` is `#[repr(C)]`, `Default`-initialized (windows-sys 0.61.2), and its size
        // is passed explicitly as `dwBufferSize`, so a well-formed `&mut` pointer to it is always a
        // valid, correctly-sized write target for what this call writes on success. On failure it
        // writes nothing we read; the OS error is read via `std::io::Error::last_os_error`
        // immediately after, before any other call can overwrite it.
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&raw mut id128).cast(),
                u32::try_from(size_of::<FILE_ID_INFO>()).unwrap_or(u32::MAX),
            )
        };
        if succeeded != 0 {
            return Ok(FileIdentity::Id128 {
                volume_serial_number: id128.VolumeSerialNumber,
                file_id: id128.FileId.Identifier,
            });
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(code)
                if code == ERROR_NOT_SUPPORTED as i32 || code == ERROR_INVALID_PARAMETER as i32 => {
            }
            _ => return Err(error),
        }
    }

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: same reasoning as the primary call above -- `handle` is still the same valid, open
    // HANDLE, `info` is `#[repr(C)]` and `Default`-initialized, and the OS error is read
    // immediately on failure, before any other call can overwrite it.
    let succeeded = unsafe { GetFileInformationByHandle(handle, &raw mut info) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(FileIdentity::Id64 {
        volume_serial_number: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

/// The current path of an already-open handle's object (`GetFinalPathNameByHandle`, Microsoft
/// Learn). A directory handle follows its object across a rename -- re-deriving the path this way
/// before a walk, rather than re-walking a path string captured earlier, is what lets Windows
/// continue operating correctly against the object that was validated even after its directory
/// entry has been renamed elsewhere (DC-96 implementation-ruling-v1 §4). Returned with the
/// `VOLUME_NAME_DOS` / `FILE_NAME_NORMALIZED` flags (value `0`) -- the ordinary drive-letter path
/// form every other function in this crate and its caller already expects, at the cost of the
/// well-known `\\?\` extended-length prefix Windows adds to this form; `std::fs`/`CreateFileW`
/// both accept it transparently.
#[cfg(windows)]
pub fn current_path_of(file: &std::fs::File) -> std::io::Result<std::path::PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;

    let handle: HANDLE = file.as_raw_handle();
    // Zero-initialized and grown by `Vec::resize`, never left partially uninitialized -- so
    // `buffer.as_mut_ptr()` below is always a pointer to `buffer.len()` valid, initialized `u16`
    // slots, regardless of how much of that capacity the call actually writes.
    let mut buffer: Vec<u16> = vec![0; 512];
    loop {
        let capacity = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: `handle` is a valid, open HANDLE for the duration of this call, borrowed from
        // `file`. `buffer.as_mut_ptr()` points to `capacity` initialized, writable `u16` slots
        // (see above); the function writes at most `capacity` of them and returns the count
        // actually written (success) or the count that would have been required (buffer too
        // small) -- it never writes past what `capacity` promises is available. On failure
        // (return value 0) it writes nothing we read; the OS error is read via
        // `std::io::Error::last_os_error` immediately after, before any other call can clobber
        // it.
        let written =
            unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), capacity, 0) };
        if written == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if written < capacity {
            // Success: `written` is the length actually used, excluding the null terminator.
            buffer.truncate(written as usize);
            break;
        }
        // Too small: `written` is the required size, including the null terminator this time.
        buffer.resize(written as usize, 0);
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

/// Best-effort, advisory liveness of a process id (DC-99, `prikk unlock`'s Windows primitive).
/// `prikk-ffi` cannot depend on `prikk-store`, so this returns its own enum -- map to
/// `unlock.rs::PidLiveness` at the call site, the same split `identity_of`/`current_path_of` already
/// draw between this crate's raw Win32 answer and its caller's own vocabulary.
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcessLiveness {
    /// A handle was opened and confirmed still running: `WaitForSingleObject` reports the handle
    /// nonsignaled, or the open itself failed with `ERROR_ACCESS_DENIED` -- the kernel found a
    /// process to check permissions against, the same reasoning Unix's `EPERM` branch already
    /// applies (`unlock.rs::check_pid_liveness`).
    Exists,
    /// Positively established absence: `OpenProcess` failed with `ERROR_INVALID_PARAMETER` (no such
    /// process), or a successfully opened handle's process has since terminated
    /// (`WaitForSingleObject` reports the handle signaled).
    DoesNotExist,
    /// Neither established -- an unexpected error, an unanticipated wait result, or a degenerate PID
    /// this function refuses to ask the OS about at all. Never authorization to clear anything; see
    /// `PidLiveness`'s own doc at the call site for why a negative or unknown result is advisory
    /// only.
    Indeterminate,
}

/// PID 0 names the System Idle Process on Windows -- a real PID, but never a value a lock file's own
/// `pid=` field can legitimately record for a user process. Rejected before the OS call rather than
/// let it reach `OpenProcess`, so a corrupt or hand-edited lock file recording `pid=0` produces the
/// same `Indeterminate` answer here that `rustix::process::Pid::from_raw(0)` already produces as
/// `None` on Linux/macOS (`unlock.rs::check_pid_liveness`) -- not merely to avoid a syscall, but to
/// keep the two platforms' answers equal for the same malformed input (DC-99
/// stage-1-investigation-ruling-v1 §2: without this guard, `pid=0` reaches `DoesNotExist` on Windows
/// and `Unknown` on Unix for the identical recorded value, and `DoesNotExist` is the one answer that
/// can authorize clearing a lock).
#[cfg(windows)]
pub fn process_liveness(pid: u32) -> ProcessLiveness {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    if pid == 0 {
        return ProcessLiveness::Indeterminate;
    }

    // SAFETY: `OpenProcess` takes no pointer arguments; every argument is a plain integer, and the
    // return value is either NULL (checked immediately below) or a `HANDLE` this function takes
    // ownership of via `OwnedProcessHandle`. Requesting only `PROCESS_QUERY_LIMITED_INFORMATION |
    // PROCESS_SYNCHRONIZE` (not a broader right) is deliberate -- it is queryable against a process
    // this caller does not own, which is exactly the access-denied case this function must
    // distinguish from genuine absence.
    let handle: HANDLE = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        // `std::io::Error::last_os_error` wraps `GetLastError`, read immediately after the call that
        // can set it, matching this crate's other two functions -- no separate FFI import needed for
        // it.
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if code == ERROR_INVALID_PARAMETER as i32 => ProcessLiveness::DoesNotExist,
            Some(code) if code == ERROR_ACCESS_DENIED as i32 => ProcessLiveness::Exists,
            _ => ProcessLiveness::Indeterminate,
        };
    }
    let guard = OwnedProcessHandle(handle);

    // SAFETY: `guard.0` is the valid, open HANDLE just obtained above, not yet closed (the guard
    // closes it on drop, after this call returns and its result is captured). A zero timeout never
    // blocks -- it reports the object's current state and returns immediately either way, so this
    // cannot hang the command that calls it.
    let waited = unsafe { WaitForSingleObject(guard.0, 0) };
    match waited {
        WAIT_TIMEOUT => ProcessLiveness::Exists,
        WAIT_OBJECT_0 => ProcessLiveness::DoesNotExist,
        _ => ProcessLiveness::Indeterminate,
    }
    // `guard` drops here on every path above, including the unwind path if a future edit adds a
    // panicking call between the two `unsafe` blocks -- `CloseHandle` runs in `Drop`, not repeated at
    // each branch, so there is exactly one place a handle leak could hide, and it is not a per-branch
    // decision that could be forgotten in a new arm.
}

/// The `HANDLE` `OpenProcess` returns, owned. `windows-sys` provides no RAII wrapper for `HANDLE`
/// (`identity_of`/`current_path_of` never own one -- they borrow from a `std::fs::File` that already
/// manages its own closing); this is the one function in the crate that does, so it is the one place
/// that needs its own `Drop`.
#[cfg(windows)]
struct OwnedProcessHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedProcessHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a valid, open HANDLE this struct exclusively owns (constructed only
        // from a non-null `OpenProcess` success in `process_liveness`, never copied or shared) and
        // has not been closed yet -- `Drop::drop` runs at most once per value. `CloseHandle`'s own
        // return value is not checked: there is nothing a liveness check can safely do in response to
        // a close failure, and a leaked handle here would be a real defect (DC-99 design-v1.md §
        // "close the handle") but a checked-and-ignored return would not prevent one.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Rename `from` onto `to`, refusing if `to` already exists (`prikk extract`'s own write-safety
/// ruling, 0.54.0 step 3 Part 2) -- the platform's own no-replace default: `MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING` already refuses when the target exists, for both files and
/// directories (Microsoft's own documented contract), so this passes no replace flag at all rather
/// than adding one. The Linux/macOS counterpart is `rustix::fs::renameat_with(..., RenameFlags::
/// NOREPLACE)`, in `prikk-store`, which already depends on `rustix`; this crate is the one
/// workspace member allowed to reach Win32 directly (DC-90), so the Windows half of the same
/// write-safety rule lives here instead.
///
/// **`MOVEFILE_WRITE_THROUGH`** (Part 2b R1): Microsoft's own documentation states the call does
/// not return success until the move is written through to disk, closing the same durability gap
/// `durable_output.rs`'s own module doc already names for a single-file rename on this platform.
/// Still never `MOVEFILE_REPLACE_EXISTING` -- this flag only changes when the function returns, not
/// whether it replaces an existing target.
///
/// # Errors
///
/// `to` already exists, or the rename otherwise fails (the OS error, via `GetLastError`).
#[cfg(windows)]
pub fn rename_no_replace(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    fn wide_null(path: &std::path::Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let from_wide = wide_null(from);
    let to_wide = wide_null(to);
    // SAFETY: `from_wide`/`to_wide` are each a valid, NUL-terminated UTF-16 string the OS may read
    // for the duration of this call only -- both buffers outlive the call (they are not dropped
    // until this function returns). `dwFlags = MOVEFILE_WRITE_THROUGH` passes none of
    // `MoveFileExW`'s own replace flags, so this is still exactly the platform's own no-replace
    // default, only waiting for the move to be durable before returning.
    let succeeded =
        unsafe { MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::{
        FileIdentity, ProcessLiveness, current_path_of, force_identity_fallback_once, identity_of,
        process_liveness,
    };

    /// The one case this advisory check can prove reliably: the current process's own PID, which is
    /// definitely alive because it is the process running this assertion.
    #[test]
    fn process_liveness_of_the_current_process_is_exists() {
        assert_eq!(
            process_liveness(std::process::id()),
            ProcessLiveness::Exists
        );
    }

    /// DC-99 stage-1-investigation-ruling-v1 §2: PID 0 must not reach `DoesNotExist` -- it names the
    /// System Idle Process, a real PID, and the same malformed lock-file value produces `Unknown` on
    /// Linux/macOS (`Pid::from_raw(0)` returning `None`). `Indeterminate` here keeps the two
    /// platforms' answers equal for the same degenerate input.
    #[test]
    fn process_liveness_of_pid_zero_is_indeterminate() {
        assert_eq!(process_liveness(0), ProcessLiveness::Indeterminate);
    }

    /// A PID astronomically unlikely to name a real process on a test runner -- real Windows PIDs
    /// stay far below this range in practice, the same "unlikely" reasoning
    /// `unlock/tests.rs::a_lock_recording_a_nonexistent_pid_is_reported_as_not_appearing_to_run` uses
    /// for its own `999999` on Linux/macOS. `OpenProcess` is expected to fail with
    /// `ERROR_INVALID_PARAMETER` for it -- demonstrated by running this, not assumed from the API
    /// contract (RFC criterion 1).
    #[test]
    fn process_liveness_of_an_implausible_pid_is_does_not_exist() {
        assert_eq!(process_liveness(0x7FFF_FFFF), ProcessLiveness::DoesNotExist);
    }

    /// RFC criterion 3 / design-v1.md: an honest, Windows-reachable access-denied test, not one
    /// asserted only by reading the API contract. PID 4 is the Windows System process by
    /// long-standing OS convention, and `OpenProcess`'s own reference page states explicitly that
    /// opening it "fails and the last error code is ERROR_ACCESS_DENIED because their access
    /// restrictions prevent user-level code from opening them" -- regardless of which access right is
    /// requested, unlike an ordinary process where a narrower right (`PROCESS_QUERY_LIMITED_INFORMATION`)
    /// can succeed where a broader one fails.
    #[test]
    fn process_liveness_of_the_system_process_is_exists() {
        assert_eq!(process_liveness(4), ProcessLiveness::Exists);
    }

    /// DC-96 design-v1.md §6.4: without this, a `FileIdentity` that never equals itself -- or
    /// always equals everything -- would pass every other test in this increment silently. Exercises
    /// the real primary path (DC-99 Stage 2): GitHub's `windows-latest` runner's drives are NTFS
    /// (`stage-2-investigation-v1` §2), so this is expected to return the `Id128` variant without any
    /// override, demonstrated below rather than assumed.
    #[test]
    fn identity_distinguishes_different_files_and_matches_the_same_one() -> std::io::Result<()> {
        let directory = std::env::temp_dir();
        let suffix = std::process::id();
        let path_a = directory.join(format!("prikk-ffi-identity-test-a-{suffix}"));
        let path_b = directory.join(format!("prikk-ffi-identity-test-b-{suffix}"));
        std::fs::write(&path_a, b"a")?;
        std::fs::write(&path_b, b"b")?;

        let identity_a = identity_of(&std::fs::File::open(&path_a)?)?;
        let identity_b = identity_of(&std::fs::File::open(&path_b)?)?;
        let identity_a_reopened = identity_of(&std::fs::File::open(&path_a)?)?;

        let _ = std::fs::remove_file(&path_a);
        let _ = std::fs::remove_file(&path_b);

        assert!(
            matches!(identity_a, FileIdentity::Id128 { .. }),
            "the CI runner's NTFS drives are expected to take the primary 128-bit path by default: \
             {identity_a:?}"
        );
        assert_ne!(
            identity_a, identity_b,
            "two different files must not compare equal"
        );
        assert_eq!(
            identity_a, identity_a_reopened,
            "the same file, reopened, must compare equal"
        );
        Ok(())
    }

    /// DC-99 Stage 2's own negative control for the fallback path: `force_identity_fallback_once`
    /// forces `identity_of` to skip the primary call and go straight to `GetFileInformationByHandle`,
    /// against an ordinary NTFS file that would otherwise take the 128-bit path -- proving the
    /// fallback's own construction is correct even though CI provisions no genuinely unsupported
    /// filesystem to trigger it for real (`stage-2-investigation-ruling-v1` §3). Observable per that
    /// ruling: every assertion below is on the *returned variant*, so if the override ever stopped
    /// forcing anything, `identity_of` would silently take the primary path and return `Id128`
    /// instead -- turning this test red, not green.
    #[test]
    fn identity_fallback_distinguishes_different_files_and_matches_the_same_one()
    -> std::io::Result<()> {
        let directory = std::env::temp_dir();
        let suffix = std::process::id();
        let path_a = directory.join(format!("prikk-ffi-identity-fallback-test-a-{suffix}"));
        let path_b = directory.join(format!("prikk-ffi-identity-fallback-test-b-{suffix}"));
        std::fs::write(&path_a, b"a")?;
        std::fs::write(&path_b, b"b")?;

        force_identity_fallback_once();
        let identity_a = identity_of(&std::fs::File::open(&path_a)?)?;
        force_identity_fallback_once();
        let identity_b = identity_of(&std::fs::File::open(&path_b)?)?;
        force_identity_fallback_once();
        let identity_a_reopened = identity_of(&std::fs::File::open(&path_a)?)?;

        let _ = std::fs::remove_file(&path_a);
        let _ = std::fs::remove_file(&path_b);

        assert!(
            matches!(identity_a, FileIdentity::Id64 { .. }),
            "the override must have forced the fallback branch, not the primary one: {identity_a:?}"
        );
        assert!(matches!(identity_b, FileIdentity::Id64 { .. }));
        assert!(matches!(identity_a_reopened, FileIdentity::Id64 { .. }));
        assert_ne!(
            identity_a, identity_b,
            "two different files must not compare equal, even via the fallback form"
        );
        assert_eq!(
            identity_a, identity_a_reopened,
            "the same file, reopened through the fallback form both times, must compare equal"
        );
        Ok(())
    }

    /// `force_identity_fallback_once` is consumed after exactly one call -- a second `identity_of`
    /// call on the same thread must take the real primary path again, not stay forced. Without this,
    /// the fallback test above could pass for the wrong reason: every call after the first one
    /// forced would also return `Id64`, whether or not the override actually fired again.
    #[test]
    fn identity_fallback_override_is_consumed_after_one_call() -> std::io::Result<()> {
        let path = std::env::temp_dir().join(format!(
            "prikk-ffi-identity-fallback-once-test-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"x")?;

        force_identity_fallback_once();
        let forced = identity_of(&std::fs::File::open(&path)?)?;
        let unforced = identity_of(&std::fs::File::open(&path)?)?;

        let _ = std::fs::remove_file(&path);

        assert!(matches!(forced, FileIdentity::Id64 { .. }));
        assert!(
            matches!(unforced, FileIdentity::Id128 { .. }),
            "the override must not persist past its one call: {unforced:?}"
        );
        Ok(())
    }

    /// Guards the enum's own shape: if a future edit adds a field this derive doesn't cover, or
    /// removes `PartialEq`, this fails to compile rather than silently comparing fewer fields than
    /// intended.
    #[test]
    fn file_identity_is_copy_and_comparable() {
        fn assert_bounds<T: Copy + PartialEq + Eq + std::fmt::Debug>() {}
        assert_bounds::<FileIdentity>();
    }

    fn open_directory(path: &std::path::Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        // `FILE_FLAG_BACKUP_SEMANTICS` (`0x02000000`) -- required to obtain a directory handle via
        // `CreateFile` at all (Microsoft Learn, `CreateFileA`, `dwFlagsAndAttributes`). The same
        // constant `windows.rs` uses for the same reason; not re-exported from there to keep this
        // crate's only dependency on its caller one-directional (this crate takes no dependency on
        // `prikk-store`).
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0200_0000)
            .open(path)
    }

    /// DC-96 implementation-ruling-v1 §4: the mechanism the whole correction rests on. A retained
    /// directory handle must keep resolving to its *current* path after the directory is renamed
    /// out from under it -- this is what turns identity comparison from the sole mechanism
    /// (detection only, and wrong per that ruling) into the secondary confirmation after a walk
    /// that already starts from the right place (prevention).
    #[test]
    fn current_path_of_follows_the_handle_across_a_rename() -> std::io::Result<()> {
        let temporary_root = std::env::temp_dir();
        let suffix = std::process::id();
        let original = temporary_root.join(format!("prikk-ffi-rename-test-original-{suffix}"));
        let renamed = temporary_root.join(format!("prikk-ffi-rename-test-renamed-{suffix}"));
        let _ = std::fs::remove_dir_all(&original);
        let _ = std::fs::remove_dir_all(&renamed);
        std::fs::create_dir(&original)?;

        let handle = open_directory(&original)?;
        let path_before = current_path_of(&handle)?;
        let expected_before = std::fs::canonicalize(&original)?;

        std::fs::rename(&original, &renamed)?;
        let path_after = current_path_of(&handle)?;
        let expected_after = std::fs::canonicalize(&renamed)?;

        let _ = std::fs::remove_dir_all(&renamed);

        assert_eq!(
            path_before, expected_before,
            "before any rename, the handle's path must match the directory it was opened from"
        );
        assert_eq!(
            path_after, expected_after,
            "after renaming the directory out from under the still-open handle, current_path_of \
             must report the NEW path -- this is the whole mechanism DC-96 depends on"
        );
        assert_ne!(
            path_before, path_after,
            "the rename must actually have been observed, not silently ignored"
        );
        Ok(())
    }

    /// `prikk extract`'s own write-safety ruling (0.54.0 step 3 Part 2), pinned directly: an
    /// existing target directory is never silently replaced. A plain `std::fs::rename` over an
    /// existing empty directory succeeds on Windows too (the TOCTOU risk `rename_no_replace` exists
    /// to close is real on every platform, not only Linux -- the design round's own R-A4 confirmed
    /// this for Linux/macOS by direct test; this is that same confirmation for Windows), so this
    /// test shows the real control: `rename_no_replace` itself refuses where a plain rename would
    /// not.
    #[test]
    fn rename_no_replace_refuses_an_existing_target_directory() -> std::io::Result<()> {
        let temporary_root = std::env::temp_dir();
        let suffix = std::process::id();
        let source = temporary_root.join(format!("prikk-ffi-norep-src-{suffix}"));
        let target = temporary_root.join(format!("prikk-ffi-norep-dst-{suffix}"));
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir(&source)?;
        std::fs::create_dir(&target)?;
        std::fs::write(source.join("marker.txt"), b"from the source")?;

        let result = super::rename_no_replace(&source, &target);

        assert!(
            result.is_err(),
            "rename_no_replace must refuse an existing target, not silently replace it"
        );
        assert!(
            source.join("marker.txt").exists(),
            "the source must be untouched after a refused rename"
        );
        assert!(
            !target.join("marker.txt").exists(),
            "the target must be untouched after a refused rename -- never partially overwritten"
        );

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&target);
        Ok(())
    }
}

/// What reading a process's start time found (0.57.0 step 2, L1: a lock names its holder, not just a
/// pid). Two instances of one pid number never share a start time, so a recorded start time that
/// equals the one read now is what tells "the same process" from "the pid reused by another".
/// `prikk-ffi` cannot depend on `prikk-store`; the caller maps this to its own vocabulary, the same
/// split `ProcessLiveness` draws. Linux needs no FFI for this (`/proc/<pid>/stat` is a plain file) and
/// compiles nothing here.
#[cfg(any(windows, target_os = "macos"))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcessStart {
    /// The process exists and started at this moment: microseconds since the Unix epoch on macOS, the
    /// 100 ns `FILETIME` creation stamp on Windows. Opaque to the caller beyond equality; both are
    /// absolute wall-clock instants, so unlike a since-boot count they do not repeat across a reboot.
    Started(u64),
    /// Positively established absence: no such process, or one that has terminated (a zombie on
    /// macOS, a signaled process handle on Windows).
    Gone,
    /// Neither established -- an unexpected error, or a process this caller may not query. Never
    /// authorization to treat the process as gone.
    Indeterminate,
}

/// macOS: the start time of `pid`, from `proc_pidinfo(PROC_PIDTBSDINFO)`. A zombie reports `Gone`:
/// it can never run again, though `kill(pid, 0)` still succeeds on it.
#[cfg(target_os = "macos")]
pub fn process_start_time(pid: u32) -> ProcessStart {
    /// `SZOMB` from `<sys/proc.h>`: the process has exited and awaits its parent's `wait`.
    const SZOMB: u32 = 5;

    let Ok(raw) = libc::c_int::try_from(pid) else {
        return ProcessStart::Indeterminate;
    };
    let Ok(size) = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()) else {
        return ProcessStart::Indeterminate;
    };
    // SAFETY: `proc_bsdinfo` is a plain C struct of integers and byte arrays, for which the all-zero
    // bit pattern is a valid value; it is overwritten by the call below before any field is read.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer pointer is to `info`, a live, writable `proc_bsdinfo`, and `size` is exactly
    // its size in bytes, so the kernel writes no more than the buffer holds. The other arguments are
    // plain integers. The function returns the number of bytes written, or 0 / -1 with `errno` set.
    let written = unsafe {
        libc::proc_pidinfo(
            raw,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };
    if written == size {
        if info.pbi_status == SZOMB {
            return ProcessStart::Gone;
        }
        let micros = info
            .pbi_start_tvsec
            .checked_mul(1_000_000)
            .and_then(|whole| whole.checked_add(info.pbi_start_tvusec));
        return micros.map_or(ProcessStart::Indeterminate, ProcessStart::Started);
    }
    // `errno` is read immediately after the failed call, before anything else can set it.
    match std::io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::ESRCH => ProcessStart::Gone,
        _ => ProcessStart::Indeterminate,
    }
}

/// Windows: the creation time of `pid`, from `GetProcessTimes`. A process whose handle is already
/// signaled has terminated and reports `Gone`. A process this caller may not open reports
/// `Indeterminate`, never `Gone`.
#[cfg(windows)]
pub fn process_start_time(pid: u32) -> ProcessStart {
    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        WaitForSingleObject,
    };

    if pid == 0 {
        return ProcessStart::Indeterminate;
    }
    // SAFETY: as in `process_liveness`: plain integer arguments; the result is NULL (checked below)
    // or a `HANDLE` owned from here by `OwnedProcessHandle`.
    let handle: HANDLE = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if code == ERROR_INVALID_PARAMETER as i32 => ProcessStart::Gone,
            _ => ProcessStart::Indeterminate,
        };
    }
    let guard = OwnedProcessHandle(handle);

    // SAFETY: `guard.0` is the valid, open handle just obtained; a zero timeout never blocks.
    match unsafe { WaitForSingleObject(guard.0, 0) } {
        WAIT_TIMEOUT => {}
        WAIT_OBJECT_0 => return ProcessStart::Gone,
        _ => return ProcessStart::Indeterminate,
    }

    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: `guard.0` is a valid open handle with `PROCESS_QUERY_LIMITED_INFORMATION`, which
    // `GetProcessTimes` requires; the four pointers are to live, writable `FILETIME`s that outlive the
    // call.
    let ok = unsafe {
        GetProcessTimes(
            guard.0,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    if ok == 0 {
        return ProcessStart::Indeterminate;
    }
    ProcessStart::Started(
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
    )
}

/// macOS: this machine's hardware UUID, from `gethostuuid` (0.57.0 step 2, L1b). A lock taken on a
/// repository on a shared drive may come from another machine, whose boot id, process table and
/// start times mean nothing here; the machine identity is what says "the same machine" before a
/// lock is called "not running". `None` if the call fails, is refused (a sandbox may), or does not
/// finish within about a second (`EWOULDBLOCK`), never a guess. Read once per process by the caller,
/// before its first lock, so the wait is **bounded**: a zero `timespec` means "wait indefinitely until
/// it completes" (the man page), which would hang every write on a stalled lookup instead of reading
/// "unknown".
#[cfg(target_os = "macos")]
pub fn machine_uuid() -> Option<String> {
    let mut id = [0u8; 16];
    let bounded_wait = libc::timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    // SAFETY: `id` is a live, writable 16-byte buffer, the size `gethostuuid` documents for its
    // `uuid_t` out-parameter, and `bounded_wait` is a live `timespec` that outlives the call; a
    // non-zero value bounds the call (it returns -1 with `EWOULDBLOCK` when it expires). The result
    // is 0 on success; the buffer is read only then.
    let status = unsafe { libc::gethostuuid(id.as_mut_ptr(), &raw const bounded_wait) };
    if status != 0 {
        return None;
    }
    let mut text = String::with_capacity(32);
    for byte in id {
        text.push_str(&format!("{byte:02x}"));
    }
    Some(text)
}

/// Windows: this machine's `MachineGuid` (`HKLM\SOFTWARE\Microsoft\Cryptography`), read from the
/// 64-bit registry view so a 32-bit process reads the same value (0.57.0 step 2, L1b). Created at
/// install and unique per installation. `None` if absent or unreadable, never a guess. Read once per
/// process by the caller.
#[cfg(windows)]
pub fn machine_guid() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };

    let subkey: Vec<u16> = "SOFTWARE\\Microsoft\\Cryptography\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "MachineGuid\0".encode_utf16().collect();
    let mut buffer = [0u16; 128];
    let mut size = u32::try_from(std::mem::size_of_val(&buffer)).ok()?;
    // SAFETY: `subkey` and `value` are live, NUL-terminated UTF-16 strings; `buffer` is a live
    // writable buffer of exactly `size` bytes, and `size` is a live `u32` the call updates to the
    // byte count written (including the terminating NUL). The type out-pointer is optional and
    // passed null. `RRF_RT_REG_SZ` restricts the value to a string.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let units = (usize::try_from(size).ok()? / 2).checked_sub(1)?;
    let guid = String::from_utf16(buffer.get(..units)?).ok()?;
    (!guid.is_empty()).then_some(guid)
}
