//! RFC 166 D5, §13 item 15: `prikk doctor --restore-queue-target --ref <ref>`.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use super::{plan_restore_queue_target, restore_queue_target};
use crate::commit_boundary::active::{ActiveRefMetadata, ActiveSession, read_active_ref_metadata};
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::read_witness;
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::test_gates::test_support::{
    signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};
use crate::wal::Wal;
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader,
    RefPublication, RefStore, WorktreePatchCommitOptions, add_trusted_maintainer,
};
use prikk_object::{BlockPayload, ObjectType};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc166-d5-restore", &[0x92_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "d5-restore",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap();
}

fn classify_now(layout: &RepositoryLayout) -> Verdict {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    let owning_ref = read_active_ref_metadata(layout).unwrap();
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

/// Simulates the compounded crash RFC 166 §1.6 describes: ownership is durably present right up
/// until it is not -- a tear in the very write this verb exists to repair, not a slow corruption.
fn clear_ref_name(layout: &RepositoryLayout) {
    std::fs::write(layout.default_active_ref_name_path(), []).unwrap();
}

/// Row 9, the main case: a non-empty, wholly sound WAL with no durable owner. Restoring to the ref
/// the witness already names succeeds, writes `ref-name` back, and the session reads healthy again.
#[test]
fn row9_restores_ownership_and_writes_ref_name_atomically() {
    let root = unique_temp_dir("rfc166-d5-restore-row9");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let plan = restore_queue_target(&layout, "heads/main").expect("row 9 is this verb's own job");
    assert_eq!(plan.ref_name, "heads/main");
    assert_eq!(plan.patch_ids.len(), 1);
    assert_eq!(
        plan.current_tip_block_id, None,
        "heads/main has never been published"
    );
    assert!(plan.tip_matches.is_empty());

    match read_active_ref_metadata(&layout).unwrap() {
        ActiveRefMetadata::Valid(name) => assert_eq!(name, "heads/main"),
        other => panic!("expected ownership restored, got {other:?}"),
    }
    assert!(matches!(
        classify_now(&layout),
        Verdict::Healthy { last_seq: 1 }
    ));
    std::fs::remove_dir_all(&root).ok();
}

/// D5/C2: the ref comes from the caller, but a witness naming a *different* ref refuses rather
/// than silently overruling it.
#[test]
fn witness_naming_a_different_ref_refuses_writing_nothing() {
    let root = unique_temp_dir("rfc166-d5-restore-witness-mismatch");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/other")
        .expect_err("the witness names heads/main, not heads/other");
    assert!(
        err.to_string().contains("names heads/main"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A healthy session has nothing to restore -- ownership already matches the witness.
#[test]
fn a_healthy_session_refuses_writing_nothing() {
    let root = unique_temp_dir("rfc166-d5-restore-healthy");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    assert!(matches!(classify_now(&layout), Verdict::Healthy { .. }));

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/main").expect_err("ownership is not missing");
    assert!(
        err.to_string()
            .contains("already has a durable, matching owner"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// RFC 166 D5 K1: `--plan-only` and a real run share one computation, and `--plan-only` touches
/// nothing.
#[test]
fn plan_only_matches_the_real_runs_own_plan_and_touches_nothing() {
    let root = unique_temp_dir("rfc166-d5-restore-plan-only");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);

    let before = snapshot_tree(&layout);
    let plan =
        plan_restore_queue_target(&layout, "heads/main").expect("plan-only does not refuse row 9");
    let after_plan_only = snapshot_tree(&layout);
    assert_eq!(before, after_plan_only, "plan-only must touch nothing");

    let real = restore_queue_target(&layout, "heads/main").expect("the real run");
    assert_eq!(
        plan, real,
        "the plan-only computation must equal the real run's own"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// RFC 166 §13 item 10: when more than one ref's own current tip validates against this queue,
/// the plan lists every one. Built from a realistic compounded shape: `heads/main` is already
/// sealed at a block whose patches this orphaned queue happens to hold again (the seal published
/// the block but crashed before draining its own queue), and `heads/other` was independently
/// published pointing at that same block.
#[test]
fn more_than_one_ref_validating_is_listed_in_the_plan() {
    let root = unique_temp_dir("rfc166-d5-restore-ambiguous");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed("rfc166-d5-restore-maintainer", &[0x93; 32]).unwrap();
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .unwrap();

    commit(&layout, "a.txt", b"one");
    let ref_state_id =
        simulate_one_seal(&layout, "heads/main", &maintainer).expect("seal onto heads/main");

    let ref_store = RefStore::new(layout.clone());
    let objects = FileObjectStore::new(layout.clone());
    let sealed_ref_state_envelope = objects
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .expect("the seal's own published RefState");
    let block_id = prikk_object::RefStatePayload::decode_canonical(
        &sealed_ref_state_envelope.canonical_payload,
        sealed_ref_state_envelope.schema_version,
    )
    .unwrap()
    .target_object_id;

    let other_ref_state_envelope = signed_ref_state_envelope("heads/other", None, block_id, 1);
    let other_ref_state_id = other_ref_state_envelope.object_id();
    let other_ref_update_envelope =
        signed_ref_update_envelope("heads/other", None, other_ref_state_id, block_id, 1);
    ref_store
        .publish(&RefPublication {
            ref_name: "heads/other".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: other_ref_state_envelope,
            ref_update: other_ref_update_envelope,
        })
        .expect("publish heads/other pointing at the same block");

    let block_envelope = objects
        .read_typed(block_id, ObjectType::Block)
        .unwrap()
        .unwrap();
    let block = BlockPayload::decode_canonical(&block_envelope.canonical_payload).unwrap();
    let patch_id = block.patch_ids[0];
    let patch_envelope = objects
        .read_typed(patch_id, ObjectType::Patch)
        .unwrap()
        .unwrap();
    ActiveSession::new(layout.clone())
        .append_patch(&patch_envelope, 1_000)
        .expect("re-queue the already-sealed patch into a fresh active WAL");
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let plan = restore_queue_target(&layout, "heads/main").expect("row 9 is this verb's own job");
    assert_eq!(
        plan.tip_matches,
        vec!["heads/main".to_string(), "heads/other".to_string()]
    );
    std::fs::remove_dir_all(&root).ok();
}
