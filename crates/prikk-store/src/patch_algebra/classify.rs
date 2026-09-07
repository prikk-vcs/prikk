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

    if let Some(reason) =
        deferred_reason(&left_facts.action).or(deferred_reason(&right_facts.action))
    {
        // Prefer the pairwise-shared path (correct when both operands act on the same node --
        // e.g. `RenamePath` paired with a `ChangePerm` on the same node id, where the rename
        // itself carries no path but the shared node's live path does). Only when that is
        // ambiguous or absent -- an unrelated peer, e.g. `CreateSymlink` paired with some other
        // node's `CreateFile` -- fall back to the deferred operand's own path, so an unrelated
        // peer's disagreement never discards a path the deferred operand genuinely has.
        let path = derive_path(baseline, &left_facts, &right_facts).or_else(|| {
            if deferred_reason(&left_facts.action).is_some() {
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

    if let Some(class) = classify_path_relation(baseline, &left_facts, &right_facts) {
        return Ok(class);
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

fn classify_path_relation(
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
