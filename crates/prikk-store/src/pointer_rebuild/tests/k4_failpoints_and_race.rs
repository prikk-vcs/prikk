//! RFC 165 R5 K4: the rebuild is a writer. Failpoints at every write ordinal, including the
//! generation-log switch -- each crash state must read as either the old or the new index, never a
//! mix. A race against `seal` and `commit`, under the crate's own held-lock harness.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::super::{plan_pointer_index_rebuild, rebuild_pointer_index};
use super::{fully_publish, live_pointer_index_bytes, new_block, original_signer, setup};
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    Ed25519AuthorSigner, RefStore, WorktreePatchCommitOptions, clear_lock,
    commit_worktree_changes_signed, list_held_locks, simulate_one_seal_for_test_support,
};

fn author_signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc165-r5-k4-author", &[0x93; 32]).unwrap()
}

/// See `ref_completion::tests::k4_failpoints_and_race`'s own identical helper and doc: PR-007's
/// already-accepted "no stale-lock stealing yet" limitation, shared by every container-locking
/// writer, not this rule's own concern. Mirrors the real `prikk unlock` recovery.
fn clear_every_held_lock(layout: &RepositoryLayout) {
    if let Ok(locks) = list_held_locks(layout) {
        for lock in locks {
            let _ = clear_lock(layout, &lock.path);
        }
    }
}

#[test]
fn failpoints_at_every_write_ordinal_read_as_old_or_new_never_a_mix() {
    for point in [
        TestFailPoint::Truncate,
        TestFailPoint::AppendWrite,
        TestFailPoint::RequiredFileSync,
    ] {
        let mut skip = 0usize;
        loop {
            let root = unique_temp_dir(&format!("rfc165-r5-k4-{point:?}-{skip}"));
            let layout = setup(&root);
            let target1 = new_block(&layout, None, 1);
            let before_main =
                fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);
            let target2 = new_block(&layout, None, 2);
            let before_other =
                fully_publish(&layout, "heads/other", target2, &original_signer(), None, 1);

            fail_after_for_test(point, skip);
            let attempt = rebuild_pointer_index(&layout);
            clear_failpoint_for_test();
            let crashed_cleanly = attempt.is_ok();
            clear_every_held_lock(&layout);

            let store = RefStore::new(layout.clone());
            let main_now = store.read_current_ref_state_id("heads/main").unwrap();
            let other_now = store.read_current_ref_state_id("heads/other").unwrap();

            // Never a mix: both refs must agree on "still the old index" or "now the new index" --
            // here the old and new index hold the same resolved values (a clean, uncrashed rebuild of
            // a repository with no leads reproduces the same state), so the real property under test
            // is that *neither ref* reads as something else entirely (a torn write bleeding through).
            assert_eq!(
                main_now,
                Some(before_main),
                "{point:?} skip={skip}: heads/main must read as a sound index, old or new"
            );
            assert_eq!(
                other_now,
                Some(before_other),
                "{point:?} skip={skip}: heads/other must read as a sound index, old or new"
            );

            // A crashed attempt must still leave the generation log itself readable and pointing at
            // a real, decodable slot -- not switched to something half-written.
            let generation_log_path = layout.ref_pointer_index_generation_log_path();
            let (_live_slot, trailing_partial_bytes, _) =
                crate::foundation::generation::resolve_live_slot_with_tail(
                    &layout,
                    &generation_log_path,
                    &layout.ref_pointer_index_slot_path(ContainerSlot::B),
                    "the ref pointer index",
                    "run `prikk doctor --rebuild-pointer-index`",
                )
                .unwrap_or_else(|err| {
                    panic!("{point:?} skip={skip}: generation log must stay readable, got {err}")
                });
            assert_eq!(
                trailing_partial_bytes, 0,
                "{point:?} skip={skip}: a crash must not leave a torn generation-log record live"
            );

            let _ = std::fs::remove_dir_all(&root);
            if crashed_cleanly {
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
fn rebuild_races_an_ordinary_seal_of_another_ref() {
    let root = unique_temp_dir("rfc165-r5-k4-race-seal");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);

    std::fs::write(root.join("a.txt"), b"hello\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        "heads/other",
        "queue one",
        WorktreePatchCommitOptions::file_level(),
        &author_signer(),
    )
    .unwrap();

    // Hold the same container locks the rebuild itself takes (`RefPointerIndex` + `RefLog`) --
    // `seal`'s own write (`publish_locked`) needs both too.
    let held = crate::lock::acquire_container_locks(
        &layout,
        &[
            crate::foundation::layout::LockableContainer::RefPointerIndex,
            crate::foundation::layout::LockableContainer::RefLog,
        ],
    )
    .unwrap();
    let raced = simulate_one_seal_for_test_support(&layout, "heads/other", &original_signer());
    assert!(
        matches!(raced, Err(prikk_error::PrikkError::LockConflict(_))),
        "a seal racing the rebuild's own held container locks must refuse as a lock conflict, got \
         {raced:?}"
    );
    drop(held);

    // One writer wins and nothing is lost: both proceed cleanly once run in turn.
    simulate_one_seal_for_test_support(&layout, "heads/other", &original_signer()).unwrap();
    rebuild_pointer_index(&layout).unwrap();

    let store = RefStore::new(layout.clone());
    assert!(
        store
            .read_current_ref_state_id("heads/main")
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .read_current_ref_state_id("heads/other")
            .unwrap()
            .is_some()
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rebuild_does_not_block_an_unrelated_commit() {
    // Unlike `ref complete`, the rebuild takes no `ActiveLock` -- it touches the ref-pointer-index
    // and ref-log containers only, disjoint from the active WAL `commit` writes to. A held rebuild
    // lock must not block a commit that never contends for it.
    let root = unique_temp_dir("rfc165-r5-k4-race-commit");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);
    std::fs::write(root.join("a.txt"), b"hello\n").unwrap();

    let before = live_pointer_index_bytes(&layout);
    let held = crate::lock::acquire_container_locks(
        &layout,
        &[
            crate::foundation::layout::LockableContainer::RefPointerIndex,
            crate::foundation::layout::LockableContainer::RefLog,
        ],
    )
    .unwrap();
    let report = commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "unaffected by a rebuild in progress",
        WorktreePatchCommitOptions::file_level(),
        &author_signer(),
    )
    .expect("commit does not contend for the rebuild's own locks");
    assert_eq!(report.operation_count, 1);
    drop(held);

    let after = live_pointer_index_bytes(&layout);
    assert_eq!(
        before, after,
        "the commit above queues a WAL record; it must not touch the pointer index at all"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn plan_only_shares_the_real_runs_own_lock_and_refuses_while_held_elsewhere() {
    // K1: plan-only and a real run are one computation under one lock, not a lockless peek that
    // could read a torn, mid-write state -- while another writer holds the same container locks,
    // `--plan-only` refuses exactly as a real run would, rather than reading around it.
    let root = unique_temp_dir("rfc165-r5-k4-plan-only-no-lock");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);

    let held = crate::lock::acquire_container_locks(
        &layout,
        &[
            crate::foundation::layout::LockableContainer::RefPointerIndex,
            crate::foundation::layout::LockableContainer::RefLog,
        ],
    )
    .unwrap();
    let plan_result = plan_pointer_index_rebuild(&layout);
    assert!(
        matches!(plan_result, Err(prikk_error::PrikkError::LockConflict(_))),
        "plan-only shares the real run's own lock acquisition (one computation, K1) -- while held \
         elsewhere it refuses the same way a real run would, not silently reading around it: got \
         {plan_result:?}"
    );
    drop(held);
    assert!(plan_pointer_index_rebuild(&layout).is_ok());

    let _ = std::fs::remove_dir_all(&root);
}
