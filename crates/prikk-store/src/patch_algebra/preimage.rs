use prikk_object::{NodeId, NodeKind, ObjectId};

use super::evidence_types::{EvidenceError, NoPatchAlgebraEvidence, PatchAlgebraEvidence};
use super::facts::deferred_reason;
use super::text_preimage::{TextPreimage, validate_text_preimage};
use super::types::{Action, ConflictWitnessKind, OperationFacts, PairClass, UnknownReason};
use super::witness::{conflict, unknown_from_facts};
use crate::node::node_lifecycle::{NodeContent, NodeLifecycleState};
use crate::path::RepoPath;

pub(super) fn invalid_preimage_class<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    text_resolver: &R,
    subject: &OperationFacts,
    peer: &OperationFacts,
) -> Result<Option<PairClass>, EvidenceError> {
    let class = match validate_preimage(baseline, text_resolver, subject)? {
        PreimageStatus::Valid => None,
        PreimageStatus::Conflict {
            kind,
            node_id,
            path,
        } => Some(conflict(
            kind,
            subject,
            peer,
            node_id.or(subject.node_id),
            path,
        )),
        PreimageStatus::Unknown {
            reason,
            node_id,
            path,
        } => Some(unknown_from_facts(
            reason,
            subject,
            peer,
            node_id.or(subject.node_id),
            path,
        )),
    };
    Ok(class)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PreimageStatus {
    Valid,
    Conflict {
        kind: ConflictWitnessKind,
        node_id: Option<NodeId>,
        path: Option<RepoPath>,
    },
    Unknown {
        reason: UnknownReason,
        node_id: Option<NodeId>,
        path: Option<RepoPath>,
    },
}

fn validate_preimage<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    text_resolver: &R,
    facts: &OperationFacts,
) -> Result<PreimageStatus, EvidenceError> {
    match &facts.action {
        Action::CreateFile {
            path,
            node_id,
            blob_id: _,
            mode: _,
        } => {
            if baseline.node_id_at(path).is_some() {
                return Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::SamePathCreate,
                    node_id: Some(*node_id),
                    path: Some(path.clone()),
                });
            }
            if baseline.contains_seen_node_id(node_id) {
                return Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::NodeIdReuse,
                    node_id: Some(*node_id),
                    path: Some(path.clone()),
                });
            }
            Ok(PreimageStatus::Valid)
        }
        Action::DeleteFile {
            path,
            node_id,
            old_node_kind,
            old_blob_id,
            old_mode,
        } => {
            if baseline_file_matches(baseline, *node_id, *old_node_kind, *old_blob_id, *old_mode)
                && baseline.node_id_at(path) == Some(*node_id)
            {
                Ok(PreimageStatus::Valid)
            } else {
                Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::LiveStateMismatch,
                    node_id: Some(*node_id),
                    path: Some(path.clone()),
                })
            }
        }
        Action::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            old_span_text,
            left_anchor_len,
            right_anchor_len,
        } => validate_text_preimage(
            baseline,
            text_resolver,
            *node_id,
            TextPreimage {
                span_id: *span_id,
                old_span_hash,
                left_anchor_hash,
                right_anchor_hash,
                old_span_text,
                left_anchor_len: *left_anchor_len,
                right_anchor_len: *right_anchor_len,
            },
        ),
        Action::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id: _,
        } => {
            let Some(live) = baseline.live_node(node_id) else {
                return Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::LiveStateMismatch,
                    node_id: Some(*node_id),
                    path: None,
                });
            };
            match &live.content {
                NodeContent::File { blob_id, .. }
                    if live.kind == NodeKind::BinaryFile && *blob_id == *old_blob_id =>
                {
                    Ok(PreimageStatus::Valid)
                }
                NodeContent::File { .. } if live.kind != NodeKind::BinaryFile => {
                    Ok(PreimageStatus::Conflict {
                        kind: ConflictWitnessKind::KindMismatch,
                        node_id: Some(*node_id),
                        path: Some(live.path.clone()),
                    })
                }
                NodeContent::File { .. } => Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::BlobMismatch,
                    node_id: Some(*node_id),
                    path: Some(live.path.clone()),
                }),
                NodeContent::Symlink { .. } => Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::KindMismatch,
                    node_id: Some(*node_id),
                    path: Some(live.path.clone()),
                }),
            }
        }
        Action::ChangePerm {
            node_id,
            old_mode,
            new_mode: _,
        } => {
            let Some(live) = baseline.live_node(node_id) else {
                return Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::LiveStateMismatch,
                    node_id: Some(*node_id),
                    path: None,
                });
            };
            match &live.content {
                NodeContent::File { mode, .. } if *mode == *old_mode => Ok(PreimageStatus::Valid),
                NodeContent::File { .. } => Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::ModeMismatch,
                    node_id: Some(*node_id),
                    path: Some(live.path.clone()),
                }),
                NodeContent::Symlink { .. } => Ok(PreimageStatus::Conflict {
                    kind: ConflictWitnessKind::KindMismatch,
                    node_id: Some(*node_id),
                    path: Some(live.path.clone()),
                }),
            }
        }
        Action::RenamePath { .. } | Action::CreateSymlink { .. } | Action::DeleteSymlink { .. } => {
            Ok(PreimageStatus::Unknown {
                reason: deferred_reason(&facts.action).unwrap_or(UnknownReason::UnknownRelation),
                node_id: facts.node_id,
                path: None,
            })
        }
    }
}

pub(super) fn is_delete_preimage_valid(
    baseline: &NodeLifecycleState,
    facts: &OperationFacts,
) -> bool {
    match &facts.action {
        Action::DeleteFile { .. } => {
            validate_preimage(baseline, &NoPatchAlgebraEvidence, facts) == Ok(PreimageStatus::Valid)
        }
        _ => false,
    }
}

pub(super) fn is_create_after_delete_valid(
    baseline: &NodeLifecycleState,
    delete: &OperationFacts,
    create: &OperationFacts,
) -> bool {
    let (
        Action::DeleteFile {
            path: deleted_path, ..
        },
        Action::CreateFile {
            path: created_path,
            node_id: created_node_id,
            ..
        },
    ) = (&delete.action, &create.action)
    else {
        return false;
    };
    if deleted_path != created_path {
        return false;
    }
    !baseline.contains_seen_node_id(created_node_id)
}

pub(super) fn baseline_file_matches(
    baseline: &NodeLifecycleState,
    node_id: NodeId,
    kind: NodeKind,
    blob_id: prikk_object::ObjectId,
    mode: u32,
) -> bool {
    let Some(live) = baseline.live_node(&node_id) else {
        return false;
    };
    if live.kind != kind {
        return false;
    }
    matches!(
        &live.content,
        NodeContent::File {
            blob_id: live_blob,
            mode: live_mode,
        } if *live_blob == blob_id && *live_mode == mode
    )
}

pub(super) fn baseline_text_blob_for_mode(
    baseline: &NodeLifecycleState,
    node_id: NodeId,
    mode: u32,
) -> Option<ObjectId> {
    let live = baseline.live_node(&node_id)?;
    if live.kind != NodeKind::TextFile {
        return None;
    }
    match &live.content {
        NodeContent::File {
            blob_id,
            mode: live_mode,
        } if *live_mode == mode => Some(*blob_id),
        _ => None,
    }
}
