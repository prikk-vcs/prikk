//! 0.59.0 RS2a: the ref view is read in the reverse of the order a publication writes it -- the log, then the pointer index, then the objects -- so a
//! publication that lands between two of the reads is never read as damage or as an incomplete publication.
//!
//! Each test registers a hook (`refs::view_hooks`) at the point between two reads that the old order made fatal, and makes a **whole first-ever
//! publication** land there. The old order (the pointer index first, or an object snapshot first) concluded a healthy repository was damaged; the
//! new order reads it as it is. Each test ends by asserting its hook ran, so a moved read cannot turn it into a test of nothing.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_error::PrikkError;
use prikk_object::ObjectId;

use crate::foundation::layout::ref_name_key_bytes;
use crate::refs::view_hooks;
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout, verify_repository,
};

/// A repository with one Block written and no ref published: the target every publication in these tests points at.
fn repository(name: &str) -> (RepositoryLayout, std::path::PathBuf, ObjectId) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let block = signed_empty_block_envelope();
    let target = block.object_id();
    FileObjectStore::new(layout.clone())
        .write_object(&block)
        .unwrap();
    (layout, root, target)
}

/// A whole first-ever publication of `ref_name`: its RefState, its pointer, its log record.
fn publish_first(layout: &RepositoryLayout, ref_name: &str, target: ObjectId) -> ObjectId {
    let ref_state = signed_ref_state_envelope(ref_name, None, target, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(ref_name, None, ref_state_id, target, 1);
    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        })
        .unwrap()
}

/// The precondition reads the log, then the pointer index. A first-ever publication landing between the two reads leaves a pointer the log read did
/// not see -- a lead one transition ahead, which `decide` finds gone on its second read -- and the precondition passes. Read the other way round (the
/// pointer index first), the same landing leaves a log record with no pointer entry and the precondition refused a healthy repository with the
/// generic `incomplete ref publication … use signer-backed seal retry` (the tmpfs tag race, 728–756 refusals in 800 rounds on 0.58.0).
#[test]
fn a_publication_landing_between_the_log_and_pointer_reads_passes_the_precondition() {
    let (layout, root, target) = repository("rs2a-precondition-order");
    let publisher = layout.clone();
    view_hooks::set("precondition-between-log-and-pointer", move || {
        publish_first(&publisher, "heads/main", target);
    });
    crate::refs::ensure_no_incomplete_publication(&layout).expect("a healthy repository");
    assert!(!view_hooks::pending("precondition-between-log-and-pointer"));
    let _ = std::fs::remove_dir_all(root);
}

/// The second read a log that leads gets: a log record whose pointer entry the first pointer read did not see is read again before it is judged. A
/// pointer that is there on the second read is a finished (or running) publication, judged like any pointer lead; one that is still not there is the
/// log leading for real, and keeps today's refusal.
#[test]
fn a_log_record_without_a_pointer_entry_is_read_again_before_it_is_judged() {
    let (layout, root, target) = repository("rs2a-log-leads-second-read");
    publish_first(&layout, "heads/main", target);
    let key = ref_name_key_bytes("heads/main");
    let names = std::collections::BTreeMap::from([(key, "heads/main".to_string())]);
    // The first pointer read (empty) did not see the entry; the pointer index has it now.
    let stale = crate::refs::empty_pointer_index_replay();
    let carried_on =
        crate::refs::in_flight::refusal_for_mismatch(&layout, &stale, &names, key).unwrap();
    assert!(carried_on.is_none(), "{carried_on:?}");

    // The control: the entry is truly gone (the log leads for real) -- today's refusal, unchanged.
    crate::refs::remove_pointer_entries_for_test(&layout, key).unwrap();
    let refusal = crate::refs::in_flight::refusal_for_mismatch(&layout, &stale, &names, key)
        .unwrap()
        .expect("a log with no pointer is a refusal");
    assert!(
        matches!(refusal, PrikkError::IncompletePublication(_)),
        "{refusal:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// `verify` reads the ref log, then the pointer index **once**, then opens its object snapshots; every stage takes its refs from that read. A
/// publication that lands after the snapshot is open is invisible to the run (it judges one moment), so no stage reads a pointer whose RefState
/// the snapshot lacks. Before, `local-tag-trust` read the pointer index after the snapshot and failed with `ref … names missing RefState`
/// (055's `inflight.sh`: 177 of 274 runs on 0.58.0).
#[test]
fn a_publication_landing_after_verifys_snapshot_is_not_a_failed_stage() {
    let (layout, root, target) = repository("rs2a-verify-one-view");
    publish_first(&layout, "heads/main", target);
    let baseline = verify_repository(&layout).unwrap();
    assert!(!baseline.has_stage_failure() && !baseline.has_item_failure());

    let publisher = layout.clone();
    view_hooks::set("verify-after-snapshot", move || {
        publish_first(&publisher, "heads/other", target);
    });
    let report = verify_repository(&layout).unwrap();
    assert!(!view_hooks::pending("verify-after-snapshot"));
    assert!(
        !report.has_stage_failure(),
        "a stage failed: {:?}",
        report.stage_outcomes
    );
    assert!(!report.has_item_failure());
    // The next run sees the publication, whole.
    let next = verify_repository(&layout).unwrap();
    assert_eq!(next.checked_refs, Some(2));
    let _ = std::fs::remove_dir_all(root);
}

/// A truly missing RefState is still a failed stage, with today's words: the view does not hide a pointer whose object is not there.
#[test]
fn a_truly_missing_ref_state_is_still_a_failed_local_tag_trust_stage() {
    let (layout, root, target) = repository("rs2a-verify-missing-object");
    let state_id = publish_first(&layout, "heads/main", target);
    crate::foundation::index::remove_index_entry_for_test(&layout, state_id).unwrap();
    let report = verify_repository(&layout).unwrap();
    let local_tag_trust = report
        .stage_outcomes
        .iter()
        .find(|outcome| outcome.stage == crate::VerificationStage::LocalTagTrust)
        .expect("one outcome per stage");
    assert!(
        matches!(&local_tag_trust.status, crate::StageStatus::Failed { message }
            if message == &format!("integrity error: ref heads/main names missing RefState {state_id}")),
        "{:?}",
        local_tag_trust.status
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The first commit on a branch decides "published" or "genesis" from the pointer, then the log. A first-ever publication landing between the pointer
/// read and the log read left a log with history and a pointer read that said "absent": the corruption claim `pointer is missing but ref-log history
/// exists; preserve the repository and restore from backup`. The pointer is read again after the log; a pointer there is a published ref.
#[test]
fn a_publication_landing_between_the_baselines_pointer_and_log_reads_is_a_published_ref() {
    let (layout, root, target) = repository("rs2a-baseline-order");
    let publisher = layout.clone();
    view_hooks::set("baseline-before-log", move || {
        publish_first(&publisher, "heads/main", target);
    });
    let baseline = crate::patch_replay::resolve_worktree_baseline(&layout, "heads/main");
    assert!(!view_hooks::pending("baseline-before-log"));
    assert!(
        baseline.is_ok(),
        "a published ref read as corruption: {:?}",
        baseline.as_ref().err()
    );
    let _ = std::fs::remove_dir_all(root);
}
