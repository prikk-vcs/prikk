//! Filesystem utility helpers for storage operations.

use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::path::PathBuf;

use prikk_error::{PrikkError, Result};

mod anchored;
mod contract;

// DC-97 gated this whole module off Windows because the failpoint mechanism itself had no Windows
// implementation. DC-98 built and wired that mechanism -- re-evaluated per its own criterion 5, and
// the gate stays Unix-only for what remains here, but the reason is per-test, not per-file (an
// earlier version of this comment claimed "every one of these 18," found false for one test by
// review: a per-file grep is not a per-test count). 17 of 18 exercise `CreatedDirectoryParentSync`/
// `ObservedDirectoryParentSync`/`RequiredDirectorySync`/`MutableParentSync`, the directory-entry-sync
// points DC-98's classification (rows #10-#14) found have no Windows operation to inject at at all
// (no `FlushFileBuffers` contract for a directory handle) -- not a missing mechanism, a missing
// operation. The 18th, `sync_matrix::object_write_sync_failure_retains_and_classifies`, uses only
// `RequiredFileSync` (row #6, wired on Windows) and its Windows ordinals are now established by
// observation (`anchored.rs`'s own `fail_after_for_test` re-export doc comment) -- it stays here
// unmoved, but has its own Windows twin,
// `windows::tests::object_write_sync_failure_retains_and_classifies_windows`, rather than this
// module being restructured to free one test from three files' worth of shared per-file `use`
// statements. Nothing in this module uses a unix-only OS facility directly; a future caller-level
// test exercising only the points DC-98 did wire (`RequiredOpen`, `DirectoryCreate`,
// `MutableFileSync`, `MutableRename`, `RequiredFileSync`, `AppendWrite`, `Truncate`, `Unlink`) can be
// Windows-portable the same way.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod caller_tests;
// `tests` (and its `conformance`/`directory` submodules) is genuinely mixed: some tests use
// unix-only facilities (FIFOs, symlinks) or the same failpoint mechanism above, and are individually
// gated inline where that's true; the rest -- including the whole `conformance` suite's Windows
// wrappers -- compile and run on Windows too.
#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tests;

pub(crate) use anchored::{
    EntryKind, MutationRoot, RootDirEntry, RootFileStat, append_file_required,
    create_new_file_required, ensure_directory_required, inspect_entry, list_directory,
    read_file_if_exists, read_file_required, remove_file_cleanup_best_effort,
    remove_worktree_file_required, set_regular_file_mode_required, stat_file_state_if_exists,
    sync_directory_required, truncate_existing_file_required, truncate_file_empty_required,
    write_file_atomically, write_worktree_file_atomically,
};

#[cfg(all(test, target_os = "linux"))]
pub(crate) use anchored::LinuxDurability;
#[cfg(all(test, target_os = "macos"))]
pub(crate) use anchored::MacosDurability;
// DC-97: conformance.rs's own architecture -- one shared `assert_*` body, a thin per-platform
// `#[test]` wrapper naming a concrete type -- is what a new platform plugs into. Windows is that
// platform now.
#[cfg(all(test, target_os = "windows"))]
pub(crate) use anchored::WindowsDurability;
// DC-82: visible in test builds regardless of platform (`none`'s own gate), but only re-exported
// here where `fsutil::tests` — the only consumer — actually compiles. Still Linux/macOS-only even
// though `fsutil::tests` itself now also compiles on Windows (DC-97): `NoDurability` itself has no
// Windows arm in test builds either (`anchored.rs`'s own gate excludes it there, since Windows has a
// real `WindowsDurability` now) -- the one test that uses it stays inline-gated to match.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) use anchored::NoDurability;
// `remove_file_required` itself carries no platform gate at its own definition (it dispatches
// through `ACTIVE_DURABILITY`, which resolves per-platform internally). Unconditional since RFC 102
// Stage 6 Step 2's `unlock.rs` (design-v1.md §15.7 decision 3) is a genuine production caller, not
// just tests -- `prikk unlock` needs a real `Result`, not `remove_file_cleanup_best_effort`'s
// swallowed-error shape, since an operator-initiated removal that silently failed would be worse than
// an error surfaced.
pub(crate) use anchored::remove_file_required;
// DC-97: needed on Windows now too -- conformance.rs's shared `assert_*` functions take
// `impl DurabilityContract`, and that includes the new Windows wrapper.
#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(crate) use contract::DurabilityContract;

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(crate) use anchored::{TestFailPoint, fail_once_for_test};
// DC-98: see `anchored.rs`'s own re-export of this for the established Windows ordinals it exists
// to carry.
#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(crate) use anchored::fail_after_for_test;

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(crate) use anchored::set_directory_create_barrier_for_test;

#[cfg(all(test, target_os = "windows"))]
pub(crate) use anchored::set_anchor_verification_barrier_for_test;

/// Return a process-unique temporary path next to the destination.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) fn temporary_path(path: &Path) -> Result<PathBuf> {
    let Some(file_name) = path.file_name() else {
        return Err(PrikkError::Io {
            kind: None,
            context: "temporary path destination has no file name".to_string(),
        });
    };
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|error| PrikkError::Io {
        kind: None,
        context: format!("temporary path randomness failed: {error}"),
    })?;
    let mut name = file_name.to_os_string();
    name.push(format!(
        ".tmp.{}.{:032x}",
        std::process::id(),
        u128::from_le_bytes(random)
    ));
    Ok(path.with_file_name(name))
}

/// Convert a usize length to u16.
pub(crate) fn len_to_u16(len: usize) -> Result<u16> {
    u16::try_from(len).map_err(|_| PrikkError::MalformedData("length exceeds u16".to_string()))
}

/// Convert a usize length to u32.
pub(crate) fn len_to_u32(len: usize) -> Result<u32> {
    u32::try_from(len).map_err(|_| PrikkError::MalformedData("length exceeds u32".to_string()))
}

/// Convert a usize length to u64.
pub(crate) fn len_to_u64(len: usize) -> Result<u64> {
    u64::try_from(len).map_err(|_| PrikkError::MalformedData("length exceeds u64".to_string()))
}

/// List `relative`'s entries, sorted by raw name bytes, tolerating the directory's absence rather
/// than erroring the way `list_directory` itself does. Recovery and diagnostic surfaces (`unlock`,
/// `verify`) are consulted precisely when a repository may not be fully valid, so a bare
/// `list_directory` call on one of their required directories makes the tool fail on exactly the
/// damage it exists to handle.
///
/// Returns `None` when `dir` is absent at all, distinct from `Some(entries)` on an empty-but-present
/// directory -- the caller decides what to do with that distinction (this project's rule is that
/// absence must be reported explicitly somewhere, not silently treated as "nothing to report", so a
/// caller that gets `None` back is expected to have -- or feed -- a path for that report, not merely
/// discard it the same way it would discard an empty `Some`).
///
/// `label` names what the caller expects `dir` to be, used only in the error text if something other
/// than a directory occupies it -- that is still a real defect, not tolerated the way absence is.
///
/// This exact three-way match (`inspect_entry` then `list_directory` then a sort on
/// `name.as_encoded_bytes()`) already existed twice before this function --
/// `verify/objects.rs::scan_loose_file_temp_debris` and `layout.rs::active_session_names` -- as two
/// honest, uncoordinated copies. This is the shared home for a third and fourth call site
/// (`unlock.rs`'s per-ref lock listing, `refs/verify.rs`'s ref-candidate-debris scan), which
/// additionally need the `None` case surfaced rather than silently treated as empty; the two earlier
/// copies are unaffected and not migrated onto this by this change.
pub(crate) fn list_directory_tolerating_absence(
    root: &MutationRoot,
    dir: &Path,
    relative: &Path,
    label: &str,
) -> Result<Option<Vec<RootDirEntry>>> {
    match inspect_entry(root, relative)? {
        None => Ok(None),
        Some(EntryKind::Directory) => {
            let mut entries = list_directory(root, relative)?;
            entries.sort_by(|left, right| {
                left.name
                    .as_encoded_bytes()
                    .cmp(right.name.as_encoded_bytes())
            });
            Ok(Some(entries))
        }
        Some(_) => Err(PrikkError::Integrity(format!(
            "unexpected non-directory where {label} should be: {}",
            dir.display()
        ))),
    }
}
