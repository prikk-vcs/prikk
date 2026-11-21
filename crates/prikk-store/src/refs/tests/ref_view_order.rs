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

/// 0.59.0 RS2c: the active session's three stores are read **until a read leaves them unchanged**. A writer that changes one of them while the view
/// is being read (here: the owner of the queue is rewritten after the first read) gives the view of the settled state, not a mixture. With no retry
/// the view is the first read -- the state no writer ever left whole.
#[test]
fn the_active_view_is_read_until_it_is_unchanged() {
    let (layout, root, _target) = repository("rs2c-active-view");
    crate::commit_boundary::active::prepare_empty_active_ref_for_append(&layout, "heads/a")
        .unwrap();
    let writer = layout.clone();
    view_hooks::set("active-view-after-read", move || {
        crate::commit_boundary::active::prepare_empty_active_ref_for_append(&writer, "heads/bb")
            .unwrap();
    });
    let view = crate::commit_boundary::active_view::read_active_view(&layout);
    assert!(!view_hooks::pending("active-view-after-read"));
    assert_eq!(
        view.metadata.unwrap(),
        crate::commit_boundary::active::ActiveRefMetadata::Valid("heads/bb".to_string())
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 0.59.0 RS2c: `lead_now` reads the log, **then** the pointer. A publisher that completes a whole publication between the two reads (a `sync seal`
/// publishes claim after claim) leaves the pointer **ahead** of the tip the reader holds -- a changed lead, which `decide` calls a publication in
/// flight. Read the other way round (the pointer, then the log) it left the pointer **behind** the log: a lead that was never there, the same one the
/// first read named, and `decide` answered "interrupted" with no lock held at either holder read (1–5 `tag create` refusals per tag race on tmpfs).
#[test]
fn a_publication_completing_between_the_leads_two_reads_is_in_flight_not_interrupted() {
    let (layout, root, target) = repository("rs2c-lead-order");
    let first = publish_first(&layout, "heads/main", target);
    let publisher = layout.clone();
    view_hooks::set("lead-between-log-and-pointer", move || {
        let next = signed_ref_state_envelope("heads/main", Some(first), target, 2);
        let next_id = next.object_id();
        let update = signed_ref_update_envelope("heads/main", Some(first), next_id, target, 2);
        RefStore::new(publisher.clone())
            .publish(&RefPublication {
                ref_name: "heads/main".to_string(),
                expected_previous_ref_state_id: Some(first),
                ref_state: next,
                ref_update: update,
            })
            .unwrap();
    });
    let lead = crate::refs::in_flight::decide(&layout, "heads/main", first, || {
        crate::refs::in_flight::lead_now(&layout, "heads/main")
    })
    .unwrap();
    assert!(!view_hooks::pending("lead-between-log-and-pointer"));
    assert!(
        matches!(lead, crate::refs::in_flight::Lead::InFlight { pid: None }),
        "{lead:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A writer that never lets the queue settle: after every read the view makes, the owner of the queue is rewritten (a name of a different length, so the
/// change is a length change and not only a timestamp). The hook re-arms itself, so every one of the bounded reads sees a change.
fn churn_the_queue(layout: RepositoryLayout, round: usize) {
    view_hooks::set("active-view-after-read", move || {
        let name = format!("heads/c{}", "x".repeat(round % 40));
        crate::commit_boundary::active::prepare_empty_active_ref_for_append(&layout, &name)
            .unwrap();
        churn_the_queue(layout, round + 1);
    });
}

/// 0.59.0 RS2d: the active view's **exhausted case**. The bounded rereads run out while the queue is still changing. If the active lock's holder
/// appears to be running and is not this process, the view says so (`unsettled: Some(pid)`) and a caller reports the publication in flight; with no lock,
/// a holder that is gone, or this process as the holder, it is the last read, as before the field existed.
#[test]
fn an_exhausted_active_view_is_in_flight_only_beside_a_running_holder() {
    let (layout, root, _target) = repository("rs2d-active-view-exhausted");
    crate::commit_boundary::active::prepare_empty_active_ref_for_append(&layout, "heads/a")
        .unwrap();
    let lock_path = layout.active_lock_path(crate::foundation::layout::DEFAULT_ACTIVE_NAME);

    // No lock: the last read.
    churn_the_queue(layout.clone(), 1);
    assert_eq!(
        crate::commit_boundary::active_view::read_active_view(&layout).unsettled,
        None
    );
    // This process holds the lock: not another command.
    std::fs::write(
        &lock_path,
        crate::lock_holder::render_body(
            "active",
            std::process::id(),
            crate::lock_holder::own_facts(),
        ),
    )
    .unwrap();
    churn_the_queue(layout.clone(), 1);
    assert_eq!(
        crate::commit_boundary::active_view::read_active_view(&layout).unsettled,
        None
    );
    // Another running process holds it: in flight, with its pid; and `verify` reports the existing finding instead of classifying the mixture.
    let (mut child, body) = crate::lock_holder::body_naming_another_running_process("active");
    std::fs::write(&lock_path, body).unwrap();
    churn_the_queue(layout.clone(), 1);
    assert_eq!(
        crate::commit_boundary::active_view::read_active_view(&layout).unsettled,
        Some(child.id())
    );
    churn_the_queue(layout.clone(), 1);
    let report = crate::verify_repository(&layout).unwrap();
    let issues: Vec<(&str, bool)> = report
        .ref_publication_issues
        .iter()
        .map(|issue| (issue.code, issue.blocking))
        .collect();
    assert!(
        issues.contains(&("PRIKK-VERIFY-REF-PUBLICATION-IN-FLIGHT", false)),
        "{issues:?}"
    );
    assert!(!report.has_stage_failure(), "{:?}", report.stage_outcomes);
    // The two stages stay `evaluated` and say what is true (text only): not classified, and why.
    for stage in [
        crate::VerificationStage::ActiveWalMetadata,
        crate::VerificationStage::CommitWitness,
    ] {
        let note = report
            .stage_notes
            .iter()
            .find(|(noted, _)| *noted == stage)
            .map(|(_, note)| note.as_str())
            .unwrap_or("");
        assert_eq!(
            note,
            format!(
                "not classified: the active queue changed on every read while another command (pid {}) held the active lock",
                child.id()
            )
        );
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

/// A Block nothing reaches, written into `layout`'s store: what a `seal` has written and not yet published.
fn write_unreached_block(layout: &RepositoryLayout, target: ObjectId) -> ObjectId {
    let block = crate::test_gates::test_support::signed_block(
        prikk_object::BlockKind::Normal,
        vec![target],
        Vec::new(),
        None,
    );
    FileObjectStore::new(layout.clone())
        .write_object(&block)
        .unwrap()
}

/// 0.59.0 RS2e: a Block that nothing reaches, seen while **another running process holds the active lock**, is that writer's, not a leftover:
/// `unreached_withheld` is set and the existing in-flight finding is reported (`info`), while `unreached_objects` still lists what the read found (the
/// JSON form's field). With no lock, this process as the holder, or a holder that is gone, nothing is withheld and the object is a leftover as before.
#[test]
fn an_unreached_block_beside_a_running_writer_is_withheld_from_the_text_and_reported_in_flight() {
    let (layout, root, target) = repository("rs2e-unreached");
    publish_first(&layout, "heads/main", target);
    let block_id = write_unreached_block(&layout, target);
    let lock_path = layout.active_lock_path(crate::foundation::layout::DEFAULT_ACTIVE_NAME);

    let leftover = crate::verify_repository(&layout).unwrap();
    assert!(
        leftover
            .unreached_objects
            .iter()
            .any(|o| o.object_id == block_id)
    );
    assert!(
        !leftover.unreached_withheld,
        "no lock: a leftover, as before"
    );

    std::fs::write(
        &lock_path,
        crate::lock_holder::render_body(
            "active",
            std::process::id(),
            crate::lock_holder::own_facts(),
        ),
    )
    .unwrap();
    assert!(
        !crate::verify_repository(&layout)
            .unwrap()
            .unreached_withheld,
        "this process is not another writer"
    );

    let (mut child, body) = crate::lock_holder::body_naming_another_running_process("active");
    std::fs::write(&lock_path, body).unwrap();
    let report = crate::verify_repository(&layout).unwrap();
    assert!(report.unreached_withheld);
    assert!(
        report
            .unreached_objects
            .iter()
            .any(|o| o.object_id == block_id),
        "the list keeps what the read found"
    );
    assert!(
        report
            .ref_publication_issues
            .iter()
            .any(|issue| issue.code == "PRIKK-VERIFY-REF-PUBLICATION-IN-FLIGHT" && !issue.blocking),
        "{:?}",
        report.ref_publication_issues
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

/// The lock file changing between two looks says a writer ran in between, even when no holder was seen at either (a writer that started and finished
/// inside the stage): `Some(None)`; a holder at either look is `Some(Some(pid))`; an unchanged lock is `None`.
#[test]
fn a_lock_that_appears_between_two_looks_is_a_writer_that_ran() {
    use crate::commit_boundary::active_view::LockWatch;
    let (layout, root, _target) = repository("rs2e-lock-watch");
    let before = LockWatch::take(&layout);
    assert_eq!(before.writer_until(&LockWatch::take(&layout)), None);
    let lock_path = layout.active_lock_path(crate::foundation::layout::DEFAULT_ACTIVE_NAME);
    std::fs::write(&lock_path, b"").unwrap();
    std::fs::remove_file(&lock_path).unwrap();
    let after = LockWatch::take(&layout);
    // Created and removed: the file is absent at both looks, so nothing says a writer ran -- the stamp of an absent file is the same.
    assert_eq!(before.writer_until(&after), None);
    std::fs::write(&lock_path, b"x").unwrap();
    assert_eq!(before.writer_until(&LockWatch::take(&layout)), Some(None));
    let _ = std::fs::remove_dir_all(root);
}

/// 0.59.0 RS2e: the F2 pattern for what two looks at the lock cannot see. A Block nothing reaches, **no lock held at either look**: a writer that
/// started after the first and was done before the second leaves nothing. The object is read again against the pointers as they are now; a hook
/// publishes a ref at the Block between the unreached finding and that second read, so the Block was published while the run read it: withheld.
/// Without the hook the same Block, with no lock, is a leftover and is not withheld.
#[test]
fn an_unreached_block_published_while_the_run_read_it_is_withheld() {
    let (layout, root, target) = repository("rs2e-published-since");
    publish_first(&layout, "heads/main", target);
    let block_id = write_unreached_block(&layout, target);
    assert!(
        !crate::verify_repository(&layout)
            .unwrap()
            .unreached_withheld,
        "a leftover"
    );

    let publisher = layout.clone();
    view_hooks::set("verify-after-objects", move || {
        publish_first(&publisher, "heads/other", block_id);
    });
    let report = crate::verify_repository(&layout).unwrap();
    assert!(!view_hooks::pending("verify-after-objects"));
    assert!(
        report
            .unreached_objects
            .iter()
            .any(|o| o.object_id == block_id)
    );
    assert!(report.unreached_withheld, "published while the run read it");
    let _ = std::fs::remove_dir_all(root);
}
