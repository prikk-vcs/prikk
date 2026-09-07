//! DC-38 pointer-first publication and bounded recovery evidence.

mod candidate_cleanup;
mod compatibility;
mod failpoints;
mod partial_tail_refusal;
mod state_matrix;

use std::io::Write;

use super::super::{append_log_record_for_signature_test, append_torn_ref_log_tail_for_test};
use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test};
use crate::foundation::layout::ref_name_key_bytes;
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    ActiveSession, DEFAULT_ACTIVE_PATCH_LIMIT, DoctorRepairOptions, FileObjectStore, ObjectWriter,
    RefPublication, RefStore, RepositoryLayout, add_trusted_maintainer, doctor_repository,
    repair_repository, verify_repository,
};

fn root_publication(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> prikk_error::Result<RefPublication> {
    let block = signed_empty_block_envelope();
    let target = FileObjectStore::new(layout.clone()).write_object(&block)?;
    let ref_state = signed_ref_state_envelope(ref_name, None, target, 1);
    let ref_state_id = ref_state.object_id();
    Ok(RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_update: signed_ref_update_envelope(ref_name, None, ref_state_id, target, 1),
        ref_state,
    })
}

fn next_publication(
    layout: &RepositoryLayout,
    previous: &RefPublication,
) -> prikk_error::Result<RefPublication> {
    let previous_id = previous.ref_state.object_id();
    let block = signed_empty_block_envelope();
    let target = FileObjectStore::new(layout.clone()).write_object(&block)?;
    let ref_state = signed_ref_state_envelope("heads/main", Some(previous_id), target, 2);
    let ref_state_id = ref_state.object_id();
    Ok(RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(previous_id),
        ref_update: signed_ref_update_envelope(
            "heads/main",
            Some(previous_id),
            ref_state_id,
            target,
            2,
        ),
        ref_state,
    })
}

fn assert_blocking_issue(layout: &RepositoryLayout, code: &str) -> prikk_error::Result<()> {
    let report = verify_repository(layout)?;
    assert!(report.has_blocking_ref_publication_issues());
    assert!(
        report
            .ref_publication_issues
            .iter()
            .any(|issue| issue.code == code)
    );
    assert!(!doctor_repository(layout).is_healthy());
    Ok(())
}

#[test]
fn candidate_failure_warns_and_retry_publishes_once() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-candidate-retry");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let store = RefStore::new(layout.clone());
    // RFC 102 Stage 4, design-v1.md §13.11: no real publish failure can leave `refs/tmp/` debris
    // anymore (the candidate-write mechanism `MutableParentSync` instrumented is gone) -- plant it
    // directly, matching `state_matrix::PersistedState::Candidate`'s own technique. This test's own
    // claim never depended on *how* the debris got there, only that it blocks every mutation entry
    // point below and that a normal publish still completes once it stops mattering to the ref
    // itself (`state_matrix`'s own coverage is per-state, not per-entry-point -- this is what
    // proves the wedge reaches `append_patch`, `add_trusted_maintainer`, and `repair_repository`,
    // not just ref publication).
    std::fs::write(layout.ref_tmp_path("heads/main"), b"candidate")?;
    let report = verify_repository(&layout)?;
    assert!(!report.has_blocking_ref_publication_issues());
    assert!(
        report
            .ref_publication_issues
            .iter()
            .any(|issue| issue.code == "PRIKK-VERIFY-REF-CANDIDATE-DEBRIS")
    );
    assert!(
        ActiveSession::new(layout.clone())
            .append_patch(&signed_patch_envelope(), DEFAULT_ACTIVE_PATCH_LIMIT)
            .is_err()
    );
    assert!(
        add_trusted_maintainer(
            &layout,
            "blocked-maintainer",
            "1111111111111111111111111111111111111111111111111111111111111111"
        )
        .is_err()
    );
    assert!(repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail()).is_err());
    assert_eq!(
        store.finish_interrupted_publication_for_test(&publication)?,
        publication.ref_state.object_id()
    );
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn complete_record_sync_failure_retries_without_duplicate() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-complete-log-retry");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let store = RefStore::new(layout.clone());
    // RFC 102 Stage 3: an object write now durably appends to both its container and the index
    // (two `RequiredFileSync` calls, not one), and precedes those with the ref lock's own creation
    // (a third). RFC 102 Stage 4 adds one more before the log append: the pointer-index append (a
    // fourth `RequiredFileSync`). RFC 102 Stage 6 Step 2, design-v1.md §15.8, adds two more before
    // that -- `acquire_container_locks`'s own `RefPointerIndex`/`RefLog` lock creations, each firing
    // `RequiredFileSync` via `create_exclusive`, the same as `RefLock`'s own creation always has.
    // Skip 6, not 4, to land the injected failure on the ref log's own append -- the "complete
    // record" this test is named for -- so the object and pointer are already durably committed by
    // the time it fails, matching the scenario this test exists to prove.
    fail_after_for_test(TestFailPoint::RequiredFileSync, 6);
    assert!(store.publish(&publication).is_err());
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    assert!(!verify_repository(&layout)?.has_blocking_ref_publication_issues());
    store.finish_interrupted_publication_for_test(&publication)?;
    store.finish_interrupted_publication_for_test(&publication)?;
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn pointer_lead_with_partial_tail_is_truncated_then_completed() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-partial-log-retry");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let store = RefStore::new(layout.clone());
    // RFC 102 Stage 3: the ref-state object's own container and index appends now precede the
    // log append and each fire `AppendWrite` too. RFC 102 Stage 4 adds one more: the pointer-index
    // append. Skip 3, not 2, to land the torn write on the log's own append -- this is the ref's
    // first-ever publish, so it leaves no real record behind to duplicate; encode the pending
    // update directly instead (`append_torn_ref_log_tail_for_test`'s own doc).
    fail_after_for_test(TestFailPoint::AppendWrite, 3);
    assert!(store.publish(&publication).is_err());
    append_torn_ref_log_tail_for_test(
        &layout,
        ref_name_key_bytes("heads/main"),
        &publication.ref_update,
    )?;
    assert_blocking_issue(&layout, "PRIKK-VERIFY-REF-DIVERGENCE")?;
    store.finish_interrupted_publication_for_test(&publication)?;
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    assert_eq!(store.replay_log("heads/main")?.trailing_partial_bytes, 0);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn fully_framed_checksum_failure_is_never_truncated() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-checksum-refusal");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let store = RefStore::new(layout.clone());
    store.publish(&publication)?;
    // `heads/main` is the only ref in this fixture, so the whole container is exactly its own
    // subsequence.
    let path = layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
    let mut bytes = std::fs::read(&path)?;
    let last = bytes.last_mut().ok_or_else(|| {
        prikk_error::PrikkError::Integrity("expected a complete ref-log record".to_string())
    })?;
    *last ^= 0xff;
    std::fs::write(path, bytes)?;

    // DC-95 Stage 2 Level 2: a single ref's own checksum corruption is now an item-level failure
    // (its log file's own read), not a whole-`Refs`-stage failure.
    let report = verify_repository(&layout)?;
    assert!(report.has_item_failure());
    // RFC 102 Stage 4 checkpoint review, design-v1.md §13.10: an explicit acceptance criterion for
    // this migration -- checks the specific ref's own item outcome, not just that *some* item
    // failed anywhere, since `has_item_failure()` alone stayed true through `log_outcomes` even
    // while the ref-name cross-reference this exercises (`refs/verify.rs`'s `ref_name_key`-keyed
    // lookup, fixed in `8098753`) was still broken.
    assert!(report.ref_item_outcomes.iter().any(|outcome| {
        outcome.ref_name == "heads/main"
            && matches!(outcome.status, crate::RefItemStatus::Failed { .. })
    }));
    assert!(!doctor_repository(&layout).is_healthy());
    assert!(
        store
            .finish_interrupted_publication_for_test(&publication)
            .is_err()
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn format2_ahead_log_refuses_pointer_promotion() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-legacy-ahead");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&publication.ref_state)?;
    append_log_record_for_signature_test(&layout, "heads/main", &publication.ref_update)?;
    let store = RefStore::new(layout.clone());
    assert_blocking_issue(&layout, "PRIKK-VERIFY-REF-DIVERGENCE")?;
    assert!(
        store
            .finish_interrupted_publication_for_test(&publication)
            .is_err()
    );
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    assert_eq!(store.read_current_ref_state_id("heads/main")?, None);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn existing_ref_pointer_lead_finishes_but_format2_ahead_log_refuses() -> prikk_error::Result<()> {
    for legacy_ahead in [false, true] {
        let root = unique_temp_dir("dc38-existing-recovery");
        let layout = RepositoryLayout::init(root.clone())?;
        let first = root_publication(&layout, "heads/main")?;
        let store = RefStore::new(layout.clone());
        store.publish(&first)?;
        let second = next_publication(&layout, &first)?;
        FileObjectStore::new(layout.clone()).write_object(&second.ref_state)?;
        if legacy_ahead {
            append_log_record_for_signature_test(&layout, "heads/main", &second.ref_update)?;
            assert_blocking_issue(&layout, "PRIKK-VERIFY-REF-DIVERGENCE")?;
        } else {
            // RFC 102 Stage 4, design-v1.md §13.11: unlike the root-publication tests elsewhere in
            // this file, `second.ref_state` was already durably written above (line ~237) before
            // this call -- `write_object_to_container`'s own same-id-same-bytes idempotency check
            // makes `store.publish`'s own object write here a no-op, firing zero `AppendWrite`
            // calls, not the usual two. Only the pointer-index append (1) and the log append (1)
            // remain, so skip 1, not 3, to land the interruption on the log append -- this ref's
            // *second* transition, exercising `classify_ref_state`'s arm 2 through a real `Some(log)`
            // tip rather than the root case's `None`-log short-circuit.
            fail_after_for_test(TestFailPoint::AppendWrite, 1);
            assert!(store.publish(&second).is_err());
            assert_blocking_issue(&layout, "PRIKK-VERIFY-REF-DIVERGENCE")?;
        }
        if legacy_ahead {
            assert!(
                store
                    .finish_interrupted_publication_for_test(&second)
                    .is_err()
            );
            assert_eq!(
                store.read_current_ref_state_id("heads/main")?,
                Some(first.ref_state.object_id())
            );
        } else {
            store.finish_interrupted_publication_for_test(&second)?;
            store.finish_interrupted_publication_for_test(&second)?;
            assert_eq!(store.replay_log("heads/main")?.records.len(), 2);
            assert_eq!(
                store.read_current_ref_state_id("heads/main")?,
                Some(second.ref_state.object_id())
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn duplicate_and_greater_than_one_log_divergence_fail_closed() -> prikk_error::Result<()> {
    for duplicate in [false, true] {
        let root = unique_temp_dir("dc38-divergence");
        let layout = RepositoryLayout::init(root.clone())?;
        let first = root_publication(&layout, "heads/main")?;
        let mut objects = FileObjectStore::new(layout.clone());
        objects.write_object(&first.ref_state)?;
        append_log_record_for_signature_test(&layout, "heads/main", &first.ref_update)?;
        if duplicate {
            // `heads/main` is the only ref in this fixture, so the whole container is exactly its
            // own subsequence -- duplicating the whole file duplicates only this ref's own record.
            let path =
                layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
            let record = std::fs::read(&path)?;
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)?
                .write_all(&record)?;
        } else {
            let second = next_publication(&layout, &first)?;
            objects.write_object(&second.ref_state)?;
            append_log_record_for_signature_test(&layout, "heads/main", &second.ref_update)?;
        }
        // DC-95 Stage 2 Level 2: a single ref's own duplicate/divergent log record is now an
        // item-level failure (its log file's own read, or its own `classify_ref_state` call), not
        // a whole-`Refs`-stage failure.
        assert!(verify_repository(&layout)?.has_item_failure());
        assert!(!doctor_repository(&layout).is_healthy());
        assert!(
            RefStore::new(layout.clone())
                .finish_interrupted_publication_for_test(&first)
                .is_err()
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn ref_log_sequence_gap_fails_closed() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-sequence-gap");
    let layout = RepositoryLayout::init(root.clone())?;
    let first = root_publication(&layout, "heads/main")?;
    let first_id = first.ref_state.object_id();
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&first.ref_state)?;
    append_log_record_for_signature_test(&layout, "heads/main", &first.ref_update)?;
    let target = objects.write_object(&signed_empty_block_envelope())?;
    let gap_state = signed_ref_state_envelope("heads/main", Some(first_id), target, 3);
    let gap_id = objects.write_object(&gap_state)?;
    let gap_update = signed_ref_update_envelope("heads/main", Some(first_id), gap_id, target, 3);
    append_log_record_for_signature_test(&layout, "heads/main", &gap_update)?;

    // DC-95 Stage 2 Level 2: this ref's own sequence-gap defect is now an item-level failure (its
    // log file's own read), not a whole-`Refs`-stage failure.
    assert!(verify_repository(&layout)?.has_item_failure());
    assert!(!doctor_repository(&layout).is_healthy());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn doctor_missing_pointer_repair_is_refused() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-doctor-refusal");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    FileObjectStore::new(layout.clone()).write_object(&publication.ref_state)?;
    append_log_record_for_signature_test(&layout, "heads/main", &publication.ref_update)?;
    let error = repair_repository(&layout, DoctorRepairOptions::reconstruct_main_ref())
        .err()
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity(
                "doctor missing-pointer repair unexpectedly succeeded".to_string(),
            )
        })?;
    assert!(
        error
            .to_string()
            .contains("--repair-main-ref has no implemented repair")
    );
    assert_eq!(
        RefStore::new(layout.clone()).read_current_ref_state_id("heads/main")?,
        None
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
