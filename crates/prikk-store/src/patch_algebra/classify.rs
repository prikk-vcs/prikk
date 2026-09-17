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
    // Only symlinks are deferred (merge with renames, design §2.1): a rename is classified and replayed
    // like any other operation.
    if left_deferred.is_none() && right_deferred.is_none() {
        if let Some(class) = classify_path_relation(baseline, &left_facts, &right_facts) {
            return Ok(class);
        }
    }

    if let Some(reason) = left_deferred.or(right_deferred) {
        // Prefer the pairwise-shared path (correct when both operands act on the same node, where
        // the shared node's live path names it). Only when that is
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
/// through `classify_path_relation` like any other operation (merge with renames, design §2.1); this
/// function's own logic and label are unchanged.
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
/// The same-destination case (§4o.5 §4) is deliberately left to `classify_same_path_create`, which runs
/// next and fires `SamePathCreate` for it: identical renames on both sides stay a conflict (merge with
/// renames, ruling 3).
fn classify_rename_destination_conflict(
    baseline: &NodeLifecycleState,
    left: &OperationFacts,
    right: &OperationFacts,
) -> Option<PairClass> {
    let (
        Action::RenamePath {
            node_id: left_node,
            new_path: left_new,
            ..
        },
        Action::RenamePath {
            node_id: right_node,
            new_path: right_new,
            ..
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
    // RFC 144 §4o.5, the thirteenth: first, so a same-node rename to two destinations is named for what
    // it is rather than for the source path both free (merge with renames, design row 15).
    if let Some(class) = classify_rename_destination_conflict(baseline, left, right) {
        return Some(class);
    }
    if let Some(class) = classify_same_path_create(baseline, left, right) {
        return Some(class);
    }

    // Merge with renames, ruling 2: the `freed`/`required_free` checks below assume `DeleteFile`/
    // `CreateFile` preimage semantics (`is_delete_preimage_valid`/`is_create_after_delete_valid` are
    // typed to exactly those), so a pair with a rename skips them. What they would catch -- an
    // operation that cannot replay against the baseline -- the flat-sequence check reports, and every
    // `Independent` verdict on a rename pair is proven by replay (`commutation::prove_pair_replay`).
    let has_rename = |facts: &OperationFacts| matches!(facts.action, Action::RenamePath { .. });
    if has_rename(left) || has_rename(right) {
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
        return None;
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
        // Merge with renames, design §1 and §2.1 item 3: a rename moves the path index and nothing else,
        // and these three address the node by id alone -- they commute by construction. The verdict is
        // still proven by replay (`commutation::prove_pair_replay`), as every `Independent` is.
        (
            Action::RenamePath { .. },
            Action::EditText { .. } | Action::ChangePerm { .. } | Action::ReplaceBinary { .. },
        )
        | (
            Action::EditText { .. } | Action::ChangePerm { .. } | Action::ReplaceBinary { .. },
            Action::RenamePath { .. },
        ) => Ok(PairClass::Independent),
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
