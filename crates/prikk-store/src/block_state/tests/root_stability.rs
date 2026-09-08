//! RFC 144 §4j / increment 2 — control 3: no existing state root changes. §2 of the increment 2
//! handoff predicts this trivially (no sealed history anywhere contains a `RenamePath`, so nothing
//! this round touches can be reached by rename-free replay), but the handoff's own instruction is to
//! prove it, not cite it. This exercises every non-rename operation kind `derive_next_state_root`
//! ever folds (`CreateFile`, `ChangePerm`, `ReplaceBinary`, `DeleteNode`, `EditText`) across a
//! three-block chain and asserts the resulting roots against values captured by running this exact
//! construction against the pre-round code (`git stash` back to the commit before this round, add
//! only this file, run, record the hex, restore) -- confirmed byte-identical, not merely expected to
//! be.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, ChangePerm, CreateFile,
    DeleteNode, DeleteNodePreimage, EditText, NodeId, NodeKind, ObjectEnvelope, ObjectId,
    ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, ReplaceBinary,
};

use crate::block_state::{LineageStateMemo, verify_block_v2_state};
use crate::object_store::ObjectReader;
use crate::test_gates::test_support::{dummy_signature, signed_block_with_state_root, write_blob};
use crate::{FileObjectStore, ObjectWriter, RepositoryLayout, derive_next_state_root};

fn write_patch(store: &mut FileObjectStore, operations: Vec<Operation>) -> prikk_object::ObjectId {
    write_patch_with_schema(store, operations, 1)
}

fn write_patch_with_schema(
    store: &mut FileObjectStore,
    operations: Vec<Operation>,
    schema_version: u32,
) -> prikk_object::ObjectId {
    let payload = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        schema_version,
        payload.to_canonical_bytes().unwrap(),
    );
    envelope.add_signature(dummy_signature()).unwrap();
    store.write_object(&envelope).unwrap()
}

/// Hex `state_merkle_root` values after each of the three blocks below, captured by running this
/// exact construction against `main` before this round (stash this round's changes, add only this
/// file plus its `mod` registration, run with `--nocapture`, record, restore).
const ROOT_AFTER_BLOCK_1: &str = "6b120ff4e0e6e812b051c295525b802b0bc25c4b6ef59b12d8d438e91f866d05";
const ROOT_AFTER_BLOCK_2: &str = "458a3498dcd68c4384188e2032c1f7fa31042335561f1027ef0a0433587b3c9d";
const ROOT_AFTER_BLOCK_3: &str = "ff9cdfe0b5371906758ef0a48646813db9c4400d3d9d0b06170886905fc8a6b1";

/// Build the shared three-block, rename-free chain: `CreateFile` x3, then `ChangePerm` +
/// `ReplaceBinary` + `DeleteNode`, then `EditText`. Returns the store, the temp root (caller must
/// clean it up), the three derived roots, and the three sealed block ids.
fn build_chain() -> (
    FileObjectStore,
    std::path::PathBuf,
    RepositoryLayout,
    [prikk_object::MerkleRoot; 3],
    [ObjectId; 3],
) {
    let root = crate::test_gates::test_support::unique_temp_dir("root-stability");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let mut store = FileObjectStore::new(layout.clone());

    let text_blob = write_blob(&mut store, b"probe text\n").unwrap();
    let binary_payload = BlobPayload::new(BlobKind::Binary, b"\x00\x01old-binary".to_vec());
    let mut binary_envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        binary_payload.to_canonical_bytes().unwrap(),
    );
    binary_envelope.add_signature(dummy_signature()).unwrap();
    let binary_blob = store.write_object(&binary_envelope).unwrap();

    let node_text = NodeId::from_bytes([0xA1; 32]);
    let node_binary = NodeId::from_bytes([0xA2; 32]);
    let node_deleted = NodeId::from_bytes([0xA3; 32]);

    // Block 1 (root): create three nodes.
    let p1 = write_patch(
        &mut store,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "text.txt".to_string(),
                    node_id: node_text,
                    blob_id: text_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "binary.bin".to_string(),
                    node_id: node_binary,
                    blob_id: binary_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 3,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "gone.txt".to_string(),
                    node_id: node_deleted,
                    blob_id: text_blob,
                    mode: 0o100644,
                }),
            },
        ],
    );
    let r1 = derive_next_state_root(&store, None, &[p1]).unwrap();
    let b1 = signed_block_with_state_root(BlockKind::Root, Vec::new(), vec![p1], None, r1);
    let b1_id = store.write_object(&b1).unwrap();

    // Block 2: ChangePerm, ReplaceBinary, DeleteNode.
    let new_binary_payload =
        BlobPayload::new(BlobKind::Binary, b"\x00\x01new-binary-longer".to_vec());
    let mut new_binary_envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        new_binary_payload.to_canonical_bytes().unwrap(),
    );
    new_binary_envelope
        .add_signature(dummy_signature())
        .unwrap();
    let new_binary_blob = store.write_object(&new_binary_envelope).unwrap();

    let p2 = write_patch(
        &mut store,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ChangePerm(ChangePerm {
                    node_id: node_text,
                    old_mode: 0o100644,
                    new_mode: 0o100755,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ReplaceBinary(ReplaceBinary {
                    node_id: node_binary,
                    old_blob_id: binary_blob,
                    new_blob_id: new_binary_blob,
                }),
            },
            Operation {
                op_seq: 3,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::DeleteNode(DeleteNode {
                    path: "gone.txt".to_string(),
                    node_id: node_deleted,
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: text_blob,
                        old_mode: 0o100644,
                    },
                }),
            },
        ],
    );
    let r2 = derive_next_state_root(&store, Some(b1_id), &[p2]).unwrap();
    let b2 = signed_block_with_state_root(BlockKind::Normal, vec![b1_id], vec![p2], None, r2);
    let b2_id = store.write_object(&b2).unwrap();

    // Block 3: EditText.
    let span =
        crate::text_span::plan_authored_text_span(b"probe text\n", b"probe TEXT\n", node_text)
            .unwrap()
            .unwrap();
    let p3 = write_patch_with_schema(
        &mut store,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::EditText(EditText {
                node_id: node_text,
                span_id: span.span_id,
                old_span_hash: span.old_span_hash,
                left_anchor_hash: span.left_anchor_hash,
                right_anchor_hash: span.right_anchor_hash,
                replacement_text: span.replacement_text,
                presentation_hint_line: None,
                presentation_hint_column: None,
                old_span_text: span.old_span_text,
                left_anchor_len: Some(span.left_anchor_len),
                right_anchor_len: Some(span.right_anchor_len),
            }),
        }],
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
    );
    let r3 = derive_next_state_root(&store, Some(b2_id), &[p3]).unwrap();
    let b3 = signed_block_with_state_root(BlockKind::Normal, vec![b2_id], vec![p3], None, r3);
    let b3_id = store.write_object(&b3).unwrap();

    (store, root, layout, [r1, r2, r3], [b1_id, b2_id, b3_id])
}

#[test]
fn rename_free_root_stability() {
    let (_store, root, _layout, [r1, r2, r3], _block_ids) = build_chain();

    assert_eq!(
        prikk_hash::to_hex(&r1.0),
        ROOT_AFTER_BLOCK_1,
        "block 1 (CreateFile x3) state root changed"
    );
    assert_eq!(
        prikk_hash::to_hex(&r2.0),
        ROOT_AFTER_BLOCK_2,
        "block 2 (ChangePerm, ReplaceBinary, DeleteNode) state root changed"
    );
    assert_eq!(
        prikk_hash::to_hex(&r3.0),
        ROOT_AFTER_BLOCK_3,
        "block 3 (EditText) state root changed"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Control 4: `verify` still passes over rename-free history -- the independent check on control 3.
/// `verify_block_v2_state` re-derives each block's state (through its own lineage-walk/memoization
/// path, not a bare `derive_next_state_root` call) and compares against the block's own recorded
/// `state_merkle_root`; every block in the chain must verify clean.
#[test]
fn rename_free_history_verifies() {
    let (store, root, _layout, _roots, [b1_id, b2_id, b3_id]) = build_chain();

    let mut memo = LineageStateMemo::new();
    for block_id in [b1_id, b2_id, b3_id] {
        let envelope = store.read_object(block_id).unwrap().unwrap();
        let payload = BlockPayload::decode_canonical(&envelope.canonical_payload).unwrap();
        verify_block_v2_state(&store, block_id, &payload, &mut memo)
            .unwrap_or_else(|err| panic!("block {block_id} failed verification: {err}"));
    }

    let _ = std::fs::remove_dir_all(root);
}
