use prikk_object::{NodeId, ObjectId, text_span_hash};

use crate::node::node_lifecycle::{LiveNode, NodeContent};
use crate::patch_replay::decode::{DecodedDeletePreimage, DecodedPatchOperation};
use crate::path::RepoPath;
use crate::text_span;

use super::*;

pub(super) fn seed_binary(
    state: &mut NodeLifecycleState,
    node_id: NodeId,
    path_value: &str,
    blob_id: ObjectId,
    mode: u32,
) {
    state
        .seed_live_node(
            node_id,
            LiveNode {
                path: path(path_value),
                kind: NodeKind::BinaryFile,
                content: NodeContent::File { blob_id, mode },
            },
        )
        .expect("seed binary");
}

pub(super) fn seed_text(
    state: &mut NodeLifecycleState,
    node_id: NodeId,
    path_value: &str,
    text: &[u8],
    mode: u32,
) {
    state
        .seed_live_node(
            node_id,
            LiveNode {
                path: path(path_value),
                kind: NodeKind::TextFile,
                content: NodeContent::File {
                    blob_id: text_span::text_blob_id(text).expect("text blob id"),
                    mode,
                },
            },
        )
        .expect("seed text");
}

pub(super) fn create_file(
    op_seq: u32,
    path: &str,
    node_id: NodeId,
    blob_id: ObjectId,
    mode: u32,
) -> DecodedPatchOperation {
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::CreateFile {
            path: path.to_string(),
            node_id,
            blob_id,
            mode,
        },
    }
}

pub(super) fn delete_file(
    op_seq: u32,
    path: &str,
    node_id: NodeId,
    old_node_kind: NodeKind,
    old_blob_id: ObjectId,
    old_mode: u32,
) -> DecodedPatchOperation {
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::DeleteNode {
            path: path.to_string(),
            node_id,
            preimage: DecodedDeletePreimage::File {
                old_node_kind,
                old_blob_id,
                old_mode,
            },
        },
    }
}

pub(super) fn change_perm(
    op_seq: u32,
    node_id: NodeId,
    old_mode: u32,
    new_mode: u32,
) -> DecodedPatchOperation {
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        },
    }
}

pub(super) fn replace_binary(
    op_seq: u32,
    node_id: NodeId,
    old_blob_id: ObjectId,
    new_blob_id: ObjectId,
) -> DecodedPatchOperation {
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        },
    }
}

pub(super) fn rename_path(
    op_seq: u32,
    node_id: NodeId,
    old_path: &str,
    new_path: &str,
) -> DecodedPatchOperation {
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::RenamePath {
            node_id,
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
        },
    }
}

pub(super) fn create_symlink(
    op_seq: u32,
    path: &str,
    node_id: NodeId,
    target: &str,
) -> DecodedPatchOperation {
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::CreateSymlink {
            path: path.to_string(),
            node_id,
            target: target.to_string(),
        },
    }
}

pub(super) fn edit_text(
    op_seq: u32,
    node_id: NodeId,
    old_text: &[u8],
    new_text: &[u8],
) -> DecodedPatchOperation {
    // RFC 134 §8 handoff trap #2: stays v1 (plan_authored_text_span_v1), deliberately, so this
    // generator keeps building v1-shaped operations. Do not switch this to the v2-authoring
    // plan_authored_text_span -- that is a separate, deliberate future step for the property
    // generator, not a side effect of this increment.
    let plan = text_span::plan_authored_text_span_v1(old_text, new_text, node_id)
        .expect("text span plan")
        .expect("changed text");
    DecodedPatchOperation {
        op_seq,
        kind: DecodedOperationKind::EditText {
            node_id,
            span_id: plan.span_id,
            old_span_hash: text_span_hash(&plan.old_span_text),
            left_anchor_hash: plan.left_anchor_hash,
            right_anchor_hash: plan.right_anchor_hash,
            replacement_text: plan.replacement_text,
            old_span_text: plan.old_span_text,
            left_anchor_len: None,
            right_anchor_len: None,
        },
    }
}

pub(super) fn node(byte: u8) -> NodeId {
    NodeId::from_bytes([byte; 32])
}

pub(super) fn blob(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

pub(super) fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).expect("repo path")
}
