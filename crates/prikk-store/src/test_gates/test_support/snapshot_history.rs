use prikk_object::{
    BlobKind, BlobPayload, BlockKind, CanonicalEncode, CreateFile, DeleteNode, DeleteNodePreimage,
    NodeId, NodeKind, ObjectEnvelope, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose,
};

use super::{
    dummy_signature, maintainer_signature, signed_block_with_state_root, signed_ref_state_envelope,
    signed_ref_update_envelope, write_blob,
};
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath, RepositoryLayout,
    SnapshotEntry, SnapshotManifest,
};

pub(crate) fn publish_snapshot_then_patch_block(
    layout: &RepositoryLayout,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let readme_blob = write_blob(&mut object_store, b"hello\n")?;
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

    let root_patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id: NodeId::from_bytes([0x70; 32]),
                    blob_id: readme_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "old.txt".to_string(),
                    node_id: NodeId::from_bytes([0x71; 32]),
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut root_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        root_patch_payload.to_canonical_bytes()?,
    );
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

    // ReplaceBinary replay is deferred to the node model, so this fixture uses
    // the supported DeleteNode plus CreateFile path over the snapshot baseline.
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

    let patch_state =
        crate::derive_next_state_root(&object_store, Some(root_block_id), &[patch_id])?;
    let patch_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![root_block_id],
        vec![patch_id],
        Some(snapshot_blob_id),
        patch_state,
    );
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
