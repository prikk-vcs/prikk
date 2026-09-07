use std::collections::BTreeMap;

use prikk_object::{BlobKind, NodeId, NodeKind, ObjectId};

use super::evidence_types::{
    Evidence, EvidenceError, EvidenceFact, EvidenceScope, PatchAlgebraEvidence,
};
use super::types::UnknownReason;
use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation,
};
use crate::path::RepoPath;
use crate::text_span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OracleState {
    pub(super) lifecycle: NodeLifecycleState,
    texts: BTreeMap<NodeId, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OracleFailure {
    Evidence(EvidenceError),
    Unknown(UnknownReason),
    Replay,
}

pub(super) fn replay_operations<'a, R, I>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    operations: I,
) -> Result<OracleState, OracleFailure>
where
    R: PatchAlgebraEvidence,
    I: IntoIterator<Item = &'a DecodedPatchOperation>,
{
    let mut oracle = OracleState {
        lifecycle: baseline.clone(),
        texts: BTreeMap::new(),
    };
    for operation in operations {
        apply_operation(&mut oracle, evidence, candidate_scope, operation)?;
    }
    Ok(oracle)
}

fn apply_operation<R: PatchAlgebraEvidence>(
    oracle: &mut OracleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    operation: &DecodedPatchOperation,
) -> Result<(), OracleFailure> {
    match &operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => apply_create(
            oracle,
            evidence,
            candidate_scope,
            path,
            *node_id,
            *blob_id,
            *mode,
        ),
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage:
                DecodedDeletePreimage::File {
                    old_node_kind,
                    old_blob_id,
                    old_mode,
                },
        } => apply_delete_file(
            oracle,
            candidate_scope,
            path,
            *node_id,
            *old_node_kind,
            *old_blob_id,
            *old_mode,
        ),
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => oracle
            .lifecycle
            .change_file_mode(*node_id, *old_mode, *new_mode)
            .map_err(|_| OracleFailure::Replay),
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => apply_replace_binary(
            oracle,
            evidence,
            candidate_scope,
            *node_id,
            *old_blob_id,
            *new_blob_id,
        ),
        DecodedOperationKind::EditText { .. } => apply_text_edit(oracle, evidence, operation),
        DecodedOperationKind::RenamePath { .. }
        | DecodedOperationKind::CreateSymlink { .. }
        | DecodedOperationKind::DeleteNode {
            preimage: DecodedDeletePreimage::Symlink { .. },
            ..
        } => Err(OracleFailure::Unknown(UnknownReason::UnknownRelation)),
    }
}

fn apply_create<R: PatchAlgebraEvidence>(
    oracle: &mut OracleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    path: &str,
    node_id: NodeId,
    blob_id: prikk_object::ObjectId,
    mode: u32,
) -> Result<(), OracleFailure> {
    let blob_kind = match evidence.blob_kind(candidate_scope, blob_id) {
        Evidence::Known(kind) => kind,
        Evidence::Missing {
            scope: EvidenceScope::UnsealedCandidateOptional,
            ..
        } => {
            return Err(OracleFailure::Unknown(
                UnknownReason::MissingCandidateEvidence,
            ));
        }
        other => return Err(OracleFailure::Evidence(other.into_error())),
    };
    let node_kind = NodeKind::from_file_blob_kind(blob_kind).map_err(|_| OracleFailure::Replay)?;
    oracle
        .lifecycle
        .create_node(
            node_id,
            LiveNode {
                path: parse_repo_path(candidate_scope, path)?,
                kind: node_kind,
                content: NodeContent::File { blob_id, mode },
            },
        )
        .map_err(|_| OracleFailure::Replay)
}

fn apply_delete_file(
    oracle: &mut OracleState,
    candidate_scope: EvidenceScope,
    path: &str,
    node_id: NodeId,
    old_node_kind: NodeKind,
    old_blob_id: prikk_object::ObjectId,
    old_mode: u32,
) -> Result<(), OracleFailure> {
    oracle
        .lifecycle
        .delete_node_checked(
            node_id,
            &LiveNode {
                path: parse_repo_path(candidate_scope, path)?,
                kind: old_node_kind,
                content: NodeContent::File {
                    blob_id: old_blob_id,
                    mode: old_mode,
                },
            },
        )
        .map(|_| ())
        .map_err(|_| OracleFailure::Replay)
}

fn apply_replace_binary<R: PatchAlgebraEvidence>(
    oracle: &mut OracleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    node_id: NodeId,
    old_blob_id: ObjectId,
    new_blob_id: ObjectId,
) -> Result<(), OracleFailure> {
    match evidence.blob_kind(candidate_scope, new_blob_id) {
        Evidence::Known(BlobKind::Binary) => {}
        Evidence::Known(actual) => {
            return Err(OracleFailure::Evidence(EvidenceError::WrongBlobKind {
                scope: candidate_scope,
                blob_id: new_blob_id,
                expected: BlobKind::Binary,
                actual,
            }));
        }
        Evidence::Missing {
            scope: EvidenceScope::UnsealedCandidateOptional,
            ..
        } => {
            return Err(OracleFailure::Unknown(
                UnknownReason::MissingCandidateEvidence,
            ));
        }
        other => return Err(OracleFailure::Evidence(other.into_error())),
    }
    oracle
        .lifecycle
        .replace_file_blob(node_id, old_blob_id, new_blob_id)
        .map_err(|_| OracleFailure::Replay)
}

fn apply_text_edit<R: PatchAlgebraEvidence>(
    oracle: &mut OracleState,
    evidence: &R,
    operation: &DecodedPatchOperation,
) -> Result<(), OracleFailure> {
    let DecodedOperationKind::EditText {
        node_id,
        span_id,
        old_span_hash,
        left_anchor_hash,
        right_anchor_hash,
        replacement_text,
        old_span_text,
        left_anchor_len,
        right_anchor_len,
    } = &operation.kind
    else {
        return Err(OracleFailure::Replay);
    };
    let current_text = current_text(oracle, evidence, *node_id)?;
    let (start, end) = text_span::resolve_text_span(
        &current_text,
        old_span_text,
        left_anchor_hash,
        right_anchor_hash,
        span_id,
        *node_id,
        old_span_hash,
        *left_anchor_len,
        *right_anchor_len,
    )
    .map_err(|_| OracleFailure::Replay)?;
    let new_text = text_span::splice_text(&current_text, start, end, replacement_text)
        .map_err(|_| OracleFailure::Replay)?;
    let new_blob_id = text_span::text_blob_id(&new_text).map_err(|_| OracleFailure::Replay)?;
    oracle
        .lifecycle
        .set_text_blob(*node_id, new_blob_id)
        .map_err(|_| OracleFailure::Replay)?;
    oracle.texts.insert(*node_id, new_text);
    Ok(())
}

fn current_text<R: PatchAlgebraEvidence>(
    oracle: &mut OracleState,
    evidence: &R,
    node_id: NodeId,
) -> Result<Vec<u8>, OracleFailure> {
    if let Some(text) = oracle.texts.get(&node_id) {
        return Ok(text.clone());
    }
    let live = oracle
        .lifecycle
        .live_node(&node_id)
        .ok_or(OracleFailure::Replay)?;
    if live.kind != NodeKind::TextFile {
        return Err(OracleFailure::Replay);
    }
    let NodeContent::File { blob_id, .. } = live.content else {
        return Err(OracleFailure::Replay);
    };
    match evidence.baseline_text(EvidenceScope::SealedBaselineRequired, node_id, blob_id) {
        Evidence::Known(text) => {
            oracle.texts.insert(node_id, text.clone());
            Ok(text)
        }
        other => Err(OracleFailure::Evidence(other.into_error())),
    }
}

fn parse_repo_path(scope: EvidenceScope, path: &str) -> Result<RepoPath, OracleFailure> {
    RepoPath::parse(path).map_err(|_| malformed_operation(scope, "malformed oracle path"))
}

fn malformed_operation(scope: EvidenceScope, reason: &str) -> OracleFailure {
    if scope == EvidenceScope::UnsealedCandidateOptional {
        OracleFailure::Unknown(UnknownReason::MalformedOperation)
    } else {
        OracleFailure::Evidence(EvidenceError::Malformed {
            scope,
            fact: EvidenceFact::Operation,
            object_id: None,
            reason: reason.to_string(),
        })
    }
}
