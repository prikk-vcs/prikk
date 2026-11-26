//! 0.55.0 step 3 Part 2: the sync-per-object-write accounting, measured directly with
//! `sync_call_count_for_test` rather than inferred by sweeping `fail_after` ordinals upward until
//! one stops firing. 0.58.0 P1: counted at the platform's sync primitive (files and directories), so the numbers below are the
//! kernel's, not a list's; the pins moved from 5 and `3 + 2N` to 10 and `5 + 5N` (the directory walk's syncs, uncounted before), and 0.58.0 P2a to 7 and `5 + 2N`
//! (a directory opened once per hold).

use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::fsutil::{
    directory_sync_call_count_for_test, reset_sync_call_count_for_test, sync_call_count_for_test,
};
use crate::lock::acquire_container_locks;
use crate::test_gates::test_support::{dummy_signature, unique_temp_dir};
use crate::{
    FileObjectStore, LockableContainer, ObjectWriteSession, ObjectWriter, RepositoryLayout,
};

fn signed_blob(payload: &[u8]) -> prikk_error::Result<ObjectEnvelope> {
    let mut object = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_vec());
    object.add_signature(dummy_signature())?;
    Ok(object)
}

/// A single object write, outside any lock the caller holds, costs 5 syncs: 3 files' and 2 directories'. (0.59.0 P5b3a: the lock's file sync
/// stays and **its two directory syncs, at the create and at the removal, are gone**, so 7 became 5. 0.58.0 P2a: through P1 it was 10 =
/// 3 + 7: the write takes the object-store lock itself, and with the lock's hold open the container append and the index append share one
/// walk instead of making two. Through 0.57.0's counter it read 5 for what the kernel counted as 10.)
/// `strace -f -e trace=fsync,fdatasync` on a real `bundle import` agrees per operation (`commit_and_seal::p1_sync_count_per_operation`).
#[test]
fn single_object_write_outside_a_held_lock_costs_five_syncs() -> prikk_error::Result<()> {
    let root = unique_temp_dir("object-sync-count-single");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let object = signed_blob(b"sync-count-single")?;
    reset_sync_call_count_for_test();
    store.write_object(&object)?;
    assert_eq!(
        (
            sync_call_count_for_test(),
            directory_sync_call_count_for_test()
        ),
        (5, 2),
        "the lock (1 file, its body; 2 directories, its walk) + 2 files (container, index)"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// **A lock costs one file sync, and no directory sync of its own** (0.59.0 P5b3a, option C). Acquired and released, the active lock makes
/// exactly the syncs of its first walk (the directories its path passes through, 2) and the one sync of its **body**.
/// **Perturb:** the body's `sync_file` removed from the lock's create reads one fewer and goes red (the failpoint before it would not notice);
/// the old durable create and removal (`create_exclusive`, `remove_if_present`) read two more directories.
#[test]
fn a_lock_costs_one_file_sync_and_no_directory_sync_of_its_own() -> prikk_error::Result<()> {
    let root = unique_temp_dir("lock-sync-count");
    let layout = RepositoryLayout::init(root.clone())?;
    reset_sync_call_count_for_test();
    {
        let _lock = crate::ActiveLock::acquire(&layout, crate::DEFAULT_ACTIVE_NAME)?;
    }
    assert_eq!(
        (
            sync_call_count_for_test(),
            directory_sync_call_count_for_test()
        ),
        (3, 2),
        "the lock's body (1 file) + its first walk (2 directories), and nothing for its name"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Inside a loop that holds the `ObjectStore` container lock once (`write_object_under_held_lock`, the shape `archive import`,
/// `bundle import`, `sync accept` and `commit` use), N objects cost `3 + 2*N` syncs: the lock once (1 file, 2 directories), then per
/// object 2 files (container, index) and **no directory sync**, because the directories the object's path walks through are opened
/// once, and their parents synced once, at the first object under the hold. Measured at N = 1, 10 and 100, so the marginal cost is
/// shown flat at 2. Through P1 this read `5 + 5*N` (directories `4 + 3*N`); the 3 per object were the walk's parent syncs. Through P5b3a it read `5 + 2*N` (directories 4): the lock's creation and removal each synced a directory.
#[test]
fn held_lock_loop_costs_two_file_syncs_per_object_and_no_directory_sync() -> prikk_error::Result<()>
{
    for object_count in [1_usize, 10, 100] {
        let root = unique_temp_dir(&format!("object-sync-count-loop-{object_count}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let mut session = ObjectWriteSession::open(&layout)?;
        reset_sync_call_count_for_test();
        {
            let guard = acquire_container_locks(&layout, &[LockableContainer::ObjectStore])?;
            for index in 0..object_count {
                let object = signed_blob(format!("sync-count-loop-{index}").as_bytes())?;
                session.write_object_under_held_lock(&object, &guard)?;
            }
        }
        let total = sync_call_count_for_test();
        let directories = directory_sync_call_count_for_test();
        assert_eq!(
            (total, directories),
            (3 + 2 * object_count, 2),
            "object_count {object_count}: the lock (1 file + 2 directories) once, then per object 2 files and no directory"
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}
