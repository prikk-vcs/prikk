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
    BlockKind, CanonicalEncode, CreateFile, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose,
};

use super::{
    abandon_unpublished_materialization, mark_current_branch_switch_pending,
    materialize_after_publish, require_nothing_in_the_way, require_worktree_clean_and_capture,
};
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_once_for_test};
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::patch_replay::{ReplayManifest, ReplayManifestEntry};
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
    let pending = require_worktree_clean_and_capture(&layout, MAIN, &active_replay, "testing")
        .expect("clean and captured");
    mark_current_branch_switch_pending(&layout, &pending).expect("marked");
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
    let pending = require_worktree_clean_and_capture(&layout, MAIN, &active_replay, "testing")
        .expect("clean and captured");
    mark_current_branch_switch_pending(&layout, &pending).expect("marked");

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

/// 0.56.0 step W3c-4b: when a producer's own batch publishes nothing at all, there is no new tip
/// to replay -- on a genuinely **unborn** current branch (never published, not even once), there
/// is no tip to replay *at all*, not even the pre-batch one. `prikk-cli`'s own `run_seal` could not
/// reach this exact shape through the CLI (an `Unverifiable` claim signature still proceeds, so it
/// never itself produces a first-claim refusal with nothing published), but the code path exists
/// and must not need a tip that does not exist: construct the pending capture and the marker
/// directly, the same way [`a_clean_worktree_is_materialized_after_the_publish`] does except that
/// `heads/main` is never published here, then confirm `abandon_unpublished_materialization` clears
/// the marker without erroring, consuming `pending` without ever needing a real tip.
#[test]
fn abandon_unpublished_materialization_clears_the_marker_on_an_unborn_ref() {
    let root = unique_temp_dir("w3c4b-abandon-unborn");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    // `heads/main` is never published -- genuinely unborn throughout.

    let active_replay = empty_replay(&layout).expect("replay");
    let pending = require_worktree_clean_and_capture(&layout, MAIN, &active_replay, "testing")
        .expect("clean and captured on an unborn ref");
    mark_current_branch_switch_pending(&layout, &pending).expect("marked");
    assert!(worktree_is_dirty(&layout).expect("dirty check"));

    abandon_unpublished_materialization(&layout, pending)
        .expect("abandoning an unpublished materialization must not error on an unborn ref");

    assert!(
        !worktree_is_dirty(&layout).expect("dirty check"),
        "the marker must be cleared, even though no tip was ever published"
    );
    let comparison =
        compare_worktree_baseline(&layout, MAIN, &empty_replay(&layout).unwrap()).unwrap();
    assert!(
        matches!(comparison, WorktreeBaselineComparison::NoRecord),
        "an unborn, never-materialized worktree must still read with no record (treated exactly \
         like Current by every caller -- R1's migration-safety rule), never Behind: {comparison:?}"
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
    let pending = require_worktree_clean_and_capture(&layout, MAIN, &active_replay, "testing")
        .expect("clean and captured");
    mark_current_branch_switch_pending(&layout, &pending).expect("marked");
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

/// A one-entry replay manifest built the identical way the 0.56.0-step-W3b-era
/// `test_gates::test_support::text_replay_manifest` helper does (not reused directly: that helper
/// is `#[cfg(not(target_os = "windows"))]`, gated for an unrelated fsutil reason these tests have
/// no part of), carrying `bytes` under `path` at the ordinary text-file mode.
fn one_file_manifest(path: &str, bytes: &[u8]) -> ReplayManifest {
    ReplayManifest {
        files: vec![ReplayManifestEntry {
            node_id: Some(NodeId::from_bytes([0x01; 32])),
            path: crate::RepoPath::parse(path).expect("repo path"),
            bytes: bytes.to_vec(),
            mode: 0o100644,
            kind: Some(NodeKind::TextFile),
            blob_id: None,
        }],
    }
}

/// Test (0.56.0 step W3c-3, the ruling's own list, item 1): an untracked file at a path the target
/// *adds* refuses -- [`plan_materialization`](super::plan_materialization)'s first `in_the_way`
/// source ("a file that is not part of the prior state"), and the sentence follows
/// `branch_switch.rs`'s own wording, ending "nothing was written".
#[test]
fn require_nothing_in_the_way_refuses_an_untracked_file_where_the_target_adds_one() {
    let root = unique_temp_dir("w3c3-in-the-way-add");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    let gen1 = publish_genesis(&layout, &[("a.txt", b"a\n")]).expect("publish genesis");
    materialize_patch_checkout(&layout, MAIN).expect("materialize gen1");
    record_worktree_baseline(&layout, MAIN, Some(gen1), 0, &[]).expect("record gen1");

    let active_replay = empty_replay(&layout).expect("replay");
    let pending = require_worktree_clean_and_capture(&layout, MAIN, &active_replay, "testing")
        .expect("clean and captured");

    // `p.txt` is already on disk, untracked -- not part of `pending`'s own captured `old_files`
    // (only `a.txt` is) -- with content the target's own manifest does not share.
    std::fs::write(root.join("p.txt"), b"zzz\n").unwrap();
    let target_manifest = one_file_manifest("p.txt", b"target-p\n");

    let error = require_nothing_in_the_way(&layout, &pending, &target_manifest, "testing", None)
        .expect_err("a file in the way of the target's own files must refuse");
    let message = error.to_string();
    assert!(
        message.contains("are in the way")
            && message.contains("p.txt (a file that is not part of the prior state)")
            && message.contains(&format!("in the way of {MAIN}'s files"))
            && message.contains("move them aside before testing")
            && message.contains("nothing was written"),
        "unexpected message: {message}"
    );

    // Nothing was written: the untracked file is exactly as it was, and the marker (never set by
    // this call at all) stays unset.
    assert_eq!(std::fs::read(root.join("p.txt")).unwrap(), b"zzz\n");
    assert!(!worktree_is_dirty(&layout).expect("dirty check"));

    let _ = std::fs::remove_dir_all(root);
}

/// Test (0.56.0 step W3c-3, the ruling's own list, item 6):
/// [`plan_materialization`](super::plan_materialization)'s **second** `in_the_way` source --
/// "changed since the prior state" -- in its own deletion loop, distinct from the other tests'
/// "not part of the prior state" (first-loop, add) source.
///
/// **Why this needs a hand-built window, not an ordinary CLI fixture:** the ordinary clean check
/// ([`require_worktree_clean_for_current_branch`](super::require_worktree_clean_for_current_branch))
/// already refuses, at step 1, on exactly this same mismatch for every path it reports as a
/// change -- a file that no longer matches its own recorded content is "dirty" and refused before
/// ever reaching this check, for every path the clean check's own `worktree_status` compares. The
/// only way this second window opens at all is the one the ruling's own item 3 names: "under the
/// lock, only a process outside prikk" can create or rewrite a file *between* the capture
/// ([`require_worktree_clean_and_capture`]'s own already-completed step 1-2) and this call -- a
/// real, if narrow, exposure under the same lock this producer already holds, not a fabricated
/// one. This test drives that exact window directly, the same way
/// `a_crash_between_publish_and_materialize_recovers_via_branch_switch` above drives a different
/// one, rather than trying (and failing) to reach it through the clean check, which cannot let it
/// through.
#[test]
fn require_nothing_in_the_way_refuses_a_file_changed_since_the_prior_state_at_a_deleted_path() {
    let root = unique_temp_dir("w3c3-in-the-way-delete");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    let gen1 =
        publish_genesis(&layout, &[("a.txt", b"a\n"), ("c.txt", b"c\n")]).expect("publish genesis");
    materialize_patch_checkout(&layout, MAIN).expect("materialize gen1");
    record_worktree_baseline(&layout, MAIN, Some(gen1), 0, &[]).expect("record gen1");

    let active_replay = empty_replay(&layout).expect("replay");
    let pending = require_worktree_clean_and_capture(&layout, MAIN, &active_replay, "testing")
        .expect("clean and captured");
    // `pending`'s own captured `old_files` now holds `c.txt`'s real recorded content (`c\n`),
    // read before the line below ever runs.

    // The window the ruling's own item 3 names: a process outside prikk rewrites `c.txt` on disk,
    // after the capture above but before this round's new check runs -- the only way a path can
    // disagree with `old_files` without the clean check having already refused it.
    std::fs::write(root.join("c.txt"), b"tampered-outside-prikk\n").unwrap();

    // The target: `a.txt` only -- `c.txt` is not part of it, so the plan's second (deletion) loop
    // is the one that inspects it.
    let target_manifest = one_file_manifest("a.txt", b"a\n");

    let error = require_nothing_in_the_way(&layout, &pending, &target_manifest, "testing", None)
        .expect_err(
            "a file changed since the prior state at a path the target deletes must refuse",
        );
    let message = error.to_string();
    assert!(
        message.contains("are in the way")
            && message.contains("c.txt (changed since the prior state)")
            && message.contains(&format!("in the way of {MAIN}'s files"))
            && message.contains("move them aside before testing")
            && message.contains("nothing was written"),
        "unexpected message: {message}"
    );

    // Nothing was written: the tampered file is untouched by this read-only check, and the marker
    // stays unset.
    assert_eq!(
        std::fs::read(root.join("c.txt")).unwrap(),
        b"tampered-outside-prikk\n"
    );
    assert!(!worktree_is_dirty(&layout).expect("dirty check"));

    let _ = std::fs::remove_dir_all(root);
}

/// Test: nothing in the way succeeds -- the positive case [`materialize_after_publish`]'s own
/// later, redundant call to the same planner also exercises, checked here directly against the
/// new, earlier check itself.
#[test]
fn require_nothing_in_the_way_succeeds_when_nothing_collides() {
    let root = unique_temp_dir("w3c3-in-the-way-clean");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    let gen1 = publish_genesis(&layout, &[("a.txt", b"a\n")]).expect("publish genesis");
    materialize_patch_checkout(&layout, MAIN).expect("materialize gen1");
    record_worktree_baseline(&layout, MAIN, Some(gen1), 0, &[]).expect("record gen1");

    let active_replay = empty_replay(&layout).expect("replay");
    let pending = require_worktree_clean_and_capture(&layout, MAIN, &active_replay, "testing")
        .expect("clean and captured");

    // A target manifest adding a brand-new path nothing on disk occupies at all.
    let target_manifest = one_file_manifest("b.txt", b"b\n");
    require_nothing_in_the_way(&layout, &pending, &target_manifest, "testing", None)
        .expect("nothing is in the way of an unoccupied path");
    assert!(!worktree_is_dirty(&layout).expect("dirty check"));

    let _ = std::fs::remove_dir_all(root);
}
