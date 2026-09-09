use super::*;

#[test]
fn confluent_report_includes_required_baseline_and_sequence_summaries() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    let left = [replace_binary(10, node(1), blob(1), blob(3))];
    let right = [change_perm(20, node(2), MODE_REGULAR, MODE_EXECUTABLE)];
    let evidence = TestTextResolver::empty().with_blob(blob(3), BlobKind::Binary, b"new".to_vec());

    let report = analyze_merge_evidence(
        blob(0xb0),
        Some(blob(0xa0)),
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.baseline_block_id, blob(0xb0));
    assert_eq!(report.replay_horizon, Some(blob(0xa0)));
    assert_eq!(report.outcome, MergeEvidenceOutcome::Confluent);
    assert_eq!(report.left_sequence.label, "left");
    assert_eq!(report.left_sequence.operation_count, 1);
    let left_operation = report
        .left_sequence
        .operations
        .first()
        .expect("left operation summary");
    assert_eq!(
        left_operation.operation_kind,
        MergeEvidenceOperationKind::ReplaceBinary
    );
    assert_eq!(left_operation.op_seq, 10);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::ProvenConfluent
    );
}

#[test]
fn concrete_conflict_report_is_not_generic_not_confluent() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "tool", blob(1), MODE_REGULAR);
    let left = [change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE)];
    let right = [change_perm(2, node(1), MODE_REGULAR, 0o100600)];

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Conflict);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::PairConflict
    );
    let item = first_item(&report.items);
    assert_eq!(item.operation_index, Some(0));
    assert_eq!(item.peer_operation_index, Some(0));
    assert_eq!(item.op_seq, Some(1));
    assert_eq!(item.peer_op_seq, Some(2));
    // ChangePerm vs ChangePerm on the same node falls through `classify_same_node`'s catch-all
    // arm, not a more specific kind -- this is genuinely `UnknownRelation`, not an oversight.
    assert_eq!(
        item.witness_kind,
        Some(ConflictWitnessKind::UnknownRelation)
    );
}

#[test]
fn ordered_dependency_report_has_dedicated_outcome() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "same.bin", blob(1), MODE_REGULAR);
    let left = delete_file(
        1,
        "same.bin",
        node(1),
        NodeKind::BinaryFile,
        blob(1),
        MODE_REGULAR,
    );
    let right = create_file(2, "same.bin", node(2), blob(2), MODE_REGULAR);
    let evidence =
        TestTextResolver::empty().with_blob(blob(2), BlobKind::Binary, b"fresh".to_vec());

    let report = analyze_pair_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &evidence,
        EvidenceScope::UnsealedCandidateOptional,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::OrderedDependency);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::OrderedDependency
    );
}

#[test]
fn unsupported_operation_report_does_not_expose_unknown() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    let left = [rename_path(1, node(1), "left.bin", "moved.bin")];
    let right = [replace_binary(2, node(2), blob(2), blob(3))];
    let evidence = TestTextResolver::empty().with_blob(blob(3), BlobKind::Binary, b"new".to_vec());

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Unsupported);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::UnsupportedOperation
    );
    assert!(!format!("{report:?}").contains("Unknown"));
}

#[test]
fn same_node_text_transform_is_deferred_not_not_confluent() {
    let old = b"alpha beta gamma";
    let mut baseline = NodeLifecycleState::new();
    seed_text(&mut baseline, node(1), "doc.txt", old, MODE_REGULAR);
    let left = [edit_text(1, node(1), old, b"alpha BETA gamma")];
    let right = [edit_text(2, node(1), old, b"alpha beta GAMMA")];
    let evidence = TestTextResolver::new([(node(1), old.to_vec())]);

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Deferred);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::SameNodeTextTransformDeferred
    );
}

#[test]
fn required_sealed_evidence_failure_has_highest_public_outcome() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    let left = [rename_path(1, node(1), "left.bin", "moved.bin")];
    let right = [replace_binary(2, node(2), blob(2), blob(3))];

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::EvidenceFailure);
    assert_eq!(
        first_item(&report.items).evidence_scope,
        Some(MergeEvidenceScope::SealedCandidate)
    );
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::MissingRequiredEvidence
    );
}

#[test]
fn optional_unsealed_candidate_evidence_failure_is_invalid_candidate() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "bin", blob(1), MODE_REGULAR);
    let left = [replace_binary(1, node(1), blob(1), blob(2))];
    let right = [change_perm(2, node(1), MODE_REGULAR, MODE_EXECUTABLE)];

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::UnsealedCandidateOptional,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::InvalidCandidate);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::InsufficientUnsealedCandidateEvidence
    );
}

#[test]
fn malformed_unsealed_candidate_operation_is_invalid_candidate() {
    let baseline = NodeLifecycleState::new();
    let left = [create_file(1, "/absolute", node(1), blob(1), MODE_REGULAR)];
    let right = [change_perm(2, node(2), MODE_REGULAR, MODE_EXECUTABLE)];

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::UnsealedCandidateOptional,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::InvalidCandidate);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::InvalidUnsealedCandidate
    );
    assert_eq!(
        report
            .left_sequence
            .operations
            .first()
            .expect("left operation summary")
            .path,
        None
    );
}

/// Control 1 (conflict-witness-presentation handoff v1 §6.1), kind 1 of 3: two sides create
/// distinct nodes at the identical path.
#[test]
fn same_path_create_conflict_names_its_kind_and_path() {
    let baseline = NodeLifecycleState::new();
    let left = [create_file(1, "shared.txt", node(1), blob(1), MODE_REGULAR)];
    let right = [create_file(2, "shared.txt", node(2), blob(2), MODE_REGULAR)];
    let evidence = TestTextResolver::empty()
        .with_blob(blob(1), BlobKind::Binary, b"left".to_vec())
        .with_blob(blob(2), BlobKind::Binary, b"right".to_vec());

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Conflict);
    let item = first_item(&report.items);
    assert_eq!(item.witness_kind, Some(ConflictWitnessKind::SamePathCreate));
    assert_eq!(
        item.path.as_ref().map(|path| path.as_str()),
        Some("shared.txt")
    );
}

/// RFC 144 §4o.5 control 4: the thirteenth conflict witness reaches the report item and carries
/// its label -- through `commute_pair`'s own real classification (`analyze_pair_merge_evidence`,
/// the same pairwise entry point every other single-pair report test in this file uses), not a
/// constructed `PairClass::Conflict` value (`every_conflict_witness_kind_reaches_the_report_item`,
/// just above this file's own witness-kind sweep, only proves the *mapping* is kind-agnostic; it
/// says nothing about whether real classification ever actually produces this kind). Also proves
/// both destinations are independently recoverable from the report's own per-side sequences --
/// the mechanism `operation_path`'s own doc comment describes (`report/input.rs`), exercised here
/// rather than only asserted in isolation.
///
/// **Deliberately not `analyze_merge_evidence`** (the sequence-level entry point real
/// `prikk merge-evidence` reaches): see this round's own report for why -- `commutation.rs`'s
/// `ensure_flat_sequence` defers on *any* individually-deferred operation before the pairwise loop
/// this round's own classification fix lives in ever runs, so a rename-destination conflict is not
/// yet reachable through that fuller path. A genuine finding, not fixed in this round.
#[test]
fn rename_destination_conflict_reaches_the_report_item_with_both_destinations() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(
        &mut baseline,
        node(1),
        "shared-source.bin",
        blob(1),
        MODE_REGULAR,
    );
    let left = rename_path(1, node(1), "shared-source.bin", "left-dest.bin");
    let right = rename_path(2, node(1), "shared-source.bin", "right-dest.bin");

    let report = analyze_pair_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Conflict);
    let item = first_item(&report.items);
    assert_eq!(
        item.witness_kind,
        Some(ConflictWitnessKind::RenameDestinationConflict)
    );
    assert_eq!(
        ConflictWitnessKind::RenameDestinationConflict.label(),
        "rename-destination-conflict"
    );

    let left_destination = report
        .left_sequence
        .operations
        .get(item.operation_index.expect("operation index"))
        .and_then(|op| op.path.as_ref().map(|path| path.as_str()));
    let right_destination = report
        .right_sequence
        .operations
        .get(item.peer_operation_index.expect("peer operation index"))
        .and_then(|op| op.path.as_ref().map(|path| path.as_str()));
    assert_eq!(left_destination, Some("left-dest.bin"));
    assert_eq!(right_destination, Some("right-dest.bin"));
}

/// Control 1, kind 2 of 3: two sides edit the identical text span (same technique
/// `same_node_identical_text_span_is_conflict_not_independent` uses to force a shared `span_id`).
#[test]
fn text_span_overlap_conflict_names_its_kind() {
    let mut baseline = NodeLifecycleState::new();
    seed_text(
        &mut baseline,
        node(1),
        "note.txt",
        b"alpha beta gamma",
        MODE_REGULAR,
    );
    let left = edit_text(1, node(1), b"alpha beta gamma", b"alpha BETA gamma");
    let mut right = edit_text(2, node(1), b"alpha beta gamma", b"alpha BETTER gamma");
    if let (
        DecodedOperationKind::EditText {
            span_id: right_span,
            ..
        },
        DecodedOperationKind::EditText {
            span_id: left_span, ..
        },
    ) = (&mut right.kind, &left.kind)
    {
        *right_span = *left_span;
    }
    let left = [left];
    let right = [right];
    let evidence = TestTextResolver::new([(node(1), b"alpha beta gamma".to_vec())]);

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Conflict);
    assert_eq!(
        first_item(&report.items).witness_kind,
        Some(ConflictWitnessKind::TextSpanOverlap)
    );
}

/// Control 1, kind 3 of 3 (delete-related): both sides delete the same node -- a genuine
/// `DeleteMutationConflict`, not an ordinary "one deletes, one is untouched" case.
#[test]
fn delete_mutation_conflict_names_its_kind_and_path() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "doomed.bin", blob(1), MODE_REGULAR);
    let left = [delete_file(
        1,
        "doomed.bin",
        node(1),
        NodeKind::BinaryFile,
        blob(1),
        MODE_REGULAR,
    )];
    let right = [delete_file(
        2,
        "doomed.bin",
        node(1),
        NodeKind::BinaryFile,
        blob(1),
        MODE_REGULAR,
    )];

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Conflict);
    let item = first_item(&report.items);
    assert_eq!(
        item.witness_kind,
        Some(ConflictWitnessKind::DeleteMutationConflict)
    );
    assert_eq!(
        item.path.as_ref().map(|path| path.as_str()),
        Some("doomed.bin")
    );
}

/// Control 2 (conflict-witness-presentation handoff v1 §6.2): every `ConflictWitnessKind` reaches
/// `MergeEvidenceItem::witness_kind` through the shared mapping path, not only the ones this file
/// happens to construct a real classification scenario for.
#[test]
fn every_conflict_witness_kind_reaches_the_report_item() {
    for &kind in ConflictWitnessKind::ALL {
        let witness = ConflictWitness {
            kind,
            left_op_seq: 1,
            right_op_seq: 2,
            node_id: None,
            path: None,
            text_span: None,
        };
        let (_, items) = pair_class_report(
            &PairClass::Conflict { witness },
            EvidenceScope::SealedCandidateRequired,
        );
        assert_eq!(
            first_item(&items).witness_kind,
            Some(kind),
            "kind {kind:?} did not reach the report item"
        );
    }
}

fn first_item(items: &[MergeEvidenceItem]) -> &MergeEvidenceItem {
    items.first().expect("report item")
}
