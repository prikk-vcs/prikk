//! RFC 166 D5, §13 item 14: `prikk doctor --discard-damaged-commits`.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use super::{DiscardDamagedCommitsPlan, discard_damaged_commits, plan_discard_damaged_commits};
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{
    WitnessState, clear_witness, read_witness, rebuild_witness_over_sound_wal,
};
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::{Ed25519AuthorSigner, WorktreePatchCommitOptions};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc166-d5-discard", &[0x91_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "d5",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap();
}

fn classify_now(layout: &RepositoryLayout) -> Verdict {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    let owning_ref = crate::commit_boundary::active::read_active_ref_metadata(layout).unwrap();
    let witness = read_witness(layout, DEFAULT_ACTIVE_NAME).unwrap();
    classify(layout, &replay, &owning_ref, &witness).unwrap()
}

fn snapshot_tree(layout: &RepositoryLayout) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![layout.prikk_dir().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.push((
                    path.strip_prefix(layout.prikk_dir())
                        .unwrap()
                        .display()
                        .to_string(),
                    bytes,
                ));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Row 4 (N6): a damaged acknowledged record is discarded, the bytes it removes are kept exactly
/// as `--repair-wal-tail` keeps them, and the witness is rebuilt to cover the resulting (empty)
/// sound prefix -- reading `NoWitness` (rule 1, never worse than 0.48.0) immediately afterward.
#[test]
fn row4_discards_the_damaged_record_and_rebuilds_the_witness() {
    let root = unique_temp_dir("rfc166-d5-row4");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 }
    );

    let plan = discard_damaged_commits(&layout).expect("row 4 is this verb's own job");
    assert_eq!(plan.witnessed_seq, Some(1));
    assert!(plan.patch_id.is_some());
    assert!(plan.truncated_bytes > 0);
    let recovery_id = &plan.recovery.as_ref().expect("recovery entry named").id;
    let recovered = crate::recovery_log::removed_bytes(&layout, recovery_id)
        .unwrap()
        .expect("the recovery log holds the entry");
    assert_eq!(
        recovered,
        bytes[bytes.len() - plan.truncated_bytes..],
        "the recovery file holds exactly the removed bytes"
    );
    assert!(
        std::fs::read(&wal_path).unwrap().is_empty(),
        "the lone damaged record is gone"
    );
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Absent => {}
        other => panic!("expected the witness cleared over an empty sound prefix, got {other:?}"),
    }
    assert!(matches!(
        classify_now(&layout),
        Verdict::NoWitness { stale: false, .. }
    ));
    std::fs::remove_dir_all(&root).ok();
}

/// Review v1 (carried into Addendum 1 item 3): the "may still be in your working tree" note did not
/// print, even though the discarded commit's own file was still there, because the check used to
/// call `worktree_status`'s own public entry point, which re-derives the active replay from disk
/// and refuses over the very damage this verb exists to act on -- the `.ok()` around it swallowed
/// that refusal every single time, silently. Fixed by computing the check over the already-sound
/// replay this verb already has in hand (`worktree_status_over_sound_replay`), which never re-trips
/// that guard. **Control:** when the file is genuinely gone from the working tree too, the same
/// check must read `false` -- proving this is a real, state-sensitive answer, not a constant that
/// happens to look right in the acting case.
#[test]
fn the_working_tree_note_prints_when_the_file_is_still_there_and_not_when_it_is_not() {
    let root = unique_temp_dir("rfc166-d5-worktree-note");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 }
    );

    let plan =
        plan_discard_damaged_commits(&layout).expect("row 4 is this verb's own job (plan-only)");
    assert!(
        plan.working_tree_may_still_hold_content,
        "a.txt is still on disk; the note must fire"
    );

    // Control: the same acknowledged-damage shape, but the file is genuinely gone this time.
    let root_b = unique_temp_dir("rfc166-d5-worktree-note-control");
    let layout_b = RepositoryLayout::init(root_b.clone()).unwrap();
    commit(&layout_b, "a.txt", b"one");
    let wal_path_b = layout_b.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes_b = std::fs::read(&wal_path_b).unwrap();
    let flip_at_b = bytes_b.len() - 5;
    bytes_b[flip_at_b] ^= 0xFF;
    std::fs::write(&wal_path_b, &bytes_b).unwrap();
    std::fs::remove_file(layout_b.root().join("a.txt")).unwrap();
    let plan_b =
        plan_discard_damaged_commits(&layout_b).expect("row 4 is this verb's own job (plan-only)");
    assert!(
        !plan_b.working_tree_may_still_hold_content,
        "a.txt is gone; the note must not fire"
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&root_b).ok();
}

/// RFC 166 D5 K1: `--plan-only` and a real run share one computation. The plan it prints is
/// exactly what a real run would do, and it leaves the repository byte for byte as it found it.
#[test]
fn plan_only_matches_the_real_runs_own_plan_and_touches_nothing() {
    let root = unique_temp_dir("rfc166-d5-plan-only");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();

    let before = snapshot_tree(&layout);
    let plan = plan_discard_damaged_commits(&layout).expect("plan-only does not refuse row 4");
    let after_plan_only = snapshot_tree(&layout);
    assert_eq!(before, after_plan_only, "plan-only must touch nothing");

    let real = discard_damaged_commits(&layout).expect("the real run");
    assert_eq!(
        plan,
        DiscardDamagedCommitsPlan {
            witnessed_seq: real.witnessed_seq,
            patch_id: real.patch_id,
            truncated_bytes: real.truncated_bytes,
            recovery: real.recovery.clone(),
            working_tree_may_still_hold_content: real.working_tree_may_still_hold_content,
        },
        "the plan-only computation must equal the real run's own"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Row 5: nothing in the WAL to truncate (it is already as short as it will get) -- the loss is
/// declared by rewriting the witness alone, `truncated_bytes` and `recovery_file` both empty.
#[test]
fn row5_declares_the_loss_with_nothing_to_truncate() {
    let root = unique_temp_dir("rfc166-d5-row5");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    std::fs::write(layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME), []).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedLoss { witnessed_seq: 2 }
    );

    let plan = discard_damaged_commits(&layout).expect("row 5 is this verb's own job");
    assert_eq!(plan.witnessed_seq, Some(2));
    assert!(plan.patch_id.is_some());
    assert_eq!(plan.truncated_bytes, 0);
    assert!(plan.recovery.is_none());
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Absent => {}
        other => panic!("expected the witness cleared over an empty WAL, got {other:?}"),
    }
    assert!(matches!(
        classify_now(&layout),
        Verdict::NoWitness { stale: false, .. }
    ));
    std::fs::remove_dir_all(&root).ok();
}

/// Row 7: the witness itself is unreadable, so no specific sequence is named, but the tail is
/// still discarded and a fresh witness is rebuilt over whatever sound prefix remains.
#[test]
fn row7_discards_the_tail_with_no_sequence_named() {
    let root = unique_temp_dir("rfc166-d5-row7");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut wal_bytes = std::fs::read(&wal_path).unwrap();
    wal_bytes.extend_from_slice(&[0u8; 10]);
    std::fs::write(&wal_path, &wal_bytes).unwrap();
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let mut witness_bytes = std::fs::read(&witness_path).unwrap();
    let flip_at = witness_bytes.len() / 2;
    witness_bytes[flip_at] ^= 0xFF;
    std::fs::write(&witness_path, &witness_bytes).unwrap();
    assert_eq!(classify_now(&layout), Verdict::UnknownWithDamagedWitness);

    let plan = discard_damaged_commits(&layout).expect("row 7 is this verb's own job");
    assert_eq!(plan.witnessed_seq, None);
    assert_eq!(plan.patch_id, None);
    assert_eq!(plan.truncated_bytes, 10);
    assert!(plan.recovery.is_some());
    // The one real, sound record (seq 1) survives, and the rebuilt witness covers exactly it.
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => assert_eq!(record.last_seq, 1),
        other => panic!("expected a rebuilt witness covering the sound record, got {other:?}"),
    }
    assert!(matches!(
        classify_now(&layout),
        Verdict::Healthy { last_seq: 1 }
    ));
    std::fs::remove_dir_all(&root).ok();
}

/// Row 6 (a substituted record) is not this verb's job -- it refuses, writing nothing, and names
/// a copy as the way out rather than inventing a verb that does not act on it.
#[test]
fn row6_substituted_record_refuses_writing_nothing() {
    let root_a = unique_temp_dir("rfc166-d5-row6-a");
    let layout_a = RepositoryLayout::init(root_a.clone()).unwrap();
    commit(&layout_a, "a.txt", b"one");

    let root_b = unique_temp_dir("rfc166-d5-row6-b");
    let layout_b = RepositoryLayout::init(root_b.clone()).unwrap();
    commit(&layout_b, "a.txt", b"DIFFERENT");

    std::fs::write(
        layout_a.active_queue_wal_path(DEFAULT_ACTIVE_NAME),
        std::fs::read(layout_b.active_queue_wal_path(DEFAULT_ACTIVE_NAME)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        classify_now(&layout_a),
        Verdict::SubstitutedRecord { witnessed_seq: 1 }
    );

    let before = snapshot_tree(&layout_a);
    let err = discard_damaged_commits(&layout_a).expect_err("row 6 is not this verb's job");
    assert!(
        err.to_string().contains("a copy is the way out"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout_a),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root_a).ok();
    std::fs::remove_dir_all(&root_b).ok();
}

/// A sound, healthy session has nothing to discard.
#[test]
fn a_healthy_session_refuses_writing_nothing() {
    let root = unique_temp_dir("rfc166-d5-healthy");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    assert!(matches!(classify_now(&layout), Verdict::Healthy { .. }));

    let before = snapshot_tree(&layout);
    let err = discard_damaged_commits(&layout).expect_err("nothing acknowledged is damaged");
    assert!(
        err.to_string()
            .contains("nothing acknowledged is damaged or lost"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Control: a legacy (no-witness) session is rule 1, not acknowledged damage at all -- this verb
/// must not act on a shape it was never meant to touch.
#[test]
fn no_witness_is_not_this_verbs_job_either() {
    let root = unique_temp_dir("rfc166-d5-no-witness");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    assert!(matches!(
        classify_now(&layout),
        Verdict::NoWitness { stale: false, .. }
    ));

    let before = snapshot_tree(&layout);
    let err = discard_damaged_commits(&layout).expect_err("no witness is not acknowledged damage");
    assert!(
        err.to_string()
            .contains("nothing acknowledged is damaged or lost"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ---- K3: the remaining refusal conditions, each with its own constructed case ----------------

/// Simulates RFC 166 §1.6's own stranding: ownership present right up until it is not.
fn clear_ref_name(layout: &RepositoryLayout) {
    std::fs::write(layout.default_active_ref_name_path(), []).unwrap();
}

/// K3, cross-verb boundary: row 9 (ownership missing) is `--restore-queue-target`'s job, not this
/// verb's. This verb must refuse it by name, never with the "nothing acknowledged is damaged or
/// lost" text the wildcard arm uses for an unrelated condition.
#[test]
fn row9_ownership_missing_is_not_this_verbs_job() {
    let root = unique_temp_dir("rfc166-d5-discard-row9-boundary");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let before = snapshot_tree(&layout);
    let err = discard_damaged_commits(&layout).expect_err("row 9 is restore_queue_target's job");
    assert!(
        err.to_string()
            .contains("no durable owner names this session's queue"),
        "unexpected error: {err}"
    );
    assert!(
        !err.to_string()
            .contains("nothing acknowledged is damaged or lost"),
        "row 9 must not be folded into the generic wildcard refusal: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// K3: row 3 (a genuine, never-acknowledged crash tail past an agreeing witness) is not
/// acknowledged damage either -- `--repair-wal-tail` is its own way out, not this verb's.
#[test]
fn row3_crash_tail_is_not_this_verbs_job() {
    let root = unique_temp_dir("rfc166-d5-discard-row3");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    bytes.extend_from_slice(&[0u8; 10]);
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::CrashTail {
            sound_through: Some(1)
        }
    );

    let before = snapshot_tree(&layout);
    let err = discard_damaged_commits(&layout).expect_err("row 3 is a crash tail, not damage");
    assert!(
        err.to_string()
            .contains("nothing acknowledged is damaged or lost"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();

    // Control: removing the tail (the one condition this case is built from) turns row 3 into
    // plain row 1 -- confirming the refusal really did depend on the tail, not on something else.
    let root2 = unique_temp_dir("rfc166-d5-discard-row3-control");
    let layout2 = RepositoryLayout::init(root2.clone()).unwrap();
    commit(&layout2, "a.txt", b"one");
    assert!(matches!(classify_now(&layout2), Verdict::Healthy { .. }));
    std::fs::remove_dir_all(&root2).ok();
}

/// K3: row 8 (a damaged witness over a wholly sound WAL) is a warning, not acknowledged damage --
/// `--repair-tails` rebuilds it; this verb does not act on it.
#[test]
fn row8_witness_damaged_over_a_sound_wal_is_not_this_verbs_job() {
    let root = unique_temp_dir("rfc166-d5-discard-row8");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let mut witness_bytes = std::fs::read(&witness_path).unwrap();
    let flip_at = witness_bytes.len() / 2;
    witness_bytes[flip_at] ^= 0xFF;
    std::fs::write(&witness_path, &witness_bytes).unwrap();
    assert_eq!(classify_now(&layout), Verdict::WitnessDamaged);

    let before = snapshot_tree(&layout);
    let err =
        discard_damaged_commits(&layout).expect_err("row 8 is a warning, not acknowledged damage");
    assert!(
        err.to_string()
            .contains("nothing acknowledged is damaged or lost"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ---- K4: failpoints at every write ordinal, raced against commit under the lock harness -------

/// K4: this verb is a writer -- it takes `ActiveLock` first, the same lock every commit-boundary
/// appender takes. A racing commit attempted while it holds the lock must refuse as a lock
/// conflict, never interleave with it.
#[test]
fn discard_races_an_ordinary_commit_under_the_shared_active_lock() {
    let root = unique_temp_dir("rfc166-d5-discard-k4-race");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();

    let held = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let raced = discard_damaged_commits(&layout);
    assert!(
        matches!(raced, Err(prikk_error::PrikkError::LockConflict(_))),
        "discard_damaged_commits racing a held ActiveLock must refuse as a lock conflict, got \
         {raced:?}"
    );
    drop(held);

    discard_damaged_commits(&layout).expect("succeeds once the active lock is free");
    std::fs::remove_dir_all(&root).ok();
}

/// K4: a crash during the recovery-file write (before its own rename lands) leaves both the WAL
/// and the witness exactly as they were -- `save_removed_bytes` runs first in this verb's own
/// write order, and nothing past it has run yet.
#[test]
fn a_crash_saving_the_recovery_file_leaves_the_wal_and_witness_untouched() {
    let root = unique_temp_dir("rfc166-d5-discard-k4-recovery-crash");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();

    let wal_before = std::fs::read(&wal_path).unwrap();
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let witness_before = std::fs::read(&witness_path).unwrap();

    // The save is an append to `recovery/log` (RFC 168 §3.1; `init` created the log): the failure is injected at that append,
    // after the write and before the sync that makes it durable, so the WAL is still untouched.
    fail_after_for_test(TestFailPoint::AppendWrite, 0);
    let crashed = discard_damaged_commits(&layout);
    clear_failpoint_for_test();
    let message = crashed
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        message.contains("recovery/log"),
        "the injected failure must fire in the log's create, not elsewhere: {message}"
    );
    assert_eq!(
        std::fs::read(&wal_path).unwrap(),
        wal_before,
        "the WAL must be untouched -- the crash is in the recovery-file write, before truncation"
    );
    assert_eq!(
        std::fs::read(&witness_path).unwrap(),
        witness_before,
        "the witness must be untouched either"
    );

    // A second run, with no injected failure, completes the job.
    let plan = discard_damaged_commits(&layout).expect("a clean second run completes it");
    assert_eq!(plan.witnessed_seq, Some(1));
    std::fs::remove_dir_all(&root).ok();
}

/// K4: "each crash state classified and finished by a second run." Reproduces the crash window
/// between a successful truncation and the witness rewrite that follows it (the two genuinely
/// separate durable writes in this verb's own order) by calling the same primitives directly, in
/// the same order `run` uses, and crashing only the second one. The resulting state -- a WAL
/// truncated to its sound prefix, with the old witness still naming the now-removed record -- is
/// not corruption: `classify` reads it as row 5 (acknowledged loss, since the record the witness
/// names is genuinely no longer present), and a second call to this verb finishes the job.
#[test]
fn a_crash_between_truncation_and_the_witness_rewrite_is_read_as_acknowledged_loss_and_finished() {
    let root = unique_temp_dir("rfc166-d5-discard-k4-reorder");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 }
    );

    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.truncate_trailing_partial()
        .expect("the truncation half of this verb's own write order");
    // Simulate the crash: the witness rewrite that would normally follow immediately never runs.
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedLoss { witnessed_seq: 1 },
        "a truncated WAL with the old witness still naming the removed record is row 5, not \
         corruption -- the record the witness names is genuinely no longer present"
    );

    let plan = discard_damaged_commits(&layout).expect("row 5 is this verb's own job too");
    assert_eq!(plan.witnessed_seq, Some(1));
    assert_eq!(plan.truncated_bytes, 0, "the truncation already happened");
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Absent => {}
        other => panic!("expected the witness cleared over the empty sound prefix, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

/// K3/K4's own "a control must be able to fail": rebuilding the witness *before* truncating the
/// WAL (the order this verb never uses) leaves a witness claiming the WAL is healthy while the
/// damaged record it was built over is, at that instant, still physically present -- a real
/// defect shape this verb's own established order (truncate, then rebuild) exists to rule out.
/// Confirms the ordering claims above are sensitive to the order, not vacuous.
#[test]
fn the_control_rebuilding_the_witness_before_truncating_is_a_real_inconsistency() {
    let root = unique_temp_dir("rfc166-d5-discard-k4-control");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 }
    );

    // The wrong order: rebuild the witness over "no sound records" first (the truncation has not
    // happened yet, but this call only reads the replay it is given -- the damaged record is
    // still physically in the file).
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().unwrap();
    rebuild_witness_over_sound_wal(&layout, DEFAULT_ACTIVE_NAME, "heads/main", &replay).unwrap();
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Absent => {}
        other => panic!(
            "expected the witness cleared over the (not yet truncated) sound prefix, got {other:?}"
        ),
    }
    // The WAL bytes are untouched -- the damaged record is still there, but the witness no longer
    // mentions it at all, so a plain `verify`/`status` read now sees rule 1 (no witness), silently
    // dropping the fact that an acknowledged commit is damaged. That silent loss is exactly what
    // this verb's own real order (truncate, THEN rebuild) is built to avoid.
    assert_eq!(std::fs::read(&wal_path).unwrap(), bytes);
    assert!(
        matches!(
            classify_now(&layout),
            Verdict::NoWitness { stale: false, .. }
        ),
        "the control must fail: the wrong order silently loses the acknowledged-damage finding"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ---- K5: no other command writes the WAL, the witness, or ref-name ----------------------------

/// K5: `verify`, `status` (`doctor_repository`), and the other doctor repair modes all leave the
/// WAL, the witness, and `ref-name` byte-identical against a row-4 fixture -- none of them calls
/// this verb, and none of them can touch what only this verb's own classification has cleared to
/// act on.
#[test]
fn no_other_command_touches_the_wal_witness_or_ref_name_over_row4() {
    let root = unique_temp_dir("rfc166-d5-discard-k5");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 }
    );

    // K5's own scope is the WAL, the witness, and `ref-name` specifically -- not the whole tree.
    // `--rebuild-pointer-index` legitimately advances the ref-pointer index's own generation log
    // even when it changes nothing else, so a whole-tree snapshot would be the wrong instrument.
    fn session_snapshot(layout: &RepositoryLayout) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            std::fs::read(layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME)).unwrap(),
            std::fs::read(
                layout
                    .active_session_dir(DEFAULT_ACTIVE_NAME)
                    .join("witness"),
            )
            .unwrap(),
            std::fs::read(layout.default_active_ref_name_path()).unwrap(),
        )
    }

    let before = session_snapshot(&layout);

    let _ = crate::verify_repository(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "verify must write nothing"
    );

    let _ = crate::doctor_repository(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "status/doctor_repository must write nothing"
    );

    let _ = crate::repair_repository(&layout, crate::DoctorRepairOptions::none());
    assert_eq!(
        before,
        session_snapshot(&layout),
        "repair_repository with nothing requested must write nothing"
    );
    let _ = crate::repair_repository(
        &layout,
        crate::DoctorRepairOptions {
            truncate_wal_tail: true,
            reconstruct_main_ref: false,
        },
    );
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--repair-wal-tail refuses over a damaged record (not a tail) and must write nothing"
    );

    let _ = crate::repair_tails(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--repair-tails must write nothing over a damaged WAL record either"
    );

    let _ = crate::rebuild_pointer_index(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--rebuild-pointer-index touches the ref-pointer index only"
    );

    let _ = crate::restore_queue_target(&layout, "heads/main", false);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--restore-queue-target only acts on row 9, never row 4"
    );

    std::fs::remove_dir_all(&root).ok();
}
