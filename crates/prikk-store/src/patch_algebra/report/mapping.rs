use super::types::{
    MergeEvidenceItem, MergeEvidenceOutcome, MergeEvidenceProofPhase, MergeEvidenceReasonCode,
    MergeEvidenceScope, MergeEvidenceSide,
};
use crate::patch_algebra::evidence_types::EvidenceScope;
use crate::patch_algebra::types::{
    ConflictWitness, ConfluenceWitness, ConfluenceWitnessKind, PairClass, UnknownReason,
};

pub(super) fn unknown_report(
    reason: UnknownReason,
    candidate_scope: EvidenceScope,
) -> (MergeEvidenceOutcome, Vec<MergeEvidenceItem>) {
    let (outcome, proof_phase, reason_code) = match reason {
        UnknownReason::MalformedOperation => return malformed_operation_report(candidate_scope),
        UnknownReason::MissingCandidateEvidence => (
            MergeEvidenceOutcome::InvalidCandidate,
            MergeEvidenceProofPhase::Classification,
            MergeEvidenceReasonCode::InsufficientUnsealedCandidateEvidence,
        ),
        UnknownReason::SameNodeTextCommutationDeferred => (
            MergeEvidenceOutcome::Deferred,
            MergeEvidenceProofPhase::Classification,
            MergeEvidenceReasonCode::SameNodeTextTransformDeferred,
        ),
        UnknownReason::SequenceInternalDependencyDeferred => (
            MergeEvidenceOutcome::Deferred,
            MergeEvidenceProofPhase::Flatness,
            MergeEvidenceReasonCode::SequenceInternalDependencyDeferred,
        ),
        UnknownReason::RenameDeferred
        | UnknownReason::SymlinkDeferred
        | UnknownReason::UnknownRelation => (
            MergeEvidenceOutcome::Unsupported,
            MergeEvidenceProofPhase::Classification,
            MergeEvidenceReasonCode::UnsupportedOperation,
        ),
        #[cfg(test)]
        UnknownReason::FuturePreconditionDeferred => (
            MergeEvidenceOutcome::Unsupported,
            MergeEvidenceProofPhase::Classification,
            MergeEvidenceReasonCode::UnsupportedOperation,
        ),
    };
    (
        outcome,
        vec![report_item(outcome, proof_phase, reason_code)],
    )
}

pub(super) fn witness_report(
    witness: &ConfluenceWitness,
) -> (MergeEvidenceOutcome, Vec<MergeEvidenceItem>) {
    match witness.kind {
        ConfluenceWitnessKind::OrderedDependency => pair_witness_report(
            MergeEvidenceOutcome::OrderedDependency,
            MergeEvidenceReasonCode::OrderedDependency,
            witness,
        ),
        ConfluenceWitnessKind::Conflict => pair_witness_report(
            MergeEvidenceOutcome::Conflict,
            MergeEvidenceReasonCode::PairConflict,
            witness,
        ),
        ConfluenceWitnessKind::ReplayFailure => (
            MergeEvidenceOutcome::NotConfluent,
            vec![witness_item(
                MergeEvidenceOutcome::NotConfluent,
                MergeEvidenceProofPhase::ReplayBothOrders,
                MergeEvidenceReasonCode::PairReplayFailed,
                witness,
                None,
            )],
        ),
        ConfluenceWitnessKind::FinalStateInequality => (
            MergeEvidenceOutcome::NotConfluent,
            vec![report_item(
                MergeEvidenceOutcome::NotConfluent,
                MergeEvidenceProofPhase::FinalStateComparison,
                MergeEvidenceReasonCode::FinalStateMismatch,
            )],
        ),
    }
}

#[cfg(test)]
pub(in crate::patch_algebra) fn pair_class_report(
    pair_class: &PairClass,
    candidate_scope: EvidenceScope,
) -> (MergeEvidenceOutcome, Vec<MergeEvidenceItem>) {
    match pair_class {
        PairClass::OrderedDependency { witness, .. } => (
            MergeEvidenceOutcome::OrderedDependency,
            vec![conflict_witness_item(
                MergeEvidenceOutcome::OrderedDependency,
                MergeEvidenceReasonCode::OrderedDependency,
                witness,
            )],
        ),
        PairClass::Conflict { witness } => (
            MergeEvidenceOutcome::Conflict,
            vec![conflict_witness_item(
                MergeEvidenceOutcome::Conflict,
                MergeEvidenceReasonCode::PairConflict,
                witness,
            )],
        ),
        PairClass::Unknown { reason, .. } => unknown_report(*reason, candidate_scope),
        PairClass::Independent => (
            MergeEvidenceOutcome::Confluent,
            vec![report_item(
                MergeEvidenceOutcome::Confluent,
                MergeEvidenceProofPhase::Classification,
                MergeEvidenceReasonCode::ProvenConfluent,
            )],
        ),
    }
}

fn malformed_operation_report(
    candidate_scope: EvidenceScope,
) -> (MergeEvidenceOutcome, Vec<MergeEvidenceItem>) {
    match candidate_scope {
        EvidenceScope::UnsealedCandidateOptional => (
            MergeEvidenceOutcome::InvalidCandidate,
            vec![report_item(
                MergeEvidenceOutcome::InvalidCandidate,
                MergeEvidenceProofPhase::Classification,
                MergeEvidenceReasonCode::InvalidUnsealedCandidate,
            )],
        ),
        EvidenceScope::SealedBaselineRequired | EvidenceScope::SealedCandidateRequired => (
            MergeEvidenceOutcome::EvidenceFailure,
            vec![MergeEvidenceItem {
                side: MergeEvidenceSide::Report,
                operation_index: None,
                peer_operation_index: None,
                op_seq: None,
                peer_op_seq: None,
                operation_kind: None,
                node_id: None,
                path: None,
                witness_kind: None,
                outcome: MergeEvidenceOutcome::EvidenceFailure,
                evidence_scope: Some(candidate_scope.into()),
                proof_phase: MergeEvidenceProofPhase::Classification,
                reason_code: MergeEvidenceReasonCode::MalformedRequiredEvidence,
            }],
        ),
    }
}

pub(super) fn report_item(
    outcome: MergeEvidenceOutcome,
    proof_phase: MergeEvidenceProofPhase,
    reason_code: MergeEvidenceReasonCode,
) -> MergeEvidenceItem {
    MergeEvidenceItem {
        side: MergeEvidenceSide::Report,
        operation_index: None,
        peer_operation_index: None,
        op_seq: None,
        peer_op_seq: None,
        operation_kind: None,
        node_id: None,
        path: None,
        witness_kind: None,
        outcome,
        evidence_scope: None,
        proof_phase,
        reason_code,
    }
}

fn pair_witness_report(
    outcome: MergeEvidenceOutcome,
    reason_code: MergeEvidenceReasonCode,
    witness: &ConfluenceWitness,
) -> (MergeEvidenceOutcome, Vec<MergeEvidenceItem>) {
    let conflict_witness = witness.pair_class.as_ref().and_then(pair_conflict_witness);
    (
        outcome,
        vec![witness_item(
            outcome,
            MergeEvidenceProofPhase::Classification,
            reason_code,
            witness,
            conflict_witness,
        )],
    )
}

fn pair_conflict_witness(pair_class: &PairClass) -> Option<&ConflictWitness> {
    match pair_class {
        PairClass::OrderedDependency { witness, .. }
        | PairClass::Conflict { witness }
        | PairClass::Unknown { witness, .. } => Some(witness),
        PairClass::Independent => None,
    }
}

fn witness_item(
    outcome: MergeEvidenceOutcome,
    proof_phase: MergeEvidenceProofPhase,
    reason_code: MergeEvidenceReasonCode,
    witness: &ConfluenceWitness,
    conflict_witness: Option<&ConflictWitness>,
) -> MergeEvidenceItem {
    MergeEvidenceItem {
        side: MergeEvidenceSide::Cross,
        operation_index: witness.left_index,
        peer_operation_index: witness.right_index,
        op_seq: conflict_witness.map(|witness| witness.left_op_seq),
        peer_op_seq: conflict_witness.map(|witness| witness.right_op_seq),
        operation_kind: None,
        node_id: conflict_witness.and_then(|witness| witness.node_id),
        path: conflict_witness.and_then(|witness| witness.path.clone()),
        witness_kind: conflict_witness.map(|witness| witness.kind),
        outcome,
        evidence_scope: None,
        proof_phase,
        reason_code,
    }
}

#[cfg(test)]
fn conflict_witness_item(
    outcome: MergeEvidenceOutcome,
    reason_code: MergeEvidenceReasonCode,
    witness: &ConflictWitness,
) -> MergeEvidenceItem {
    MergeEvidenceItem {
        side: MergeEvidenceSide::Cross,
        operation_index: Some(0),
        peer_operation_index: Some(0),
        op_seq: Some(witness.left_op_seq),
        peer_op_seq: Some(witness.right_op_seq),
        operation_kind: None,
        node_id: witness.node_id,
        path: witness.path.clone(),
        witness_kind: Some(witness.kind),
        outcome,
        evidence_scope: None,
        proof_phase: MergeEvidenceProofPhase::Classification,
        reason_code,
    }
}

impl From<EvidenceScope> for MergeEvidenceScope {
    fn from(scope: EvidenceScope) -> Self {
        match scope {
            EvidenceScope::SealedBaselineRequired => Self::SealedBaseline,
            EvidenceScope::SealedCandidateRequired => Self::SealedCandidate,
            EvidenceScope::UnsealedCandidateOptional => Self::UnsealedCandidate,
        }
    }
}
