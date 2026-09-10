use prikk_object::ObjectId;

use super::error::evidence_error_report;
use super::input::{
    first_required_evidence_error, malformed_unsealed_item, sequence_summary, sort_report_items,
};
#[cfg(test)]
use super::mapping::pair_class_report;
use super::mapping::{report_item, unknown_report, witness_report};
use super::types::{
    MergeEvidenceItem, MergeEvidenceOutcome, MergeEvidenceProofPhase, MergeEvidenceReasonCode,
    MergeEvidenceReport, MergeEvidenceSequence,
};
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::patch_replay::decode::DecodedPatchOperation;

use crate::patch_algebra::commutation::check_confluence;
#[cfg(test)]
use crate::patch_algebra::commutation::commute_pair;
use crate::patch_algebra::evidence_types::{EvidenceScope, PatchAlgebraEvidence};
#[cfg(test)]
use crate::patch_algebra::types::{CommutationAnalysisResult, CommutationResult};
use crate::patch_algebra::types::{ConfluenceAnalysisResult, ConfluenceResult};

pub(crate) fn analyze_merge_evidence<R: PatchAlgebraEvidence>(
    baseline_block_id: ObjectId,
    replay_horizon: Option<ObjectId>,
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    left: &[DecodedPatchOperation],
    right: &[DecodedPatchOperation],
) -> MergeEvidenceReport {
    let left_sequence = sequence_summary("left", left);
    let right_sequence = sequence_summary("right", right);
    let malformed_item = malformed_unsealed_item(candidate_scope, left, right);
    if let Some(error) =
        first_required_evidence_error(baseline, evidence, candidate_scope, left, right)
    {
        return report_from_analysis(
            baseline_block_id,
            replay_horizon,
            left_sequence,
            right_sequence,
            Err(error),
            candidate_scope,
        );
    }
    if let Some(item) = malformed_item {
        return report_from_items(
            baseline_block_id,
            replay_horizon,
            left_sequence,
            right_sequence,
            MergeEvidenceOutcome::InvalidCandidate,
            vec![item],
        );
    }
    report_from_analysis(
        baseline_block_id,
        replay_horizon,
        left_sequence,
        right_sequence,
        check_confluence(baseline, evidence, candidate_scope, left, right),
        candidate_scope,
    )
}

#[cfg(test)]
pub(in crate::patch_algebra) fn analyze_pair_merge_evidence<R: PatchAlgebraEvidence>(
    baseline_block_id: ObjectId,
    replay_horizon: Option<ObjectId>,
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> MergeEvidenceReport {
    let left_sequence = sequence_summary("left", std::slice::from_ref(left));
    let right_sequence = sequence_summary("right", std::slice::from_ref(right));
    let left_slice = std::slice::from_ref(left);
    let right_slice = std::slice::from_ref(right);
    let malformed_item = malformed_unsealed_item(candidate_scope, left_slice, right_slice);
    if let Some(error) =
        first_required_evidence_error(baseline, evidence, candidate_scope, left_slice, right_slice)
    {
        return report_from_commutation(
            baseline_block_id,
            replay_horizon,
            left_sequence,
            right_sequence,
            Err(error),
            candidate_scope,
        );
    }
    if let Some(item) = malformed_item {
        return report_from_items(
            baseline_block_id,
            replay_horizon,
            left_sequence,
            right_sequence,
            MergeEvidenceOutcome::InvalidCandidate,
            vec![item],
        );
    }
    report_from_commutation(
        baseline_block_id,
        replay_horizon,
        left_sequence,
        right_sequence,
        commute_pair(baseline, evidence, candidate_scope, left, right),
        candidate_scope,
    )
}

fn report_from_analysis(
    baseline_block_id: ObjectId,
    replay_horizon: Option<ObjectId>,
    left_sequence: MergeEvidenceSequence,
    right_sequence: MergeEvidenceSequence,
    analysis: ConfluenceAnalysisResult,
    candidate_scope: EvidenceScope,
) -> MergeEvidenceReport {
    let (outcome, items) = match analysis {
        Ok(ConfluenceResult::Confluent { .. }) => (
            MergeEvidenceOutcome::Confluent,
            vec![report_item(
                MergeEvidenceOutcome::Confluent,
                MergeEvidenceProofPhase::FinalStateComparison,
                MergeEvidenceReasonCode::ProvenConfluent,
            )],
        ),
        Ok(ConfluenceResult::Unknown { reason }) => unknown_report(reason, candidate_scope),
        Ok(ConfluenceResult::NotConfluent { witness }) => witness_report(&witness),
        Err(error) => evidence_error_report(&error),
    };
    report_from_items(
        baseline_block_id,
        replay_horizon,
        left_sequence,
        right_sequence,
        outcome,
        items,
    )
}

#[cfg(test)]
fn report_from_commutation(
    baseline_block_id: ObjectId,
    replay_horizon: Option<ObjectId>,
    left_sequence: MergeEvidenceSequence,
    right_sequence: MergeEvidenceSequence,
    analysis: CommutationAnalysisResult,
    candidate_scope: EvidenceScope,
) -> MergeEvidenceReport {
    let (outcome, items) = match analysis {
        Ok(CommutationResult::Commutes { .. }) => (
            MergeEvidenceOutcome::Confluent,
            vec![report_item(
                MergeEvidenceOutcome::Confluent,
                MergeEvidenceProofPhase::ReplayBothOrders,
                MergeEvidenceReasonCode::ProvenConfluent,
            )],
        ),
        Ok(CommutationResult::Unknown { reason }) => unknown_report(reason, candidate_scope),
        Ok(CommutationResult::DoesNotCommute { pair_class }) => {
            pair_class_report(&pair_class, candidate_scope)
        }
        Err(error) => evidence_error_report(&error),
    };
    report_from_items(
        baseline_block_id,
        replay_horizon,
        left_sequence,
        right_sequence,
        outcome,
        items,
    )
}

fn report_from_items(
    baseline_block_id: ObjectId,
    replay_horizon: Option<ObjectId>,
    left_sequence: MergeEvidenceSequence,
    right_sequence: MergeEvidenceSequence,
    outcome: MergeEvidenceOutcome,
    mut items: Vec<MergeEvidenceItem>,
) -> MergeEvidenceReport {
    sort_report_items(&mut items);
    MergeEvidenceReport {
        baseline_block_id,
        replay_horizon,
        left_sequence,
        right_sequence,
        outcome,
        items,
    }
}
