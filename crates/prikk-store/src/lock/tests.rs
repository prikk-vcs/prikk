use std::fs;

use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    ActiveLock, DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryLayout, acquire_container_locks,
};

#[test]
fn failed_lock_directory_sync_retains_stale_lock() {
    let root = unique_temp_dir("lock-directory-sync-failure");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let path = layout.default_active_lock_path();
        fail_once_for_test(TestFailPoint::RequiredDirectorySync);
        assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_err());
        assert!(path.is_file());
        assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_err());
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_lock_file_sync_retains_stale_lock() -> prikk_error::Result<()> {
    let root = unique_temp_dir("lock-file-sync-failure");
    let layout = RepositoryLayout::init(root.clone())?;
    let path = layout.default_active_lock_path();
    fail_once_for_test(TestFailPoint::RequiredFileSync);
    assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_err());
    assert!(path.is_file());
    assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_err());
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3a control 1, `ActiveLock`'s half: a non-UTF-8 session name must reach
/// `ActiveLock::acquire` and produce a byte-exact lock path. Unlike `Wal::for_layout`,
/// `ActiveLock::acquire` already derived its `relative` field from `path` via `repository_relative`
/// rather than reconstructing it from `name` -- this pins that it keeps working once `name` is
/// widened to `impl AsRef<Path>`, it does not newly fix anything here.
///
/// **Gated to `target_os = "linux"`, not `unix`** -- see the sibling test in `wal/tests.rs` for why.
#[cfg(target_os = "linux")]
#[test]
fn active_lock_acquires_a_byte_exact_path_for_a_non_utf8_session_name() -> prikk_error::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let root = unique_temp_dir("active-lock-non-utf8-name");
    let layout = RepositoryLayout::init(root.clone())?;
    let bad_name = std::ffi::OsStr::from_bytes(b"bad\xFFname");
    std::fs::create_dir_all(layout.active_session_dir(bad_name))?;

    let lock = ActiveLock::acquire(&layout, bad_name)?;
    assert_eq!(lock.path(), layout.active_lock_path(bad_name).as_path());
    assert!(layout.active_lock_path(bad_name).is_file());
    assert!(ActiveLock::acquire(&layout, bad_name).is_err());

    drop(lock);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Holding a container lock creates its file; dropping the guard removes it -- the same round trip
/// `ActiveLock`/`RefLock` already prove, extended to the new container locks.
#[test]
fn container_lock_creates_its_file_and_releases_it_on_drop() -> prikk_error::Result<()> {
    let root = unique_temp_dir("container-lock-round-trip");
    let layout = RepositoryLayout::init(root.clone())?;
    let path = layout.lockable_container_lock_path(LockableContainer::ReceivedIndex);
    {
        let _guard = acquire_container_locks(&layout, &[LockableContainer::ReceivedIndex])?;
        assert!(path.is_file());
    }
    assert!(!path.is_file());
    let _ = fs::remove_dir_all(root);
    Ok(())
}

/// design-v1.md §15.7's deadlock ruling: the acquisition order is the sorted `LockableContainer::Ord`
/// order, not the caller's argument order -- proven by requesting the same pair in both orders and
/// getting the same result (success) either way, not by inspecting acquisition order directly (which
/// would couple the test to an implementation detail rather than the externally observable contract).
#[test]
fn requesting_a_pair_in_either_argument_order_succeeds_identically() -> prikk_error::Result<()> {
    for containers in [
        [
            LockableContainer::RefLog,
            LockableContainer::RefPointerIndex,
        ],
        [
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
        ],
    ] {
        let root = unique_temp_dir("container-lock-order-independence");
        let layout = RepositoryLayout::init(root.clone())?;
        let guard = acquire_container_locks(&layout, &containers);
        assert!(guard.is_ok());
        drop(guard);
        assert!(
            !layout
                .lockable_container_lock_path(LockableContainer::RefPointerIndex)
                .is_file()
        );
        assert!(
            !layout
                .lockable_container_lock_path(LockableContainer::RefLog)
                .is_file()
        );
        let _ = fs::remove_dir_all(root);
    }
    Ok(())
}

/// A container already locked refuses a second acquisition -- the exclusion property Step 2 exists to
/// add.
#[test]
fn a_held_container_lock_refuses_a_second_acquisition() -> prikk_error::Result<()> {
    let root = unique_temp_dir("container-lock-conflict");
    let layout = RepositoryLayout::init(root.clone())?;
    let first = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy])?;
    let second = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy]);
    assert!(
        second.is_err(),
        "a second acquisition while held must refuse"
    );
    if let Err(error) = second {
        // RFC 132 follow-up: an already-held container lock is a genuine lock conflict -- another
        // writer is active, and retrying (once it releases) is exactly the right response, unlike
        // the six sites this taxonomy round moved off `LockConflict`.
        assert!(
            matches!(error, prikk_error::PrikkError::LockConflict(_)),
            "unexpected error variant: {error:?}"
        );
        assert!(
            error.to_string().starts_with("lock conflict:"),
            "unexpected error: {error}"
        );
        assert!(
            error.to_string().contains("lock already exists"),
            "unexpected error: {error}"
        );
    }
    drop(first);
    assert!(acquire_container_locks(&layout, &[LockableContainer::TrustPolicy]).is_ok());
    let _ = fs::remove_dir_all(root);
    Ok(())
}

/// The rollback property the helper's own doc promises: if the second lock in sorted order (`RefLog`,
/// since `RefPointerIndex < RefLog`) is already held by someone else, the whole call fails **and** the
/// `RefPointerIndex` lock it acquired first is released, not leaked -- the wedge-avoidance property, not
/// just "the call returns an error."
#[test]
fn a_partial_failure_releases_every_lock_already_acquired_in_that_call() -> prikk_error::Result<()>
{
    let root = unique_temp_dir("container-lock-partial-failure-rollback");
    let layout = RepositoryLayout::init(root.clone())?;
    let held_elsewhere = acquire_container_locks(&layout, &[LockableContainer::RefLog])?;

    let result = acquire_container_locks(
        &layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
        ],
    );
    assert!(result.is_err());
    assert!(
        !layout
            .lockable_container_lock_path(LockableContainer::RefPointerIndex)
            .is_file(),
        "the lock acquired before the failing one must not be leaked"
    );
    assert!(
        layout
            .lockable_container_lock_path(LockableContainer::RefLog)
            .is_file(),
        "the pre-existing holder's own lock must be untouched"
    );

    drop(held_elsewhere);
    let _ = fs::remove_dir_all(root);
    Ok(())
}

/// The declared total order itself, checked directly rather than only through its effects: the whole
/// deadlock-avoidance design (design-v1.md §15.7/§15.8) rests on `LockableContainer`'s `Ord` being
/// exactly `RefPointerIndex < RefLog < ReceivedIndex < TrustPolicy`, the one order every multi-
/// container acquisition sorts into. A future reordering of the enum's variants would silently change
/// this without any other test here noticing, since the other tests only prove acquisition succeeds,
/// not which order it happened in.
#[test]
fn lockable_container_ord_matches_the_declared_total_order() {
    let mut containers = [
        LockableContainer::TrustPolicy,
        LockableContainer::RefLog,
        LockableContainer::ReceivedIndex,
        LockableContainer::RefPointerIndex,
    ];
    containers.sort_unstable();
    assert_eq!(
        containers,
        [
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
            LockableContainer::TrustPolicy,
        ]
    );
}

/// Requesting the same container twice in one call is deduplicated, not double-acquired against
/// itself (which would otherwise deadlock a single caller against its own first acquisition).
#[test]
fn duplicate_containers_in_one_request_are_deduplicated() -> prikk_error::Result<()> {
    let root = unique_temp_dir("container-lock-dedup");
    let layout = RepositoryLayout::init(root.clone())?;
    let guard = acquire_container_locks(
        &layout,
        &[
            LockableContainer::ReceivedIndex,
            LockableContainer::ReceivedIndex,
        ],
    );
    assert!(guard.is_ok());
    drop(guard);
    let _ = fs::remove_dir_all(root);
    Ok(())
}

/// RFC 132 follow-up: `ActiveLock::require_layout` refuses an `ActiveLock` acquired against a
/// different repository's own mutation authority -- a genuine lock conflict (the lock in hand is
/// simply the wrong one for this repository), not a caller precondition. No production call site
/// constructs this cross-repository mismatch through ordinary use (`active.rs`/`refs.rs` both call
/// it with the same `layout` the lock was just acquired from); exercised directly here against two
/// independently initialized repositories, the only way to construct the mismatch at all.
#[test]
fn active_lock_require_layout_refuses_a_different_repository_authority() -> prikk_error::Result<()>
{
    let root_a = unique_temp_dir("active-lock-authority-a");
    let root_b = unique_temp_dir("active-lock-authority-b");
    let layout_a = RepositoryLayout::init(root_a.clone())?;
    let layout_b = RepositoryLayout::init(root_b.clone())?;
    let active_lock = ActiveLock::acquire(&layout_a, DEFAULT_ACTIVE_NAME)?;

    let result = active_lock.require_layout(&layout_b);
    assert!(
        result.is_err(),
        "a lock from a different repository authority must refuse"
    );
    if let Err(error) = result {
        assert!(
            matches!(error, prikk_error::PrikkError::LockConflict(_)),
            "unexpected error variant: {error:?}"
        );
        assert!(
            error.to_string().starts_with("lock conflict:"),
            "unexpected error: {error}"
        );
        assert!(
            error
                .to_string()
                .contains("active lock belongs to a different repository authority"),
            "unexpected error: {error}"
        );
    }

    assert!(active_lock.require_layout(&layout_a).is_ok());

    drop(active_lock);
    let _ = fs::remove_dir_all(root_a);
    let _ = fs::remove_dir_all(root_b);
    Ok(())
}
