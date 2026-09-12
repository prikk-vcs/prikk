//! Root-scoped filesystem mutation primitives. Every function here is a thin, **unconditional** call
//! through the durability contract (DC-76, `super::contract::DurabilityContract`) — the guarantee
//! each one provides is stated on the trait method it calls, not repeated here. `Linux`
//! (`linux::LinuxDurability`), `Macos` (`macos::MacosDurability`, DC-81), `Windows`
//! (`windows::WindowsDurability`, DC-87 Stage 2), and `NoDurability` (`none::NoDurability`, every
//! method an "unsupported" error, for every remaining target) are the implementors.
//! `ACTIVE_DURABILITY` below is the single gated constant that picks among them; no `target_os` gate
//! appears at any call site in this file (DC-82's bar) — a further platform is one more `#[cfg]` arm
//! on `ACTIVE_DURABILITY`, not one more arm at every one of these nine functions.

use std::path::Path;

use prikk_error::{PrikkError, Result};

mod directory;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod failpoints;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(
    all(test, not(target_os = "windows")),
    not(any(target_os = "linux", target_os = "macos", target_os = "windows"))
))]
mod none;
mod read;
mod regular;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
mod windows_authority;

// RFC 149 §6c closure: waypoint widened with its type.
pub use directory::MutationRoot;
// RFC 149 §6c: same split as `fsutil.rs`'s, one level down.
pub use read::read_file_if_exists;
pub(crate) use read::{
    EntryKind, RootDirEntry, RootFileStat, inspect_entry, list_directory, read_file_required,
    stat_file_state_if_exists,
};

use crate::foundation::fsutil::contract::DurabilityContract;
#[cfg(target_os = "linux")]
pub(in crate::foundation::fsutil) use linux::LinuxDurability;
#[cfg(target_os = "macos")]
pub(in crate::foundation::fsutil) use macos::MacosDurability;
#[cfg(any(
    all(test, not(target_os = "windows")),
    not(any(target_os = "linux", target_os = "macos", target_os = "windows"))
))]
pub(in crate::foundation::fsutil) use none::NoDurability;
#[cfg(target_os = "windows")]
pub(in crate::foundation::fsutil) use windows::WindowsDurability;

#[cfg(all(test, target_os = "windows"))]
pub(in crate::foundation::fsutil) use failpoints::set_anchor_verification_barrier as set_anchor_verification_barrier_for_test;
#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(in crate::foundation::fsutil) use failpoints::set_directory_create_barrier as set_directory_create_barrier_for_test;
// RFC 149 §6b: reachable under the feature too; `pub`, because a `pub(crate)` waypoint cannot be
// re-exported from `lib.rs` (E0364).
#[cfg(all(
    any(test, feature = "test-support"),
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub use failpoints::{Point as TestFailPoint, fail_once as fail_once_for_test};
// DC-98: `windows/tests.rs::object_write_sync_failure_retains_and_classifies_windows` needs a
// specific skip-count -- `RequiredFileSync` at skip 0 (container append's own sync) and skip 1
// (index append's own sync), the same two ordinals `caller_tests::sync_matrix`'s Unix original
// uses. Confirmed identical on Windows by a probe (`.git-exclude/reviewed/DC-98-stage-2-followups-\
// ruling-v1.md` §2, CI run `31983187612`) before this widening, not assumed from the call graph.
#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(crate) use failpoints::fail_after as fail_after_for_test;

/// The one gated symbol DC-82 exists to introduce: picks the active `DurabilityContract`
/// implementor for this build. Every function below calls through it unconditionally.
#[cfg(target_os = "linux")]
const ACTIVE_DURABILITY: LinuxDurability = LinuxDurability;
#[cfg(target_os = "macos")]
const ACTIVE_DURABILITY: MacosDurability = MacosDurability;
#[cfg(target_os = "windows")]
const ACTIVE_DURABILITY: WindowsDurability = WindowsDurability;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const ACTIVE_DURABILITY: NoDurability = NoDurability;

/// Write mutable metadata through a unique same-directory temporary file.
pub(crate) fn write_file_atomically(
    root: &MutationRoot,
    relative: &Path,
    bytes: &[u8],
) -> Result<()> {
    ACTIVE_DURABILITY.atomic_replace(root, relative, bytes)
}

/// Write a worktree file through its retained worktree-root authority.
pub(crate) fn write_worktree_file_atomically(
    root: &MutationRoot,
    relative: &Path,
    bytes: &[u8],
) -> Result<()> {
    write_file_atomically(root, relative, bytes)
}

/// Append bytes, sync the file, and always re-establish retained-parent durability.
pub(crate) fn append_file_required(
    root: &MutationRoot,
    relative: &Path,
    bytes: &[u8],
) -> Result<()> {
    ACTIVE_DURABILITY.durable_append(root, relative, bytes)
}

/// Truncate an existing regular file to a retained length and sync its parent.
pub(crate) fn truncate_existing_file_required(
    root: &MutationRoot,
    relative: &Path,
    len: u64,
) -> Result<()> {
    ACTIVE_DURABILITY.durable_truncate(root, relative, len)
}

/// Set an existing regular file's mode bits (DC-73: worktree materialization needs to write the
/// mode a `CreateFile`/`ChangePerm` operation recorded, not whatever the anchored create primitive
/// defaults new files to). No-follow, matching every other anchored open — a symlink at the final
/// component is refused rather than chmod'd through.
pub(crate) fn set_regular_file_mode_required(
    root: &MutationRoot,
    relative: &Path,
    mode: u32,
) -> Result<()> {
    ACTIVE_DURABILITY.set_permission_bits(root, relative, mode)
}

/// Create or truncate a regular file, then sync it and its retained parent.
pub(crate) fn truncate_file_empty_required(root: &MutationRoot, relative: &Path) -> Result<()> {
    ACTIVE_DURABILITY.durable_truncate_to_empty(root, relative)
}

/// Create, write, and durably publish an exclusive regular file.
pub(crate) fn create_new_file_required(
    root: &MutationRoot,
    relative: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    ACTIVE_DURABILITY.create_exclusive(root, relative, bytes)
}

/// Remove a file and sync the exact parent handle that owned the unlink.
pub(crate) fn remove_file_required(root: &MutationRoot, relative: &Path) -> Result<()> {
    remove_file_if_present_required(root, relative).map(|_| ())
}

/// Remove a file if present and sync its exact parent even for observed absence.
pub(crate) fn remove_file_if_present_required(
    root: &MutationRoot,
    relative: &Path,
) -> Result<bool> {
    ACTIVE_DURABILITY.remove_if_present(root, relative)
}

/// Attempt fallible removal where a destructor cannot report the result.
pub(crate) fn remove_file_cleanup_best_effort(root: &MutationRoot, relative: &Path) {
    let _ = remove_file_required(root, relative);
}

/// Remove a worktree file through its retained worktree-root authority.
pub(crate) fn remove_worktree_file_required(root: &MutationRoot, relative: &Path) -> Result<()> {
    remove_file_required(root, relative)
}

/// Ensure a relative directory tree exists, durably, tolerating a concurrent creator (G8).
pub(crate) fn ensure_directory_required(root: &MutationRoot, relative: &Path) -> Result<()> {
    ACTIVE_DURABILITY.ensure_directory(root, relative)
}

/// Durably confirm that `relative` — an existing regular file — is recorded in its containing
/// directory (DC-88). `relative` names the entry to confirm, not the directory to sync.
pub(crate) fn sync_directory_required(root: &MutationRoot, relative: &Path) -> Result<()> {
    ACTIVE_DURABILITY.durable_directory_entry(root, relative)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn io_error(error: rustix::io::Errno) -> PrikkError {
    PrikkError::from(std::io::Error::from(error))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn prikk_to_io(error: PrikkError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

/// Shared by `directory.rs`'s `PathOnlyAuthority` (`MutationRoot::ensure_root`'s fallback,
/// unrelated to the durability dispatch above) and `none::NoDurability` (every method). Gated to
/// match both callers' own gate exactly — visible in test builds on every target that is not
/// Windows (Windows has its own real authority and implementor, in test builds too), and for real
/// on every target that is none of Linux, macOS, or Windows.
#[cfg(any(
    all(test, not(target_os = "windows")),
    not(any(target_os = "linux", target_os = "macos", target_os = "windows"))
))]
fn unsupported_mutation<T>() -> Result<T> {
    Err(PrikkError::Io {
        kind: None,
        context: "repository mutation requires Linux, macOS, or Windows root-scoped filesystem \
         capabilities"
            .to_string(),
    })
}
