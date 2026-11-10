//! 0.57.0 step 3, S5b: `prikk uncommit`. One test per case-table row; the crash sweep proves every state a failure at any
//! write leaves is either untouched or finished by a re-run to the same end state, and that the run's recovery entries undo
//! it or refuse naming a reason; the race proves it against a committing thread.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use prikk_object::ObjectId;

use super::{OperationFate, RecordOutcome, plan_uncommit, uncommit};
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{WitnessState, read_witness};
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::{Ed25519AuthorSigner, WorktreePatchCommitOptions};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("s5b-uncommit", &[0x5b_u8; 32]).unwrap()
}

fn new_repo(tag: &str) -> (PathBuf, RepositoryLayout) {
    let root = unique_temp_dir(tag);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    crate::record_initial_worktree_baseline(&layout).unwrap();
    (root, layout)
}

/// Write `body` to `path` and commit it on `heads/main`; returns the new patch's id.
fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) -> ObjectId {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "s5b",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap()
    .patch_id
}

fn queue(layout: &RepositoryLayout) -> Vec<ObjectId> {
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap()
        .records
        .iter()
        .map(|record| record.envelope.object_id())
        .collect()
}

fn classify_now(layout: &RepositoryLayout) -> Verdict {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    let owner = crate::commit_boundary::active::read_active_ref_metadata(layout).unwrap();
    let witness = read_witness(layout, DEFAULT_ACTIVE_NAME).unwrap();
    classify(layout, &replay, &owner, &witness).unwrap()
}

/// The five files `uncommit` writes (and nothing else): the WAL, the witness, `ref-name`, the worktree's record and the rename
/// declarations. Absent reads as empty.
fn state(layout: &RepositoryLayout) -> Vec<(&'static str, Vec<u8>)> {
    let read = |path: PathBuf| std::fs::read(path).unwrap_or_default();
    vec![
        (
            "wal",
            read(layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME)),
        ),
        (
            "witness",
            read(
                layout
                    .active_session_dir(DEFAULT_ACTIVE_NAME)
                    .join("witness"),
            ),
        ),
        ("ref-name", read(layout.default_active_ref_name_path())),
        ("record", read(layout.prikk_dir().join("worktree.baseline"))),
        ("declarations", read(layout.default_declarations_path())),
    ]
}

/// Row: the newest of three. Two stay, the witness covers them, the record equals them, and the removed frame is in the log.
#[test]
fn the_newest_of_three_leaves_two_a_consistent_queue_and_the_frame_in_the_log() {
    let (root, layout) = new_repo("s5b-newest-of-three");
    let first = commit(&layout, "a.txt", b"one");
    let second = commit(&layout, "b.txt", b"two");
    let third = commit(&layout, "c.txt", b"three");
    let wal_before = std::fs::read(layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME)).unwrap();

    let plan = uncommit(&layout, third).unwrap();

    assert_eq!(plan.sequence, Some(3));
    assert_eq!(plan.queue_after, vec![first, second]);
    assert_eq!(queue(&layout), vec![first, second]);
    assert_eq!(plan.record, RecordOutcome::Rewritten);
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => {
            assert_eq!((record.last_seq, record.patch_id), (2, second));
        }
        other => panic!("expected a witness covering the two that stay, got {other:?}"),
    }
    assert!(matches!(
        classify_now(&layout),
        Verdict::Healthy { last_seq: 2 }
    ));
    let status = crate::worktree_status::worktree_status(&layout, "heads/main").unwrap();
    assert_eq!(
        status.behind_ref, None,
        "the record equals the queue: not behind"
    );
    let run = plan
        .recovery_run
        .expect("a real run names its recovery run");
    let wal_after = std::fs::read(layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME)).unwrap();
    let saved = crate::recovery_log::removed_bytes(&layout, &run)
        .unwrap()
        .expect("the log holds the run's first entry");
    assert_eq!(
        saved,
        wal_before[wal_after.len()..],
        "the log holds exactly the frame that was cut"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Row: the only queued patch. The state afterwards is byte for byte the state a fresh repository has (what `seal` leaves).
#[test]
fn the_only_patch_leaves_the_state_a_fresh_repository_has() {
    let (root, layout) = new_repo("s5b-only");
    let fresh = {
        let (fresh_root, fresh_layout) = new_repo("s5b-only-fresh");
        let fresh_state = state(&fresh_layout);
        std::fs::remove_dir_all(&fresh_root).ok();
        fresh_state
    };
    let only = commit(&layout, "a.txt", b"one");

    let plan = uncommit(&layout, only).unwrap();

    assert!(plan.clears_owner);
    assert!(queue(&layout).is_empty());
    let after = state(&layout);
    for ((name, now), (_, fresh)) in after.iter().zip(&fresh) {
        if *name == "record" {
            continue;
        }
        assert_eq!(now, fresh, "{name} is as a fresh repository has it");
    }
    let record = after.iter().find(|(name, _)| *name == "record").unwrap();
    let fresh_record = fresh.iter().find(|(name, _)| *name == "record").unwrap();
    assert_eq!(
        record.1, fresh_record.1,
        "the record is the empty queue's, as a fresh repository has it"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Row: a middle patch is refused, naming the later ones; nothing is written.
#[test]
fn a_middle_patch_is_refused_naming_the_later_patches_and_nothing_is_written() {
    let (root, layout) = new_repo("s5b-middle");
    let first = commit(&layout, "a.txt", b"one");
    let second = commit(&layout, "b.txt", b"two");
    let third = commit(&layout, "c.txt", b"three");
    let before = state(&layout);

    let error = uncommit(&layout, first).expect_err("the oldest is not the newest");
    let text = error.to_string();
    assert!(
        text.contains(&second.to_string()) && text.contains(&third.to_string()),
        "{text}"
    );
    assert!(text.contains("2 patches are queued after it"), "{text}");
    assert!(text.contains("new patch ids"), "{text}");
    assert_eq!(state(&layout), before, "a refusal writes nothing");
    assert!(plan_uncommit(&layout, first).is_err());
    std::fs::remove_dir_all(&root).ok();
}

/// Row: an id that is not queued and not an object; the refusal says so, and a re-run after completion names the run.
#[test]
fn an_unknown_id_and_a_taken_back_id_are_refused_by_what_they_are() {
    let (root, layout) = new_repo("s5b-unknown");
    let first = commit(&layout, "a.txt", b"one");
    let second = commit(&layout, "b.txt", b"two");
    let before = state(&layout);

    let unknown: ObjectId = "ab".repeat(32).parse().unwrap();
    let text = uncommit(&layout, unknown).expect_err("unknown").to_string();
    assert!(
        text.contains("no object in this repository has that id"),
        "{text}"
    );
    assert_eq!(state(&layout), before);

    let plan = uncommit(&layout, second).unwrap();
    let again = uncommit(&layout, second)
        .expect_err("already out")
        .to_string();
    assert!(again.contains("already took it out"), "{again}");
    assert!(
        again.contains(plan.recovery_run.as_deref().unwrap()),
        "a re-run after completion names the run: {again}"
    );
    assert_eq!(queue(&layout), vec![first]);
    std::fs::remove_dir_all(&root).ok();
}

/// Row: a patch with a rename puts the rename back into the declarations, so the next commit authors a rename again.
#[test]
fn a_rename_is_put_back_as_a_declaration_and_the_next_commit_authors_it_again() {
    let (root, layout) = new_repo("s5b-rename");
    // A sealed baseline holding b.txt: commit and seal it, so the rename is of a tracked node.
    let signer_m =
        crate::Ed25519MaintainerSigner::from_seed("s5b-maintainer", &[0x5c_u8; 32]).unwrap();
    crate::add_trusted_maintainer(
        &layout,
        signer_m.key_id(),
        &prikk_hash::to_hex(&signer_m.public_key_bytes()),
    )
    .unwrap();
    commit(&layout, "b.txt", b"base\n");
    crate::rfc111_seal_simulation::simulate_one_seal(&layout, "heads/main", &signer_m).unwrap();
    crate::record_rename_declaration(&layout, "b.txt", "b2.txt").unwrap();
    std::fs::rename(layout.root().join("b.txt"), layout.root().join("b2.txt")).unwrap();
    let renamed = commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "rename",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap();
    assert!(
        crate::read_rename_declarations(&layout).unwrap().is_empty(),
        "commit consumed the declaration"
    );

    let plan = uncommit(&layout, renamed.patch_id).unwrap();

    assert_eq!(plan.declarations_restored.len(), 1);
    assert!(
        plan.operations
            .iter()
            .any(|op| op.kind == "rename-path" && op.fate == OperationFate::RenameDeclared)
    );
    let declared = crate::read_rename_declarations(&layout).unwrap();
    assert_eq!(
        (declared[0].old_path.as_str(), declared[0].new_path.as_str()),
        ("b.txt", "b2.txt")
    );
    let again = commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "rename again",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap();
    let kinds: Vec<&str> = again
        .changes
        .iter()
        .map(|change| change.operation.as_str())
        .collect();
    assert!(
        kinds.contains(&"rename-path"),
        "the next commit authors a rename again, not a delete and a create: {kinds:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Row: a rename declared since overlaps the removed commit's: refused before any write, saying so.
#[test]
fn a_declaration_made_since_that_overlaps_refuses_before_any_write() {
    let (root, layout) = new_repo("s5b-overlap");
    let signer_m =
        crate::Ed25519MaintainerSigner::from_seed("s5b-maintainer", &[0x5c_u8; 32]).unwrap();
    crate::add_trusted_maintainer(
        &layout,
        signer_m.key_id(),
        &prikk_hash::to_hex(&signer_m.public_key_bytes()),
    )
    .unwrap();
    commit(&layout, "b.txt", b"base\n");
    commit(&layout, "e.txt", b"base\n");
    crate::rfc111_seal_simulation::simulate_one_seal(&layout, "heads/main", &signer_m).unwrap();
    crate::record_rename_declaration(&layout, "b.txt", "b2.txt").unwrap();
    std::fs::rename(layout.root().join("b.txt"), layout.root().join("b2.txt")).unwrap();
    let renamed = commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "rename",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap();
    // Since: the same destination declared from another file.
    crate::record_rename_declaration(&layout, "e.txt", "b2.txt").unwrap();
    let before = state(&layout);

    let text = uncommit(&layout, renamed.patch_id)
        .expect_err("the declarations overlap")
        .to_string();

    assert!(text.contains("overlaps the removed commit's"), "{text}");
    assert_eq!(state(&layout), before, "a refusal writes nothing");
    std::fs::remove_dir_all(&root).ok();
}

/// Rows 4, 5 (another id), 6 and 9 refuse with the row's own sentence and verb; nothing is written.
#[test]
fn rows_4_to_9_refuse_with_their_own_verb_and_write_nothing() {
    // Row 4: a damaged acknowledged record.
    let (root, layout) = new_repo("s5b-row4");
    let id = commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let at = bytes.len() - 5;
    bytes[at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    let before = state(&layout);
    let text = uncommit(&layout, id).expect_err("row 4").to_string();
    assert!(text.contains("--discard-damaged-commits"), "{text}");
    assert_eq!(state(&layout), before);
    std::fs::remove_dir_all(&root).ok();

    // Row 5 with another id: the witness holds a patch that is gone, and the person named a different one.
    let (root, layout) = new_repo("s5b-row5");
    let first = commit(&layout, "a.txt", b"one");
    let second = commit(&layout, "b.txt", b"two");
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .remove_newest_record(second)
        .unwrap();
    let before = state(&layout);
    let text = uncommit(&layout, first)
        .expect_err("row 5, another id")
        .to_string();
    assert!(text.contains("--discard-damaged-commits"), "{text}");
    assert!(text.contains("interrupted `prikk uncommit`"), "{text}");
    assert_eq!(state(&layout), before);
    std::fs::remove_dir_all(&root).ok();

    // Row 6: a substituted record.
    let (root_a, layout_a) = new_repo("s5b-row6-a");
    let a = commit(&layout_a, "a.txt", b"one");
    let (root_b, layout_b) = new_repo("s5b-row6-b");
    commit(&layout_b, "a.txt", b"DIFFERENT");
    std::fs::write(
        layout_a.active_queue_wal_path(DEFAULT_ACTIVE_NAME),
        std::fs::read(layout_b.active_queue_wal_path(DEFAULT_ACTIVE_NAME)).unwrap(),
    )
    .unwrap();
    let before = state(&layout_a);
    let text = uncommit(&layout_a, a).expect_err("row 6").to_string();
    assert!(text.contains("a copy is the way out"), "{text}");
    assert_eq!(state(&layout_a), before);
    std::fs::remove_dir_all(&root_a).ok();
    std::fs::remove_dir_all(&root_b).ok();

    // Row 9: no owner.
    let (root, layout) = new_repo("s5b-row9");
    let id = commit(&layout, "a.txt", b"one");
    std::fs::write(layout.default_active_ref_name_path(), b"").unwrap();
    let before = state(&layout);
    let text = uncommit(&layout, id).expect_err("row 9").to_string();
    assert!(text.contains("--restore-queue-target"), "{text}");
    assert_eq!(state(&layout), before);
    std::fs::remove_dir_all(&root).ok();
}

/// Row 5 with the patch the witness holds: an interrupted `uncommit`. A re-run finishes it, and says so.
#[test]
fn a_rerun_after_the_wal_cut_finishes_what_the_interrupted_run_left() {
    let (root, layout) = new_repo("s5b-rerun-cut");
    let first = commit(&layout, "a.txt", b"one");
    let second = commit(&layout, "b.txt", b"two");
    // The state a crash right after the WAL cut leaves: the witness still names `second`.
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .remove_newest_record(second)
        .unwrap();
    assert!(matches!(
        classify_now(&layout),
        Verdict::AcknowledgedLoss { witnessed_seq: 2 }
    ));

    let planned = plan_uncommit(&layout, second).unwrap();
    assert!(planned.finishing);
    let plan = uncommit(&layout, second).unwrap();

    assert!(plan.finishing);
    assert_eq!(plan.record, RecordOutcome::Rewritten);
    assert_eq!(queue(&layout), vec![first]);
    assert!(matches!(
        classify_now(&layout),
        Verdict::Healthy { last_seq: 1 }
    ));
    let status = crate::worktree_status::worktree_status(&layout, "heads/main").unwrap();
    assert_eq!(status.behind_ref, None);
    std::fs::remove_dir_all(&root).ok();
}

/// Past the witness: the record is one commit ahead. A re-run brings it up to date; a second re-run says "not queued".
#[test]
fn a_rerun_past_the_witness_brings_the_record_up_to_date() {
    let (root, layout) = new_repo("s5b-rerun-record");
    let first = commit(&layout, "a.txt", b"one");
    let second = commit(&layout, "b.txt", b"two");
    let record_before = std::fs::read(layout.prikk_dir().join("worktree.baseline")).unwrap();
    // Crash after the witness: cut the WAL and rewrite the witness, leave the record.
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.remove_newest_record(second).unwrap();
    crate::commit_boundary::witness::rebuild_witness_over_sound_wal(
        &layout,
        DEFAULT_ACTIVE_NAME,
        "heads/main",
        &wal.replay().unwrap(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(layout.prikk_dir().join("worktree.baseline")).unwrap(),
        record_before
    );
    let stale = crate::worktree_status::worktree_status(&layout, "heads/main").unwrap();
    assert!(
        stale.behind_ref.is_some(),
        "the stale record reads as behind"
    );

    let plan = uncommit(&layout, second).unwrap();

    assert!(plan.finishing);
    assert_eq!(plan.record, RecordOutcome::Rewritten);
    let status = crate::worktree_status::worktree_status(&layout, "heads/main").unwrap();
    assert_eq!(status.behind_ref, None);
    assert_eq!(queue(&layout), vec![first]);
    assert!(
        uncommit(&layout, second).is_err(),
        "the next run says it is not queued"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A queue owned by a ref other than the current branch (`commit --ref`). `commit` records the fold under the ref it committed to,
/// so that record is this queue's and is rewritten; a record that names another ref (the current branch's, after a switch) is not
/// this queue's and is left exactly as it was.
#[test]
fn the_record_is_rewritten_only_where_it_is_this_queues() {
    for (tag, record_names_main) in [("s5b-other-ref-own", false), ("s5b-other-ref-main", true)] {
        let (root, layout) = new_repo(tag);
        std::fs::write(layout.root().join("a.txt"), b"one").unwrap();
        let id = commit_worktree_changes_signed(
            &layout,
            "heads/feature",
            "other ref",
            WorktreePatchCommitOptions::file_level(),
            &signer(),
        )
        .unwrap()
        .patch_id;
        if record_names_main {
            crate::record_worktree_baseline(&layout, "heads/main", None, 0, &[]).unwrap();
        }
        let record_before = std::fs::read(layout.prikk_dir().join("worktree.baseline")).unwrap();

        let plan = uncommit(&layout, id).unwrap();

        let record_after = std::fs::read(layout.prikk_dir().join("worktree.baseline")).unwrap();
        if record_names_main {
            assert_eq!(plan.record, RecordOutcome::LeftAsItWas);
            assert_eq!(
                record_after, record_before,
                "a record that is not this queue's is untouched"
            );
        } else {
            assert_eq!(plan.record, RecordOutcome::Rewritten);
            assert_ne!(
                record_after, record_before,
                "this queue's record moves with it"
            );
        }
        assert!(queue(&layout).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}

/// `--plan-only` is the run: the plan a real run prints is the plan `plan_uncommit` returned, apart from the run id.
#[test]
fn the_plan_equals_the_run() {
    let (root, layout) = new_repo("s5b-plan-equals-run");
    commit(&layout, "a.txt", b"one");
    let newest = commit(&layout, "b.txt", b"two");
    let before = state(&layout);
    let planned = plan_uncommit(&layout, newest).unwrap();
    assert_eq!(state(&layout), before, "a plan writes nothing");
    let mut ran = uncommit(&layout, newest).unwrap();
    assert!(ran.recovery_run.is_some() && planned.recovery_run.is_none());
    ran.recovery_run = None;
    assert_eq!(planned, ran);
    std::fs::remove_dir_all(&root).ok();
}

/// The operations of a patch whose files the worktree no longer has say so: the next commit would not author them.
#[test]
fn an_operation_the_worktree_no_longer_holds_is_not_promised() {
    let (root, layout) = new_repo("s5b-fate");
    commit(&layout, "a.txt", b"one");
    let held = commit(&layout, "b.txt", b"two");
    let plan = plan_uncommit(&layout, held).unwrap();
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].fate, OperationFate::Reauthored);
    std::fs::remove_file(layout.root().join("b.txt")).unwrap();
    let plan = plan_uncommit(&layout, held).unwrap();
    assert_eq!(
        plan.operations[0].fate,
        OperationFate::NotInWorktree,
        "the file is gone: the next commit does not author it"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// **The crash sweep.** A failure injected at the n-th occurrence of each write primitive, for every n until the run no longer
/// fails: whatever it leaves is (a) read honestly by `classify` (the untouched queue, or row 5 for exactly this patch -- never a
/// substituted record or a missing owner), (b) finished by a re-run to the same end state an uninterrupted run reaches, and (c)
/// undone by the run's recovery entries, or the restore refuses and the state is as it was.
#[test]
fn every_crash_state_is_read_honestly_finished_by_a_rerun_and_undone_or_refused() {
    let points = [
        TestFailPoint::AppendWrite,
        TestFailPoint::Truncate,
        TestFailPoint::InPlaceWrite,
        TestFailPoint::RequiredFileSync,
        TestFailPoint::MutableFileSync,
        TestFailPoint::MutableRename,
    ];
    // Both shapes: a queue of two (witness rewritten) and a queue of one (witness cleared, owner cleared).
    for queued in [2_usize, 1] {
        let mut states_seen = std::collections::BTreeSet::new();
        for point in points {
            for ordinal in 0..40 {
                let (root, layout) = new_repo("s5b-sweep");
                let target = commits(&layout, queued);
                let pre = state(&layout);
                let stays: Vec<ObjectId> = queue(&layout)[..queued - 1].to_vec();
                fail_after_for_test(point, ordinal);
                let result = uncommit(&layout, target);
                clear_failpoint_for_test();
                // A failure while the lock itself is taken leaves the lock file, as a real crash does; `prikk unlock` is the
                // way past it, so the sweep does the same and goes on to what the writes left.
                std::fs::remove_file(
                    layout
                        .active_session_dir(DEFAULT_ACTIVE_NAME)
                        .join("active.lock"),
                )
                .ok();
                if result.is_ok() {
                    assert_end_state(&layout, &pre, &stays, "uninterrupted", false);
                    std::fs::remove_dir_all(&root).ok();
                    break;
                }
                // (a) read honestly.
                let verdict = classify_now(&layout);
                let label = match &verdict {
                    Verdict::Healthy { .. } | Verdict::Pending { .. } => "queue-readable",
                    Verdict::NoWitness { .. } => "no-witness",
                    Verdict::AcknowledgedLoss { .. } => "row-5",
                    other => {
                        panic!("{point:?}@{ordinal} (queued {queued}): dishonest state {other:?}")
                    }
                };
                states_seen.insert(label);
                // The write order, as invariants of every intermediate state: the record is never touched while the patch is
                // still queued (it is written last), and once the WAL is cut the cut is in the log (saved first).
                if queue(&layout).contains(&target) {
                    let status =
                        crate::worktree_status::worktree_status(&layout, "heads/main").unwrap();
                    assert_eq!(
                        status.behind_ref, None,
                        "{point:?}@{ordinal} (queued {queued}): the record moved before the WAL was cut"
                    );
                } else {
                    assert!(
                        latest_run(&layout).is_some(),
                        "{point:?}@{ordinal} (queued {queued}): the WAL was cut with nothing saved to the log"
                    );
                }
                // (c) undone by the run's own entries, or refused with the state untouched.
                let crashed = state(&layout);
                if let Some(run) = latest_run(&layout) {
                    let plan = crate::recovery_log::restore(&layout, &run, false).unwrap();
                    if plan.written {
                        assert_eq!(
                            state(&layout),
                            pre,
                            "{point:?}@{ordinal} (queued {queued}): a restore that wrote returns every file to how it was"
                        );
                        std::fs::remove_dir_all(&root).ok();
                        continue;
                    }
                    assert_eq!(
                        state(&layout),
                        crashed,
                        "{point:?}@{ordinal} (queued {queued}): a refused restore writes nothing"
                    );
                }
                // (b) finished by a re-run to the uninterrupted end state.
                let what = format!("{point:?}@{ordinal} (queued {queued})");
                match uncommit(&layout, target) {
                    Ok(_) => assert_end_state(&layout, &pre, &stays, &what, false),
                    Err(error) => {
                        // The one state a re-run cannot tell from a finished run: a crash inside the record's own write (the
                        // writer every command uses empties the file, then appends), which leaves it empty -- "no record", read
                        // as matching by every reader and written afresh by the next commit, seal or switch.
                        assert!(
                            error.to_string().contains("already took it out")
                                && state(&layout)
                                    .iter()
                                    .any(|(name, bytes)| *name == "record" && bytes.is_empty()),
                            "{what}: the re-run must finish it, or find the record empty: {error}"
                        );
                        assert_end_state(&layout, &pre, &stays, &what, true);
                    }
                }
                std::fs::remove_dir_all(&root).ok();
            }
        }
        assert!(
            states_seen.len() >= 2,
            "the sweep reached more than one kind of state (queued {queued}): {states_seen:?}"
        );
    }
}

/// The end state of a finished `uncommit`, stated structurally (two repositories never share bytes: node ids are random): the
/// WAL is `pre`'s with the newest frame cut, the witness covers what stays (or is cleared), `ref-name` is kept or cleared with it,
/// the record equals the queue that stays, and the declarations are `pre`'s.
fn assert_end_state(
    layout: &RepositoryLayout,
    pre: &[(&'static str, Vec<u8>)],
    stays: &[ObjectId],
    what: &str,
    record_may_be_empty: bool,
) {
    let now = state(layout);
    let get = |snapshot: &[(&'static str, Vec<u8>)], name: &str| {
        snapshot.iter().find(|(n, _)| *n == name).unwrap().1.clone()
    };
    assert_eq!(queue(layout), stays, "{what}: the queue that stays");
    let pre_wal = get(pre, "wal");
    assert!(
        pre_wal.starts_with(&get(&now, "wal")),
        "{what}: the WAL is the old one with its newest frame cut"
    );
    assert_eq!(
        get(&now, "declarations"),
        get(pre, "declarations"),
        "{what}: declarations"
    );
    if stays.is_empty() {
        assert!(
            get(&now, "wal").is_empty() && get(&now, "witness").is_empty(),
            "{what}: emptied"
        );
        assert!(
            get(&now, "ref-name").is_empty(),
            "{what}: owner cleared, as seal clears it"
        );
    } else {
        assert_eq!(
            get(&now, "ref-name"),
            get(pre, "ref-name"),
            "{what}: owner kept"
        );
        match read_witness(layout, DEFAULT_ACTIVE_NAME).unwrap() {
            WitnessState::Valid(record) => assert_eq!(
                (record.last_seq, Some(record.patch_id)),
                (stays.len() as u64, stays.last().copied()),
                "{what}: the witness covers what stays"
            ),
            other => panic!("{what}: expected a witness over what stays, got {other:?}"),
        }
        assert!(
            matches!(classify_now(layout), Verdict::Healthy { .. }),
            "{what}: healthy"
        );
    }
    let status = crate::worktree_status::worktree_status(layout, "heads/main").unwrap();
    assert_eq!(
        status.behind_ref, None,
        "{what}: the record equals the queue"
    );
    if !record_may_be_empty {
        assert!(
            !get(&now, "record").is_empty(),
            "{what}: the record is written"
        );
    }
}

/// Commit `queued` patches on `heads/main`; returns the id of the newest.
fn commits(layout: &RepositoryLayout, queued: usize) -> ObjectId {
    let mut newest = None;
    for index in 0..queued {
        newest = Some(commit(
            layout,
            &format!("f{index}.txt"),
            format!("body {index}").as_bytes(),
        ));
    }
    newest.unwrap()
}

/// The id of the most recent recovery run in the log, if any entry exists.
fn latest_run(layout: &RepositoryLayout) -> Option<String> {
    crate::recovery_log::list(layout.repository_mutation_root())
        .ok()?
        .entries
        .last()
        .map(|listed| listed.id.clone())
}

/// **The race.** `uncommit` of the newest patch against a thread committing: both take the active lock, so each round ends in one
/// of two states -- the commit landed first (and `uncommit` of the older patch then refuses, naming it as not the newest), or
/// `uncommit` ran first (and the commit then queued on what was left). The queue is readable and classifies healthy after every
/// round, and no acknowledged commit is lost.
#[test]
fn uncommit_raced_against_a_commit_ends_in_one_of_two_states_every_round() {
    let rounds = 20;
    let mut uncommit_first = 0;
    let mut commit_first = 0;
    for round in 0..rounds {
        let (root, layout) = new_repo(&format!("s5b-race-{round}"));
        commit(&layout, "base.txt", b"base");
        let target = commit(&layout, "target.txt", b"target");
        let committer_layout = layout.clone();
        let handle = std::thread::spawn(move || {
            // Retry through lock conflicts, as a user would.
            for _ in 0..2000 {
                std::fs::write(committer_layout.root().join("racing.txt"), b"racing").unwrap();
                match commit_worktree_changes_signed(
                    &committer_layout,
                    "heads/main",
                    "racing",
                    WorktreePatchCommitOptions::file_level(),
                    &signer(),
                ) {
                    Ok(report) => return Some(report.patch_id),
                    Err(_) => std::thread::yield_now(),
                }
            }
            None
        });
        let mut outcome = None;
        for _ in 0..2000 {
            match uncommit(&layout, target) {
                Err(error) if error.to_string().contains("lock") => std::thread::yield_now(),
                other => {
                    outcome = Some(other);
                    break;
                }
            }
        }
        let raced = handle.join().unwrap().expect("the commit landed");
        let ids = queue(&layout);
        match outcome.expect("uncommit finished") {
            Ok(_) => {
                uncommit_first += 1;
                assert!(!ids.contains(&target), "round {round}: taken back");
            }
            Err(error) => {
                commit_first += 1;
                let text = error.to_string();
                assert!(
                    text.contains("is not the newest queued commit"),
                    "round {round}: the commit landed first, so the refusal says so: {text}"
                );
                assert!(ids.contains(&target), "round {round}: refused, so it stays");
            }
        }
        assert!(
            ids.contains(&raced),
            "round {round}: no acknowledged commit is lost"
        );
        assert!(
            matches!(classify_now(&layout), Verdict::Healthy { .. }),
            "round {round}: the queue reads healthy"
        );
        std::fs::remove_dir_all(&root).ok();
    }
    assert_eq!(uncommit_first + commit_first, rounds);
}

/// **The rename's crash row.** A patch that renames a file: a failure at any write leaves the renames either already back in the
/// declarations while the patch is still queued (they are merged first, while the patch can still be read), or not yet touched --
/// never the patch cut with the rename lost. A re-run reaches the end state with the declaration present, and the next commit
/// authors a rename again.
#[test]
fn a_rename_survives_a_crash_at_every_write() {
    let points = [
        TestFailPoint::AppendWrite,
        TestFailPoint::Truncate,
        TestFailPoint::InPlaceWrite,
        TestFailPoint::RequiredFileSync,
        TestFailPoint::MutableFileSync,
        TestFailPoint::MutableRename,
    ];
    let mut crashes = 0;
    for point in points {
        for ordinal in 0..60 {
            let (root, layout) = new_repo("s5b-rename-sweep");
            let signer_m =
                crate::Ed25519MaintainerSigner::from_seed("s5b-maintainer", &[0x5c_u8; 32])
                    .unwrap();
            crate::add_trusted_maintainer(
                &layout,
                signer_m.key_id(),
                &prikk_hash::to_hex(&signer_m.public_key_bytes()),
            )
            .unwrap();
            commit(&layout, "b.txt", b"base\n");
            crate::rfc111_seal_simulation::simulate_one_seal(&layout, "heads/main", &signer_m)
                .unwrap();
            crate::record_rename_declaration(&layout, "b.txt", "b2.txt").unwrap();
            std::fs::rename(layout.root().join("b.txt"), layout.root().join("b2.txt")).unwrap();
            let renamed = commit_worktree_changes_signed(
                &layout,
                "heads/main",
                "rename",
                WorktreePatchCommitOptions::file_level(),
                &signer(),
            )
            .unwrap()
            .patch_id;
            fail_after_for_test(point, ordinal);
            let result = uncommit(&layout, renamed);
            clear_failpoint_for_test();
            std::fs::remove_file(
                layout
                    .active_session_dir(DEFAULT_ACTIVE_NAME)
                    .join("active.lock"),
            )
            .ok();
            let declared = |layout: &RepositoryLayout| {
                crate::read_rename_declarations(layout)
                    .unwrap()
                    .iter()
                    .any(|d| d.old_path == "b.txt" && d.new_path == "b2.txt")
            };
            if result.is_ok() {
                assert!(
                    declared(&layout),
                    "{point:?}@{ordinal}: declared at the end"
                );
                std::fs::remove_dir_all(&root).ok();
                break;
            }
            crashes += 1;
            let what = format!("{point:?}@{ordinal}");
            if !queue(&layout).contains(&renamed) {
                assert!(
                    declared(&layout),
                    "{what}: the patch was cut, so its rename must already be back in the declarations"
                );
            }
            match uncommit(&layout, renamed) {
                Ok(_) => {}
                Err(error) => assert!(
                    error.to_string().contains("already took it out"),
                    "{what}: {error}"
                ),
            }
            assert!(
                declared(&layout),
                "{what}: the rename is declared after the re-run"
            );
            std::fs::remove_dir_all(&root).ok();
        }
    }
    assert!(
        crashes > 5,
        "the sweep crashed the run at several writes ({crashes})"
    );
}
