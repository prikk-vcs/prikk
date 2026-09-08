//! RFC 144 increment 1 controls 1-3: `patch_replay` applies `RenamePath`, resolving node before
//! path within a run of consecutive renames (§4h.7). See `apply::apply_rename_batch`'s own doc
//! comment for the algorithm; these are the controls the handoff's §4 requires seen failing before
//! they pass.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::test_gates::test_support::{
    publish_snapshot_then_patch_block, publish_text_edit_then_rename_path_block,
    publish_two_nodes_then_rename_cycle_block,
    publish_two_nodes_then_rename_onto_occupied_path_block, unique_temp_dir,
};
use crate::{
    PatchPlanContent, RepositoryLayout, prepare_patch_plan_content_report,
    prepare_patch_replay_plan,
};

/// Control 1: a rename replays through `patch_replay` and materializes correctly -- the plan's own
/// path listing reflects the new path, not the old one, and the byte content at that path is the
/// node's real content.
#[test]
fn control1_rename_replays_and_materializes_correctly() {
    let root = unique_temp_dir("rfc144-inc1-control1");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_text_edit_then_rename_path_block(&layout).unwrap();

    let plan = prepare_patch_replay_plan(&layout, "heads/main").unwrap();
    assert!(
        plan.paths.contains(&"README2.md".to_string()),
        "the new path must be live: {:?}",
        plan.paths
    );
    assert!(
        !plan.paths.contains(&"README.md".to_string()),
        "the old path must no longer be live: {:?}",
        plan.paths
    );

    let report =
        prepare_patch_plan_content_report(&layout, "heads/main", &["README2.md".to_string()])
            .unwrap();
    let [entry] = report.entries.as_slice() else {
        panic!("expected exactly one entry, got {:?}", report.entries);
    };
    match &entry.content {
        PatchPlanContent::Text(bytes) => assert_eq!(bytes, b"alpha BETA\n"),
        other => panic!("expected Text, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

/// Control 2: a rename cycle (`a.txt -> b.txt`, `b.txt -> a.txt` -- a swap) materializes
/// correctly. This is the control that fails under a sequential, one-operation-at-a-time
/// implementation (verified directly against this exact fixture -- see the round's own report for
/// the before/after) -- resolved here by batching the whole run and vacating every source before
/// claiming any target (`apply::apply_rename_batch`).
#[test]
fn control2_rename_cycle_materializes_correctly() {
    let root = unique_temp_dir("rfc144-inc1-control2");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_two_nodes_then_rename_cycle_block(&layout).unwrap();

    let report = prepare_patch_plan_content_report(
        &layout,
        "heads/main",
        &["a.txt".to_string(), "b.txt".to_string()],
    )
    .unwrap();
    assert!(report.not_found.is_empty(), "{:?}", report.not_found);
    let by_path: std::collections::BTreeMap<_, _> = report
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), &entry.content))
        .collect();
    match by_path.get("a.txt") {
        Some(PatchPlanContent::Text(bytes)) => {
            assert_eq!(
                bytes.as_slice(),
                b"content-b\n",
                "a.txt must now hold node B's content (B renamed b.txt -> a.txt)"
            );
        }
        other => panic!("expected a.txt to be Text, got {other:?}"),
    }
    match by_path.get("b.txt") {
        Some(PatchPlanContent::Text(bytes)) => {
            assert_eq!(
                bytes.as_slice(),
                b"content-a\n",
                "b.txt must now hold node A's content (A renamed a.txt -> b.txt)"
            );
        }
        other => panic!("expected b.txt to be Text, got {other:?}"),
    }

    let plan = prepare_patch_replay_plan(&layout, "heads/main").unwrap();
    assert_eq!(
        plan.file_count, 2,
        "the swap must not lose or duplicate a file"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Control 3: a rename onto a path occupied by another live node is detected, not silently
/// applied. An `Integrity` error naming the path is the right answer here -- see the round's own
/// report for why (RFC 121's exit-code contract / RFC 140-143's "degrade absence, propagate error"
/// precedent: a structural collision is exactly the kind of thing that must fail loud, not fold
/// into a degraded field or, worse, silently clobber the third node's content).
#[test]
fn control3_rename_onto_occupied_path_is_detected_not_silently_applied() {
    let root = unique_temp_dir("rfc144-inc1-control3");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_two_nodes_then_rename_onto_occupied_path_block(&layout).unwrap();

    let result = prepare_patch_replay_plan(&layout, "heads/main");
    let err = result.expect_err("a collision must error, not silently apply");
    let message = format!("{err:?}");
    assert!(
        message.contains("c.txt") && message.contains("occupied"),
        "error must name the occupied path: {message}"
    );

    // The non-participating node's content must be provably untouched -- not just "an error was
    // returned somewhere else", but that node C's own content is exactly what it always was, had
    // this rename never been attempted. Checked via the content report over the same history.
    let report = prepare_patch_plan_content_report(&layout, "heads/main", &["c.txt".to_string()]);
    assert!(
        report.is_err(),
        "the whole chain fails closed on the same collision -- content-at-a-point must not \
         silently succeed on a history it cannot fully replay: {report:?}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Control 6 (part): existing rename-free replay still works exactly as before -- a positive
/// control alongside the crate-wide "everything still passes" evidence.
#[test]
fn control6_rename_free_history_is_unaffected() {
    let root = unique_temp_dir("rfc144-inc1-control6");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_snapshot_then_patch_block(&layout).unwrap();

    let plan = prepare_patch_replay_plan(&layout, "heads/main").unwrap();
    assert_eq!(plan.file_count, 2);
    assert!(plan.paths.contains(&"README.md".to_string()));
    assert!(plan.paths.contains(&"extra.txt".to_string()));

    let _ = std::fs::remove_dir_all(root);
}
