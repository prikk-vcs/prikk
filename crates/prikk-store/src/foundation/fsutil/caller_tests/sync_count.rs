//! 0.55.0 step 3 Part 2: the sync-per-object-write accounting, measured directly with
//! `sync_call_count_for_test` rather than inferred by sweeping `fail_after` ordinals upward until
//! one stops firing. 0.58.0 P1: counted at the platform's sync primitive (files and directories), so the numbers below are the
//! kernel's, not a list's; the pins moved from 5 and `3 + 2N` to 10 and `5 + 5N` (the directory walk's syncs, uncounted before).

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

/// A single object write, outside any held lock, costs 10 syncs: 7 directories' and 3 files'. 0.58.0 P1 counts at the primitive, so the
/// directory walk's parent syncs (`open_validated_child`, `ensure_child`) are in the number; through 0.57.0 the counter named only
/// the `Point`s `is_sync_point` listed and read 5 (lock creation's file and directory sync, the container file, the index file, the
/// unlock's directory), which was 5 short of what the kernel saw. `strace -f -e trace=fsync,fdatasync` on a real `bundle import`
/// agrees with the new number per operation (`commit_and_seal::p1_sync_count_per_operation`).
#[test]
fn single_object_write_outside_a_held_lock_costs_ten_syncs() -> prikk_error::Result<()> {
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
        (10, 7),
        "5 for the lock (1 file, 4 directories) + 5 for the object (2 files, 3 directories)"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Inside a loop that holds the `ObjectStore` container lock once (`write_object_under_held_lock`, the shape `archive import`,
/// `bundle import`, `sync accept` and `commit` use), the lock's cost is paid once: N objects cost `5 + 5*N` syncs. The fixed 5 is the
/// lock (1 file, 4 directories). The marginal 5 is 2 files (container, index) and 3 directories (the walk's parent syncs for the
/// object's path). Through 0.57.0 this read `3 + 2*N`: the marginal 3 were the walk, uncounted. Measured at N = 1, 10 and 100, so the
/// marginal cost is shown flat at 5. **This is P2's baseline**: P2's change is the 3 directories per object.
#[test]
fn held_lock_loop_costs_five_plus_five_per_object() -> prikk_error::Result<()> {
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
            (5 + 5 * object_count, 4 + 3 * object_count),
            "object_count {object_count}: the lock (1 file + 4 directories) once, then per object 2 files + 3 directories"
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}
