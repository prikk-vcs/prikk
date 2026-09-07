use prikk_object::NodeId;

use crate::node::node_lifecycle::NodeLifecycleState;

use super::preimage::baseline_file_matches;
use super::types::{Action, ConflictWitnessKind, OperationFacts, PairClass, RequiredOrder};
use super::witness::{conflict, derive_path, ordered};

pub(super) fn classify_mutate_then_delete(
    baseline: &NodeLifecycleState,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: NodeId,
) -> Option<PairClass> {
    match (&left.action, &right.action) {
        (
            Action::ChangePerm {
                old_mode, new_mode, ..
            },
            Action::DeleteFile {
                old_blob_id,
                old_mode: delete_mode,
                old_node_kind,
                ..
            },
        ) if *new_mode == *delete_mode
            && baseline_file_matches(
                baseline,
                node_id,
                *old_node_kind,
                *old_blob_id,
                *old_mode,
            ) =>
        {
            Some(ordered(
                RequiredOrder::LeftBeforeRight,
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                Some(node_id),
                derive_path(baseline, left, right),
            ))
        }
        (
            Action::DeleteFile {
                old_blob_id,
                old_mode: delete_mode,
                old_node_kind,
                ..
            },
            Action::ChangePerm {
                old_mode, new_mode, ..
            },
        ) if *new_mode == *delete_mode
            && baseline_file_matches(
                baseline,
                node_id,
                *old_node_kind,
                *old_blob_id,
                *old_mode,
            ) =>
        {
            Some(ordered(
                RequiredOrder::RightBeforeLeft,
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                Some(node_id),
                derive_path(baseline, left, right),
            ))
        }
        (
            Action::ReplaceBinary {
                old_blob_id,
                new_blob_id,
                ..
            },
            Action::DeleteFile {
                old_blob_id: delete_blob,
                old_mode,
                old_node_kind,
                ..
            },
        ) if *new_blob_id == *delete_blob
            && baseline_file_matches(
                baseline,
                node_id,
                *old_node_kind,
                *old_blob_id,
                *old_mode,
            ) =>
        {
            Some(ordered(
                RequiredOrder::LeftBeforeRight,
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                Some(node_id),
                derive_path(baseline, left, right),
            ))
        }
        (
            Action::DeleteFile {
                old_blob_id: delete_blob,
                old_mode,
                old_node_kind,
                ..
            },
            Action::ReplaceBinary {
                old_blob_id,
                new_blob_id,
                ..
            },
        ) if *new_blob_id == *delete_blob
            && baseline_file_matches(
                baseline,
                node_id,
                *old_node_kind,
                *old_blob_id,
                *old_mode,
            ) =>
        {
            Some(ordered(
                RequiredOrder::RightBeforeLeft,
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                Some(node_id),
                derive_path(baseline, left, right),
            ))
        }
        (Action::DeleteFile { .. }, Action::ChangePerm { .. } | Action::ReplaceBinary { .. })
        | (Action::ChangePerm { .. } | Action::ReplaceBinary { .. }, Action::DeleteFile { .. }) => {
            Some(conflict(
                ConflictWitnessKind::DeleteMutationConflict,
                left,
                right,
                Some(node_id),
                derive_path(baseline, left, right),
            ))
        }
        _ => None,
    }
}
