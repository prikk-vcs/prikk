//! DC-96 Windows Anchor Identity: the positive control (design-v1.md §6.3) -- ordinary operation,
//! no anchor swap, must still succeed. Without this, a `verify_anchor` that always refuses would
//! pass acceptance criterion 1 (the two negative-control tests would just never run their write)
//! for the wrong reason.

use std::path::Path;
use std::sync::{Arc, Barrier};

use crate::foundation::fsutil::{
    MutationRoot, create_new_file_required, read_file_if_exists,
    set_anchor_verification_barrier_for_test,
};
use crate::test_gates::test_support::unique_temp_dir;

fn mutation_root(path: &Path) -> MutationRoot {
    match MutationRoot::open(path) {
        Ok(root) => root,
        Err(error) => panic!("test mutation root failed: {error}"),
    }
}

#[test]
fn identity_verification_does_not_disturb_ordinary_operation() {
    let root_path = unique_temp_dir("windows-authority-positive-control");
    let root = mutation_root(&root_path);
    let relative = Path::new("state");

    assert!(
        create_new_file_required(&root, relative, b"unchanged anchor").is_ok(),
        "a write through the same, unswapped anchor must still succeed"
    );
    assert_eq!(
        read_file_if_exists(&root, relative).ok().flatten(),
        Some(b"unchanged anchor".to_vec()),
        "a read through the same, unswapped anchor must still see what was written"
    );

    let _ = std::fs::remove_dir_all(root_path);
}

/// DC-96 implementation-ruling-v1 §5: §8.4 reopened by retaining a handle -- confirm with a test,
/// not by reading the share flags. `WindowsAuthority::bind` now keeps a directory handle open for
/// the authority's whole lifetime; every open in this backend already requests
/// `FILE_SHARE_DELETE` (`windows.rs`'s own module doc), which is specifically what lets Windows
/// rename or delete a path while another handle holds it open. If that ever regressed, a user
/// renaming their own repository while prikk holds it open would start failing with a sharing
/// violation instead of the retained handle simply following the rename, as it must.
#[test]
fn a_users_own_rename_of_the_anchor_succeeds_while_the_handle_is_retained() {
    let root_path = unique_temp_dir("windows-authority-rename-not-blocked");
    let root = mutation_root(&root_path);
    let renamed_path = root_path.with_extension("renamed-by-user");
    let _ = std::fs::remove_dir_all(&renamed_path);

    let rename_result = std::fs::rename(&root_path, &renamed_path);

    assert!(
        rename_result.is_ok(),
        "a user must be able to rename the repository root even while prikk retains an open \
         handle to it -- FILE_SHARE_DELETE exists exactly for this: {rename_result:?}"
    );

    drop(root);
    let _ = std::fs::remove_dir_all(&renamed_path);
}

/// RFC 106: constructs the race `verified_anchor_path`'s identity comparison exists to catch --
/// DC-99's own negative control found no test depended on it (936/936 with the comparison
/// neutralised). One thread blocks at `failpoints::wait_at_anchor_verification` with
/// `current_path` already captured from the retained handle; while it is blocked, this thread
/// renames the anchor aside (not an ancestor -- NTFS refuses that while a descendant handle is
/// open, but permits renaming the held object itself, as
/// `full_verification_retains_wal_objects_trust_and_recovery_diagnosis_after_root_replacement`
/// already demonstrates) and creates a replacement directory at the original path; the blocked
/// thread then resumes, opens the replacement, and must refuse it by name -- `is_err()` alone
/// would also pass if the open failed for an unrelated reason, so the specific diagnostic is
/// asserted.
#[test]
fn a_replacement_installed_between_path_re_derivation_and_open_is_refused()
-> prikk_error::Result<()> {
    let root_path = unique_temp_dir("windows-authority-anchor-race");
    let root = mutation_root(&root_path);
    let aside_path = root_path.with_extension("raced-aside");
    let _ = std::fs::remove_dir_all(&aside_path);

    let barrier = Arc::new(Barrier::new(2));

    let thread_root = root.clone();
    let thread_barrier = Arc::clone(&barrier);
    let handle = std::thread::spawn(move || {
        // `ANCHOR_VERIFICATION_BARRIER` is a thread-local: it must be installed on the thread that
        // will call `wait_at_anchor_verification`, not on the driver thread that merely constructs
        // it. Installing it before spawning would leave the spawned thread's own slot empty, so
        // its `wait_at_anchor_verification` call would find nothing and run straight through --
        // and the driver would then block on `barrier.wait()` for a party that never arrives, a
        // hang rather than a failure (ruled in `RFC-106-implementation-ruling-v1.md` §1).
        set_anchor_verification_barrier_for_test(thread_barrier);
        create_new_file_required(&thread_root, Path::new("state"), b"raced")
    });

    // Rendezvous 1: the operation thread's first `wait()` (inside
    // `wait_at_anchor_verification`) only happens after `current_path_of` above it has already
    // returned, so this proving it has been reached is what lets the rename below run strictly
    // after the retained path was captured, not before -- the ordering the race depends on.
    barrier.wait();
    let rename_result = std::fs::rename(&root_path, &aside_path);
    let create_result = std::fs::create_dir(&root_path);
    // Rendezvous 2: releases the operation thread now that the replacement is in place.
    barrier.wait();

    let join_result = handle.join().map_err(|_| prikk_error::PrikkError::Io {
        kind: None,
        context: "anchor race thread panicked".to_string(),
    })?;

    let _ = std::fs::remove_dir_all(&root_path);
    let _ = std::fs::remove_dir_all(&aside_path);

    rename_result.map_err(|error| prikk_error::PrikkError::Io {
        kind: None,
        context: format!("renaming the anchor aside: {error}"),
    })?;
    create_result.map_err(|error| prikk_error::PrikkError::Io {
        kind: None,
        context: format!("creating the replacement anchor: {error}"),
    })?;

    match join_result {
        Ok(()) => panic!(
            "an operation that opens a replacement installed at the anchor's own path must be \
             refused, not succeed against the impostor"
        ),
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("Windows anchor replaced"),
                "expected the specific anchor-replaced diagnostic, not merely any error: {message}"
            );
        }
    }

    Ok(())
}
