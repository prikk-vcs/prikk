use prikk_error::Result;
use prikk_object::{BlockKind, ChangePerm, CreateFile, NodeId, ObjectId, OperationKind};

use super::{write_blob, write_block, write_create_block, write_operation_block};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    MergeEvidenceDisplay, MergeEvidenceDisplaySelector, MergeEvidenceTarget, MergePlanDisplay,
    RepositoryLayout, prepare_merge_plan,
};

#[test]
fn maps_confluent_evidence_to_confluent_subset() -> Result<()> {
    let root = unique_temp_dir("merge-plan-confluent-status");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let left = write_create_block(&layout, BlockKind::Normal, vec![baseline], "left.txt", 0x31)?;
    let right = write_create_block(
        &layout,
        BlockKind::Normal,
        vec![baseline],
        "right.txt",
        0x32,
    )?;

    let plan = prepare_merge_plan(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(left),
        MergeEvidenceTarget::Block(right),
    )?;

    assert_eq!(plan.status, "ConfluentSubset");
    assert_eq!(
        plan.action,
        "review the evidence, then run 'prikk merge' to execute"
    );
    assert_eq!(plan.evidence.outcome, "Confluent");
    assert_eq!(plan.evidence.reason, Some("proven_confluent"));
    assert_eq!(plan.evidence.left_selector.target_block_id, left);
    assert_eq!(plan.evidence.right_selector.target_block_id, right);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn maps_conflict_evidence_to_blocked_conflict() -> Result<()> {
    let root = unique_temp_dir("merge-plan-conflict-status");
    let layout = RepositoryLayout::init(root.clone())?;
    let node_id = NodeId::from_bytes([0x51; 32]);
    let blob_id = write_blob(&layout, b"tracked.txt\n")?;
    let baseline = write_operation_block(
        &layout,
        BlockKind::Root,
        Vec::new(),
        OperationKind::CreateFile(CreateFile {
            path: "tracked.txt".to_string(),
            node_id,
            blob_id,
            mode: 0o100_644,
        }),
    )?;
    let left = write_operation_block(
        &layout,
        BlockKind::Normal,
        vec![baseline],
        OperationKind::ChangePerm(ChangePerm {
            node_id,
            old_mode: 0o100_644,
            new_mode: 0o100_755,
        }),
    )?;
    let right = write_operation_block(
        &layout,
        BlockKind::Normal,
        vec![baseline],
        OperationKind::ChangePerm(ChangePerm {
            node_id,
            old_mode: 0o100_644,
            new_mode: 0o100_755,
        }),
    )?;

    let plan = prepare_merge_plan(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(left),
        MergeEvidenceTarget::Block(right),
    )?;

    assert_eq!(plan.status, "BlockedConflict");
    assert_eq!(
        plan.action,
        "inspect evidence; conflict resolution is not implemented"
    );
    assert_eq!(plan.evidence.outcome, "Conflict");
    assert_eq!(plan.evidence.reason, Some("pair_conflict"));
    assert_eq!(plan.displayed_item_count(), plan.total_item_count());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn status_mapping_covers_all_evidence_outcomes() {
    let cases = [
        (
            "Confluent",
            "ConfluentSubset",
            "review the evidence, then run 'prikk merge' to execute",
        ),
        (
            "Conflict",
            "BlockedConflict",
            "inspect evidence; conflict resolution is not implemented",
        ),
        (
            "OrderedDependency",
            "BlockedOrderedDependency",
            "inspect ordering evidence; execution ordering policy is not implemented",
        ),
        (
            "Unsupported",
            "BlockedUnsupported",
            "inspect unsupported operation evidence",
        ),
        (
            "Deferred",
            "BlockedDeferred",
            "inspect deferred design evidence",
        ),
        (
            "NotConfluent",
            "BlockedNotConfluent",
            "inspect replay/final-state mismatch evidence",
        ),
        (
            "EvidenceFailure",
            "BlockedEvidenceFailure",
            "repair or verify repository evidence before planning",
        ),
        (
            "InvalidCandidate",
            "BlockedInvalidCandidate",
            "select valid sealed candidates before planning",
        ),
    ];

    for (outcome, status, action) in cases {
        let plan = MergePlanDisplay::from_evidence(display_with_outcome(outcome));
        assert_eq!(plan.status, status);
        assert_eq!(plan.action, action);
    }
}

fn display_with_outcome(outcome: &'static str) -> MergeEvidenceDisplay {
    let id = ObjectId::from_bytes([0x77; 32]);
    MergeEvidenceDisplay {
        baseline_block_id: id,
        left_selector: MergeEvidenceDisplaySelector {
            selector: "block left".to_string(),
            target_block_id: id,
        },
        right_selector: MergeEvidenceDisplaySelector {
            selector: "block right".to_string(),
            target_block_id: id,
        },
        outcome,
        reason: None,
        left_operation_count: 0,
        right_operation_count: 0,
        items: Vec::new(),
    }
}
