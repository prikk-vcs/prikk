use std::collections::BTreeMap;

use prikk_error::{PrikkError, Result};
use prikk_object::NodeId;

use crate::node::node_lifecycle::{LiveNode, NodeContent};
use crate::patch_replay::decode::{DecodedDeletePreimage, DecodedPatchOperation};
use crate::path::RepoPath;
use crate::text_span;

use super::*;

pub(super) fn apply_for_oracle(
    state: &mut NodeLifecycleState,
    operation: &DecodedPatchOperation,
) -> Result<()> {
    match &operation.kind {
        DecodedOperationKind::CreateFile { .. } => Err(PrikkError::Integrity(
            "oracle CreateFile application requires blob-kind evidence".to_string(),
        )),
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage:
                DecodedDeletePreimage::File {
                    old_node_kind,
                    old_blob_id,
                    old_mode,
                },
        } => state
            .delete_node_checked(
                *node_id,
                &LiveNode {
                    path: RepoPath::parse(path)?,
                    kind: *old_node_kind,
                    content: NodeContent::File {
                        blob_id: *old_blob_id,
                        mode: *old_mode,
                    },
                },
            )
            .map(|_| ()),
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => state.change_file_mode(*node_id, *old_mode, *new_mode),
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => state.replace_file_blob(*node_id, *old_blob_id, *new_blob_id),
        _ => Err(PrikkError::Integrity(
            "oracle fixture does not support this operation kind".to_string(),
        )),
    }
}

pub(super) fn apply_for_oracle_with_evidence(
    state: &mut NodeLifecycleState,
    texts: &mut BTreeMap<NodeId, Vec<u8>>,
    evidence: &TestTextResolver,
    operation: &DecodedPatchOperation,
) -> Result<()> {
    match &operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => {
            let blob_kind =
                match evidence.blob_kind(EvidenceScope::UnsealedCandidateOptional, *blob_id) {
                    Evidence::Known(kind) => kind,
                    _ => {
                        return Err(PrikkError::Integrity(
                            "missing oracle blob kind".to_string(),
                        ));
                    }
                };
            let node_kind = NodeKind::from_file_blob_kind(blob_kind)?;
            if node_kind == NodeKind::TextFile {
                let (kind, content) = match evidence
                    .blob_content(EvidenceScope::UnsealedCandidateOptional, *blob_id)
                {
                    Evidence::Known(content) => content,
                    _ => {
                        return Err(PrikkError::Integrity(
                            "missing oracle text blob content".to_string(),
                        ));
                    }
                };
                if kind != BlobKind::Text {
                    return Err(PrikkError::Integrity(
                        "oracle text create has non-text content".to_string(),
                    ));
                }
                texts.insert(*node_id, content);
            }
            state.create_node(
                *node_id,
                LiveNode {
                    path: RepoPath::parse(path)?,
                    kind: node_kind,
                    content: NodeContent::File {
                        blob_id: *blob_id,
                        mode: *mode,
                    },
                },
            )
        }
        DecodedOperationKind::EditText { .. } => {
            apply_for_oracle_with_text(state, texts, operation)
        }
        _ => apply_for_oracle(state, operation),
    }
}

pub(super) fn apply_for_oracle_with_text(
    state: &mut NodeLifecycleState,
    texts: &mut BTreeMap<NodeId, Vec<u8>>,
    operation: &DecodedPatchOperation,
) -> Result<()> {
    match &operation.kind {
        DecodedOperationKind::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            replacement_text,
            old_span_text,
            ..
        } => {
            let current_text = texts
                .get(node_id)
                .cloned()
                .ok_or_else(|| PrikkError::Integrity("missing oracle text content".to_string()))?;
            let (start, end) = text_span::locate_text_span(
                &current_text,
                old_span_text,
                left_anchor_hash,
                right_anchor_hash,
                span_id,
                *node_id,
                old_span_hash,
            )
            .map_err(|err| PrikkError::Integrity(err.to_string()))?;
            let new_text = text_span::splice_text(&current_text, start, end, replacement_text)
                .map_err(|err| PrikkError::Integrity(err.to_string()))?;
            state.set_text_blob(*node_id, text_span::text_blob_id(&new_text)?)?;
            texts.insert(*node_id, new_text);
            Ok(())
        }
        _ => apply_for_oracle(state, operation),
    }
}
