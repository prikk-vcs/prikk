//! 0.55.0 step 3 Part 2: the sync-per-object-write accounting, measured directly with
//! `sync_call_count_for_test` rather than inferred by sweeping `fail_after` ordinals upward until
//! one stops firing.

use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::fsutil::{reset_sync_call_count_for_test, sync_call_count_for_test};
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

/// A single object write, outside any held lock, now costs 5 real syncs (lock creation's file
/// and directory sync, the container append's file sync, the index append's file sync, and the
/// unlock's directory sync) -- down from 7, because the container and index appends no longer
/// sync the directory at all (this part's own change). This is the red control the handoff asks
/// for: a one-line revert of that change (confirmed by hand, see the Part 2 report) makes this
/// instrument read 7 again.
#[test]
fn single_object_write_outside_a_held_lock_costs_five_syncs() -> prikk_error::Result<()> {
    let root = unique_temp_dir("object-sync-count-single");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let object = signed_blob(b"sync-count-single")?;
    reset_sync_call_count_for_test();
    store.write_object(&object)?;
    assert_eq!(
        sync_call_count_for_test(),
        5,
        "lock creation (file+directory) + container file sync + index file sync + unlock \
         (directory) = 5"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Inside a loop that holds the `ObjectStore` container lock once (`write_object_under_held_lock`,
/// the shape `archive import` already uses and this part adds to `bundle import`/`sync accept`/
/// `commit`), the lock-creation and unlock cost is paid once for the whole loop, not once per
/// object -- so N objects cost `3 + 2*N` syncs: the one lock creation (2) and one unlock (1),
/// amortized, plus 2 real syncs (container file, index file) per object. Measured at N = 1, 10 and
/// 100 to show the marginal cost is flat at 2, not merely "less than 7" at one data point.
#[test]
fn held_lock_loop_amortizes_lock_overhead_to_two_syncs_per_object() -> prikk_error::Result<()> {
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
        let expected = 3 + 2 * object_count;
        assert_eq!(
            sync_call_count_for_test(),
            expected,
            "object_count {object_count}: one lock creation (2) + one unlock (1) + 2 per object"
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}
