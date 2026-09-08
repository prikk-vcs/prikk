//! Patch inverse planning tests.

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, DeleteNode,
    DeleteNodePreimage, EditText, MerkleRoot, NodeId, NodeKind, ObjectEnvelope, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose,
};

use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath, RepositoryLayout,
    SnapshotEntry, SnapshotManifest, prepare_patch_inverse_plan,
};

use crate::test_gates::test_support::{
    dummy_signature, maintainer_signature, publish_text_create_then_edit_block,
    publish_text_edit_then_rename_path_block, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};

#[test]
fn inverse_plan_reverses_supported_file_operations() {
    let root = unique_temp_dir("patch-inverse-file-ops");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_snapshot_then_patch_block(&layout);
        assert!(result.is_ok());
        let plan = prepare_patch_inverse_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.block_count, 2);
            assert_eq!(plan.patch_count, 1);
            assert_eq!(plan.original_operation_count, 2);
            assert_eq!(plan.inverse_operation_count, 2);
            let labels: Vec<&str> = plan
                .operations
                .iter()
                .map(|operation| operation.kind.as_str())
                .collect();
            assert_eq!(labels, vec!["delete-file", "create-file"]);
            let paths: Vec<&str> = plan
                .operations
                .iter()
                .map(|operation| operation.path.as_str())
                .collect();
            assert_eq!(paths, vec!["extra.txt", "old.txt"]);
            let seqs: Vec<u32> = plan
                .operations
                .iter()
                .map(|operation| operation.op_seq)
                .collect();
            assert_eq!(seqs, vec![1, 2]);
            assert_eq!(plan.inverse_payload.operations.len(), 2);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

fn publish_snapshot_then_patch_block(layout: &RepositoryLayout) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let old_blob = write_blob(&mut object_store, b"old\n")?;
    let extra_blob = write_blob(&mut object_store, b"extra\n")?;

    let snapshot_manifest = SnapshotManifest {
        files: vec![
            SnapshotEntry {
                path: RepoPath::parse("README.md")?,
                bytes: b"hello\n".to_vec(),
            },
            SnapshotEntry {
                path: RepoPath::parse("old.txt")?,
                bytes: b"old\n".to_vec(),
            },
        ],
    };
    let snapshot_blob = BlobPayload::new(BlobKind::Snapshot, snapshot_manifest.encode()?);
    let snapshot_bytes = snapshot_blob.to_canonical_bytes()?;
    let mut snapshot_envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, snapshot_bytes);
    snapshot_envelope.add_signature(maintainer_signature())?;
    let snapshot_blob_id = object_store.write_object(&snapshot_envelope)?;

    let root_block = signed_block(
        BlockKind::Root,
        Vec::new(),
        Vec::new(),
        Some(snapshot_blob_id),
    );
    let root_block_id = object_store.write_object(&root_block)?;

    let patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::DeleteNode(DeleteNode {
                    path: "old.txt".to_string(),
                    node_id: NodeId::from_bytes([0x71; 32]),
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: old_blob,
                        old_mode: 0o100644,
                    },
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "extra.txt".to_string(),
                    node_id: NodeId::from_bytes([0x72; 32]),
                    blob_id: extra_blob,
                    mode: 0o100644,
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let patch_id = write_patch(&mut object_store, patch_payload)?;
    publish_root_then_patch_ref(layout, root_block_id, patch_id)
}

#[test]
fn inverse_plan_derives_arbitrary_span_edit_text_inverse() {
    let root = unique_temp_dir("patch-inverse-edit-text");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let published = publish_text_create_then_edit_block(
            &layout,
            b"alpha beta gamma\n",
            b"alpha BETA gamma\n",
        );
        assert!(published.is_ok());
        let plan = prepare_patch_inverse_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.original_operation_count, 2);
            assert_eq!(plan.inverse_operation_count, 2);
            let labels: Vec<&str> = plan
                .operations
                .iter()
                .map(|operation| operation.kind.as_str())
                .collect();
            assert_eq!(labels, vec!["edit-text", "delete-file"]);
            assert_eq!(
                plan.operations
                    .first()
                    .map(|operation| operation.path.as_str()),
                Some("README.md")
            );
            let first_operation = plan.inverse_payload.operations.first();
            assert!(first_operation.is_some());
            match first_operation.map(|operation| &operation.kind) {
                Some(OperationKind::EditText(edit)) => {
                    assert_eq!(edit.old_span_text, b"BETA");
                    assert_eq!(edit.replacement_text, b"beta");
                    assert!(edit.presentation_hint_line.is_none());
                    assert!(edit.presentation_hint_column.is_none());
                }
                other => panic!("expected inverse EditText, got {other:?}"),
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn direct_inverse_vectors_cover_text_span_shapes() {
    let vectors: &[(&[u8], &[u8])] = &[
        (b"alpha beta gamma\n", b"alpha BETA gamma\n"),
        (b"alpha gamma\n", b"alpha beta gamma\n"),
        (b"alpha beta gamma\n", b"alpha gamma\n"),
        (b"same same same\n", b"same SAME same\n"),
        (b"line1\r\nline2\r\n", b"line1\r\nLINE2\r\n"),
        ("caf\u{e9}\n".as_bytes(), "caf\u{e8}\n".as_bytes()),
        (
            "one \u{4e16}\u{754c} three\n".as_bytes(),
            "one \u{4e16}\u{754c}! three\n".as_bytes(),
        ),
        (b"a 1 b 2 c 3 d\n", b"a one b two c 3 d\n"),
    ];
    for (old, new) in vectors {
        assert_direct_inverse_round_trip(old, new);
    }
}

#[test]
fn inverse_plan_reverses_same_node_dependent_text_edits() {
    let root = unique_temp_dir("patch-inverse-two-edit-text");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let old = b"alpha beta gamma\n";
        let mid = b"alpha BETA gamma\n";
        let new = b"prefix alpha BETA gamma\n";
        assert!(publish_text_create_then_two_edit_block(&layout, old, mid, new).is_ok());
        let plan = prepare_patch_inverse_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.original_operation_count, 3);
            assert_eq!(plan.inverse_operation_count, 3);
            assert_eq!(
                plan.operations
                    .iter()
                    .map(|operation| operation.kind.as_str())
                    .collect::<Vec<_>>(),
                vec!["edit-text", "edit-text", "delete-file"]
            );

            let first_op = plan.inverse_payload.operations.first();
            let second_op = plan.inverse_payload.operations.get(1);
            assert!(first_op.is_some());
            assert!(second_op.is_some());
            if let (Some(first_op), Some(second_op)) = (first_op, second_op) {
                let first = apply_inverse_edit(first_op, new);
                assert_eq!(first, mid);
                let second = apply_inverse_edit(second_op, &first);
                assert_eq!(second, old);
                let wrong_first = try_apply_inverse_edit(second_op, new);
                assert!(
                    wrong_first.is_err() || wrong_first.as_deref() != Ok(old),
                    "forward-order inverse application must not restore the original text"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn inverse_plan_fails_closed_on_supported_text_plus_unsupported_operation() {
    let root = unique_temp_dir("patch-inverse-edit-text-unsupported");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let published = publish_text_edit_then_rename_path_block(&layout);
        assert!(published.is_ok());
        let plan = prepare_patch_inverse_plan(&layout, "heads/main");
        assert!(plan.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

fn write_patch(
    object_store: &mut FileObjectStore,
    payload: PatchPayload,
) -> prikk_error::Result<prikk_object::ObjectId> {
    write_patch_with_schema(object_store, payload, 1)
}

/// RFC 134 §8: a payload whose `EditText` operations carry v2 anchor-length fields must be written
/// at `PATCH_TEXT_SPAN_V2_SCHEMA` or decode will refuse tags 10/11 as unauthorized at this schema.
fn write_patch_with_schema(
    object_store: &mut FileObjectStore,
    payload: PatchPayload,
    schema_version: u32,
) -> prikk_error::Result<prikk_object::ObjectId> {
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        schema_version,
        payload.to_canonical_bytes()?,
    );
    patch.add_signature(dummy_signature())?;
    object_store.write_object(&patch)
}

fn publish_root_then_patch_ref(
    layout: &RepositoryLayout,
    root_block_id: prikk_object::ObjectId,
    patch_id: prikk_object::ObjectId,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let patch_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let patch_block_id = object_store.write_object(&patch_block)?;
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

    let patch_ref_state =
        signed_ref_state_envelope("heads/main", Some(root_ref_state_id), patch_block_id, 2);
    let patch_ref_state_id = patch_ref_state.object_id();
    let patch_ref_update = signed_ref_update_envelope(
        "heads/main",
        Some(root_ref_state_id),
        patch_ref_state_id,
        patch_block_id,
        2,
    );
    ref_store
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: Some(root_ref_state_id),
            ref_state: patch_ref_state,
            ref_update: patch_ref_update,
        })
        .map(|_object_id| ())
}

fn write_blob(
    store: &mut FileObjectStore,
    bytes: &[u8],
) -> prikk_error::Result<prikk_object::ObjectId> {
    let payload = BlobPayload::new(BlobKind::Text, bytes.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    store.write_object(&envelope)
}

fn assert_direct_inverse_round_trip(old: &[u8], new: &[u8]) {
    let node_id = NodeId::from_bytes([0x91; 32]);
    let span = match crate::text_span::plan_authored_text_span(old, new, node_id) {
        Ok(Some(span)) => span,
        other => panic!("expected authored span, got {other:?}"),
    };
    let inverse_result = crate::text_span::derive_inverse_edit_text(
        old,
        node_id,
        &span.span_id,
        &span.old_span_hash,
        &span.left_anchor_hash,
        &span.right_anchor_hash,
        &span.replacement_text,
        &span.old_span_text,
        Some(span.left_anchor_len),
        Some(span.right_anchor_len),
    );
    assert!(inverse_result.is_ok());
    let (inverse, post) = match inverse_result {
        Ok(value) => value,
        Err(err) => panic!("inverse derives: {err}"),
    };
    assert_eq!(post, new);
    let recovered = match apply_edit_text_bytes(new, &inverse) {
        Ok(value) => value,
        Err(err) => panic!("inverse applies: {err}"),
    };
    assert_eq!(recovered, old);
    let forward = EditText {
        node_id,
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
    };
    let round_trip = match apply_edit_text_bytes(&recovered, &forward) {
        Ok(value) => value,
        Err(err) => panic!("forward reapplies: {err}"),
    };
    assert_eq!(round_trip, new);
}

fn apply_inverse_edit(operation: &Operation, bytes: &[u8]) -> Vec<u8> {
    match try_apply_inverse_edit(operation, bytes) {
        Ok(value) => value,
        Err(err) => panic!("inverse operation applies: {err}"),
    }
}

fn try_apply_inverse_edit(operation: &Operation, bytes: &[u8]) -> prikk_error::Result<Vec<u8>> {
    match &operation.kind {
        OperationKind::EditText(edit) => apply_edit_text_bytes(bytes, edit),
        other => Err(prikk_error::PrikkError::Integrity(format!(
            "expected EditText operation, got {other:?}"
        ))),
    }
}

fn apply_edit_text_bytes(bytes: &[u8], edit: &EditText) -> prikk_error::Result<Vec<u8>> {
    let (start, end) = crate::text_span::resolve_text_span(
        bytes,
        &edit.old_span_text,
        &edit.left_anchor_hash,
        &edit.right_anchor_hash,
        &edit.span_id,
        edit.node_id,
        &edit.old_span_hash,
        edit.left_anchor_len,
        edit.right_anchor_len,
    )
    .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))?;
    crate::text_span::splice_text(bytes, start, end, &edit.replacement_text)
        .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))
}

fn publish_text_create_then_two_edit_block(
    layout: &RepositoryLayout,
    old: &[u8],
    mid: &[u8],
    new: &[u8],
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_id = NodeId::from_bytes([0x92; 32]);
    let old_blob = write_blob(&mut object_store, old)?;
    let first = crate::text_span::plan_authored_text_span(old, mid, node_id)
        .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("first edit unchanged".to_string()))?;
    let second = crate::text_span::plan_authored_text_span(mid, new, node_id)
        .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("second edit unchanged".to_string()))?;
    let patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id,
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::EditText(EditText {
                    node_id,
                    span_id: first.span_id,
                    old_span_hash: first.old_span_hash,
                    left_anchor_hash: first.left_anchor_hash,
                    right_anchor_hash: first.right_anchor_hash,
                    replacement_text: first.replacement_text,
                    presentation_hint_line: None,
                    presentation_hint_column: None,
                    old_span_text: first.old_span_text,
                    left_anchor_len: Some(first.left_anchor_len),
                    right_anchor_len: Some(first.right_anchor_len),
                }),
            },
            Operation {
                op_seq: 3,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::EditText(EditText {
                    node_id,
                    span_id: second.span_id,
                    old_span_hash: second.old_span_hash,
                    left_anchor_hash: second.left_anchor_hash,
                    right_anchor_hash: second.right_anchor_hash,
                    replacement_text: second.replacement_text,
                    presentation_hint_line: None,
                    presentation_hint_column: None,
                    old_span_text: second.old_span_text,
                    left_anchor_len: Some(second.left_anchor_len),
                    right_anchor_len: Some(second.right_anchor_len),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let patch_id = write_patch_with_schema(
        &mut object_store,
        patch_payload,
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
    )?;
    let block = signed_block(BlockKind::Root, Vec::new(), vec![patch_id], None);
    let block_id = object_store.write_object(&block)?;
    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        })
        .map(|_object_id| ())
}

fn signed_block(
    kind: BlockKind,
    parent_block_ids: Vec<prikk_object::ObjectId>,
    patch_ids: Vec<prikk_object::ObjectId>,
    snapshot_blob_ref: Option<prikk_object::ObjectId>,
) -> ObjectEnvelope {
    let payload = BlockPayload {
        parent_block_ids,
        kind,
        patch_ids,
        state_merkle_root: MerkleRoot([0_u8; 32]),
        snapshot_blob_ref,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload_bytes.unwrap_or_default());
    assert!(envelope.add_signature(maintainer_signature()).is_ok());
    envelope
}

/// Publish a snapshot containing a binary file, then a block whose only operation
/// deletes it as a `BINARY_FILE` node referencing a `BlobKind::Binary` blob. This
/// exercises inverse(DeleteNode) over a binary blob through the kind-aware
/// `ensure_blob_matches_node_kind` consume-path check (FDD-03 §9.3).
fn publish_binary_file_delete_block(layout: &RepositoryLayout) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let bin_bytes: Vec<u8> = vec![0x00, 0x01, 0xff, 0x10];
    let bin_blob = BlobPayload::new(BlobKind::Binary, bin_bytes.clone());
    let mut bin_envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, bin_blob.to_canonical_bytes()?);
    bin_envelope.add_signature(maintainer_signature())?;
    let bin_blob_id = object_store.write_object(&bin_envelope)?;

    let snapshot_manifest = SnapshotManifest {
        files: vec![SnapshotEntry {
            path: RepoPath::parse("data.bin")?,
            bytes: bin_bytes,
        }],
    };
    let snapshot_blob = BlobPayload::new(BlobKind::Snapshot, snapshot_manifest.encode()?);
    let mut snapshot_envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, snapshot_blob.to_canonical_bytes()?);
    snapshot_envelope.add_signature(maintainer_signature())?;
    let snapshot_blob_id = object_store.write_object(&snapshot_envelope)?;

    let root_block = signed_block(
        BlockKind::Root,
        Vec::new(),
        Vec::new(),
        Some(snapshot_blob_id),
    );
    let root_block_id = object_store.write_object(&root_block)?;

    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::DeleteNode(DeleteNode {
                path: "data.bin".to_string(),
                node_id: NodeId::from_bytes([0x81; 32]),
                old_node_kind: NodeKind::BinaryFile,
                preimage: DeleteNodePreimage::File {
                    old_blob_id: bin_blob_id,
                    old_mode: 0o100644,
                },
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let patch_id = write_patch(&mut object_store, patch_payload)?;
    publish_root_then_patch_ref(layout, root_block_id, patch_id)
}

#[test]
fn inverse_plan_reverses_binary_file_deletion() {
    let root = unique_temp_dir("patch-inverse-binary-delete");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_binary_file_delete_block(&layout).is_ok());
        let plan = prepare_patch_inverse_plan(&layout, "heads/main");
        assert!(
            plan.is_ok(),
            "binary-file DeleteNode inverse should succeed"
        );
        if let Ok(plan) = plan {
            assert_eq!(plan.original_operation_count, 1);
            assert_eq!(plan.inverse_operation_count, 1);
            // inverse(DeleteNode) reconstructs the file via CreateFile.
            assert_eq!(
                plan.operations.first().map(|op| op.kind.as_str()),
                Some("create-file")
            );
            assert_eq!(
                plan.operations.first().map(|op| op.path.as_str()),
                Some("data.bin")
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
