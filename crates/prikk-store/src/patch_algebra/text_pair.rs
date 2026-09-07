use prikk_object::{NodeId, text_span_hash};

use crate::node::node_lifecycle::NodeLifecycleState;
use crate::text_span;

use super::evidence_types::{
    ClassificationResult, Evidence, EvidenceError, EvidenceScope, PatchAlgebraEvidence,
};
use super::preimage::baseline_text_blob_for_mode;
use super::types::{Action, ConflictWitnessKind, OperationFacts, PairClass};
use super::witness::{conflict, conflict_with_span, derive_path};

pub(super) fn classify_mode_and_text_edit<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    text_resolver: &R,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: NodeId,
    old_mode: u32,
    edit: &Action,
) -> ClassificationResult {
    let Some(current_blob_id) = baseline_text_blob_for_mode(baseline, node_id, old_mode) else {
        return Ok(conflict(
            ConflictWitnessKind::LiveStateMismatch,
            left,
            right,
            Some(node_id),
            derive_path(baseline, left, right),
        ));
    };
    let current_text = match text_resolver.baseline_text(
        EvidenceScope::SealedBaselineRequired,
        node_id,
        current_blob_id,
    ) {
        Evidence::Known(text) => text,
        other => return Err(evidence_error(other)),
    };
    match edit {
        Action::EditText {
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            old_span_text,
            left_anchor_len,
            right_anchor_len,
            ..
        } => {
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
                &current_text,
                old_span_text,
                left_anchor_hash,
                right_anchor_hash,
                span_id,
                node_id,
                old_span_hash,
                *left_anchor_len,
                *right_anchor_len,
            ) {
                Ok(_) => Ok(PairClass::Independent),
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
        _ => unreachable!("caller passes only EditText actions"),
    }
}

fn evidence_error<T>(evidence: Evidence<T>) -> EvidenceError {
    evidence.into_error()
}
