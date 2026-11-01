//! Tests for the shared R3 sequence (0.56.0 step W3b,
//! `.git-exclude/reviewed/0.56.0-step-3w3-review-v1.md`'s ruling): judge clean, mark dirty as a
//! `Switch` to the current branch, (the producer's own publish, simulated here by directly
//! republishing `heads/main`), then materialize through the verified path.
//!
//! These exercise the shared primitive directly, the same way `branch_switch/tests.rs` exercises
//! `switch_branch` directly rather than through a CLI command -- `execute_merge`'s own clean/dirty
//! cases are covered end to end through the compiled binary
//! (`crates/prikk-cli/tests/commit_and_seal/step3w3b_merge_into_current_branch.rs`); the crash/
//! recovery case needs a failpoint, which only fires in-process, so it lives here.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose,
};

use super::{materialize_after_publish, require_clean_and_mark_current_branch_switch};
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_once_for_test};
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::test_gates::test_support::{
    dummy_signature, signed_block_with_state_root, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir, write_blob,
};
use crate::wal::{Wal, WalReplay};
use crate::worktree_baseline::{
    WorktreeBaselineComparison, compare_worktree_baseline, record_worktree_baseline,
};
use crate::worktree_marker::worktree_is_dirty;
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    derive_next_state_root, materialize_patch_checkout, switch_branch,
};

const MAIN: &str = "heads/main";

/// Publish `heads/main`'s genesis block, containing `files`, as a root block.
fn publish_genesis(
    layout: &RepositoryLayout,
    files: &[(&str, &[u8])],
) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let mut operations = Vec::new();
    for (index, (path, bytes)) in files.iter().enumerate() {
        let blob_id = write_blob(&mut object_store, bytes)?;
        let mut node = [0x61_u8; 32];
        node[31] = u8::try_from(index).unwrap_or(u8::MAX);
        operations.push(Operation {
            op_seq: u32::try_from(index + 1).unwrap_or(u32::MAX),
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: (*path).to_string(),
                node_id: NodeId::from_bytes(node),
                blob_id,
                mode: 0o100644,
            }),
        });
    }
    let payload = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state = derive_next_state_root(&object_store, None, &[patch_id])?;
    let block =
        signed_block_with_state_root(BlockKind::Root, Vec::new(), vec![patch_id], None, state);
    let block_id = object_store.write_object(&block)?;
    let ref_state = signed_ref_state_envelope(MAIN, None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(MAIN, None, ref_state_id, block_id, 1);
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: MAIN.to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(block_id)
}

/// Republish `heads/main` with one more file, on top of `parent_block_id` -- simulating a
/// producer's own step 3 (the ref move this round's shared materialization runs after).
fn publish_second_generation(
    layout: &RepositoryLayout,
    parent_block_id: ObjectId,
    new_path: &str,
    new_bytes: &[u8],
) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let blob_id = write_blob(&mut object_store, new_bytes)?;
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: new_path.to_string(),
                node_id: NodeId::from_bytes([0x62_u8; 32]),
                blob_id,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state = derive_next_state_root(&object_store, Some(parent_block_id), &[patch_id])?;
    let block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![parent_block_id],
        vec![patch_id],
        None,
        state,
    );
    let block_id = object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let previous_ref_state_id = ref_store.read_current_ref_state_id(MAIN)?;
    let ref_state = signed_ref_state_envelope(MAIN, previous_ref_state_id, block_id, 2);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope(MAIN, previous_ref_state_id, ref_state_id, block_id, 2);
    ref_store.publish(&RefPublication {
        ref_name: MAIN.to_string(),
        expected_previous_ref_state_id: previous_ref_state_id,
        ref_state,
        ref_update,
    })?;
    Ok(block_id)
}

fn empty_replay(layout: &RepositoryLayout) -> prikk_error::Result<WalReplay> {
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()
}

/// Test 1 (the ruling's own list): a clean worktree is materialized -- the new tip's file appears,
/// the pre-existing one is reported unchanged, the record ends current.
#[test]
fn a_clean_worktree_is_materialized_after_the_publish() {
    let root = unique_temp_dir("w3b-materialize-clean");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    let gen1 = publish_genesis(&layout, &[("a.txt", b"a\n")]).expect("publish genesis");
    materialize_patch_checkout(&layout, MAIN).expect("materialize gen1");
    record_worktree_baseline(&layout, MAIN, Some(gen1), 0, &[]).expect("record gen1");

    let active_replay = empty_replay(&layout).expect("replay");
    let pending =
        require_clean_and_mark_current_branch_switch(&layout, MAIN, &active_replay, "testing")
            .expect("clean and marked");
    assert!(worktree_is_dirty(&layout).expect("dirty check"));

    publish_second_generation(&layout, gen1, "b.txt", b"b\n").expect("publish gen2");

    let report = materialize_after_publish(&layout, pending).expect("materialize gen2");
    assert_eq!(report.written_files, 1);
    assert_eq!(report.unchanged_files, 1);
    assert_eq!(report.deleted_files, 0);
    assert!(!worktree_is_dirty(&layout).expect("dirty check"));
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"a\n");
    assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"b\n");
    let comparison =
        compare_worktree_baseline(&layout, MAIN, &empty_replay(&layout).unwrap()).unwrap();
    assert!(
        matches!(comparison, WorktreeBaselineComparison::Current),
        "{comparison:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Test 3 (the ruling's own list): a crash between the publish and the materialize leaves the
/// marker set and the record stale -- both of which `branch switch <the current branch>` (the
/// marker's own named route) resolves by completing the interrupted write.
#[test]
fn a_crash_between_publish_and_materialize_recovers_via_branch_switch() {
    let root = unique_temp_dir("w3b-materialize-crash");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    let gen1 = publish_genesis(&layout, &[("a.txt", b"a\n")]).expect("publish genesis");
    materialize_patch_checkout(&layout, MAIN).expect("materialize gen1");
    record_worktree_baseline(&layout, MAIN, Some(gen1), 0, &[]).expect("record gen1");

    let active_replay = empty_replay(&layout).expect("replay");
    let pending =
        require_clean_and_mark_current_branch_switch(&layout, MAIN, &active_replay, "testing")
            .expect("clean and marked");

    // Step 3: the producer's own publish -- `heads/main` now names a new tip the worktree has not
    // been written from yet.
    publish_second_generation(&layout, gen1, "b.txt", b"b\n").expect("publish gen2");

    // Step 4 crashes at its first worktree write.
    fail_once_for_test(TestFailPoint::MutableRename);
    let crashed = materialize_after_publish(&layout, pending);
    clear_failpoint_for_test();
    assert!(
        crashed.is_err(),
        "materialize_after_publish must fail at the injected failpoint"
    );

    // The crash state: the marker is still set (which already makes `commit` refuse unconditionally
    // -- `commit_boundary/worktree_patch/node_authoring.rs`'s own guard) and the record reads
    // `Behind` the now-published tip (a second, independent reason `commit` would refuse).
    assert!(
        worktree_is_dirty(&layout).expect("dirty check"),
        "the marker must still be set after the crash"
    );
    let comparison =
        compare_worktree_baseline(&layout, MAIN, &empty_replay(&layout).unwrap()).unwrap();
    assert!(
        matches!(comparison, WorktreeBaselineComparison::Behind { .. }),
        "the record must read stale against the published tip: {comparison:?}"
    );
    assert_eq!(
        std::fs::read(root.join("a.txt")).unwrap(),
        b"a\n",
        "the pre-crash file must be untouched"
    );
    assert!(
        !root.join("b.txt").exists(),
        "the crashed write must not have left a renamed b.txt"
    );

    // Recovery: `branch switch <the current branch>` -- the marker's own named route -- finishes
    // the interrupted write through the ordinary verified path (the identical operation step 4
    // was in the middle of).
    let recovered = switch_branch(&layout, Some(MAIN), MAIN).expect("recover via self-switch");
    assert!(!recovered.already_current);
    assert!(!worktree_is_dirty(&layout).expect("dirty check"));
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"a\n");
    assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"b\n");
    let comparison =
        compare_worktree_baseline(&layout, MAIN, &empty_replay(&layout).unwrap()).unwrap();
    assert!(
        matches!(comparison, WorktreeBaselineComparison::Current),
        "{comparison:?}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Red control (the ruling's own list): with step 4's write/record/clear removed, the clean-
/// worktree case (test 1) must fail -- the ref moves, but the new file never actually appears,
/// which is exactly what the red control is supposed to catch. This test does not flip the control
/// itself (the report records how the control was applied and reverted); it documents, in-line,
/// the assertion the control must break.
#[test]
fn clean_worktree_materialization_asserts_the_new_file_is_actually_written() {
    let root = unique_temp_dir("w3b-materialize-red-control-shape");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    let gen1 = publish_genesis(&layout, &[("a.txt", b"a\n")]).expect("publish genesis");
    materialize_patch_checkout(&layout, MAIN).expect("materialize gen1");
    record_worktree_baseline(&layout, MAIN, Some(gen1), 0, &[]).expect("record gen1");

    let active_replay = empty_replay(&layout).expect("replay");
    let pending =
        require_clean_and_mark_current_branch_switch(&layout, MAIN, &active_replay, "testing")
            .expect("clean and marked");
    publish_second_generation(&layout, gen1, "b.txt", b"b\n").expect("publish gen2");
    materialize_after_publish(&layout, pending).expect("materialize gen2");

    // The assertion a step-4-removed red control must break: the new tip's file is really on disk,
    // not merely that the ref moved.
    assert!(
        root.join("b.txt").is_file(),
        "a producer's own materialization must actually write the new tip's files"
    );
    let _ = std::fs::remove_dir_all(root);
}
