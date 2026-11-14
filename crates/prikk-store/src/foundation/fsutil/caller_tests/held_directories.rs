//! 0.58.0 P2a: the table of held directories on `MutationRoot` (`anchored/held_directories.rs`).
//!
//! What these hold: every lock guard registers a hold; the table is absent outside a hold and cleared by the last release; a directory
//! created under a hold keeps its created-parent sync; two threads writing under one root share one consistent table.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use crate::foundation::fsutil::{
    ensure_directory_required, reset_sync_call_count_for_test, sync_call_count_for_test,
    write_file_atomically,
};
use crate::lock::acquire_container_locks;
use crate::test_gates::test_support::{dummy_signature, unique_temp_dir};
use crate::{ActiveLock, FileObjectStore, LockableContainer, RefLock, RepositoryLayout};
use prikk_object::{ObjectEnvelope, ObjectType};

fn signed_blob(payload: &[u8]) -> prikk_error::Result<ObjectEnvelope> {
    let mut object = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_vec());
    object.add_signature(dummy_signature())?;
    Ok(object)
}

/// Condition 1 (a rule over a class: every member). The three guard types, and the container guard's several handles, each register
/// one hold when taken and release it when dropped. They all take their lock file through `lock::acquire_lock_file`, which returns the
/// `DirectoryHold` a guard cannot be built without; this test counts, per type, so a type that stopped registering is named.
#[test]
fn every_lock_guard_type_registers_a_hold() -> prikk_error::Result<()> {
    let root = unique_temp_dir("held-directories-every-guard");
    let layout = RepositoryLayout::init(root.clone())?;
    let mutation = layout.repository_mutation_root();
    assert_eq!(mutation.held_directories_for_test(), (0, 0));

    let active = ActiveLock::acquire(&layout, "default")?;
    assert_eq!(mutation.held_directories_for_test().0, 1, "ActiveLock");

    let reference = RefLock::acquire(&layout, "heads/main")?;
    assert_eq!(mutation.held_directories_for_test().0, 2, "RefLock");

    let containers = acquire_container_locks(
        &layout,
        &[LockableContainer::ObjectStore, LockableContainer::RefLog],
    )?;
    assert_eq!(
        mutation.held_directories_for_test().0,
        4,
        "ContainerLockGuard: one hold per container lock"
    );

    drop(containers);
    assert_eq!(mutation.held_directories_for_test().0, 2);
    drop(reference);
    drop(active);
    assert_eq!(
        mutation.held_directories_for_test(),
        (0, 0),
        "the last release clears the table"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Condition 2: no table outside a hold; nested holds are one table; the last release clears it.
#[test]
fn the_table_is_absent_outside_a_hold_and_cleared_by_the_last_release() -> prikk_error::Result<()> {
    let root = unique_temp_dir("held-directories-absent");
    let layout = RepositoryLayout::init(root.clone())?;
    let mutation = layout.repository_mutation_root();

    // Outside any hold a write walks as it always did and keeps nothing.
    write_file_atomically(mutation, Path::new("refs/containers/p2a-outside"), b"x")?;
    ensure_directory_required(mutation, Path::new("p2a-outside-dir/inner"))?;
    assert_eq!(
        mutation.held_directories_for_test(),
        (0, 0),
        "no hold, no table"
    );

    let first = ActiveLock::acquire(&layout, "default")?;
    let (holds, held) = mutation.held_directories_for_test();
    assert_eq!(holds, 1);
    assert!(held >= 1, "the lock file's own walk is kept: {held}");

    let second = RefLock::acquire(&layout, "heads/main")?;
    drop(first);
    let (holds, held_after_first) = mutation.held_directories_for_test();
    assert_eq!(holds, 1, "one hold left");
    assert!(
        held_after_first >= 1,
        "releasing one of two holds keeps the table"
    );

    drop(second);
    assert_eq!(
        mutation.held_directories_for_test(),
        (0, 0),
        "the last release clears it"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A directory created under a hold keeps its created-parent sync (the `mkdirat` branch of `ensure_child`), and a directory opened
/// again under the hold costs no further sync. Creating `a/b` new costs 2 syncs (one per created component); the same call again, 0.
#[test]
fn a_directory_created_under_a_hold_keeps_its_created_parent_sync() -> prikk_error::Result<()> {
    let root = unique_temp_dir("held-directories-created");
    let layout = RepositoryLayout::init(root.clone())?;
    let mutation = layout.repository_mutation_root();
    let _hold = ActiveLock::acquire(&layout, "default")?;

    reset_sync_call_count_for_test();
    ensure_directory_required(mutation, Path::new("p2a-new/inner"))?;
    let creating = sync_call_count_for_test();
    reset_sync_call_count_for_test();
    ensure_directory_required(mutation, Path::new("p2a-new/inner"))?;
    let again = sync_call_count_for_test();
    assert_eq!(
        (creating, again),
        (CREATING_NEW_NESTED_DIRECTORY_SYNCS, 0),
        "creating two nested directories syncs each parent once; opening them again under the hold syncs nothing"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Syncs `ensure_directory_required` makes for a two-component path that does not exist: each created component's created-parent sync
/// (measured: 2).
const CREATING_NEW_NESTED_DIRECTORY_SYNCS: usize = 2;

/// Condition 5. Production never writes from two threads (no `spawn` in the store or the CLI); a library caller can, and the table is
/// shared by every clone of one root, behind one `Mutex`. One thread writes objects under the object-store lock while another holds a
/// ref lock and rewrites a file under `refs/containers`, both through one root: every write lands, and the table is empty afterwards.
#[test]
fn two_threads_writing_under_one_root_share_one_consistent_table() -> prikk_error::Result<()> {
    let root = unique_temp_dir("held-directories-threads");
    let layout = RepositoryLayout::init(root.clone())?;
    let writer_layout = layout.clone();
    let objects = std::thread::spawn(move || -> prikk_error::Result<Vec<ObjectEnvelope>> {
        let mut written = Vec::new();
        let guard = acquire_container_locks(&writer_layout, &[LockableContainer::ObjectStore])?;
        let mut session = crate::ObjectWriteSession::open(&writer_layout)?;
        for index in 0..40 {
            let object = signed_blob(format!("thread-object-{index}").as_bytes())?;
            session.write_object_under_held_lock(&object, &guard)?;
            written.push(object);
        }
        drop(guard);
        Ok(written)
    });
    let files_layout = layout.clone();
    let files = std::thread::spawn(move || -> prikk_error::Result<()> {
        let _ref_lock = RefLock::acquire(&files_layout, "heads/main")?;
        let mutation = files_layout.repository_mutation_root();
        for index in 0..40 {
            write_file_atomically(
                mutation,
                Path::new("refs/containers/p2a-thread-file"),
                format!("version {index}").as_bytes(),
            )?;
        }
        Ok(())
    });
    let written = objects.join().expect("the object thread")?;
    files.join().expect("the file thread")?;

    assert_eq!(
        std::fs::read_to_string(root.join(".prikk/refs/containers/p2a-thread-file")).unwrap(),
        "version 39"
    );
    let store = FileObjectStore::new(layout.clone());
    for object in &written {
        assert!(
            crate::ObjectReader::read_object(&store, object.object_id())?.is_some(),
            "an object written by the thread is readable"
        );
    }
    assert_eq!(
        layout
            .repository_mutation_root()
            .held_directories_for_test(),
        (0, 0),
        "both threads released: the table is empty"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
