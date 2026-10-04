//! RFC 166 D5, §13 item 14: `prikk doctor --discard-damaged-commits`.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use super::{DiscardDamagedCommitsPlan, discard_damaged_commits, plan_discard_damaged_commits};
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{WitnessState, clear_witness, read_witness};
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
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
    let recovery_file = plan.recovery_file.as_ref().expect("recovery file named");
    let recovered = std::fs::read(layout.prikk_dir().join(recovery_file)).unwrap();
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
            recovery_file: real.recovery_file.clone(),
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
    assert!(plan.recovery_file.is_none());
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
    assert!(plan.recovery_file.is_some());
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
