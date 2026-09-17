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

impl OracleState {
    /// The materialized text of `node_id`, when this replay read or edited it.
    pub(super) fn text(&self, node_id: &NodeId) -> Option<&[u8]> {
        self.texts.get(node_id).map(Vec::as_slice)
    }

    /// Every node whose text this replay materialized.
    pub(super) fn text_nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.texts.keys()
    }
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
    let mut operations = operations.into_iter().peekable();
    while let Some(operation) = operations.next() {
        if let DecodedOperationKind::RenamePath { .. } = operation.kind {
            // A patch's consecutive renames are one batch, resolved together (a swap), exactly as the
            // lifecycle fold applies them (`lifecycle_cache::replay::effect::collect_rename_run`). Only
            // a declared patch boundary joins renames; an operation with no declared patch is its own
            // batch.
            let mut batch = vec![rename_triple(candidate_scope, operation)?];
            while let Some(next) = operations.next_if(|next| same_rename_batch(operation, next)) {
                batch.push(rename_triple(candidate_scope, next)?);
            }
            // The one rename semantics of the algebra: the lifecycle state's own batch rename.
            oracle
                .lifecycle
                .rename_nodes_checked_batch(&batch)
                .map_err(|_| OracleFailure::Replay)?;
            continue;
        }
        apply_operation(&mut oracle, baseline, evidence, candidate_scope, operation)?;
    }
    Ok(oracle)
}

/// Whether two operations adjacent in a sequence belong to one rename batch: both renames, from the same
/// declared patch. The one grouping rule, shared by [`replay_operations`] and [`replay_unit`].
fn same_rename_batch(first: &DecodedPatchOperation, second: &DecodedPatchOperation) -> bool {
    matches!(first.kind, DecodedOperationKind::RenamePath { .. })
        && matches!(second.kind, DecodedOperationKind::RenamePath { .. })
        && first.patch_id.is_some()
        && first.patch_id == second.patch_id
}

/// The operations `sequence[index]` must replay together with to replay at all: the declared patch's run
/// of consecutive renames it belongs to (a swap resolves only as one batch), or the operation alone. The
/// same grouping `replay_operations` applies, so replaying this unit is replaying that operation.
pub(super) fn replay_unit(
    sequence: &[DecodedPatchOperation],
    index: usize,
) -> &[DecodedPatchOperation] {
    let joins = same_rename_batch;
    let Some(subject) = sequence.get(index) else {
        return &[];
    };
    let mut start = index;
    while start > 0
        && sequence
            .get(start - 1)
            .is_some_and(|prior| joins(prior, subject))
    {
        start -= 1;
    }
    let mut end = index + 1;
    while sequence.get(end).is_some_and(|next| joins(subject, next)) {
        end += 1;
    }
    sequence.get(start..end).unwrap_or(&[])
}

fn rename_triple(
    candidate_scope: EvidenceScope,
    operation: &DecodedPatchOperation,
) -> Result<(NodeId, RepoPath, RepoPath), OracleFailure> {
    let DecodedOperationKind::RenamePath {
        node_id,
        old_path,
        new_path,
    } = &operation.kind
    else {
        return Err(OracleFailure::Replay);
    };
    Ok((
        *node_id,
        parse_repo_path(candidate_scope, old_path)?,
        parse_repo_path(candidate_scope, new_path)?,
    ))
}

fn apply_operation<R: PatchAlgebraEvidence>(
    oracle: &mut OracleState,
    baseline: &NodeLifecycleState,
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
        DecodedOperationKind::EditText { .. } => {
            apply_text_edit(oracle, baseline, evidence, candidate_scope, operation)
        }
        // Renames are applied in batches by `replay_operations`, never one at a time here.
        DecodedOperationKind::RenamePath { .. } => Err(OracleFailure::Replay),
        DecodedOperationKind::CreateSymlink { .. }
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
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
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
    let current_text = current_text(oracle, baseline, evidence, candidate_scope, *node_id)?;
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
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
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
    // DC-75 two-edits handoff §7.3, R4(a): a node the baseline does not hold was created on this side,
    // and its content is that create's stored Blob. Read with the caller's scope, and only as `Text`.
    // This is the candidate `blob_content` read `create.rs` already makes, not a new kind of read.
    if baseline.live_node(&node_id).is_none() {
        return match evidence.blob_content(candidate_scope, blob_id) {
            Evidence::Known((BlobKind::Text, text)) => {
                oracle.texts.insert(node_id, text.clone());
                Ok(text)
            }
            Evidence::Known((actual, _)) => {
                Err(OracleFailure::Evidence(EvidenceError::WrongBlobKind {
                    scope: candidate_scope,
                    blob_id,
                    expected: BlobKind::Text,
                    actual,
                }))
            }
            Evidence::Missing {
                scope: EvidenceScope::UnsealedCandidateOptional,
                ..
            } => Err(OracleFailure::Unknown(
                UnknownReason::MissingCandidateEvidence,
            )),
            other => Err(OracleFailure::Evidence(other.into_error())),
        };
    }
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
