//! RFC 144 increment 1 fixtures: raw-patch-then-seal history containing `RenamePath`, since
//! `commit` never authors one (that stays true through increment 3). Two nodes created in a root
//! block, then a second block whose own patch carries the `RenamePath` operations under test --
//! mirrors `snapshot_history.rs`'s own two-block shape.

use prikk_object::{
    CanonicalEncode, CreateFile, NodeId, NodeKind, ObjectEnvelope, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose, RenamePath,
};

use super::{
    dummy_signature, signed_block_with_state_root, signed_ref_state_envelope,
    signed_ref_update_envelope, write_blob,
};
use prikk_object::BlockKind;

use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState};
use crate::state_root::{compute_state_root, entries_from_state};
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath, RepositoryLayout};

/// Two nodes, `a.txt` (node `0x91`) and `b.txt` (node `0x92`), created in a root block, then a
/// second block whose single patch renames both -- `a.txt -> b.txt` immediately followed by
/// `b.txt -> a.txt` -- a genuine cycle: applied one operation at a time, the first half's target
/// path is still occupied by the second half's own not-yet-moved node (RFC 144 §4h.7's
/// materialization analogue of the merge-ordering hazard). This is the fixture behind control 2.
///
/// The rename block's own state root is **not** computed via [`crate::derive_next_state_root`]:
/// that function walks the same `lifecycle_cache`-based sequential replay this fixture exists to
/// route around (`apply_candidate_patches`/`apply_one_block_with_text_cache`, both looping
/// `apply_state_effect` one operation at a time), so it fails on this exact cycle with the same
/// collision `apply_rename_batch` was written to dissolve -- a load-bearing finding in its own
/// right (see the round's own report). Built by hand instead: a fresh [`NodeLifecycleState`]
/// seeded directly with the two nodes at their *post-swap* paths (`create_node` never routes
/// through a rename check, so there is nothing here for the sequential bug to trip on), then
/// [`entries_from_state`]/[`compute_state_root`] over that -- the same function `patch_replay`'s
/// own read path never calls, so this bypass cannot mask a defect the controls below would catch.
pub(crate) fn publish_two_nodes_then_rename_cycle_block(
    layout: &RepositoryLayout,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_a = NodeId::from_bytes([0x91; 32]);
    let node_b = NodeId::from_bytes([0x92; 32]);
    let blob_a = write_blob(&mut object_store, b"content-a\n")?;
    let blob_b = write_blob(&mut object_store, b"content-b\n")?;

    let root_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "a.txt".to_string(),
                    node_id: node_a,
                    blob_id: blob_a,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "b.txt".to_string(),
                    node_id: node_b,
                    blob_id: blob_b,
                    mode: 0o100644,
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut root_patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, root_payload.to_canonical_bytes()?);
    root_patch.add_signature(dummy_signature())?;
    let root_patch_id = object_store.write_object(&root_patch)?;
    let root_state = crate::derive_next_state_root(&object_store, None, &[root_patch_id])?;
    let root_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![root_patch_id],
        None,
        root_state,
    );
    let root_block_id = object_store.write_object(&root_block)?;

    let rename_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id: node_a,
                    old_path: "a.txt".to_string(),
                    new_path: "b.txt".to_string(),
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id: node_b,
                    old_path: "b.txt".to_string(),
                    new_path: "a.txt".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut rename_patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, rename_payload.to_canonical_bytes()?);
    rename_patch.add_signature(dummy_signature())?;
    let rename_patch_id = object_store.write_object(&rename_patch)?;

    // Hand-built post-swap state -- see this function's own doc comment for why
    // `derive_next_state_root` cannot be used here.
    let mut post_swap_state = NodeLifecycleState::new();
    post_swap_state.create_node(
        node_a,
        LiveNode {
            path: RepoPath::parse("b.txt")?,
            kind: NodeKind::TextFile,
            content: NodeContent::File {
                blob_id: blob_a,
                mode: 0o100644,
            },
        },
    )?;
    post_swap_state.create_node(
        node_b,
        LiveNode {
            path: RepoPath::parse("a.txt")?,
            kind: NodeKind::TextFile,
            content: NodeContent::File {
                blob_id: blob_b,
                mode: 0o100644,
            },
        },
    )?;
    let rename_state = compute_state_root(&entries_from_state(&post_swap_state)?)?;

    let rename_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![root_block_id],
        vec![rename_patch_id],
        None,
        rename_state,
    );
    let rename_block_id = object_store.write_object(&rename_block)?;

    let ref_store = RefStore::new(layout.clone());
    let root_ref_state = signed_ref_state_envelope("heads/main", None, root_block_id, 1);
    let root_ref_state_id = root_ref_state.object_id();
    let root_ref_update =
        signed_ref_update_envelope("heads/main", None, root_ref_state_id, root_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: root_ref_state,
        ref_update: root_ref_update,
    })?;

    let rename_ref_state =
        signed_ref_state_envelope("heads/main", Some(root_ref_state_id), rename_block_id, 2);
    let rename_ref_state_id = rename_ref_state.object_id();
    let rename_ref_update = signed_ref_update_envelope(
        "heads/main",
        Some(root_ref_state_id),
        rename_ref_state_id,
        rename_block_id,
        2,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(root_ref_state_id),
        ref_state: rename_ref_state,
        ref_update: rename_ref_update,
    })?;
    Ok(())
}

/// Two nodes, `a.txt` (node `0x93`) and `c.txt` (node `0x94`), created in a root block, then a
/// second block whose single patch renames `a.txt -> c.txt` -- a genuine, structural collision:
/// `c.txt` is occupied by a live node (`0x94`) that is not part of this rename. This is the
/// fixture behind control 3: it must be detected, not silently applied.
///
/// The rename block's own state root is **not** computed via [`crate::derive_next_state_root`]
/// (see that function's own call site below for why): a real, non-cycle collision has no valid
/// post-state, and `derive_next_state_root` already refuses this exact patch itself, for the same
/// reason -- which is a genuinely correct refusal on its own path (sequential per-operation
/// application handles a real, non-cycle collision correctly; only the cycle case needs batch
/// resolution). This fixture exists to test `patch_replay`'s *own*, independent detection of the
/// same collision, so it must reach `patch_replay` at all -- hence the bypass.
pub(crate) fn publish_two_nodes_then_rename_onto_occupied_path_block(
    layout: &RepositoryLayout,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_a = NodeId::from_bytes([0x93; 32]);
    let node_c = NodeId::from_bytes([0x94; 32]);
    let blob_a = write_blob(&mut object_store, b"content-a\n")?;
    let blob_c = write_blob(&mut object_store, b"content-c\n")?;

    let root_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "a.txt".to_string(),
                    node_id: node_a,
                    blob_id: blob_a,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "c.txt".to_string(),
                    node_id: node_c,
                    blob_id: blob_c,
                    mode: 0o100644,
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut root_patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, root_payload.to_canonical_bytes()?);
    root_patch.add_signature(dummy_signature())?;
    let root_patch_id = object_store.write_object(&root_patch)?;
    let root_state = crate::derive_next_state_root(&object_store, None, &[root_patch_id])?;
    let root_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![root_patch_id],
        None,
        root_state,
    );
    let root_block_id = object_store.write_object(&root_block)?;

    let rename_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id: node_a,
                old_path: "a.txt".to_string(),
                new_path: "c.txt".to_string(),
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut rename_patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, rename_payload.to_canonical_bytes()?);
    rename_patch.add_signature(dummy_signature())?;
    let rename_patch_id = object_store.write_object(&rename_patch)?;
    // Not `derive_next_state_root`: a genuine, structural collision has no valid post-state to
    // derive a root from -- `derive_next_state_root` itself refuses this patch for exactly the
    // same reason `patch_replay`'s own apply is meant to (a real, non-cycle collision, which
    // sequential per-operation application already detects correctly, unlike the cycle case).
    // `patch_replay`'s read path never re-verifies a block's state root against replayed content
    // (that is `verify`'s job, a wholly separate lane -- see `read.rs`), so any syntactically
    // valid `MerkleRoot` here is sufficient to exercise `patch_replay`'s *own* defense-in-depth
    // detection of this collision, independent of `derive_next_state_root` already refusing to
    // let it be sealed through the ordinary path.
    let rename_state = root_state;
    let rename_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![root_block_id],
        vec![rename_patch_id],
        None,
        rename_state,
    );
    let rename_block_id = object_store.write_object(&rename_block)?;

    let ref_store = RefStore::new(layout.clone());
    let root_ref_state = signed_ref_state_envelope("heads/main", None, root_block_id, 1);
    let root_ref_state_id = root_ref_state.object_id();
    let root_ref_update =
        signed_ref_update_envelope("heads/main", None, root_ref_state_id, root_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: root_ref_state,
        ref_update: root_ref_update,
    })?;

    let rename_ref_state =
        signed_ref_state_envelope("heads/main", Some(root_ref_state_id), rename_block_id, 2);
    let rename_ref_state_id = rename_ref_state.object_id();
    let rename_ref_update = signed_ref_update_envelope(
        "heads/main",
        Some(root_ref_state_id),
        rename_ref_state_id,
        rename_block_id,
        2,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(root_ref_state_id),
        ref_state: rename_ref_state,
        ref_update: rename_ref_update,
    })?;
    Ok(())
}
