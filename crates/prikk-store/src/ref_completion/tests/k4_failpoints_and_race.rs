//! RFC 165 R4 K4: `ref complete` is a writer. Failpoints at every write ordinal of its own append,
//! each crash state classified by the same table and finished by a second call; and a race against
//! an ordinary `seal` of another ref and against `commit`, under this crate's own established
//! held-lock harness (`lock::tests::a_held_container_lock_refuses_a_second_acquisition`'s shape --
//! acquire, hold, attempt the other writer, assert the conflict, drop, confirm it then proceeds).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::super::{CompletionRefusal, complete_ref_publication, plan_ref_completion};
use super::{crash_branch_create, original_signer, root_block, setup};
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::foundation::layout::RepositoryLayout;
use crate::lock::ActiveLock;
use crate::object_store::ObjectWriteSession;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, RefStore, WorktreePatchCommitOptions, clear_lock,
    commit_worktree_changes_signed, list_held_locks, simulate_one_seal_for_test_support,
};

fn author_signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc165-r4-k4-author", &[0x80; 32]).unwrap()
}

/// PR-007's own, already-accepted limitation ("lock has no stale-lock stealing yet", `lock.rs`): a
/// crash during a lock's own creation -- its post-write fsync, specifically, the same durability
/// primitive every lock file's creation and every container append share -- can leave that lock
/// file behind with nothing to drop it, for *any* publish verb, not something this rule introduces
/// or is asked to fix. The real recovery is `prikk unlock`, i.e. `unlock::clear_lock`; mirrored here
/// so this sweep's own retry matches the procedure a real operator would actually run, rather than
/// asserting a stronger claim ("no `prikk unlock` ever needed") this rule's own handoff never makes.
fn clear_every_held_lock(layout: &RepositoryLayout) {
    if let Ok(locks) = list_held_locks(layout) {
        for lock in locks {
            let _ = clear_lock(layout, &lock.path);
        }
    }
}

#[test]
fn failpoints_at_every_write_ordinal_then_a_second_call_finishes_it() {
    for point in [TestFailPoint::AppendWrite, TestFailPoint::RequiredFileSync] {
        let mut skip = 0usize;
        loop {
            let root = unique_temp_dir(&format!("rfc165-r4-k4-{point:?}-{skip}"));
            let layout = setup(&root);
            let target = root_block(&layout);
            crash_branch_create(&layout, "heads/topic", target, &original_signer());
            let plan = plan_ref_completion(&layout, "heads/topic")
                .unwrap()
                .expect("heads/topic must be a completable lead before the crashed attempt");
            let completer = original_signer();

            // Lock acquisition and object-store opening happen outside the armed window -- this
            // sweep is over `complete_ref_publication`'s own write ordinals specifically, not over
            // `ActiveLock::acquire`'s own (a crash there is a pre-existing, separate recovery story,
            // `prikk unlock`'s own, and a stale lock file surviving it would wrongly read here as
            // "a second `ref complete` cannot even start," which is not this test's claim).
            let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
            let mut object_store = ObjectWriteSession::open(&layout).unwrap();
            fail_after_for_test(point, skip);
            let attempt = complete_ref_publication(
                &layout,
                &mut object_store,
                &active_lock,
                &plan,
                &completer,
            );
            clear_failpoint_for_test();
            let crashed_cleanly = attempt.is_ok();
            drop(object_store);
            drop(active_lock);
            // See `clear_every_held_lock`'s own doc: PR-007, not this rule's concern.
            clear_every_held_lock(&layout);

            // "Classified by the same table": whatever the crash left behind is either already
            // caught up (the write actually landed despite the injected failure) or still a
            // completable lead -- never a divergence this rule's own conditions would refuse.
            match plan_ref_completion(&layout, "heads/topic").unwrap() {
                Ok(second_plan) => {
                    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
                    let mut object_store = ObjectWriteSession::open(&layout).unwrap();
                    complete_ref_publication(
                        &layout,
                        &mut object_store,
                        &active_lock,
                        &second_plan,
                        &completer,
                    )
                    .unwrap_or_else(|err| {
                        panic!(
                            "{point:?} skip={skip}: second ref complete must finish it, got {err}"
                        )
                    });
                }
                Err(CompletionRefusal::NotALead) => {
                    // The crashed attempt's own write already landed -- nothing left to complete.
                }
                Err(other) => panic!(
                    "{point:?} skip={skip}: unexpected classification after a crashed ref complete: {other:?}"
                ),
            }

            // Either way, `heads/topic` must now be fully settled: exactly one log record, no tail.
            let store = RefStore::new(layout.clone());
            let replay = store.replay_log("heads/topic").unwrap();
            assert_eq!(
                replay.records.len(),
                1,
                "{point:?} skip={skip}: expected exactly one settled record, not duplicated or lost"
            );
            assert_eq!(replay.trailing_partial_bytes, 0);
            assert_eq!(
                plan_ref_completion(&layout, "heads/topic").unwrap(),
                Err(CompletionRefusal::NotALead),
                "{point:?} skip={skip}: fully settled afterward, nothing left to complete"
            );

            let _ = std::fs::remove_dir_all(&root);
            if crashed_cleanly {
                // The loop's only way past `skip`'s own bound check below -- reaching here means
                // the sweep found at least one ordinal where the write lands uninterrupted.
                break;
            }
            skip += 1;
            assert!(
                skip < 32,
                "{point:?}: swept past 32 ordinals without the write ever landing uninterrupted"
            );
        }
    }
}

#[test]
fn ref_complete_races_an_ordinary_seal_of_another_ref() {
    let root = unique_temp_dir("rfc165-r4-k4-race-seal");
    let layout = setup(&root);
    let target = root_block(&layout);

    // Queue one patch for a *different* ref before `heads/topic` ever crashes -- `commit` itself
    // refuses while any publication is incomplete, so this must happen first, not after.
    std::fs::write(root.join("a.txt"), b"hello\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        "heads/other",
        "queue one",
        WorktreePatchCommitOptions::file_level(),
        &author_signer(),
    )
    .unwrap();

    crash_branch_create(&layout, "heads/topic", target, &original_signer());
    let plan = plan_ref_completion(&layout, "heads/topic")
        .unwrap()
        .expect("heads/topic must be a completable lead");

    // Hold the active-session lock `ref complete`'s own write also takes, exactly as a real
    // in-progress `ref complete` would while racing against the seal below.
    let held = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let raced = simulate_one_seal_for_test_support(&layout, "heads/other", &original_signer());
    assert!(
        matches!(raced, Err(prikk_error::PrikkError::LockConflict(_))),
        "a seal racing a held active-session lock must refuse as a lock conflict, got {raced:?}"
    );
    drop(held);

    // One writer wins and nothing is lost: the seal now proceeds cleanly...
    simulate_one_seal_for_test_support(&layout, "heads/other", &original_signer()).unwrap();
    // ...and `ref complete`'s own earlier plan is unaffected -- heads/topic completes too.
    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let mut object_store = ObjectWriteSession::open(&layout).unwrap();
    complete_ref_publication(
        &layout,
        &mut object_store,
        &active_lock,
        &plan,
        &original_signer(),
    )
    .unwrap();
    drop(object_store);
    drop(active_lock);

    let store = RefStore::new(layout.clone());
    assert_eq!(store.replay_log("heads/topic").unwrap().records.len(), 1);
    assert_eq!(store.replay_log("heads/other").unwrap().records.len(), 1);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ref_complete_races_a_commit() {
    let root = unique_temp_dir("rfc165-r4-k4-race-commit");
    let layout = setup(&root);
    let target = root_block(&layout);
    crash_branch_create(&layout, "heads/topic", target, &original_signer());
    let plan = plan_ref_completion(&layout, "heads/topic")
        .unwrap()
        .expect("heads/topic must be a completable lead");

    std::fs::write(root.join("a.txt"), b"hello\n").unwrap();
    let try_commit = || {
        commit_worktree_changes_signed(
            &layout,
            "heads/main",
            "a commit",
            WorktreePatchCommitOptions::file_level(),
            &author_signer(),
        )
    };

    // Held: the commit loses the race on the lock itself.
    let held = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let raced = try_commit();
    assert!(
        matches!(raced, Err(prikk_error::PrikkError::LockConflict(_))),
        "a commit racing a held active-session lock must refuse as a lock conflict, got {raced:?}"
    );
    drop(held);

    // Lock released, but `heads/topic` is still an unfinished publication: dropping the lock alone
    // must not let `commit` skip past the real, separate safety check `ref complete` exists to
    // resolve -- a different refusal than the lock conflict above, not a silent success.
    let still_blocked = try_commit();
    assert!(
        matches!(
            still_blocked,
            Err(prikk_error::PrikkError::IncompletePublication(_))
        ),
        "a commit after the lock is free, but before the lead is completed, must still refuse \
         (incomplete ref publication), got {still_blocked:?}"
    );

    // Now finish what `ref complete` was doing -- the real way out.
    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let mut object_store = ObjectWriteSession::open(&layout).unwrap();
    complete_ref_publication(
        &layout,
        &mut object_store,
        &active_lock,
        &plan,
        &original_signer(),
    )
    .unwrap();
    drop(object_store);
    drop(active_lock);

    // Nothing was lost: the commit succeeds cleanly now.
    let report = try_commit().unwrap();
    assert_eq!(report.operation_count, 1);

    let store = RefStore::new(layout.clone());
    assert_eq!(store.replay_log("heads/topic").unwrap().records.len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn doctor_never_writes_a_ref_log_record_even_when_a_lead_is_completable() {
    // K5: no command calls `ref complete` implicitly. `doctor`, in every mode this fixture can
    // reach, must leave the ref log container byte-identical -- including `--repair-tails`, which
    // touches the same container for a lead-free tail but refuses outright over a completable lead
    // (U1's own safety addition) rather than ever appending to it.
    let root = unique_temp_dir("rfc165-r4-k5-doctor-never-writes");
    let layout = setup(&root);
    let target = root_block(&layout);
    crash_branch_create(&layout, "heads/topic", target, &original_signer());
    assert!(
        plan_ref_completion(&layout, "heads/topic").unwrap().is_ok(),
        "fixture precondition: heads/topic must be a completable lead"
    );

    let log_path = layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
    let snapshot = || std::fs::read(&log_path).unwrap();

    // Every doctor mode `main.rs::run_doctor` can reach, each called the same way it is there
    // (`--repair-tails` is mutually exclusive with every other repair flag, so each is its own
    // separate invocation, never combined).
    let before = snapshot();
    let _ = crate::doctor_repository(&layout);
    assert_eq!(
        before,
        snapshot(),
        "plain `doctor` (no repair flag) wrote to the ref log"
    );

    let before = snapshot();
    let _ = crate::repair_tails(&layout);
    assert_eq!(
        before,
        snapshot(),
        "`doctor --repair-tails` wrote to the ref log"
    );

    let before = snapshot();
    let _ = crate::repair_object_index(&layout);
    assert_eq!(
        before,
        snapshot(),
        "`doctor --repair-index` wrote to the ref log"
    );

    let before = snapshot();
    let _ = crate::repair_pointer_index_tail(&layout);
    assert_eq!(
        before,
        snapshot(),
        "`doctor --repair-pointer-index-tail` wrote to the ref log"
    );

    let before = snapshot();
    let _ = crate::repair_repository(
        &layout,
        crate::DoctorRepairOptions {
            truncate_wal_tail: true,
            reconstruct_main_ref: false,
        },
    );
    assert_eq!(
        before,
        snapshot(),
        "`doctor --repair-wal-tail` wrote to the ref log"
    );

    let before = snapshot();
    let _ = crate::repair_repository(
        &layout,
        crate::DoctorRepairOptions {
            truncate_wal_tail: false,
            reconstruct_main_ref: true,
        },
    );
    let after = snapshot();
    assert_eq!(
        before, after,
        "`doctor --repair-main-ref` wrote to the ref log"
    );
    // The lead itself must still be exactly as completable as before: doctor neither advanced nor
    // damaged it.
    assert!(
        plan_ref_completion(&layout, "heads/topic").unwrap().is_ok(),
        "the completable lead must survive every doctor mode unchanged"
    );

    let _ = std::fs::remove_dir_all(&root);
}
