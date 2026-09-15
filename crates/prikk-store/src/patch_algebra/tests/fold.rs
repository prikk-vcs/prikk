//! DC-75 two-edits handoff §6, ruling R1: per-side net-effect folding. One control per fold kind, the
//! net no-op and empty-side controls, validation before folding, the unfolded fallback, and report
//! indices on the original sequence.

use super::*;
use crate::patch_algebra::report::MergeEvidenceReport;
use crate::patch_replay::decode::DecodedPatchOperation;
use crate::text_span;
use prikk_object::NodeId;

const SCOPE: EvidenceScope = EvidenceScope::SealedCandidateRequired;

fn judge(
    baseline: &NodeLifecycleState,
    evidence: &TestTextResolver,
    left: &[DecodedPatchOperation],
    right: &[DecodedPatchOperation],
) -> ConfluenceResult {
    let left = fold_side(baseline, evidence, SCOPE, left).operations;
    let right = fold_side(baseline, evidence, SCOPE, right).operations;
    check_confluence_result(baseline, evidence, SCOPE, &left, &right).expect("confluence evidence")
}

fn report(
    baseline: &NodeLifecycleState,
    evidence: &TestTextResolver,
    left: &[DecodedPatchOperation],
    right: &[DecodedPatchOperation],
) -> MergeEvidenceReport {
    analyze_merge_evidence(blob(0xBB), None, baseline, evidence, SCOPE, left, right)
}

/// An `EditText` authored the way `commit` authors one (v2 anchors), so its `span_id` is comparable
/// with a folded edit's.
fn edit_text_v2(op_seq: u32, node_id: NodeId, old: &[u8], new: &[u8]) -> DecodedPatchOperation {
    let plan = text_span::plan_authored_text_span(old, new, node_id)
        .expect("text span plan")
        .expect("changed text");
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::EditText {
            node_id,
            span_id: plan.span_id,
            old_span_hash: plan.old_span_hash,
            left_anchor_hash: plan.left_anchor_hash,
            right_anchor_hash: plan.right_anchor_hash,
            replacement_text: plan.replacement_text,
            old_span_text: plan.old_span_text,
            left_anchor_len: Some(plan.left_anchor_len),
            right_anchor_len: Some(plan.right_anchor_len),
        },
    }
}

const T0: &[u8] = b"alpha beta gamma";

fn text_baseline() -> (NodeLifecycleState, TestTextResolver) {
    let mut baseline = NodeLifecycleState::new();
    seed_text(&mut baseline, node(1), "e.txt", T0, MODE_REGULAR);
    let evidence = TestTextResolver::new([(node(1), T0.to_vec())]).with_blob(
        blob(2),
        BlobKind::Binary,
        b"g".to_vec(),
    );
    (baseline, evidence)
}

fn create_g(op_seq: u32) -> DecodedPatchOperation {
    create_file(op_seq, "g.bin", node(2), blob(2), MODE_REGULAR)
}

fn assert_confluent(result: ConfluenceResult, left_len: usize, right_len: usize) {
    match result {
        ConfluenceResult::Confluent { proof } => {
            assert_eq!(
                (proof.left_len, proof.right_len),
                (left_len, right_len),
                "judged lengths"
            );
        }
        other => panic!("expected confluent, got {other:?}"),
    }
}

#[test]
fn twenty_edits_of_one_text_are_judged_as_one_and_are_confluent() {
    let (baseline, evidence) = text_baseline();
    let texts: Vec<Vec<u8>> = (0..=20)
        .map(|k| {
            if k == 0 {
                T0.to_vec()
            } else {
                format!("alpha {k} gamma").into_bytes()
            }
        })
        .collect();
    let left: Vec<_> = texts
        .windows(2)
        .zip(1u32..)
        .filter_map(|(pair, op_seq)| match pair {
            [old, new] => Some(edit_text(op_seq, node(1), old, new)),
            _ => None,
        })
        .collect();
    assert_eq!(left.len(), 20);
    let right = [create_g(30)];

    assert!(
        !matches!(
            check_confluence_result(&baseline, &evidence, SCOPE, &left, &right),
            Ok(ConfluenceResult::Confluent { .. })
        ),
        "fixture sanity: unfolded, the run is refused"
    );
    assert_confluent(judge(&baseline, &evidence, &left, &right), 1, 1);
    let report = report(&baseline, &evidence, &left, &right);
    assert_eq!(report.outcome, MergeEvidenceOutcome::Confluent);
    assert_eq!(report.left_sequence.operation_count, 20);
    assert_eq!(
        report.left_sequence.folded_through,
        BTreeMap::from([(0, 19)])
    );
}

#[test]
fn edits_then_a_delete_are_judged_as_one_delete_of_the_baseline_content() {
    let (baseline, evidence) = text_baseline();
    let t1: &[u8] = b"alpha BETA gamma";
    let left = [
        edit_text(1, node(1), T0, t1),
        delete_file(
            2,
            "e.txt",
            node(1),
            NodeKind::TextFile,
            text_span::text_blob_id(t1).expect("blob id"),
            MODE_REGULAR,
        ),
    ];
    let right = [create_g(3)];

    let folded = fold_side(&baseline, &evidence, SCOPE, &left);
    assert_eq!(folded.origins, vec![(0, 1)]);
    match &folded.operations.as_slice() {
        [
            DecodedPatchOperation {
                kind:
                    DecodedOperationKind::DeleteNode {
                        preimage:
                            crate::patch_replay::decode::DecodedDeletePreimage::File {
                                old_blob_id, ..
                            },
                        ..
                    },
                ..
            },
        ] => assert_eq!(
            *old_blob_id,
            text_span::text_blob_id(T0).expect("blob id"),
            "the folded delete names the baseline content"
        ),
        other => panic!("expected one folded delete, got {other:?}"),
    }
    assert_confluent(judge(&baseline, &evidence, &left, &right), 1, 1);
}

#[test]
fn a_change_perm_run_is_judged_as_one_change_perm() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "tool", blob(1), MODE_REGULAR);
    let evidence = TestTextResolver::empty().with_blob(blob(2), BlobKind::Binary, b"g".to_vec());
    let left = [
        change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE),
        change_perm(2, node(1), MODE_EXECUTABLE, 0o100600),
    ];
    let right = [create_g(3)];

    assert!(
        !matches!(
            check_confluence_result(&baseline, &evidence, SCOPE, &left, &right),
            Ok(ConfluenceResult::Confluent { .. })
        ),
        "fixture sanity: unfolded, the run is refused"
    );
    assert_eq!(
        fold_side(&baseline, &evidence, SCOPE, &left).operations,
        vec![change_perm(1, node(1), MODE_REGULAR, 0o100600)]
    );
    assert_confluent(judge(&baseline, &evidence, &left, &right), 1, 1);
}

#[test]
fn a_replace_binary_run_is_judged_as_one_replace_binary() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "data.bin", blob(1), MODE_REGULAR);
    let evidence = TestTextResolver::empty()
        .with_blob(blob(2), BlobKind::Binary, b"g".to_vec())
        .with_blob(blob(3), BlobKind::Binary, b"three".to_vec())
        .with_blob(blob(4), BlobKind::Binary, b"four".to_vec());
    let left = [
        replace_binary(1, node(1), blob(1), blob(3)),
        replace_binary(2, node(1), blob(3), blob(4)),
    ];
    let right = [create_g(3)];

    assert!(
        !matches!(
            check_confluence_result(&baseline, &evidence, SCOPE, &left, &right),
            Ok(ConfluenceResult::Confluent { .. })
        ),
        "fixture sanity: unfolded, the run is refused"
    );
    assert_eq!(
        fold_side(&baseline, &evidence, SCOPE, &left).operations,
        vec![replace_binary(1, node(1), blob(1), blob(4))]
    );
    assert_confluent(judge(&baseline, &evidence, &left, &right), 1, 1);
}

#[test]
fn a_net_no_op_drops_the_node_from_the_side() {
    let (baseline, evidence) = text_baseline();
    let t1: &[u8] = b"alpha BETA gamma";
    let left = [
        edit_text(1, node(1), T0, t1),
        edit_text(2, node(1), t1, T0),
        create_g(3),
    ];
    // Without the drop, the left edits and this edit share node 1 and would be deferred.
    let right = [edit_text(4, node(1), T0, b"alpha beta GAMMA")];

    let folded = fold_side(&baseline, &evidence, SCOPE, &left);
    assert_eq!(folded.operations, vec![create_g(3)]);
    assert_eq!(folded.origins, vec![(2, 2)]);
    assert_confluent(judge(&baseline, &evidence, &left, &right), 1, 1);
}

#[test]
fn a_side_that_folds_to_nothing_is_judged_as_empty() {
    let (baseline, evidence) = text_baseline();
    let t1: &[u8] = b"alpha BETA gamma";
    let left = [edit_text(1, node(1), T0, t1), edit_text(2, node(1), t1, T0)];
    let right = [create_g(3)];

    assert!(
        fold_side(&baseline, &evidence, SCOPE, &left)
            .operations
            .is_empty()
    );
    assert_confluent(judge(&baseline, &evidence, &left, &right), 0, 1);
    let report = report(&baseline, &evidence, &left, &right);
    assert_eq!(report.outcome, MergeEvidenceOutcome::Confluent);
    assert_eq!(report.left_sequence.operation_count, 2);
}

#[test]
fn a_side_that_does_not_replay_is_judged_unfolded_as_before() {
    let (baseline, evidence) = text_baseline();
    let left = [
        edit_text(1, node(1), T0, b"alpha BETA gamma"),
        // Authored against text the first edit never produced: the side does not replay, so no fold.
        edit_text(2, node(1), b"zeta eta theta", b"zeta ETA theta"),
    ];
    let right = [create_g(3)];

    let folded = fold_side(&baseline, &evidence, SCOPE, &left);
    assert_eq!(folded.operations, left.to_vec());
    assert_eq!(folded.origins, vec![(0, 0), (1, 1)]);
    match judge(&baseline, &evidence, &left, &right) {
        ConfluenceResult::NotConfluent { witness } => {
            assert_eq!(witness.kind, ConfluenceWitnessKind::ReplayFailure);
            assert_eq!(witness.left_index, Some(1));
        }
        other => panic!("expected the unfolded refusal, got {other:?}"),
    }
}

#[test]
fn a_right_side_replay_failure_names_the_right_operation() {
    let (baseline, evidence) = text_baseline();
    let left = [create_g(1)];
    let right = [edit_text(2, node(1), b"zeta eta theta", b"zeta ETA theta")];

    let report = report(&baseline, &evidence, &left, &right);
    assert_eq!(report.outcome, MergeEvidenceOutcome::NotConfluent);
    let [item] = report.items.as_slice() else {
        panic!("expected one item, got {:?}", report.items);
    };
    assert_eq!(item.reason_code, MergeEvidenceReasonCode::PairReplayFailed);
    assert_eq!(
        (item.operation_index, item.peer_operation_index),
        (None, Some(0))
    );
}

#[test]
fn every_original_operation_is_validated_before_any_fold() {
    let (baseline, evidence) = text_baseline();
    let t1: &[u8] = b"alpha BETA gamma";
    let left = [
        edit_text(1, node(1), T0, t1),
        edit_text(2, node(1), t1, b"alpha BETA GAMMA"),
        // No evidence for this blob: a sealed candidate's missing evidence is an evidence failure.
        create_file(3, "h.bin", node(3), blob(9), MODE_REGULAR),
    ];
    let right = [create_g(4)];

    let report = report(&baseline, &evidence, &left, &right);
    assert_eq!(report.outcome, MergeEvidenceOutcome::EvidenceFailure);
    assert!(report.left_sequence.folded_through.is_empty());
}

#[test]
fn a_folded_conflict_names_the_original_range() {
    let (baseline, evidence) = text_baseline();
    let t1: &[u8] = b"alpha BETA gamma";
    let t2: &[u8] = b"alpha BETA GAMMA";
    let left = [
        create_g(1),
        edit_text_v2(2, node(1), T0, t1),
        edit_text_v2(3, node(1), t1, t2),
    ];
    // The same net change from the baseline: the same span, so a true overlap (R2).
    let right = [edit_text_v2(4, node(1), T0, t2)];

    let first = report(&baseline, &evidence, &left, &right);
    assert_eq!(first.outcome, MergeEvidenceOutcome::Conflict);
    let [item] = first.items.as_slice() else {
        panic!("expected one item, got {:?}", first.items);
    };
    assert_eq!(
        item.witness_kind,
        Some(ConflictWitnessKind::TextSpanOverlap)
    );
    assert_eq!(
        (item.operation_index, item.op_seq),
        (Some(1), Some(2)),
        "the folded operation is named by the first original operation of its run"
    );
    assert_eq!(
        (item.peer_operation_index, item.peer_op_seq),
        (Some(0), Some(4))
    );
    assert_eq!(first.left_sequence.folded_through, BTreeMap::from([(1, 2)]));
    assert_eq!(first.left_sequence.operation_count, 3);
    assert_eq!(
        first,
        report(&baseline, &evidence, &left, &right),
        "deterministic"
    );
}
