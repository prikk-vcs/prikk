use prikk_object::NodeId;

use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation,
};
use crate::path::RepoPath;

use super::types::{Action, OperationFacts, PathEffects, UnknownReason};

#[cfg(test)]
pub(crate) fn path_effects(
    operation: &DecodedPatchOperation,
) -> Result<PathEffects, UnknownReason> {
    operation_facts(operation).map(|facts| facts.path_effects)
}

pub(super) fn operation_facts(
    operation: &DecodedPatchOperation,
) -> Result<OperationFacts, UnknownReason> {
    let mut effects = PathEffects::default();
    let action = match &operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => {
            let path = parse_path(path)?;
            effects.required_free.insert(path.clone());
            effects.occupied_after.insert(path.clone());
            effects.newly_occupied.insert(path.clone());
            Action::CreateFile {
                path,
                node_id: *node_id,
                blob_id: *blob_id,
                mode: *mode,
            }
        }
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage,
        } => {
            let path = parse_path(path)?;
            effects.occupied_before.insert(path.clone());
            effects.freed.insert(path.clone());
            match preimage {
                DecodedDeletePreimage::File {
                    old_node_kind,
                    old_blob_id,
                    old_mode,
                } => Action::DeleteFile {
                    path,
                    node_id: *node_id,
                    old_node_kind: *old_node_kind,
                    old_blob_id: *old_blob_id,
                    old_mode: *old_mode,
                },
                DecodedDeletePreimage::Symlink { .. } => Action::DeleteSymlink {
                    path,
                    node_id: *node_id,
                },
            }
        }
        DecodedOperationKind::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            old_span_text,
            left_anchor_len,
            right_anchor_len,
            ..
        } => Action::EditText {
            node_id: *node_id,
            span_id: *span_id,
            old_span_hash: *old_span_hash,
            left_anchor_hash: *left_anchor_hash,
            right_anchor_hash: *right_anchor_hash,
            old_span_text: old_span_text.clone(),
            left_anchor_len: *left_anchor_len,
            right_anchor_len: *right_anchor_len,
        },
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => Action::ReplaceBinary {
            node_id: *node_id,
            old_blob_id: *old_blob_id,
            new_blob_id: *new_blob_id,
        },
        DecodedOperationKind::RenamePath {
            node_id,
            old_path,
            new_path,
        } => {
            let old_path = parse_path(old_path)?;
            let new_path = parse_path(new_path)?;
            effects.occupied_before.insert(old_path.clone());
            effects.freed.insert(old_path);
            effects.required_free.insert(new_path.clone());
            effects.occupied_after.insert(new_path.clone());
            effects.newly_occupied.insert(new_path.clone());
            Action::RenamePath {
                node_id: *node_id,
                new_path,
            }
        }
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => Action::ChangePerm {
            node_id: *node_id,
            old_mode: *old_mode,
            new_mode: *new_mode,
        },
        DecodedOperationKind::CreateSymlink {
            path,
            node_id,
            target: _,
        } => {
            let path = parse_path(path)?;
            effects.required_free.insert(path.clone());
            effects.occupied_after.insert(path.clone());
            effects.newly_occupied.insert(path.clone());
            Action::CreateSymlink {
                path,
                node_id: *node_id,
            }
        }
    };
    let node_id = action.node_id();
    Ok(OperationFacts {
        op_seq: operation.op_seq,
        node_id,
        action,
        path_effects: effects,
    })
}

fn parse_path(path: &str) -> Result<RepoPath, UnknownReason> {
    RepoPath::parse(path).map_err(|_| UnknownReason::MalformedOperation)
}

pub(super) fn deferred_reason(action: &Action) -> Option<UnknownReason> {
    match action {
        Action::RenamePath { .. } => Some(UnknownReason::RenameDeferred),
        Action::CreateSymlink { .. } | Action::DeleteSymlink { .. } => {
            Some(UnknownReason::SymlinkDeferred)
        }
        _ => None,
    }
}

impl Action {
    pub(super) fn node_id(&self) -> Option<NodeId> {
        match self {
            Self::CreateFile { node_id, .. }
            | Self::DeleteFile { node_id, .. }
            | Self::DeleteSymlink { node_id, .. }
            | Self::EditText { node_id, .. }
            | Self::ReplaceBinary { node_id, .. }
            | Self::RenamePath { node_id, .. }
            | Self::ChangePerm { node_id, .. }
            | Self::CreateSymlink { node_id, .. } => Some(*node_id),
        }
    }
}
