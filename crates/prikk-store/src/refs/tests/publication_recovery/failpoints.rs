//! Candidate, pointer-promotion, and truncation failpoint cases.

use super::root_publication;
use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test, fail_once_for_test};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

#[test]
fn object_finalization_failures_precede_pointer_and_log_mutation() -> prikk_error::Result<()> {
    // RFC 102 Stage 3: the ref-state object write no longer goes through the old immutable-install
    // primitive (`ImmutableFileSync`/`ImmutableInstallSync`) at all -- it durably appends to its
    // container, then to the index, both through the same `RequiredFileSync` point the ref lock's
    // own creation already uses once. RFC 102 Stage 6 Step 2, design-v1.md §15.8, inserts two more
    // `RequiredFileSync` occurrences between the ref lock's own creation and the object write --
    // `acquire_container_locks`'s own `RefPointerIndex`/`RefLog` lock creations, hoisted to right
    // after `RefLock::acquire`. Skip 3 (not 1) to land on the container append (the object is not
    // even durably present); skip 4 (not 2) to land on the index append instead (the container
    // record is already durable, and the index write's own bytes are already on disk -- only its
    // own sync is interrupted -- so the object is visible to a same-process read exactly as the old
    // ImmutableInstallSync case's post-install cleanup-sync failure left it visible).
    for (skip, installed_after_error) in [(3, false), (4, true)] {
        let root = unique_temp_dir("dc38-object-finalization-retry");
        let layout = RepositoryLayout::init(root.clone())?;
        let publication = root_publication(&layout, "heads/main")?;
        let state_id = publication.ref_state.object_id();
        let store = RefStore::new(layout.clone());

        fail_after_for_test(TestFailPoint::RequiredFileSync, skip);
        assert!(store.publish(&publication).is_err());
        assert_eq!(
            FileObjectStore::new(layout.clone())
                .read_object(state_id)?
                .is_some(),
            installed_after_error
        );
        assert_eq!(store.read_current_ref_state_id("heads/main")?, None);
        assert_eq!(store.replay_log("heads/main")?.records.len(), 0);

        store.publish(&publication)?;
        assert_eq!(
            store.read_current_ref_state_id("heads/main")?,
            Some(state_id)
        );
        assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn partial_tail_truncate_failure_preserves_state_for_retry() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-partial-truncate-retry");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let store = RefStore::new(layout.clone());
    // RFC 102 Stage 3: skip past the ref-state object's own container and index appends (see the
    // sibling test above for the full count). RFC 102 Stage 4 adds the pointer-index append before
    // the log append too, so skip 3, not 2. First-ever publish, so no real record survives to
    // duplicate -- encode the pending update directly (`append_torn_ref_log_tail_for_test`'s doc).
    fail_after_for_test(TestFailPoint::AppendWrite, 3);
    assert!(store.publish(&publication).is_err());
    super::super::super::append_torn_ref_log_tail_for_test(
        &layout,
        crate::foundation::layout::ref_name_key_bytes("heads/main"),
        &publication.ref_update,
    )?;
    fail_once_for_test(TestFailPoint::Truncate);
    assert!(
        store
            .finish_interrupted_publication_for_test(&publication)
            .is_err()
    );
    assert_ne!(store.replay_log("heads/main")?.trailing_partial_bytes, 0);
    store.finish_interrupted_publication_for_test(&publication)?;
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
