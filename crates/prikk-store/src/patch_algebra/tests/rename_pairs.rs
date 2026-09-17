//! Merge with renames (`rfcs/handoffs/144-two-point-comparison/merge-with-renames-design-handoff-v1.md`,
//! Addendum 1): the design report's pair table, as controls. Each row asserts the classification in both
//! orders (for single-operation sides) and the confluence verdict in both orders.
//!
//! Baseline: node 1 `a.txt` (text), node 2 `c.txt` (text), node 3 `d.bin` (binary), node 4 `b.txt`
//! (text). `R` is node 1 `a.txt → e.txt` unless the row says otherwise.

use super::*;
use crate::patch_replay::decode::DecodedPatchOperation;

const A: &[u8] = b"alpha\nline2\nline3\n";
const C: &[u8] = b"gamma\n";

fn short_class(class: &PairClass) -> String {
    match class {
        PairClass::Independent => "Independent".to_string(),
        PairClass::OrderedDependency {
            required_order,
            witness,
        } => format!("Ordered({required_order:?},{:?})", witness.kind),
        PairClass::Conflict { witness } => format!("Conflict({:?})", witness.kind),
        PairClass::Unknown { reason, .. } => format!("Unknown({reason:?})"),
    }
}

fn short_confluence(result: &Result<ConfluenceResult, EvidenceError>) -> String {
    match result {
        Ok(ConfluenceResult::Confluent { .. }) => "Confluent".to_string(),
        Ok(ConfluenceResult::NotConfluent { witness }) => format!(
            "NotConfluent({:?}{})",
            witness.kind,
            witness
                .pair_class
                .as_ref()
                .map(|class| format!(",{}", short_class(class)))
                .unwrap_or_default()
        ),
        Ok(ConfluenceResult::Unknown { reason }) => format!("Unknown({reason:?})"),
        Err(error) => format!("EvidenceError({error:?})"),
    }
}

fn baseline() -> (NodeLifecycleState, TestTextResolver) {
    let mut state = NodeLifecycleState::new();
    seed_text(&mut state, node(1), "a.txt", A, MODE_REGULAR);
    seed_text(&mut state, node(2), "c.txt", C, MODE_REGULAR);
    seed_binary(&mut state, node(3), "d.bin", blob(3), MODE_REGULAR);
    seed_text(&mut state, node(4), "b.txt", b"bee\n", MODE_REGULAR);
    let evidence = TestTextResolver::new([
        (node(1), A.to_vec()),
        (node(2), C.to_vec()),
        (node(4), b"bee\n".to_vec()),
    ])
    .with_blob(blob(9), BlobKind::Text, b"new file\n".to_vec())
    .with_blob(blob(8), BlobKind::Binary, b"new bin".to_vec());
    (state, evidence)
}

/// Assert one row. `class` is `None` when a side is not a single operation.
fn row(
    left: &[DecodedPatchOperation],
    right: &[DecodedPatchOperation],
    class: Option<&str>,
    confluence: &str,
) {
    let (state, evidence) = baseline();
    if let ([l], [r], Some(expected)) = (left, right, class) {
        for (first, second) in [(l, r), (r, l)] {
            let actual = classify_pair_with_text_resolver_result(&state, &evidence, first, second)
                .map(|class| short_class(&class))
                .unwrap_or_else(|error| format!("EvidenceError({error:?})"));
            assert_eq!(
                actual, expected,
                "classification of {first:?} against {second:?}"
            );
        }
    }
    for (first, second) in [(left, right), (right, left)] {
        let actual = short_confluence(&check_confluence_result(
            &state,
            &evidence,
            EvidenceScope::SealedCandidateRequired,
            first,
            second,
        ));
        assert_eq!(
            actual, confluence,
            "confluence of {first:?} against {second:?}"
        );
    }
}

fn rename() -> DecodedPatchOperation {
    rename_path(1, node(1), "a.txt", "e.txt")
}

fn in_patch(mut operation: DecodedPatchOperation, patch: u8) -> DecodedPatchOperation {
    operation.patch_id = Some(blob(patch));
    operation
}

fn text_id(text: &[u8]) -> ObjectId {
    crate::text_span::text_blob_id(text).expect("text blob id")
}

#[test]
fn row01_rename_against_nothing_is_confluent_s2() {
    row(&[rename()], &[], None, "Confluent");
}

#[test]
fn row02_rename_against_a_create_elsewhere_is_independent() {
    row(
        &[rename()],
        &[create_file(1, "f.txt", node(5), blob(9), MODE_REGULAR)],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row03_rename_against_a_create_at_its_destination_conflicts() {
    row(
        &[rename()],
        &[create_file(1, "e.txt", node(5), blob(9), MODE_REGULAR)],
        Some("Conflict(SamePathCreate)"),
        "NotConfluent(Conflict,Conflict(SamePathCreate))",
    );
}

#[test]
fn row04_a_create_at_the_renamed_source_is_unauthorable() {
    row(
        &[rename()],
        &[create_file(1, "a.txt", node(5), blob(9), MODE_REGULAR)],
        Some("Conflict(SamePathCreate)"),
        "NotConfluent(ReplayFailure)",
    );
}

#[test]
fn row05_rename_against_delete_of_the_same_node_is_delete_mutation_conflict() {
    row(
        &[rename()],
        &[delete_file(
            1,
            "a.txt",
            node(1),
            NodeKind::TextFile,
            text_id(A),
            MODE_REGULAR,
        )],
        Some("Conflict(DeleteMutationConflict)"),
        "NotConfluent(Conflict,Conflict(DeleteMutationConflict))",
    );
}

#[test]
fn row06_rename_against_delete_of_another_node_is_independent() {
    row(
        &[rename()],
        &[delete_file(
            1,
            "c.txt",
            node(2),
            NodeKind::TextFile,
            text_id(C),
            MODE_REGULAR,
        )],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row07_a_rename_onto_a_path_the_other_side_deletes_is_unauthorable() {
    row(
        &[rename_path(1, node(1), "a.txt", "c.txt")],
        &[delete_file(
            1,
            "c.txt",
            node(2),
            NodeKind::TextFile,
            text_id(C),
            MODE_REGULAR,
        )],
        Some("Independent"),
        "NotConfluent(ReplayFailure)",
    );
}

#[test]
fn row08_rename_and_edit_of_the_same_node_commute_s3() {
    row(
        &[rename()],
        &[edit_text(1, node(1), A, b"alpha-main\nline2\nline3\n")],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row09_rename_and_edit_of_another_node_commute_s1() {
    row(
        &[rename()],
        &[edit_text(1, node(2), C, b"gamma2\n")],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row10_rename_and_binary_replace_of_the_same_node_commute() {
    row(
        &[rename_path(1, node(3), "d.bin", "g.bin")],
        &[replace_binary(1, node(3), blob(3), blob(8))],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row11_rename_and_binary_replace_of_another_node_commute() {
    row(
        &[rename()],
        &[replace_binary(1, node(3), blob(3), blob(8))],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row12_rename_and_mode_change_of_the_same_node_commute() {
    row(
        &[rename()],
        &[change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE)],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row13_rename_and_mode_change_of_another_node_commute() {
    row(
        &[rename()],
        &[change_perm(1, node(2), MODE_REGULAR, MODE_EXECUTABLE)],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row14_identical_renames_on_both_sides_stay_a_conflict() {
    row(
        &[rename()],
        &[rename()],
        Some("Conflict(SamePathCreate)"),
        "NotConfluent(Conflict,Conflict(SamePathCreate))",
    );
}

#[test]
fn row15_one_node_renamed_to_two_paths_is_the_thirteenth() {
    row(
        &[rename()],
        &[rename_path(1, node(1), "a.txt", "h.txt")],
        Some("Conflict(RenameDestinationConflict)"),
        "NotConfluent(Conflict,Conflict(RenameDestinationConflict))",
    );
}

#[test]
fn row16_two_nodes_renamed_to_one_path_conflict() {
    row(
        &[rename()],
        &[rename_path(1, node(2), "c.txt", "e.txt")],
        Some("Conflict(SamePathCreate)"),
        "NotConfluent(Conflict,Conflict(SamePathCreate))",
    );
}

#[test]
fn row17_two_nodes_renamed_to_two_paths_commute() {
    row(
        &[rename()],
        &[rename_path(1, node(2), "c.txt", "i.txt")],
        Some("Independent"),
        "Confluent",
    );
}

#[test]
fn row18_a_swap_across_sides_is_unauthorable() {
    row(
        &[rename_path(1, node(1), "a.txt", "c.txt")],
        &[rename_path(1, node(2), "c.txt", "a.txt")],
        Some("Independent"),
        "NotConfluent(ReplayFailure)",
    );
}

#[test]
fn row19_a_swap_within_one_patch_is_confluent() {
    row(
        &[
            in_patch(rename_path(1, node(1), "a.txt", "c.txt"), 7),
            in_patch(rename_path(2, node(2), "c.txt", "a.txt"), 7),
        ],
        &[],
        None,
        "Confluent",
    );
}

#[test]
fn row19b_a_swap_split_across_two_patches_is_refused() {
    row(
        &[
            in_patch(rename_path(1, node(1), "a.txt", "c.txt"), 7),
            in_patch(rename_path(1, node(2), "c.txt", "a.txt"), 8),
        ],
        &[],
        None,
        "NotConfluent(ReplayFailure)",
    );
}

#[test]
fn row19c_a_swap_with_no_declared_patch_is_refused() {
    row(
        &[
            rename_path(1, node(1), "a.txt", "c.txt"),
            rename_path(2, node(2), "c.txt", "a.txt"),
        ],
        &[],
        None,
        "NotConfluent(ReplayFailure)",
    );
}

#[test]
fn row20_a_chain_across_sides_is_unauthorable() {
    row(
        &[rename_path(1, node(1), "a.txt", "b.txt")],
        &[rename_path(1, node(4), "b.txt", "j.txt")],
        Some("Independent"),
        "NotConfluent(ReplayFailure)",
    );
}

#[test]
fn row21_rename_against_a_symlink_create_stays_deferred() {
    row(
        &[rename()],
        &[create_symlink(1, "l.lnk", node(6), "c.txt")],
        Some("Unknown(SymlinkDeferred)"),
        "Unknown(SymlinkDeferred)",
    );
}

#[test]
fn row22_rename_then_edit_on_one_side_is_confluent() {
    row(
        &[
            rename(),
            edit_text(2, node(1), A, b"alpha\nline2\nline3-main\n"),
        ],
        &[],
        None,
        "Confluent",
    );
}

#[test]
fn row23_rename_then_edit_against_the_same_rename_stays_a_conflict() {
    row(
        &[
            rename(),
            edit_text(2, node(1), A, b"alpha\nline2\nline3-main\n"),
        ],
        &[rename()],
        None,
        "NotConfluent(Conflict,Conflict(SamePathCreate))",
    );
}

/// Ruling 2: every `Independent` verdict on a pair with a rename is proven by replay, with no path around
/// it. A cross-side chain classifies `Independent` but does not replay the same in both orders, so
/// `commute_pair` must answer `Unknown`, never `Commutes`.
#[test]
fn an_independent_rename_pair_is_proven_by_replay() {
    let (state, evidence) = baseline();
    let left = rename_path(1, node(1), "a.txt", "b.txt");
    let right = rename_path(1, node(4), "b.txt", "j.txt");
    assert_eq!(
        classify_pair_with_text_resolver_result(&state, &evidence, &left, &right)
            .map(|class| short_class(&class))
            .ok(),
        Some("Independent".to_string())
    );
    let result = commute_pair_result(
        &state,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    );
    assert!(
        matches!(
            result,
            Ok(CommutationResult::Unknown {
                reason: UnknownReason::UnknownRelation
            })
        ),
        "{result:?}"
    );
}
