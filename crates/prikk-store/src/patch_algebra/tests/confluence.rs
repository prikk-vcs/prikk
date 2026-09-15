use super::*;

#[test]
fn flat_sequences_with_only_commuting_cross_pairs_are_confluent() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    let left = [replace_binary(1, node(1), blob(1), blob(3))];
    let right = [change_perm(2, node(2), MODE_REGULAR, MODE_EXECUTABLE)];
    let evidence = TestTextResolver::empty().with_blob(blob(3), BlobKind::Binary, b"new".to_vec());

    match check_confluence_result(
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    )
    .expect("confluence evidence")
    {
        ConfluenceResult::Confluent { proof } => {
            assert_eq!(proof.left_len, 1);
            assert_eq!(proof.right_len, 1);
        }
        other => panic!("expected confluent, got {other:?}"),
    }
}

#[test]
fn cross_sequence_baseline_invalid_dependency_is_not_confluent() {
    let baseline = NodeLifecycleState::new();
    let left = [create_file(1, "fresh.bin", node(1), blob(1), MODE_REGULAR)];
    let right = [change_perm(2, node(1), MODE_REGULAR, MODE_EXECUTABLE)];
    let evidence =
        TestTextResolver::empty().with_blob(blob(1), BlobKind::Binary, b"fresh".to_vec());

    match check_confluence_result(
        &baseline,
        &evidence,
        EvidenceScope::UnsealedCandidateOptional,
        &left,
        &right,
    )
    .expect("confluence evidence")
    {
        ConfluenceResult::NotConfluent { witness } => {
            assert_eq!(witness.kind, ConfluenceWitnessKind::ReplayFailure);
            // The operation that does not replay is `right[0]`, so the witness names it as a right
            // index (DC-75 two-edits handoff §6, R1 condition 6; it used to be named as `left_index`).
            assert_eq!(witness.left_index, None);
            assert_eq!(witness.right_index, Some(0));
        }
        other => panic!("expected ordered not-confluent, got {other:?}"),
    }
}

#[test]
fn concrete_cross_sequence_conflict_is_not_confluent() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "tool", blob(1), MODE_REGULAR);
    let left = [change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE)];
    let right = [change_perm(2, node(1), MODE_REGULAR, 0o100600)];

    match check_confluence_result(
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    )
    .expect("confluence evidence")
    {
        ConfluenceResult::NotConfluent { witness } => {
            assert_eq!(witness.kind, ConfluenceWitnessKind::Conflict);
            assert_eq!(witness.left_index, Some(0));
            assert_eq!(witness.right_index, Some(0));
        }
        other => panic!("expected conflict not-confluent, got {other:?}"),
    }
}

#[test]
fn sequence_internal_dependency_is_deferred_before_cross_pair_checks() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(9), "other.bin", blob(9), MODE_REGULAR);
    let left = [
        create_file(1, "fresh.bin", node(1), blob(1), MODE_REGULAR),
        change_perm(2, node(1), MODE_REGULAR, MODE_EXECUTABLE),
    ];
    let right = [replace_binary(3, node(9), blob(9), blob(10))];
    let evidence =
        TestTextResolver::empty().with_blob(blob(1), BlobKind::Binary, b"fresh".to_vec());

    match check_confluence_result(
        &baseline,
        &evidence,
        EvidenceScope::UnsealedCandidateOptional,
        &left,
        &right,
    )
    .expect("confluence evidence")
    {
        ConfluenceResult::Unknown { reason } => {
            assert_eq!(reason, UnknownReason::SequenceInternalDependencyDeferred);
        }
        other => panic!("expected internal dependency deferral, got {other:?}"),
    }
}

#[test]
fn unsupported_cross_sequence_relation_is_unknown() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    let left = [rename_path(1, node(1), "left.bin", "moved.bin")];
    let right = [replace_binary(2, node(2), blob(2), blob(3))];
    let evidence = TestTextResolver::empty().with_blob(blob(3), BlobKind::Binary, b"new".to_vec());

    match check_confluence_result(
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    )
    .expect("confluence evidence")
    {
        ConfluenceResult::Unknown { reason } => assert_eq!(reason, UnknownReason::RenameDeferred),
        other => panic!("expected rename deferral, got {other:?}"),
    }
}

#[test]
fn sealed_candidate_evidence_error_wins_over_earlier_unknown_sequence() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    let left = [rename_path(1, node(1), "left.bin", "moved.bin")];
    let right = [replace_binary(2, node(2), blob(2), blob(3))];

    match check_confluence_result(
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    ) {
        Err(EvidenceError::Missing {
            scope,
            fact,
            object_id,
            ..
        }) => {
            assert_eq!(scope, EvidenceScope::SealedCandidateRequired);
            assert_eq!(fact, EvidenceFact::BlobKind);
            assert_eq!(object_id, Some(blob(3)));
        }
        other => panic!("expected sealed replacement evidence error, got {other:?}"),
    }
}

#[test]
fn same_sequence_evidence_error_wins_over_earlier_unknown_operation() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    seed_binary(&mut baseline, node(4), "valid.bin", blob(4), MODE_REGULAR);
    let left = [
        rename_path(1, node(1), "left.bin", "moved.bin"),
        replace_binary(2, node(2), blob(2), blob(3)),
    ];
    let right = [change_perm(3, node(4), MODE_REGULAR, MODE_EXECUTABLE)];

    match check_confluence_result(
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    ) {
        Err(EvidenceError::Missing {
            scope,
            fact,
            object_id,
            ..
        }) => {
            assert_eq!(scope, EvidenceScope::SealedCandidateRequired);
            assert_eq!(fact, EvidenceFact::BlobKind);
            assert_eq!(object_id, Some(blob(3)));
        }
        other => panic!("expected same-sequence sealed evidence error, got {other:?}"),
    }
}

#[test]
fn right_same_sequence_evidence_error_wins_over_earlier_unknown_operation() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
    seed_binary(&mut baseline, node(4), "valid.bin", blob(4), MODE_REGULAR);
    let left = [change_perm(3, node(4), MODE_REGULAR, MODE_EXECUTABLE)];
    let right = [
        rename_path(1, node(1), "left.bin", "moved.bin"),
        replace_binary(2, node(2), blob(2), blob(3)),
    ];

    match check_confluence_result(
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    ) {
        Err(EvidenceError::Missing {
            scope,
            fact: EvidenceFact::BlobKind,
            object_id,
            ..
        }) => {
            assert_eq!(scope, EvidenceScope::SealedCandidateRequired);
            assert_eq!(object_id, Some(blob(3)));
        }
        other => panic!("expected right same-sequence sealed evidence error, got {other:?}"),
    }
}

/// DC-75 two-edits handoff: the defect's smallest reproduction. Before per-side folding it failed with
/// `ReplayFailure` at left index 1, because `ensure_flat_sequence` replayed the second edit alone
/// against the baseline. This drives `check_confluence` over folded sequences, as the report does.
#[test]
fn a_side_that_edits_one_text_twice_is_confluent_with_an_unrelated_create() {
    let mut baseline = NodeLifecycleState::new();
    seed_text(
        &mut baseline,
        node(1),
        "e.txt",
        b"alpha beta gamma",
        MODE_REGULAR,
    );
    let left = [
        edit_text(1, node(1), b"alpha beta gamma", b"alpha BETA gamma"),
        edit_text(2, node(1), b"alpha BETA gamma", b"alpha BETA GAMMA"),
    ];
    let right = [create_file(3, "g.bin", node(2), blob(2), MODE_REGULAR)];
    let evidence = TestTextResolver::new([(node(1), b"alpha beta gamma".to_vec())]).with_blob(
        blob(2),
        BlobKind::Binary,
        b"g".to_vec(),
    );

    let scope = EvidenceScope::SealedCandidateRequired;
    let left = fold_side(&baseline, &evidence, scope, &left).operations;
    let right = fold_side(&baseline, &evidence, scope, &right).operations;
    match check_confluence_result(&baseline, &evidence, scope, &left, &right)
        .expect("confluence evidence")
    {
        ConfluenceResult::Confluent { proof } => {
            assert_eq!(
                proof.left_len, 1,
                "the two edits are judged as one net edit"
            );
            assert_eq!(proof.right_len, 1);
        }
        other => panic!("expected confluent, got {other:?}"),
    }
}
