use prikk_object::{BlobKind, NodeId, ObjectId, text_span_hash};

use crate::node::node_lifecycle::NodeLifecycleState;
use crate::text_span;

use super::evidence_types::{
    ClassificationResult, Evidence, EvidenceError, EvidenceScope, NoPatchAlgebraEvidence,
    PatchAlgebraEvidence,
};
use super::preimage::invalid_preimage_class;
use super::types::{Action, ConflictWitnessKind, OperationFacts, RequiredOrder, UnknownReason};
use super::witness::{conflict, conflict_with_span, derive_path, ordered, unknown_from_facts};

/// The pair being classified plus the baseline it is classified against. Bundled so the two
/// `classify_create_then_*` helpers below -- which need all four to derive a witness path --
/// stay under clippy's argument-count limit.
struct SameNodePair<'a> {
    baseline: &'a NodeLifecycleState,
    left: &'a OperationFacts,
    right: &'a OperationFacts,
    node_id: NodeId,
}

pub(super) fn classify_create_then_mutate<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: NodeId,
) -> Option<ClassificationResult> {
    if let Some(class) = invalid_create_in_same_node_pair(baseline, left, right) {
        return Some(class);
    }
    if let Some(class) = invalid_create_in_same_node_pair(baseline, right, left) {
        return Some(class);
    }
    let pair = SameNodePair {
        baseline,
        left,
        right,
        node_id,
    };
    match (&left.action, &right.action) {
        (Action::CreateFile { mode, .. }, Action::ChangePerm { old_mode, .. })
            if *mode == *old_mode =>
        {
            Some(Ok(ordered(
                RequiredOrder::LeftBeforeRight,
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                Some(node_id),
                derive_path(baseline, left, right),
            )))
        }
        (Action::ChangePerm { old_mode, .. }, Action::CreateFile { mode, .. })
            if *mode == *old_mode =>
        {
            Some(Ok(ordered(
                RequiredOrder::RightBeforeLeft,
                ConflictWitnessKind::LiveStateMismatch,
                left,
                right,
                Some(node_id),
                derive_path(baseline, left, right),
            )))
        }
        (Action::CreateFile { .. }, Action::ChangePerm { .. })
        | (Action::ChangePerm { .. }, Action::CreateFile { .. }) => Some(Ok(conflict(
            ConflictWitnessKind::ModeMismatch,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        ))),
        (Action::CreateFile { blob_id, .. }, Action::ReplaceBinary { old_blob_id, .. }) => {
            Some(classify_create_then_replace_binary(
                &pair,
                evidence,
                RequiredOrder::LeftBeforeRight,
                *blob_id,
                *old_blob_id,
            ))
        }
        (Action::ReplaceBinary { old_blob_id, .. }, Action::CreateFile { blob_id, .. }) => {
            Some(classify_create_then_replace_binary(
                &pair,
                evidence,
                RequiredOrder::RightBeforeLeft,
                *blob_id,
                *old_blob_id,
            ))
        }
        (Action::CreateFile { blob_id, .. }, edit @ Action::EditText { .. }) => {
            Some(classify_create_then_edit_text(
                &pair,
                evidence,
                RequiredOrder::LeftBeforeRight,
                *blob_id,
                edit,
            ))
        }
        (edit @ Action::EditText { .. }, Action::CreateFile { blob_id, .. }) => {
            Some(classify_create_then_edit_text(
                &pair,
                evidence,
                RequiredOrder::RightBeforeLeft,
                *blob_id,
                edit,
            ))
        }
        (Action::CreateFile { .. }, _) | (_, Action::CreateFile { .. }) => Some(Ok(conflict(
            ConflictWitnessKind::NodeIdReuse,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        ))),
        _ => None,
    }
}

fn invalid_create_in_same_node_pair(
    baseline: &NodeLifecycleState,
    create_candidate: &OperationFacts,
    peer: &OperationFacts,
) -> Option<ClassificationResult> {
    if !matches!(create_candidate.action, Action::CreateFile { .. }) {
        return None;
    }
    match invalid_preimage_class(baseline, &NoPatchAlgebraEvidence, create_candidate, peer) {
        Ok(Some(class)) => Some(Ok(class)),
        Ok(None) => None,
        Err(err) => Some(Err(err)),
    }
}

fn classify_create_then_replace_binary<R: PatchAlgebraEvidence>(
    pair: &SameNodePair<'_>,
    evidence: &R,
    required_order: RequiredOrder,
    create_blob_id: ObjectId,
    replace_old_blob_id: ObjectId,
) -> ClassificationResult {
    let SameNodePair {
        baseline,
        left,
        right,
        node_id,
    } = *pair;
    if create_blob_id != replace_old_blob_id {
        return Ok(conflict(
            ConflictWitnessKind::BlobMismatch,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        ));
    }
    match evidence.blob_kind(EvidenceScope::UnsealedCandidateOptional, create_blob_id) {
        Evidence::Known(BlobKind::Binary) => Ok(ordered(
            required_order,
            ConflictWitnessKind::LiveStateMismatch,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        )),
        Evidence::Known(BlobKind::Text | BlobKind::Snapshot) => Ok(conflict(
            ConflictWitnessKind::KindMismatch,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        )),
        Evidence::Missing {
            scope: EvidenceScope::UnsealedCandidateOptional,
            ..
        } => Ok(unknown_from_facts(
            UnknownReason::MissingCandidateEvidence,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        )),
        other => Err(evidence_error(other)),
    }
}

fn classify_create_then_edit_text<R: PatchAlgebraEvidence>(
    pair: &SameNodePair<'_>,
    evidence: &R,
    required_order: RequiredOrder,
    create_blob_id: ObjectId,
    edit: &Action,
) -> ClassificationResult {
    let SameNodePair {
        baseline,
        left,
        right,
        node_id,
    } = *pair;
    let (kind, content) =
        match evidence.blob_content(EvidenceScope::UnsealedCandidateOptional, create_blob_id) {
            Evidence::Known(content) => content,
            Evidence::Missing {
                scope: EvidenceScope::UnsealedCandidateOptional,
                ..
            } => {
                return Ok(unknown_from_facts(
                    UnknownReason::MissingCandidateEvidence,
                    left,
                    right,
                    Some(node_id),
                    derive_path(baseline, left, right),
                ));
            }
            other => return Err(evidence_error(other)),
        };
    if kind != BlobKind::Text {
        return Ok(conflict(
            ConflictWitnessKind::KindMismatch,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        ));
    }
    let Action::EditText {
        span_id,
        old_span_hash,
        left_anchor_hash,
        right_anchor_hash,
        old_span_text,
        left_anchor_len,
        right_anchor_len,
        ..
    } = edit
    else {
        unreachable!("caller passes only EditText actions");
    };
    if text_span_hash(old_span_text) != *old_span_hash {
        return Ok(conflict_with_span(
            ConflictWitnessKind::TextAnchorStale,
            left,
            right,
            node_id,
            derive_path(baseline, left, right),
            *span_id,
        ));
    }
    match text_span::resolve_text_span(
        &content,
        old_span_text,
        left_anchor_hash,
        right_anchor_hash,
        span_id,
        node_id,
        old_span_hash,
        *left_anchor_len,
        *right_anchor_len,
    ) {
        Ok(_) => Ok(ordered(
            required_order,
            ConflictWitnessKind::LiveStateMismatch,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        )),
        Err(_) => Ok(conflict_with_span(
            ConflictWitnessKind::TextAnchorStale,
            left,
            right,
            node_id,
            derive_path(baseline, left, right),
            *span_id,
        )),
    }
}

fn evidence_error<T>(evidence: Evidence<T>) -> EvidenceError {
    evidence.into_error()
}
