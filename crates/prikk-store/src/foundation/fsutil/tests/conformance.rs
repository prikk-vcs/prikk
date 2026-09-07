//! DC-76: the durability contract's conformance suite. Every `assert_*` function here is generic
//! over `impl DurabilityContract`, so a future platform's implementation is checked by the *same*
//! assertion logic, not a re-derived parallel suite — the whole argument for stating a contract
//! rather than leaving the guarantee implicit in one platform's own implementation. **Correcting an
//! overclaim the architect flagged at DC-81 addendum-1**: the two `#[test]` entry points below are
//! *not* themselves generic — each is a thin, platform-gated wrapper naming a concrete type
//! (`&LinuxDurability` under `target_os = "linux"`, `&MacosDurability` under `target_os = "macos"`),
//! because a `#[test]` function has no type parameter to be generic over. What is shared is the
//! `assert_*` body every wrapper calls into — that is the "same code" this suite's argument rests
//! on, not the `#[test]` function itself.
//!
//! **Coverage map — where each guarantee is conformance-tested, new or pre-existing:**
//!
//! | Guarantee | Test |
//! |---|---|
//! | G1 (root-anchored, no-follow) | pre-existing: `tests::directory::required_directory_rejects_symlink_component` |
//! | G2 (atomic content replacement) | pre-existing: `tests::mutable_atomic_write_replaces_complete_content` |
//! | G3 (durable-after-return) | pre-existing only, deliberately: the `fsutil::tests`/`caller_tests` failpoint suite (DC-41). No new test is added for it here — a unit test cannot observe an fsync's real effect without an actual crash, so the only way to pin "this survives a crash" is failpoint injection proving fail-safe error propagation at each sync point, which is exactly what the existing suite already does. An earlier draft of this file added a test that opened a *second* `MutationRoot` and re-read the file to stand in for "surviving a restart" — it passed even with the `fsync` call deleted, because without a real crash nothing forces the write out of the page cache. Removed once that was discovered by trying the negative control, not asserted; recorded here so the same mistake is not repeated |
//! | G4 (exclusive creation) | new: [`create_exclusive_refuses_an_already_occupied_path`] — no prior direct coverage found. DC-97: also runs on Windows now, same shared assertion body, `&WindowsDurability` |
//! | G5 (race-safe no-clobber publication) | pre-existing: `object_store::tests::immutable::*` |
//! | G6 (regular-file validation) | pre-existing, alongside G7: `tests::append_and_truncate_reject_fifo_without_blocking` |
//! | G7 (non-blocking opens) | same as G6 |
//! | G8 (concurrent-safe directory creation) | pre-existing: `tests::directory::concurrent_required_directory_creation_is_idempotent` |
//! | G9 (mode-bit isolation) | new: [`set_permission_bits_masks_file_type_bits_out_of_a_recorded_mode`] proves the accepted-input shape; **not independently negative-controllable on Linux** — see the trait doc comment on `set_permission_bits` for why. **DC-97: not given a Windows wrapper** — this assert function reads back POSIX mode bits (`PermissionsExt::mode`), which do not exist on Windows, so it cannot be reused unchanged; Windows' own no-op shape is covered separately by `windows::tests::set_permission_bits_is_a_documented_noop`, which asserts the property that actually applies there (content and existence unaffected) rather than a mode value that would never be meaningful |
//! | `durable_directory_entry`'s restated parameter shape (DC-88) | new: [`durable_directory_entry_accepts_the_named_files_own_path`] proves the interface change — `relative` is the file to confirm, not its parent — by calling with a file's own path and asserting success. This is not a G3 durability-under-crash proof (the existing G3 row's reasoning still applies unchanged); it is a parameter-resolution correctness check for the restatement itself |

// DC-97: G9's assert function is the only user, and G9 has no Windows wrapper (see the coverage map
// above) -- gated to match rather than pulled in unconditionally now that this whole file compiles
// on Windows too.
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[cfg(target_os = "linux")]
use crate::foundation::fsutil::LinuxDurability;
#[cfg(target_os = "macos")]
use crate::foundation::fsutil::MacosDurability;
#[cfg(target_os = "windows")]
use crate::foundation::fsutil::WindowsDurability;
use crate::foundation::fsutil::{DurabilityContract, MutationRoot};
use crate::test_gates::test_support::unique_temp_dir;

fn mutation_root(path: &Path) -> MutationRoot {
    match MutationRoot::open(path) {
        Ok(root) => root,
        Err(error) => panic!("test mutation root failed: {error}"),
    }
}

/// G4: creating the same path twice through `create_exclusive` must refuse the second attempt
/// rather than silently overwrite the first's content — the guarantee has no room for "usually".
fn assert_create_exclusive_refuses_an_already_occupied_path(durability: &impl DurabilityContract) {
    let path = unique_temp_dir("conformance-create-exclusive");
    let root = mutation_root(&path);
    let relative = Path::new("object");

    assert!(
        durability
            .create_exclusive(&root, relative, b"first")
            .is_ok(),
        "first create must succeed"
    );
    let second = durability.create_exclusive(&root, relative, b"second");
    assert!(
        second.is_err(),
        "second create at the same path must be refused"
    );
    assert_eq!(
        std::fs::read(path.join("object")).unwrap_or_default(),
        b"first",
        "the refused second create must not have touched the first's content"
    );
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(target_os = "linux")]
#[test]
fn create_exclusive_refuses_an_already_occupied_path() {
    assert_create_exclusive_refuses_an_already_occupied_path(&LinuxDurability);
}

#[cfg(target_os = "macos")]
#[test]
fn create_exclusive_refuses_an_already_occupied_path() {
    assert_create_exclusive_refuses_an_already_occupied_path(&MacosDurability);
}

// DC-97: the third platform this file's own architecture was designed to plug in -- no changes to
// `assert_create_exclusive_refuses_an_already_occupied_path` itself, same shared assertion body.
#[cfg(target_os = "windows")]
#[test]
fn create_exclusive_refuses_an_already_occupied_path() {
    assert_create_exclusive_refuses_an_already_occupied_path(&WindowsDurability);
}

/// G9: `set_permission_bits` must accept a "recorded mode" carrying file-type bits (exactly the
/// shape a sealed `CreateFile`/`ChangePerm` operation's `mode` field has, e.g. `0o100_755` for a
/// regular file) without corrupting the file's actual type, and the permission bits it applies
/// must match what was asked, masked to `0o7777`. This proves the *input shape* is accepted and
/// the *output* is correct — it does not, and per the trait doc comment cannot, prove the masking
/// itself is load-bearing on Linux: a reverted negative control (dropping the `& 0o7777` mask)
/// left this exact assertion passing, because `fchmod` already ignores non-permission bits at the
/// kernel level regardless of what this code does.
///
/// DC-97: not given a Windows wrapper -- unlike G4's, this function reads back POSIX mode bits
/// (`PermissionsExt::mode`, gated above), which do not exist on Windows. `set_permission_bits` is a
/// documented no-op there (NTFS has no execute bit), so there is no mode to read back; the coverage
/// map at the top of this file names where Windows' own, differently-shaped assertion lives instead.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn assert_set_permission_bits_masks_file_type_bits_out_of_a_recorded_mode(
    durability: &impl DurabilityContract,
) {
    let path = unique_temp_dir("conformance-set-permission-bits");
    let root = mutation_root(&path);
    let relative = Path::new("object");
    assert!(
        durability
            .create_exclusive(&root, relative, b"content")
            .is_ok()
    );

    // S_IFREG (0o100000) | 0o755 -- exactly the shape a recorded CreateFile/ChangePerm mode has.
    let recorded_mode = 0o100_755_u32;
    assert!(
        durability
            .set_permission_bits(&root, relative, recorded_mode)
            .is_ok(),
        "a mode carrying file-type bits must still be accepted"
    );

    let Ok(metadata) = std::fs::symlink_metadata(path.join("object")) else {
        panic!("stat after chmod failed");
    };
    assert!(
        metadata.file_type().is_file(),
        "file type must be unaffected by a permission-bit-setting call"
    );
    assert_eq!(
        metadata.permissions().mode() & 0o7777,
        0o755,
        "permission bits must match what was requested, masked to 0o7777"
    );
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(target_os = "linux")]
#[test]
fn set_permission_bits_masks_file_type_bits_out_of_a_recorded_mode() {
    assert_set_permission_bits_masks_file_type_bits_out_of_a_recorded_mode(&LinuxDurability);
}

#[cfg(target_os = "macos")]
#[test]
fn set_permission_bits_masks_file_type_bits_out_of_a_recorded_mode() {
    assert_set_permission_bits_masks_file_type_bits_out_of_a_recorded_mode(&MacosDurability);
}

/// DC-88: `durable_directory_entry`'s `relative` parameter now names the file whose directory entry
/// should be confirmed, not the directory to sync directly — the implementor resolves and syncs
/// `relative`'s parent internally. Calling with a nested file's own path (not its precomputed
/// parent) must succeed; before this restatement, passing a file path here would have attempted to
/// open that file with `OFlags::DIRECTORY` and failed.
fn assert_durable_directory_entry_accepts_the_named_files_own_path(
    durability: &impl DurabilityContract,
) {
    let path = unique_temp_dir("conformance-durable-directory-entry");
    let root = mutation_root(&path);
    let relative = Path::new("nested/object");
    assert!(
        durability
            .ensure_directory(&root, Path::new("nested"))
            .is_ok(),
        "parent directory must be creatable before the file it will contain"
    );
    assert!(
        durability
            .create_exclusive(&root, relative, b"content")
            .is_ok(),
        "file must be creatable under the freshly ensured directory"
    );

    assert!(
        durability.durable_directory_entry(&root, relative).is_ok(),
        "durable_directory_entry must accept the file's own path and resolve its parent, not \
         require the caller to precompute and pass the parent directly"
    );
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(target_os = "linux")]
#[test]
fn durable_directory_entry_accepts_the_named_files_own_path() {
    assert_durable_directory_entry_accepts_the_named_files_own_path(&LinuxDurability);
}

#[cfg(target_os = "macos")]
#[test]
fn durable_directory_entry_accepts_the_named_files_own_path() {
    assert_durable_directory_entry_accepts_the_named_files_own_path(&MacosDurability);
}

// DC-97: not one of the nine guarantees this stage is scoped to, but fully portable (no unix-only
// facility) and this file's whole point is that a new platform plugs into the shared body -- added
// rather than left as dead code needing its own gate for no technical reason.
#[cfg(target_os = "windows")]
#[test]
fn durable_directory_entry_accepts_the_named_files_own_path() {
    assert_durable_directory_entry_accepts_the_named_files_own_path(&WindowsDurability);
}
