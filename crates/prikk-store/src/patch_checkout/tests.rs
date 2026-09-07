//! Patch checkout materialization tests.

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, DeleteNode,
    DeleteNodePreimage, MerkleRoot, NodeId, NodeKind, ObjectEnvelope, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath, RepositoryLayout,
    SnapshotEntry, SnapshotManifest, materialize_patch_checkout,
    materialize_patch_checkout_with_deletions, plan_patch_checkout_deletions,
};

use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::{
    dummy_signature, maintainer_signature, publish_text_create_then_edit_block,
    signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};

#[test]
fn patch_materialization_writes_replayed_files() {
    let root = unique_temp_dir("patch-materialize");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        let report = materialize_patch_checkout(&layout, "heads/main");
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.block_count, 2);
            assert_eq!(report.patch_count, 1);
            assert_eq!(report.applied_operation_count, 2);
            assert_eq!(report.planned_files, 2);
            assert_eq!(report.written_files, 2);
            assert_eq!(report.unchanged_files, 0);
        }
        assert!(std::fs::read(root.join("README.md")).is_ok_and(|x| x == b"hello\n".to_vec()));
        assert!(std::fs::read(root.join("extra.txt")).is_ok_and(|x| x == b"extra\n".to_vec()));
        assert!(!root.join("old.txt").exists());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_writes_arbitrary_span_edit_text_result() {
    let root = unique_temp_dir("patch-materialize-edit-text");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(
            publish_text_create_then_edit_block(&layout, b"hello world\n", b"hello prikk\n")
                .is_ok()
        );
        let report = materialize_patch_checkout(&layout, "heads/main");
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.applied_operation_count, 2);
            assert_eq!(report.planned_files, 1);
            assert_eq!(report.written_files, 1);
        }
        assert!(
            std::fs::read(root.join("README.md")).is_ok_and(|x| x == b"hello prikk\n".to_vec())
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_is_idempotent_for_same_bytes() {
    let root = unique_temp_dir("patch-materialize-idempotent");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(materialize_patch_checkout(&layout, "heads/main").is_ok());
        let second = materialize_patch_checkout(&layout, "heads/main");
        assert!(second.is_ok());
        if let Ok(second) = second {
            assert_eq!(second.written_files, 0);
            assert_eq!(second.unchanged_files, 2);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_refuses_conflicting_existing_file() {
    let root = unique_temp_dir("patch-materialize-conflict");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("README.md"), b"local\n").is_ok());
        let report = materialize_patch_checkout(&layout, "heads/main");
        assert!(report.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_deletion_plan_reports_safe_removed_files() {
    let root = unique_temp_dir("patch-delete-plan");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("old.txt"), b"old\n").is_ok());
        let plan = plan_patch_checkout_deletions(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.planned_deletions, 1);
            assert_eq!(plan.deletable_files, 1);
            assert_eq!(plan.already_absent_files, 0);
            assert!(plan.conflicts.is_empty());
            assert_eq!(plan.deletable_paths, vec!["old.txt".to_string()]);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_with_deletions_removes_matching_old_file() {
    let root = unique_temp_dir("patch-materialize-delete");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("old.txt"), b"old\n").is_ok());
        let report = materialize_patch_checkout_with_deletions(&layout, "heads/main");
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.deleted_files, 1);
            assert_eq!(report.already_absent_deleted_files, 0);
            assert_eq!(report.deletion_conflicts, 0);
        }
        assert!(std::fs::read(root.join("README.md")).is_ok_and(|x| x == b"hello\n".to_vec()));
        assert!(std::fs::read(root.join("extra.txt")).is_ok_and(|x| x == b"extra\n".to_vec()));
        assert!(!root.join("old.txt").exists());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_deletion_retry_resyncs_observed_absent_parent() -> prikk_error::Result<()> {
    let root = unique_temp_dir("patch-delete-cleanup-retry");
    let layout = RepositoryLayout::init(root.clone())?;
    publish_snapshot_then_patch_block(&layout)?;
    std::fs::write(root.join("old.txt"), b"old\n")?;

    fail_once_for_test(TestFailPoint::CleanupDirectorySync);
    assert!(materialize_patch_checkout_with_deletions(&layout, "heads/main").is_err());
    assert!(!root.join("old.txt").exists());
    fail_once_for_test(TestFailPoint::CleanupDirectorySync);
    assert!(materialize_patch_checkout_with_deletions(&layout, "heads/main").is_err());
    let report = materialize_patch_checkout_with_deletions(&layout, "heads/main")?;
    assert_eq!(report.deleted_files, 0);
    assert_eq!(report.already_absent_deleted_files, 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn patch_materialization_with_deletions_refuses_modified_removed_file() {
    let root = unique_temp_dir("patch-materialize-delete-conflict");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("old.txt"), b"local edit\n").is_ok());
        let report = materialize_patch_checkout_with_deletions(&layout, "heads/main");
        assert!(report.is_err());
        assert!(!root.join("README.md").exists());
        assert!(std::fs::read(root.join("old.txt")).is_ok_and(|x| x == b"local edit\n".to_vec()));
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
    let mut patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch_payload.to_canonical_bytes()?);
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;

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
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(root_ref_state_id),
        ref_state: patch_ref_state,
        ref_update: patch_ref_update,
    })?;
    Ok(())
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
