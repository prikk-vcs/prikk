//! RFC 144 §4r.1 — `check_confluence`'s per-operation deferral no longer pre-empts pairwise
//! classification. Not rename-specific: `ensure_flat_sequence`'s `deferred_reason` check (covering
//! both `RenameDeferred` and `SymlinkDeferred`) now only decides *after* the pairwise loop has had
//! its chance, and only as a fallback for the case that loop cannot resolve on its own (one side's
//! sequence is empty). A genuine replay/evidence/prefix-dependency problem
//! (`FlatSequenceCheck::hard`) still pre-empts everything, unchanged.

use super::*;
use crate::patch_replay::decode::DecodedPatchOperation;

fn first_item(items: &[MergeEvidenceItem]) -> &MergeEvidenceItem {
    items.first().expect("report item")
}

/// Control 1 (the round's whole point): the thirteenth conflict witness reaches
/// `analyze_merge_evidence` itself -- `prikk merge-evidence`'s own real entry point, not
/// `analyze_pair_merge_evidence` (which `0e04164a`'s own control 4 used, and which never went
/// through `ensure_flat_sequence`'s gate at all).
#[test]
fn control1_the_thirteenth_reaches_analyze_merge_evidence() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(
        &mut baseline,
        node(1),
        "shared-source.bin",
        blob(1),
        MODE_REGULAR,
    );
    let left = [rename_path(
        1,
        node(1),
        "shared-source.bin",
        "left-dest.bin",
    )];
    let right = [rename_path(
        2,
        node(1),
        "shared-source.bin",
        "right-dest.bin",
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
    assert_eq!(
        first_item(&report.items).witness_kind,
        Some(ConflictWitnessKind::RenameDestinationConflict)
    );
}

/// Control 2: `same-path-create` for a rename operand -- the other newly-reachable case from
/// `0e04164a` -- also reaches `analyze_merge_evidence`.
#[test]
fn control2_same_path_create_for_a_rename_reaches_analyze_merge_evidence() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "a.bin", blob(1), MODE_REGULAR);
    let left = [rename_path(1, node(1), "a.bin", "occupied.bin")];
    let right = [create_file(
        2,
        "occupied.bin",
        node(2),
        blob(2),
        MODE_REGULAR,
    )];
    let evidence = TestTextResolver::empty().with_blob(blob(2), BlobKind::Binary, b"new".to_vec());

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
        Some(ConflictWitnessKind::SamePathCreate)
    );
}

/// Control 3 (the property that must survive): a sequence with a deferred operation and no
/// pairwise conflict still reports `Unknown` with its original reason. The pairwise loop resolves
/// (or genuinely defers) every pair a deferred operation takes part in -- the *only* way it can
/// escape the loop's own detection is an empty peer sequence, which is exactly what these two
/// cases construct, one for each deferred reason the shared gate covers.
#[test]
fn control3_rename_deferred_survives_an_empty_peer_sequence() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "a.bin", blob(1), MODE_REGULAR);
    let left = [rename_path(1, node(1), "a.bin", "b.bin")];
    let right: [DecodedPatchOperation; 0] = [];

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Unsupported);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::UnsupportedOperation
    );
}

#[test]
fn control3_symlink_deferred_survives_an_empty_peer_sequence() {
    let baseline = NodeLifecycleState::new();
    let left = [create_symlink(1, "link", node(1), "target")];
    let right: [DecodedPatchOperation; 0] = [];

    let report = analyze_merge_evidence(
        blob(0xb0),
        None,
        &baseline,
        &TestTextResolver::empty(),
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );

    assert_eq!(report.outcome, MergeEvidenceOutcome::Unsupported);
    assert_eq!(
        first_item(&report.items).reason_code,
        MergeEvidenceReasonCode::UnsupportedOperation
    );
}

/// Control 4: a regression sweep, consolidated in one place, over kinds real classification can
/// already produce through `analyze_merge_evidence` -- none of them touch `RenamePath`/
/// `CreateSymlink`/`DeleteSymlink`, so `ensure_flat_sequence`'s `deferred` field is `None` on both
/// sides for every one of them, and this round's restructuring never takes a different branch for
/// them at all (`check_confluence`'s own `left_check.deferred`/`right_check.deferred` fallback is
/// simply never reached).
///
/// **How the "before" values were captured**: each assertion below is the exact value the
/// pre-existing, dedicated test for that kind already asserts (named in each case's own comment) --
/// run once against `0e04164a` (this round's own parent commit, before any change in this file) to
/// confirm they held there, then re-run unchanged after this round's restructuring. Both runs: the
/// full existing suite plus this file, 0 failures.
#[test]
fn control4_regression_sweep_other_kinds_unchanged_through_analyze_merge_evidence() {
    // before: same_path_create_conflict_names_its_kind_and_path -> SamePathCreate
    {
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
        assert_eq!(
            first_item(&report.items).witness_kind,
            Some(ConflictWitnessKind::SamePathCreate)
        );
    }

    // before: delete_mutation_conflict_names_its_kind_and_path -> DeleteMutationConflict
    {
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
        assert_eq!(
            first_item(&report.items).witness_kind,
            Some(ConflictWitnessKind::DeleteMutationConflict)
        );
    }

    // before: concrete_conflict_report_is_not_generic_not_confluent -> UnknownRelation
    {
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
            first_item(&report.items).witness_kind,
            Some(ConflictWitnessKind::UnknownRelation)
        );
    }

    // before: same_node_text_transform_is_deferred_not_not_confluent -> Deferred
    {
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

    // before: unsupported_operation_report_does_not_expose_unknown -> Unsupported (RenameDeferred,
    // paired against an unrelated peer -- still Unsupported after this round, same as before it,
    // since the pairwise loop itself resolves it to Unknown{RenameDeferred} the same way).
    {
        let mut baseline = NodeLifecycleState::new();
        seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
        seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
        let left = [rename_path(1, node(1), "left.bin", "moved.bin")];
        let right = [replace_binary(2, node(2), blob(2), blob(3))];
        let evidence =
            TestTextResolver::empty().with_blob(blob(3), BlobKind::Binary, b"new".to_vec());
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
    }

    // before: confluent_report_includes_required_baseline_and_sequence_summaries -> Confluent
    {
        let mut baseline = NodeLifecycleState::new();
        seed_binary(&mut baseline, node(1), "left.bin", blob(1), MODE_REGULAR);
        seed_binary(&mut baseline, node(2), "right.bin", blob(2), MODE_REGULAR);
        let left = [replace_binary(10, node(1), blob(1), blob(3))];
        let right = [change_perm(20, node(2), MODE_REGULAR, MODE_EXECUTABLE)];
        let evidence =
            TestTextResolver::empty().with_blob(blob(3), BlobKind::Binary, b"new".to_vec());
        let report = analyze_merge_evidence(
            blob(0xb0),
            Some(blob(0xa0)),
            &baseline,
            &evidence,
            EvidenceScope::SealedCandidateRequired,
            &left,
            &right,
        );
        assert_eq!(report.outcome, MergeEvidenceOutcome::Confluent);
    }
}
