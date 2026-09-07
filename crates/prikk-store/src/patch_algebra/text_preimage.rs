use prikk_object::{NodeId, NodeKind, text_span_hash};

use crate::node::node_lifecycle::{NodeContent, NodeLifecycleState};
use crate::text_span;

use super::evidence_types::{Evidence, EvidenceError, EvidenceScope, PatchAlgebraEvidence};
use super::preimage::PreimageStatus;
use super::types::ConflictWitnessKind;

pub(super) struct TextPreimage<'a> {
    pub(super) span_id: [u8; 32],
    pub(super) old_span_hash: &'a [u8; 32],
    pub(super) left_anchor_hash: &'a [u8; 32],
    pub(super) right_anchor_hash: &'a [u8; 32],
    pub(super) old_span_text: &'a [u8],
    pub(super) left_anchor_len: Option<u32>,
    pub(super) right_anchor_len: Option<u32>,
}

pub(super) fn validate_text_preimage<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    text_resolver: &R,
    node_id: NodeId,
    preimage: TextPreimage<'_>,
) -> Result<PreimageStatus, EvidenceError> {
    if text_span_hash(preimage.old_span_text) != *preimage.old_span_hash {
        return Ok(PreimageStatus::Conflict {
            kind: ConflictWitnessKind::TextAnchorStale,
            node_id: Some(node_id),
            path: None,
        });
    }
    let Some(live) = baseline.live_node(&node_id) else {
        return Ok(PreimageStatus::Conflict {
            kind: ConflictWitnessKind::LiveStateMismatch,
            node_id: Some(node_id),
            path: None,
        });
    };
    if live.kind != NodeKind::TextFile {
        return Ok(PreimageStatus::Conflict {
            kind: ConflictWitnessKind::KindMismatch,
            node_id: Some(node_id),
            path: Some(live.path.clone()),
        });
    }
    let NodeContent::File { blob_id, .. } = &live.content else {
        return Ok(PreimageStatus::Conflict {
            kind: ConflictWitnessKind::KindMismatch,
            node_id: Some(node_id),
            path: Some(live.path.clone()),
        });
    };
    let current_text =
        match text_resolver.baseline_text(EvidenceScope::SealedBaselineRequired, node_id, *blob_id)
        {
            Evidence::Known(text) => text,
            other => return Err(other.into_error()),
        };
    match text_span::resolve_text_span(
        &current_text,
        preimage.old_span_text,
        preimage.left_anchor_hash,
        preimage.right_anchor_hash,
        &preimage.span_id,
        node_id,
        preimage.old_span_hash,
        preimage.left_anchor_len,
        preimage.right_anchor_len,
    ) {
        Ok(_) => Ok(PreimageStatus::Valid),
        Err(_) => Ok(PreimageStatus::Conflict {
            kind: ConflictWitnessKind::TextAnchorStale,
            node_id: Some(node_id),
            path: Some(live.path.clone()),
        }),
    }
}
