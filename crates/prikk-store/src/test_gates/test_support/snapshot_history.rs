//! The shared snapshot fixture (RFC 136 §10.1a): a two-block history on `heads/main` whose snapshot
//! is the carrying block's own state, after that block's patches. It replaces three copies of a v1
//! fixture whose snapshot was the pre-state of its block.
//!
//! The root block creates `README.md` and `old.txt`; the tip block deletes `old.txt` and creates
//! `extra.txt`. Both blocks have patches, so a reader that applied the carrying block's patches on
//! top of its snapshot would refuse, and one that ignored the snapshot would still agree -- the
//! meaning is tested either way. [`SnapshotAt`] picks the carrying block.

use prikk_error::PrikkError;
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, CanonicalEncode, CreateFile, DeleteNode, DeleteNodePreimage,
    MerkleRoot, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind,
    PatchPayload, PatchPurpose,
};

use super::{
    dummy_signature, maintainer_signature, signed_block_with_state_root, signed_ref_state_envelope,
    signed_ref_update_envelope, write_blob,
};
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath, RepositoryLayout,
    SnapshotManifest, StateRootContent, StateRootEntry,
};

/// Which block of the shared history carries the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotAt {
    /// The root block: readers seed its state, skip its patches, then replay the tip.
    Root,
    /// The tip block: the snapshot is the ref's own state.
    Tip,
}

/// The shared history with the snapshot on the root block. Every reader sees the same final state,
/// deletion and baseline the old v1 fixture produced.
pub(crate) fn publish_snapshot_then_patch_block(
    layout: &RepositoryLayout,
) -> prikk_error::Result<()> {
    publish_snapshot_history(layout, SnapshotAt::Root)
}

/// A text file's state entry at the regular mode.
pub(crate) fn text_entry(
    path: &str,
    node_seed: u8,
    blob_id: ObjectId,
) -> prikk_error::Result<StateRootEntry> {
    Ok(StateRootEntry {
        path: RepoPath::parse(path)?,
        node_id: NodeId::from_bytes([node_seed; 32]),
        kind: NodeKind::TextFile,
        mode: 0o100644,
        content: StateRootContent::Blob(blob_id),
    })
}

/// Write `content` as a `SNAPSHOT` Blob, whatever it holds -- for controls that need a manifest
/// which does not decode or does not recompute.
pub(crate) fn write_snapshot_content(
    store: &mut FileObjectStore,
    content: Vec<u8>,
) -> prikk_error::Result<ObjectId> {
    let blob = BlobPayload::new(BlobKind::Snapshot, content);
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    store.write_object(&envelope)
}

/// Write `entries` as a v2 manifest. The fixture's own guard: entries that do not recompute to
/// `state_root` are refused, so no test can start from a snapshot that lies about its block.
pub(crate) fn write_snapshot(
    store: &mut FileObjectStore,
    entries: Vec<StateRootEntry>,
    state_root: MerkleRoot,
) -> prikk_error::Result<ObjectId> {
    let manifest = SnapshotManifest { entries };
    if manifest.recomputed_state_root()? != state_root {
        return Err(PrikkError::Integrity(
            "fixture snapshot does not recompute to its block's state root".to_string(),
        ));
    }
    write_snapshot_content(store, manifest.encode()?)
}

/// Publish the shared history with the snapshot on `at`.
pub(crate) fn publish_snapshot_history(
    layout: &RepositoryLayout,
    at: SnapshotAt,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let readme_blob = write_blob(&mut object_store, b"hello\n")?;
    let old_blob = write_blob(&mut object_store, b"old\n")?;
    let extra_blob = write_blob(&mut object_store, b"extra\n")?;

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
    let root_snapshot = match at {
        SnapshotAt::Root => Some(write_snapshot(
            &mut object_store,
            vec![
                text_entry("README.md", 0x70, readme_blob)?,
                text_entry("old.txt", 0x71, old_blob)?,
            ],
            root_state,
        )?),
        SnapshotAt::Tip => None,
    };
    let root_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![root_patch_id],
        root_snapshot,
        root_state,
    );
    let root_block_id = object_store.write_object(&root_block)?;

    // ReplaceBinary replay is deferred to the node model, so this fixture uses
    // the supported DeleteNode plus CreateFile path.
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
    let tip_snapshot = match at {
        SnapshotAt::Tip => Some(write_snapshot(
            &mut object_store,
            vec![
                text_entry("README.md", 0x70, readme_blob)?,
                text_entry("extra.txt", 0x72, extra_blob)?,
            ],
            patch_state,
        )?),
        SnapshotAt::Root => None,
    };
    let patch_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![root_block_id],
        vec![patch_id],
        tip_snapshot,
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
