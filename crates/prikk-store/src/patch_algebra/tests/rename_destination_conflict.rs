//! RFC 144 §4o.5 — the thirteenth conflict witness: both sides rename the *same* node to two
//! *disjoint* destinations. Distinct from `SamePathCreate` (§4i.2's own dual): "one node, two
//! paths -- pick the path" versus `SamePathCreate`'s "two nodes, one path -- pick the node."

use super::*;

/// Control 1: both sides rename one node to disjoint destinations -> the thirteenth fires, with
/// both destinations recoverable independently (`operand_path`'s own new definition for
/// `RenamePath`, exercised here through `derive_path` via the witness's `path`, and separately
/// through each side's own `operation_path` in the merge-evidence report -- see
/// `merge_evidence_report.rs`'s own end-to-end control 4 for that second half).
#[test]
fn control1_disjoint_destinations_fire_the_thirteenth() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "old.bin", blob(1), MODE_REGULAR);
    let left = rename_path(1, node(1), "old.bin", "left-destination.bin");
    let right = rename_path(2, node(1), "old.bin", "right-destination.bin");

    assert_conflict(
        classify_pair(&baseline, &left, &right),
        ConflictWitnessKind::RenameDestinationConflict,
    );
}

/// The witness's own `path` is `None` here, by design -- not an omission. `operand_path`'s
/// `RenamePath` arm reports the *destination* (mirroring `CreateFile`'s "where it is being
/// created"), and control 1's own two destinations are, by definition, different -- so
/// `derive_path` correctly finds no single shared path, the same "neither is *the* path" rule
/// `witness.rs`'s own doc comment already precedents for `NodeIdReuse`. Both destinations stay
/// independently recoverable regardless, via each side's own `operation.path`/`peer_operation.path`
/// in the merge-evidence report (`merge_evidence_report.rs`'s own end-to-end control 4).
#[test]
fn control1_witness_path_is_none_destinations_genuinely_differ() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(
        &mut baseline,
        node(1),
        "shared-source.bin",
        blob(1),
        MODE_REGULAR,
    );
    let left = rename_path(1, node(1), "shared-source.bin", "a.bin");
    let right = rename_path(2, node(1), "shared-source.bin", "b.bin");

    match classify_pair(&baseline, &left, &right) {
        PairClass::Conflict { witness } => {
            assert_eq!(witness.kind, ConflictWitnessKind::RenameDestinationConflict);
            assert_eq!(witness.node_id, Some(node(1)));
            assert_eq!(witness.path, None);
        }
        other => panic!("expected RenameDestinationConflict, got {other:?}"),
    }
}

/// Control 2: a rename onto a path a different node's own operation also newly claims still
/// fires `SamePathCreate`, unchanged -- a regression guard on §4i.2's dual, not a new behaviour.
/// **Finding, not assumed**: before this round a `RenamePath` never reached `classify_path_relation`
/// at all (`facts::deferred_reason` deferred it unconditionally), so this scenario classified as
/// `Unknown { reason: RenameDeferred }`, not `SamePathCreate` -- this round's restructuring is what
/// makes it reachable for the first time. See the round's own report for the verification.
#[test]
fn control2_rename_onto_occupied_path_fires_same_path_create() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "a.bin", blob(1), MODE_REGULAR);
    let left = rename_path(1, node(1), "a.bin", "occupied.bin");
    let right = create_file(2, "occupied.bin", node(2), blob(2), MODE_REGULAR);

    assert_conflict(
        classify_pair(&baseline, &left, &right),
        ConflictWitnessKind::SamePathCreate,
    );
}

/// Control 3a: a one-sided rename (paired with an unrelated operation on a different node, no
/// path overlap) fires nothing -- stays `Unknown { RenameDeferred }`, unchanged from before this
/// round. The thirteenth must not over-fire: a spurious conflict blocks a merge that should
/// proceed.
#[test]
fn control3a_one_sided_rename_fires_nothing() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "a.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "b.bin", blob(2), MODE_REGULAR);
    let left = rename_path(1, node(1), "a.bin", "a-renamed.bin");
    let right = replace_binary(2, node(2), blob(2), blob(3));

    let class = classify_pair(&baseline, &left, &right);
    assert_unknown(class.clone(), UnknownReason::RenameDeferred);
    assert_ne!(
        witness_kind_of(&class),
        Some(ConflictWitnessKind::RenameDestinationConflict)
    );
}

/// Control 3b: two renames of *different* nodes, to disjoint, unrelated destinations, fire
/// nothing -- the thirteenth requires the *same* node on both sides.
#[test]
fn control3b_renames_of_different_nodes_fire_nothing() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "a.bin", blob(1), MODE_REGULAR);
    seed_binary(&mut baseline, node(2), "b.bin", blob(2), MODE_REGULAR);
    let left = rename_path(1, node(1), "a.bin", "a-renamed.bin");
    let right = rename_path(2, node(2), "b.bin", "b-renamed.bin");

    let class = classify_pair(&baseline, &left, &right);
    assert_unknown(class.clone(), UnknownReason::RenameDeferred);
    assert_ne!(
        witness_kind_of(&class),
        Some(ConflictWitnessKind::RenameDestinationConflict)
    );
}

/// §4o.5 §4 REQUIRED: report what the *same*-destination case currently classifies as, and change
/// nothing about it. Both sides rename one node to the identical destination -- `newly_occupied`
/// intersects, so `SamePathCreate` fires, the same as it always has for two operands landing on
/// one path. The resolution "pick one" is still correct even though nothing was created.
#[test]
fn same_destination_case_fires_same_path_create_not_the_thirteenth() {
    let mut baseline = NodeLifecycleState::new();
    seed_binary(&mut baseline, node(1), "old.bin", blob(1), MODE_REGULAR);
    let left = rename_path(1, node(1), "old.bin", "same-destination.bin");
    let right = rename_path(2, node(1), "old.bin", "same-destination.bin");

    assert_conflict(
        classify_pair(&baseline, &left, &right),
        ConflictWitnessKind::SamePathCreate,
    );
}

fn witness_kind_of(class: &PairClass) -> Option<ConflictWitnessKind> {
    match class {
        PairClass::Conflict { witness }
        | PairClass::OrderedDependency { witness, .. }
        | PairClass::Unknown { witness, .. } => Some(witness.kind),
        PairClass::Independent => None,
    }
}
