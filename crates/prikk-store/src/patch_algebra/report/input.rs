use prikk_object::NodeId;

use super::types::{
    MergeEvidenceItem, MergeEvidenceOperation, MergeEvidenceOperationKind, MergeEvidenceOutcome,
    MergeEvidenceProofPhase, MergeEvidenceReasonCode, MergeEvidenceScope, MergeEvidenceSequence,
    MergeEvidenceSide,
};
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::patch_algebra::evidence_types::{EvidenceError, EvidenceScope, PatchAlgebraEvidence};
use crate::patch_algebra::facts::operation_facts;
use crate::patch_algebra::replay_oracle::{OracleFailure, replay_operations};
use crate::patch_algebra::types::UnknownReason;
use crate::patch_replay::decode::{DecodedOperationKind, DecodedPatchOperation};
use crate::path::RepoPath;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ItemSortKey {
    side: MergeEvidenceSide,
    operation_index: Option<usize>,
    peer_operation_index: Option<usize>,
    op_seq: Option<u32>,
    peer_op_seq: Option<u32>,
    outcome_rank: u8,
    reason_code: MergeEvidenceReasonCode,
    evidence_scope: Option<MergeEvidenceScope>,
    proof_phase: MergeEvidenceProofPhase,
    operation_kind: Option<MergeEvidenceOperationKind>,
    node_id: Option<NodeId>,
    path: Option<RepoPath>,
}

pub(super) fn sequence_summary(
    label: &'static str,
    operations: &[DecodedPatchOperation],
) -> MergeEvidenceSequence {
    MergeEvidenceSequence {
        label,
        operation_count: operations.len(),
        operations: operations
            .iter()
            .enumerate()
            .map(|(operation_index, operation)| operation_summary(operation_index, operation))
            .collect(),
    }
}

pub(super) fn malformed_unsealed_item(
    candidate_scope: EvidenceScope,
    left: &[DecodedPatchOperation],
    right: &[DecodedPatchOperation],
) -> Option<MergeEvidenceItem> {
    if candidate_scope != EvidenceScope::UnsealedCandidateOptional {
        return None;
    }
    first_malformed_operation(MergeEvidenceSide::Left, left)
        .or_else(|| first_malformed_operation(MergeEvidenceSide::Right, right))
}

pub(super) fn first_required_evidence_error<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    left: &[DecodedPatchOperation],
    right: &[DecodedPatchOperation],
) -> Option<EvidenceError> {
    left.iter().chain(right.iter()).find_map(|operation| {
        match replay_operations(baseline, evidence, candidate_scope, [operation]) {
            Err(OracleFailure::Evidence(error)) if is_required_evidence_error(&error) => {
                Some(error)
            }
            Err(OracleFailure::Evidence(_))
            | Err(OracleFailure::Unknown(_))
            | Err(OracleFailure::Replay)
            | Ok(_) => None,
        }
    })
}

pub(crate) fn sort_report_items(items: &mut [MergeEvidenceItem]) {
    items.sort_by_key(item_sort_key);
}

fn operation_summary(
    operation_index: usize,
    operation: &DecodedPatchOperation,
) -> MergeEvidenceOperation {
    let facts = operation_facts(operation).ok();
    MergeEvidenceOperation {
        operation_index,
        op_seq: operation.op_seq,
        operation_kind: operation_kind(operation),
        node_id: facts.as_ref().and_then(|facts| facts.node_id),
        path: operation_path(operation),
    }
}

fn operation_kind(operation: &DecodedPatchOperation) -> MergeEvidenceOperationKind {
    match operation.kind {
        DecodedOperationKind::CreateFile { .. } => MergeEvidenceOperationKind::CreateFile,
        DecodedOperationKind::DeleteNode { .. } => MergeEvidenceOperationKind::DeleteNode,
        DecodedOperationKind::EditText { .. } => MergeEvidenceOperationKind::EditText,
        DecodedOperationKind::ReplaceBinary { .. } => MergeEvidenceOperationKind::ReplaceBinary,
        DecodedOperationKind::RenamePath { .. } => MergeEvidenceOperationKind::RenamePath,
        DecodedOperationKind::ChangePerm { .. } => MergeEvidenceOperationKind::ChangePerm,
        DecodedOperationKind::CreateSymlink { .. } => MergeEvidenceOperationKind::CreateSymlink,
    }
}

fn operation_path(operation: &DecodedPatchOperation) -> Option<RepoPath> {
    match &operation.kind {
        DecodedOperationKind::CreateFile { path, .. }
        | DecodedOperationKind::DeleteNode { path, .. }
        | DecodedOperationKind::CreateSymlink { path, .. } => RepoPath::parse(path).ok(),
        DecodedOperationKind::RenamePath { old_path, .. } => RepoPath::parse(old_path).ok(),
        DecodedOperationKind::EditText { .. }
        | DecodedOperationKind::ReplaceBinary { .. }
        | DecodedOperationKind::ChangePerm { .. } => None,
    }
}

fn first_malformed_operation(
    side: MergeEvidenceSide,
    operations: &[DecodedPatchOperation],
) -> Option<MergeEvidenceItem> {
    operations
        .iter()
        .enumerate()
        .find(|(_, operation)| {
            matches!(
                operation_facts(operation),
                Err(UnknownReason::MalformedOperation)
            )
        })
        .map(|(index, operation)| MergeEvidenceItem {
            side,
            operation_index: Some(index),
            peer_operation_index: None,
            op_seq: Some(operation.op_seq),
            peer_op_seq: None,
            operation_kind: Some(operation_kind(operation)),
            node_id: None,
            path: None,
            witness_kind: Some(
                crate::patch_algebra::types::ConflictWitnessKind::MalformedOperation,
            ),
            outcome: MergeEvidenceOutcome::InvalidCandidate,
            evidence_scope: Some(MergeEvidenceScope::UnsealedCandidate),
            proof_phase: MergeEvidenceProofPhase::Classification,
            reason_code: MergeEvidenceReasonCode::InvalidUnsealedCandidate,
        })
}

fn is_required_evidence_error(error: &EvidenceError) -> bool {
    match error {
        EvidenceError::Missing { scope, .. }
        | EvidenceError::WrongObjectType { scope, .. }
        | EvidenceError::WrongBlobKind { scope, .. }
        | EvidenceError::Malformed { scope, .. }
        | EvidenceError::Unreadable { scope, .. } => {
            matches!(
                scope,
                EvidenceScope::SealedBaselineRequired | EvidenceScope::SealedCandidateRequired
            )
        }
    }
}

fn item_sort_key(item: &MergeEvidenceItem) -> ItemSortKey {
    ItemSortKey {
        side: item.side,
        operation_index: item.operation_index,
        peer_operation_index: item.peer_operation_index,
        op_seq: item.op_seq,
        peer_op_seq: item.peer_op_seq,
        outcome_rank: outcome_sort_rank(item.outcome),
        reason_code: item.reason_code,
        evidence_scope: item.evidence_scope,
        proof_phase: item.proof_phase,
        operation_kind: item.operation_kind,
        node_id: item.node_id,
        path: item.path.clone(),
    }
}

fn outcome_sort_rank(outcome: MergeEvidenceOutcome) -> u8 {
    match outcome {
        MergeEvidenceOutcome::EvidenceFailure => 0,
        MergeEvidenceOutcome::InvalidCandidate => 1,
        MergeEvidenceOutcome::Unsupported => 2,
        MergeEvidenceOutcome::Deferred => 3,
        MergeEvidenceOutcome::Conflict => 4,
        MergeEvidenceOutcome::OrderedDependency => 5,
        MergeEvidenceOutcome::NotConfluent => 6,
        MergeEvidenceOutcome::Confluent => 7,
    }
}
