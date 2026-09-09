use prikk_object::{NodeId, NodeKind};

use super::create::classify_create_then_mutate;
use super::delete::classify_mutate_then_delete;
#[cfg(test)]
use super::evidence_types::NoPatchAlgebraEvidence;
use super::evidence_types::{ClassificationResult, PatchAlgebraEvidence};
use super::facts::{deferred_reason, operation_facts};
use super::preimage::{
    baseline_file_matches, invalid_preimage_class, is_create_after_delete_valid,
    is_delete_preimage_valid,
};
use super::text_pair::classify_mode_and_text_edit;
use super::types::{
    Action, ConflictWitnessKind, OperationFacts, PairClass, RequiredOrder, UnknownReason,
};
use super::witness::{
    common_node, conflict, conflict_with_span, derive_path, operand_path, ordered, unknown,
    unknown_from_facts,
};
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::patch_replay::decode::DecodedPatchOperation;

#[cfg(test)]
pub(crate) fn classify_pair(
    baseline: &NodeLifecycleState,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> ClassificationResult {
    classify_pair_with_text_resolver(baseline, &NoPatchAlgebraEvidence, left, right)
}

pub(crate) fn classify_pair_with_text_resolver<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    text_resolver: &R,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> ClassificationResult {
    let left_facts = match operation_facts(left) {
        Ok(facts) => facts,
        Err(reason) => return Ok(unknown(reason, left, right, None, None)),
    };
    let right_facts = match operation_facts(right) {
        Ok(facts) => facts,
        Err(reason) => return Ok(unknown(reason, left, right, None, None)),
    };

    let left_deferred = deferred_reason(&left_facts.action);
    let right_deferred = deferred_reason(&right_facts.action);
    let symlink_deferred = matches!(left_deferred, Some(UnknownReason::SymlinkDeferred))
        || matches!(right_deferred, Some(UnknownReason::SymlinkDeferred));

    if left_deferred.is_none() && right_deferred.is_none() {
        // Neither operand is deferred -- unchanged from before this round.
        if let Some(class) = classify_path_relation(baseline, &left_facts, &right_facts) {
            return Ok(class);
        }
    } else if !symlink_deferred {
        // RFC 144 §4o.5: a `RenamePath`'s own destination is now recoverable (`Action::RenamePath`
        // carries it), so a pair deferred only for `RenameDeferred` -- never `SymlinkDeferred`,
        // which keeps deferring immediately below exactly as before this round -- gets a chance at
        // two path-relation-based resolutions before giving up: the thirteenth conflict witness
        // (same node, disjoint destinations), and `SamePathCreate`'s own existing dual (a rename
        // destination colliding with a path someone else already claims). Deliberately **not**
        // the other two `classify_path_relation` checks (the `freed`/`required_free`
        // intersections gating `LiveStateMismatch`/ordering): those assume `DeleteFile`/
        // `CreateFile` preimage semantics (`is_delete_preimage_valid`/`is_create_after_delete_valid`
        // are typed to exactly those two actions) that do not describe what a `RenamePath`'s own
        // preimage even means, and reaching them here risks a confidently wrong conflict for a
        // cross-node rename interaction this round is not scoped to resolve (§6: "do not add a
        // resolution mechanism").
        if let Some(class) =
            classify_rename_destination_conflict(baseline, &left_facts, &right_facts)
        {
            return Ok(class);
        }
        if let Some(class) = classify_same_path_create(baseline, &left_facts, &right_facts) {
            return Ok(class);
        }
    }

    if let Some(reason) = left_deferred.or(right_deferred) {
        // Prefer the pairwise-shared path (correct when both operands act on the same node --
        // e.g. `RenamePath` paired with a `ChangePerm` on the same node id, where the rename
        // itself carries no path but the shared node's live path does). Only when that is
        // ambiguous or absent -- an unrelated peer, e.g. `CreateSymlink` paired with some other
        // node's `CreateFile` -- fall back to the deferred operand's own path, so an unrelated
        // peer's disagreement never discards a path the deferred operand genuinely has.
        let path = derive_path(baseline, &left_facts, &right_facts).or_else(|| {
            if left_deferred.is_some() {
                operand_path(baseline, &left_facts)
            } else {
                operand_path(baseline, &right_facts)
            }
        });
        return Ok(unknown(
            reason,
            left,
            right,
            common_node(&left_facts, &right_facts),
            path,
        ));
    }

    match (left_facts.node_id, right_facts.node_id) {
        (Some(left_node), Some(right_node)) if left_node == right_node => classify_same_node(
            baseline,
            text_resolver,
            &left_facts,
            &right_facts,
            left_node,
        ),
        _ => classify_cross_node(baseline, text_resolver, &left_facts, &right_facts),
    }
}

/// RFC 144 §4o.5, §4i.2's dual: "two nodes, one path -- pick the node." Fires whenever the two
/// operands' own destinations collide, regardless of which operation kinds produced them --
/// `CreateFile`/`CreateSymlink` land here the same as it always has, and a `RenamePath` reaches it
/// for the first time now that its destination is recoverable (`classify_pair_with_text_resolver`'s
/// own rename-only-deferred branch is what makes that reachable; this function's own logic and
/// label are unchanged from before this round).
fn classify_same_path_create(
    baseline: &NodeLifecycleState,
    left: &OperationFacts,
    right: &OperationFacts,
) -> Option<PairClass> {
    if !left
        .path_effects
        .newly_occupied
        .is_disjoint(&right.path_effects.newly_occupied)
    {
        return Some(conflict(
            ConflictWitnessKind::SamePathCreate,
            left,
            right,
            common_node(left, right),
            derive_path(baseline, left, right),
        ));
    }
    None
}

/// RFC 144 §4o.5 (the thirteenth), §4i.2's dual: "one node, two paths -- pick the path." Fires
/// only when both operands are a `RenamePath` for the *same* node with *different* destinations.
/// The same-destination case (§4o.5 §4, a third shape §4i.2 does not rule on) is deliberately left
/// unresolved here -- returning `None` lets it fall through to `classify_same_path_create` above,
/// whose `newly_occupied` intersection already fires `SamePathCreate` for it, unchanged.
fn classify_rename_destination_conflict(
    baseline: &NodeLifecycleState,
    left: &OperationFacts,
    right: &OperationFacts,
) -> Option<PairClass> {
    let (
        Action::RenamePath {
            node_id: left_node,
            new_path: left_new,
        },
        Action::RenamePath {
            node_id: right_node,
            new_path: right_new,
        },
    ) = (&left.action, &right.action)
    else {
        return None;
    };
    if left_node != right_node || left_new == right_new {
        return None;
    }
    Some(conflict(
        ConflictWitnessKind::RenameDestinationConflict,
        left,
        right,
        Some(*left_node),
        derive_path(baseline, left, right),
    ))
}

fn classify_path_relation(
    baseline: &NodeLifecycleState,
    left: &OperationFacts,
    right: &OperationFacts,
) -> Option<PairClass> {
    if let Some(class) = classify_same_path_create(baseline, left, right) {
        return Some(class);
    }

    if !left
        .path_effects
        .freed
        .is_disjoint(&right.path_effects.required_free)
    {
        if !is_delete_preimage_valid(baseline, left)
            || !is_create_after_delete_valid(baseline, left, right)
        {
            return Some(conflict(
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                common_node(left, right),
                derive_path(baseline, left, right),
            ));
        }
        return Some(ordered(
            RequiredOrder::LeftBeforeRight,
            ConflictWitnessKind::LiveStateMismatch,
            left,
            right,
            common_node(left, right),
            derive_path(baseline, left, right),
        ));
    }

    if !right
        .path_effects
        .freed
        .is_disjoint(&left.path_effects.required_free)
    {
        if !is_delete_preimage_valid(baseline, right)
            || !is_create_after_delete_valid(baseline, right, left)
        {
            return Some(conflict(
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                common_node(left, right),
                derive_path(baseline, left, right),
            ));
        }
        return Some(ordered(
            RequiredOrder::RightBeforeLeft,
            ConflictWitnessKind::LiveStateMismatch,
            left,
            right,
            common_node(left, right),
            derive_path(baseline, left, right),
        ));
    }

    if !left
        .path_effects
        .freed
        .is_disjoint(&right.path_effects.freed)
    {
        return Some(conflict(
            ConflictWitnessKind::DeleteMutationConflict,
            left,
            right,
            common_node(left, right),
            derive_path(baseline, left, right),
        ));
    }

    None
}

fn classify_cross_node<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    text_resolver: &R,
    left: &OperationFacts,
    right: &OperationFacts,
) -> ClassificationResult {
    if let Some(class) = invalid_preimage_class(baseline, text_resolver, left, right)? {
        return Ok(class);
    }
    if let Some(class) = invalid_preimage_class(baseline, text_resolver, right, left)? {
        return Ok(class);
    }
    Ok(PairClass::Independent)
}

fn classify_same_node<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    text_resolver: &R,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: NodeId,
) -> ClassificationResult {
    if let Some(class) = classify_create_then_mutate(baseline, text_resolver, left, right, node_id)
    {
        return class;
    }
    if let Some(class) = classify_mutate_then_delete(baseline, left, right, node_id) {
        return Ok(class);
    }
    match (&left.action, &right.action) {
        (
            Action::EditText {
                span_id: left_span, ..
            },
            Action::EditText {
                span_id: right_span,
                ..
            },
        ) if left_span == right_span => Ok(conflict_with_span(
            ConflictWitnessKind::TextSpanOverlap,
            left,
            right,
            node_id,
            derive_path(baseline, left, right),
            *left_span,
        )),
        (Action::EditText { .. }, Action::EditText { .. }) => Ok(unknown_from_facts(
            UnknownReason::SameNodeTextCommutationDeferred,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        )),
        (Action::ChangePerm { old_mode, .. }, Action::ReplaceBinary { old_blob_id, .. })
        | (Action::ReplaceBinary { old_blob_id, .. }, Action::ChangePerm { old_mode, .. }) => {
            if baseline_file_matches(
                baseline,
                node_id,
                NodeKind::BinaryFile,
                *old_blob_id,
                *old_mode,
            ) {
                Ok(PairClass::Independent)
            } else {
                Ok(conflict(
                    ConflictWitnessKind::LiveStateMismatch,
                    left,
                    right,
                    Some(node_id),
                    derive_path(baseline, left, right),
                ))
            }
        }
        (Action::ChangePerm { old_mode, .. }, edit @ Action::EditText { .. })
        | (edit @ Action::EditText { .. }, Action::ChangePerm { old_mode, .. }) => {
            classify_mode_and_text_edit(
                baseline,
                text_resolver,
                left,
                right,
                node_id,
                *old_mode,
                edit,
            )
        }
        _ => Ok(conflict(
            ConflictWitnessKind::UnknownRelation,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        )),
    }
}
