use super::classify::classify_pair_with_text_resolver;
use super::evidence_types::{EvidenceError, EvidenceFact, EvidenceScope, PatchAlgebraEvidence};
use super::facts::{deferred_reason, operation_facts};
use super::replay_oracle::{OracleFailure, OracleState, replay_operations};
use super::types::{
    CommutationAnalysisResult, CommutationProof, CommutationResult, ConfluenceAnalysisResult,
    ConfluenceProof, ConfluenceResult, ConfluenceWitness, ConfluenceWitnessKind, PairClass,
    UnknownReason,
};
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::patch_replay::decode::DecodedPatchOperation;

pub(crate) fn commute_pair<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> CommutationAnalysisResult {
    validate_operation_provenance(left, candidate_scope)?;
    validate_operation_provenance(right, candidate_scope)?;
    let pair_class = classify_pair_with_text_resolver(baseline, evidence, left, right)?;
    match pair_class {
        PairClass::Independent => {
            match prove_pair_replay(baseline, evidence, candidate_scope, left, right) {
                Ok(()) => Ok(CommutationResult::Commutes {
                    proof: CommutationProof {
                        left_op_seq: left.op_seq,
                        right_op_seq: right.op_seq,
                    },
                }),
                Err(OracleFailure::Evidence(error)) => Err(error),
                Err(OracleFailure::Unknown(reason)) => Ok(CommutationResult::Unknown { reason }),
                Err(OracleFailure::Replay) => Ok(CommutationResult::Unknown {
                    reason: UnknownReason::UnknownRelation,
                }),
            }
        }
        PairClass::Unknown { reason, .. } => Ok(CommutationResult::Unknown { reason }),
        other => Ok(CommutationResult::DoesNotCommute { pair_class: other }),
    }
}

pub(crate) fn check_confluence<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    left: &[DecodedPatchOperation],
    right: &[DecodedPatchOperation],
) -> ConfluenceAnalysisResult {
    let left_sequence_result = ensure_flat_sequence(baseline, evidence, candidate_scope, left)?;
    let right_sequence_result = ensure_flat_sequence(baseline, evidence, candidate_scope, right)?;
    if let Some(result) = left_sequence_result {
        return Ok(result);
    }
    if let Some(result) = right_sequence_result {
        return Ok(result);
    }
    for (left_index, left_operation) in left.iter().enumerate() {
        for (right_index, right_operation) in right.iter().enumerate() {
            match commute_pair(
                baseline,
                evidence,
                candidate_scope,
                left_operation,
                right_operation,
            )? {
                CommutationResult::Commutes { .. } => {}
                CommutationResult::Unknown { reason } => {
                    return Ok(ConfluenceResult::Unknown { reason });
                }
                CommutationResult::DoesNotCommute { pair_class } => {
                    return Ok(ConfluenceResult::NotConfluent {
                        witness: confluence_pair_witness(left_index, right_index, pair_class),
                    });
                }
            }
        }
    }
    let left_then_right = replay_sequence_order(baseline, evidence, candidate_scope, left, right)?;
    let right_then_left = replay_sequence_order(baseline, evidence, candidate_scope, right, left)?;
    if left_then_right == right_then_left {
        Ok(ConfluenceResult::Confluent {
            proof: ConfluenceProof {
                left_len: left.len(),
                right_len: right.len(),
            },
        })
    } else {
        Ok(ConfluenceResult::NotConfluent {
            witness: ConfluenceWitness {
                kind: ConfluenceWitnessKind::FinalStateInequality,
                left_index: None,
                right_index: None,
                pair_class: None,
            },
        })
    }
}

fn validate_operation_provenance(
    operation: &DecodedPatchOperation,
    candidate_scope: EvidenceScope,
) -> Result<(), EvidenceError> {
    match operation_facts(operation) {
        Ok(_) => Ok(()),
        Err(UnknownReason::MalformedOperation)
            if candidate_scope != EvidenceScope::UnsealedCandidateOptional =>
        {
            Err(EvidenceError::Malformed {
                scope: candidate_scope,
                fact: EvidenceFact::Operation,
                object_id: None,
                reason: "malformed sealed candidate operation".to_string(),
            })
        }
        Err(_) => Ok(()),
    }
}

fn prove_pair_replay<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> Result<(), OracleFailure> {
    let left_then_right = replay_operations(baseline, evidence, candidate_scope, [left, right])?;
    let right_then_left = replay_operations(baseline, evidence, candidate_scope, [right, left])?;
    if left_then_right == right_then_left {
        Ok(())
    } else {
        Err(OracleFailure::Replay)
    }
}

fn ensure_flat_sequence<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    sequence: &[DecodedPatchOperation],
) -> Result<Option<ConfluenceResult>, EvidenceError> {
    let mut first_algebraic_result = None;
    for (index, operation) in sequence.iter().enumerate() {
        validate_operation_provenance(operation, candidate_scope)?;
        // Scan the full sequence so sealed evidence errors are not hidden by earlier algebraic Unknown.
        if let Ok(facts) = operation_facts(operation) {
            if let Some(reason) = deferred_reason(&facts.action) {
                first_algebraic_result.get_or_insert(ConfluenceResult::Unknown { reason });
                continue;
            }
        }
        match replay_operations(baseline, evidence, candidate_scope, [operation]) {
            Ok(_) => {}
            Err(OracleFailure::Evidence(error)) => return Err(error),
            Err(OracleFailure::Unknown(reason)) => {
                first_algebraic_result.get_or_insert(ConfluenceResult::Unknown { reason });
            }
            Err(OracleFailure::Replay) => {
                let result =
                    if has_prefix_dependency(baseline, evidence, candidate_scope, sequence, index)?
                    {
                        ConfluenceResult::Unknown {
                            reason: UnknownReason::SequenceInternalDependencyDeferred,
                        }
                    } else {
                        ConfluenceResult::NotConfluent {
                            witness: ConfluenceWitness {
                                kind: ConfluenceWitnessKind::ReplayFailure,
                                left_index: Some(index),
                                right_index: None,
                                pair_class: None,
                            },
                        }
                    };
                first_algebraic_result.get_or_insert(result);
            }
        }
    }
    Ok(first_algebraic_result)
}

fn has_prefix_dependency<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    sequence: &[DecodedPatchOperation],
    index: usize,
) -> Result<bool, EvidenceError> {
    let Some(subject) = sequence.get(index) else {
        return Ok(false);
    };
    let Some(prior_operations) = sequence.get(..index) else {
        return Ok(false);
    };
    for prior in prior_operations {
        validate_operation_provenance(prior, candidate_scope)?;
        if let PairClass::OrderedDependency { .. } =
            classify_pair_with_text_resolver(baseline, evidence, prior, subject)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Replay `first` then `second` as one chained sequence, over `baseline`. `OracleFailure::Replay`
/// here is not evidence corruption: every pairwise confluence check already passed, so this is
/// specifically the RFC 134 §7.4 item 1 sequencing invariant breaking — the sequence's own
/// operations were authored against a shared baseline rather than each predecessor's own result
/// (`text_span.rs`'s own module doc states the invariant, what upholds it, and what breaks it).
/// **Keep this a refusal, not a repair**: resolving it by re-localizing against the composed state
/// was considered and refused (RFC 134 §7.4/§7.5, "shape 4") — the pairwise verdicts stay sound,
/// what fails is that the operations do not compose.
fn replay_sequence_order<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    first: &[DecodedPatchOperation],
    second: &[DecodedPatchOperation],
) -> Result<OracleState, EvidenceError> {
    replay_operations(
        baseline,
        evidence,
        candidate_scope,
        first.iter().chain(second.iter()),
    )
    .map_err(|failure| match failure {
        OracleFailure::Evidence(error) => error,
        OracleFailure::Unknown(reason) => EvidenceError::Malformed {
            scope: candidate_scope,
            fact: EvidenceFact::Operation,
            object_id: None,
            reason: format!("unexpected unknown during composed replay: {reason:?}"),
        },
        OracleFailure::Replay => EvidenceError::Malformed {
            scope: candidate_scope,
            fact: EvidenceFact::Operation,
            object_id: None,
            reason: "sequence operations do not compose against a shared baseline".to_string(),
        },
    })
}

fn confluence_pair_witness(
    left_index: usize,
    right_index: usize,
    pair_class: PairClass,
) -> ConfluenceWitness {
    let kind = match pair_class {
        PairClass::OrderedDependency { .. } => ConfluenceWitnessKind::OrderedDependency,
        PairClass::Conflict { .. } => ConfluenceWitnessKind::Conflict,
        PairClass::Independent | PairClass::Unknown { .. } => ConfluenceWitnessKind::ReplayFailure,
    };
    ConfluenceWitness {
        kind,
        left_index: Some(left_index),
        right_index: Some(right_index),
        pair_class: Some(pair_class),
    }
}
