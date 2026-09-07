//! Merge-evidence store boundary tests.

mod merge_plan;

use prikk_error::Result;
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, ChangePerm, CreateFile,
    DeleteNode, DeleteNodePreimage, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose,
};

use super::{MergeEvidenceTarget, prepare_merge_evidence};
use crate::received::write_received_pointer;
use crate::test_gates::test_support::{
    dummy_signature, maintainer_signature, signed_block, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};

#[test]
fn block_targets_report_confluent_for_independent_create_sequences() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-block-targets");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let left = write_create_block(&layout, BlockKind::Normal, vec![baseline], "left.txt", 0x21)?;
    let right = write_create_block(
        &layout,
        BlockKind::Normal,
        vec![baseline],
        "right.txt",
        0x22,
    )?;

    let report = prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(left),
        MergeEvidenceTarget::Block(right),
    )?;

    assert_eq!(report.outcome, "Confluent");
    assert_eq!(report.reason, Some("proven_confluent"));
    assert_eq!(report.left_operation_count, 1);
    assert_eq!(report.right_operation_count, 1);
    assert_eq!(report.displayed_item_count(), report.total_item_count());
    assert_eq!(report.left_selector.target_block_id, left);
    assert_eq!(report.right_selector.target_block_id, right);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn ref_target_reports_selector_and_resolved_block() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-ref-target");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let left = write_create_block(&layout, BlockKind::Normal, vec![baseline], "left.txt", 0x23)?;
    publish_ref(&layout, "heads/left", left)?;

    let report = prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::Ref("heads/left".to_string()),
        MergeEvidenceTarget::Block(baseline),
    )?;

    assert_eq!(report.left_selector.selector, "ref heads/left");
    assert_eq!(report.left_selector.target_block_id, left);
    assert_eq!(report.right_operation_count, 0);
    assert!(matches!(
        report.outcome,
        "Confluent" | "Unsupported" | "InvalidCandidate"
    ));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-85: `MergeEvidenceTarget::ReceivedRef` resolves through `received.rs`'s pointer store, not
/// `RefStore` — mirrors `ref_target_reports_selector_and_resolved_block` above for the local-ref
/// variant. Deliberately writes the RefState's embedded `ref_name` as the *origin's* own name
/// (`"heads/left"`), never the local `"remotes/..."` label, per DC-85 §3A's carried-forward asymmetry
/// note: the received-ref resolver must not (and does not) apply the local-ref arm's name-equality
/// check.
#[test]
fn received_ref_target_reports_selector_and_resolved_block() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-received-ref-target");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let left = write_create_block(&layout, BlockKind::Normal, vec![baseline], "left.txt", 0x26)?;

    let mut store = FileObjectStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/left", None, left, 1);
    let ref_state_id = store.write_object(&ref_state)?;
    write_received_pointer(&layout, "remotes/heads/left", ref_state_id)?;

    let report = prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::ReceivedRef("remotes/heads/left".to_string()),
        MergeEvidenceTarget::Block(baseline),
    )?;

    assert_eq!(
        report.left_selector.selector,
        "received ref remotes/heads/left"
    );
    assert_eq!(report.left_selector.target_block_id, left);
    assert_eq!(report.right_operation_count, 0);
    assert!(matches!(
        report.outcome,
        "Confluent" | "Unsupported" | "InvalidCandidate"
    ));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn empty_sequences_are_report_level_success() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-empty-sequences");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;

    let report = prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(baseline),
        MergeEvidenceTarget::Block(baseline),
    )?;

    assert_eq!(report.outcome, "Confluent");
    assert_eq!(report.reason, Some("proven_confluent"));
    assert_eq!(report.left_operation_count, 0);
    assert_eq!(report.right_operation_count, 0);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn cross_display_preserves_distinct_left_and_right_operations() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-cross-display-sides");
    let layout = RepositoryLayout::init(root.clone())?;
    let node_id = NodeId::from_bytes([0x41; 32]);
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
        OperationKind::DeleteNode(DeleteNode {
            path: "tracked.txt".to_string(),
            node_id,
            old_node_kind: NodeKind::TextFile,
            preimage: DeleteNodePreimage::File {
                old_blob_id: blob_id,
                old_mode: 0o100_644,
            },
        }),
    )?;

    let report = prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(left),
        MergeEvidenceTarget::Block(right),
    )?;

    assert_eq!(report.outcome, "Conflict");
    assert_eq!(report.reason, Some("pair_conflict"));
    let Some(cross_item) = report.items.iter().find(|item| item.side == "cross") else {
        panic!("missing cross display item");
    };
    let Some(left_operation) = cross_item.operation.as_ref() else {
        panic!("missing left operation summary");
    };
    let Some(right_operation) = cross_item.peer_operation.as_ref() else {
        panic!("missing right operation summary");
    };
    assert_eq!(left_operation.index, 0);
    assert_eq!(left_operation.op_seq, Some(1));
    assert_eq!(left_operation.kind, Some("ChangePerm"));
    assert_eq!(left_operation.path, None);
    assert_eq!(right_operation.index, 0);
    assert_eq!(right_operation.op_seq, Some(1));
    assert_eq!(right_operation.kind, Some("DeleteNode"));
    assert_eq!(right_operation.path.as_deref(), Some("tracked.txt"));
    // A ChangePerm/DeleteNode pair on the same node is a `DeleteMutationConflict`. The `ChangePerm`
    // operand's own summary carries no path (asserted above) -- but the pair shares one path, and
    // conflict-witness-path-derivation handoff v1 derives it from the `DeleteNode` side rather than
    // discarding it: this is the exact gap that handoff was written to close (found reviewing
    // DC-21), pinned end to end against the actual public `MergeEvidenceDisplayItem`.
    assert_eq!(cross_item.witness_kind, Some("delete-mutation-conflict"));
    assert_eq!(cross_item.witness_path.as_deref(), Some("tracked.txt"));
    assert_eq!(cross_item.witness_node_id, Some(node_id));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn missing_ancestry_fails_before_report() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-missing-ancestry");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let other_root = write_create_block(&layout, BlockKind::Root, Vec::new(), "other.txt", 0x24)?;

    let err = match prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(other_root),
        MergeEvidenceTarget::Block(baseline),
    ) {
        Ok(_) => panic!("missing ancestry unexpectedly succeeded"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("is not an ancestor"));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn multi_parent_normal_candidate_fails_before_report() -> Result<()> {
    // DC-75 changed scope deliberately: `candidate_blocks`' reachability walk now follows *all*
    // parents (a `Merge` block's secondary parent can be the only path back to a repeated merge's
    // baseline), so ">1 parent" alone is no longer, by itself, an unsupported shape. What is still
    // rejected is a *malformed* multi-parent block -- here, `BlockKind::Normal` with two parents,
    // which `format2`'s own shape gate (`validate_block_v2_shape`, now called by
    // `ancestors_inclusive` on every block the walk reads) has never permitted. The assertion and
    // this test's name were updated accordingly; see `candidate_ready_merge_block_reaches_report`
    // below for the now-open two-parent `Merge` shape.
    let root = unique_temp_dir("merge-evidence-multi-parent-candidate");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let other_parent = write_create_block(
        &layout,
        BlockKind::Root,
        Vec::new(),
        "other-parent.txt",
        0x25,
    )?;
    let mut parents = vec![baseline, other_parent];
    parents.sort();
    let target = write_block(&layout, BlockKind::Normal, parents, Vec::new())?;

    let err = match prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(target),
        MergeEvidenceTarget::Block(baseline),
    ) {
        Ok(_) => panic!("multi-parent candidate unexpectedly succeeded"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("Normal Block must have exactly one parent")
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-75: the now-open shape — a properly-formed two-parent `Merge` block, with a valid mainline
/// parent and recorded baseline — reaches the report stage rather than being refused as an
/// unsupported multi-parent shape (the pre-DC-75 behavior every other `Merge` construction in this
/// file used to hit).
#[test]
fn candidate_ready_merge_block_reaches_report() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-merge-block-candidate");
    let layout = RepositoryLayout::init(root.clone())?;
    let genesis = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let mainline_parent = write_create_block(
        &layout,
        BlockKind::Normal,
        vec![genesis],
        "mainline.txt",
        0x51,
    )?;
    let secondary_parent = write_create_block(
        &layout,
        BlockKind::Normal,
        vec![genesis],
        "secondary.txt",
        0x52,
    )?;
    let merge_block = write_merge_block(
        &layout,
        vec![mainline_parent, secondary_parent],
        mainline_parent,
        genesis,
        Vec::new(),
    )?;

    let report = prepare_merge_evidence(
        &layout,
        genesis,
        MergeEvidenceTarget::Block(merge_block),
        MergeEvidenceTarget::Block(genesis),
    )?;

    assert_eq!(
        report.left_operation_count, 2,
        "mainline's and secondary's creates, no error"
    );
    assert_eq!(report.right_operation_count, 0);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn missing_or_wrong_type_patch_fails_before_report() -> Result<()> {
    let root = unique_temp_dir("merge-evidence-missing-patch");
    let layout = RepositoryLayout::init(root.clone())?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), Vec::new())?;
    let wrong_type_id = write_blob(&layout, b"not a patch")?;
    let target = write_block(
        &layout,
        BlockKind::Normal,
        vec![baseline],
        vec![wrong_type_id],
    )?;

    let err = match prepare_merge_evidence(
        &layout,
        baseline,
        MergeEvidenceTarget::Block(target),
        MergeEvidenceTarget::Block(baseline),
    ) {
        Ok(_) => panic!("wrong-type patch unexpectedly succeeded"),
        Err(err) => err,
    };

    let error = err.to_string();
    assert!(
        error.contains("Patch") || error.contains("object type mismatch"),
        "unexpected error: {error}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

pub(super) fn write_create_block(
    layout: &RepositoryLayout,
    kind: BlockKind,
    parents: Vec<ObjectId>,
    path: &str,
    node_byte: u8,
) -> Result<ObjectId> {
    let mut store = FileObjectStore::new(layout.clone());
    let blob = BlobPayload::new(BlobKind::Text, format!("{path}\n").into_bytes());
    let blob_bytes = blob.to_canonical_bytes()?;
    let blob_id = ObjectId::from_canonical_payload(ObjectType::Blob, 1, &blob_bytes);
    let mut blob_env = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_bytes);
    blob_env.add_signature(maintainer_signature())?;
    store.write_object(&blob_env)?;

    let patch = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_byte; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    write_patch_block(layout, kind, parents, patch)
}

pub(super) fn write_operation_block(
    layout: &RepositoryLayout,
    kind: BlockKind,
    parents: Vec<ObjectId>,
    operation: OperationKind,
) -> Result<ObjectId> {
    let patch = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: operation,
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    write_patch_block(layout, kind, parents, patch)
}

fn write_patch_block(
    layout: &RepositoryLayout,
    kind: BlockKind,
    parents: Vec<ObjectId>,
    patch: PatchPayload,
) -> Result<ObjectId> {
    let mut store = FileObjectStore::new(layout.clone());
    let mut patch_env = ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch.to_canonical_bytes()?);
    patch_env.add_signature(dummy_signature())?;
    let patch_id = store.write_object(&patch_env)?;
    write_block(layout, kind, parents, vec![patch_id])
}

pub(super) fn write_blob(layout: &RepositoryLayout, content: &[u8]) -> Result<ObjectId> {
    let mut store = FileObjectStore::new(layout.clone());
    let blob = BlobPayload::new(BlobKind::Text, content.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    store.write_object(&envelope)
}

pub(super) fn write_block(
    layout: &RepositoryLayout,
    kind: BlockKind,
    parents: Vec<ObjectId>,
    patches: Vec<ObjectId>,
) -> Result<ObjectId> {
    let mut store = FileObjectStore::new(layout.clone());
    let block = signed_block(kind, parents, patches, None);
    store.write_object(&block)
}

/// DC-75: a `Merge` block, distinct from `write_block`'s always-`None` mainline/baseline fields.
/// State root is `compute_state_root(&[])`, same placeholder `write_block`/`signed_block` already
/// use elsewhere in this file — `prepare_merge_evidence`'s candidate walk checks shape, not state
/// root correctness (that is `verify`'s job, exercised separately).
pub(super) fn write_merge_block(
    layout: &RepositoryLayout,
    mut parents: Vec<ObjectId>,
    mainline: ObjectId,
    baseline: ObjectId,
    patches: Vec<ObjectId>,
) -> Result<ObjectId> {
    parents.sort();
    let mut store = FileObjectStore::new(layout.clone());
    let payload = BlockPayload {
        parent_block_ids: parents,
        kind: BlockKind::Merge,
        patch_ids: patches,
        state_merkle_root: crate::compute_state_root(&[])?,
        snapshot_blob_ref: None,
        mainline_parent_id: Some(mainline),
        merge_baseline_block_id: Some(baseline),
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    store.write_object(&envelope)
}

fn publish_ref(layout: &RepositoryLayout, ref_name: &str, block_id: ObjectId) -> Result<()> {
    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope(ref_name, None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(ref_name, None, ref_state_id, block_id, 1);
    ref_store
        .publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        })
        .map(|_| ())
}
