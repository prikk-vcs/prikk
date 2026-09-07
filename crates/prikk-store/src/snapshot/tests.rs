//! Snapshot path-safety and checkout-plan tests.

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope,
    ObjectType,
};

use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath, RepositoryLayout,
    SnapshotEntry, SnapshotManifest, prepare_snapshot_checkout_plan,
};

use crate::test_gates::test_support::{
    maintainer_signature, signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};
#[cfg(not(target_os = "windows"))]
use crate::worktree::materialize_manifest_entries;

#[test]
fn repo_path_rejects_traversal_and_reserved_names() {
    assert!(RepoPath::parse("src/main.rs").is_ok());
    assert!(RepoPath::parse("../escape").is_err());
    assert!(RepoPath::parse("src/../escape").is_err());
    assert!(RepoPath::parse("/absolute").is_err());
    assert!(RepoPath::parse("CON.txt").is_err());
    assert!(RepoPath::parse("src\\main.rs").is_err());
    assert!(RepoPath::parse("日本語.txt").is_err());
}

/// DC-96 Windows Anchor Identity, option 2 (`.git-exclude/reviewed/DC-96-ancestor-rename-ruling-v1.md`
/// §2-§3): restated criterion 1 allows a platform split only where the Windows branch asserts
/// something *at least as strong* as Linux/macOS's, the difference is documented in
/// `platform-support.md`, and the Linux/macOS branch is unchanged. All three hold here.
///
/// **On Windows the rename itself is now refused by the OS**, not merely handled correctly if it
/// happened: `layout` retains a handle on `root/.prikk` (`RepositoryLayout::init`), and NTFS
/// refuses to rename a directory that contains any open handle, unconditionally — confirmed against
/// Microsoft's own `FILE_RENAME_INFORMATION` reference and against this exact scenario on real
/// Windows CI. So the attack this test simulates (rename the root, plant an impostor) cannot be
/// *constructed* on Windows while a prikk command holds the repository open. That is a stronger
/// guarantee than "constructed, then handled correctly," which is what the rest of this test (run
/// on Linux/macOS, where the retained file descriptor makes the rename itself succeed) still
/// verifies, unchanged below.
#[test]
fn worktree_checks_and_writes_remain_on_retained_root() -> prikk_error::Result<()> {
    let root = unique_temp_dir("worktree-operation-root-replacement");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::write(root.join("conflict.txt"), b"original")?;
    let displaced = root.with_extension("displaced");
    let rename_result = std::fs::rename(&root, &displaced);

    #[cfg(target_os = "windows")]
    {
        // `layout` is never called on this branch, but it must stay alive (not be dropped) through
        // the rename attempt above -- its retained `.prikk` handle is *why* the rename fails. This
        // reference is only to satisfy the unused-variable lint; ordinary scope rules already keep
        // it alive until here regardless.
        let _ = &layout;
        // DC-96 option2-ruling-v1 §3: assert the specific refusal, not merely "some error" -- a
        // vacuous pass (e.g. the temp path colliding, or a transient unrelated I/O error) would
        // satisfy a bare `is_err()` without the guarantee this test exists to pin actually holding.
        let error_kind = rename_result.as_ref().err().map(std::io::Error::kind);
        assert_eq!(
            error_kind,
            Some(std::io::ErrorKind::PermissionDenied),
            "NTFS must refuse to rename a directory containing an open handle with the specific \
             access-denied error, not merely any error -- `layout` retains one on `.prikk`, so \
             this root-swap must be impossible to construct while a prikk command holds the \
             repository open, not merely detected after the fact: {rename_result:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        rename_result?;
        std::fs::create_dir(&root)?;

        let conflict = SnapshotManifest {
            files: vec![SnapshotEntry {
                path: RepoPath::parse("conflict.txt")?,
                bytes: b"replacement".to_vec(),
            }],
        };
        assert!(materialize_manifest_entries(&layout, &conflict).is_err());
        assert_eq!(std::fs::read(displaced.join("conflict.txt"))?, b"original");
        assert!(!root.join("conflict.txt").exists());

        let new_file = SnapshotManifest {
            files: vec![SnapshotEntry {
                path: RepoPath::parse("new.txt")?,
                bytes: b"retained-root".to_vec(),
            }],
        };
        assert!(materialize_manifest_entries(&layout, &new_file).is_ok());
        assert_eq!(std::fs::read(displaced.join("new.txt"))?, b"retained-root");
        assert!(!root.join("new.txt").exists());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(displaced);
        Ok(())
    }
}

#[test]
fn snapshot_manifest_rejects_case_collisions() {
    let upper = RepoPath::parse("README.md");
    let lower = RepoPath::parse("readme.md");
    assert!(upper.is_ok());
    assert!(lower.is_ok());
    if let (Ok(upper), Ok(lower)) = (upper, lower) {
        let manifest = SnapshotManifest {
            files: vec![
                SnapshotEntry {
                    path: upper,
                    bytes: b"a".to_vec(),
                },
                SnapshotEntry {
                    path: lower,
                    bytes: b"b".to_vec(),
                },
            ],
        };
        assert!(manifest.encode().is_err());
    }
}

#[test]
fn snapshot_checkout_plan_validates_snapshot_manifest() {
    let root = unique_temp_dir("snapshot-plan");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let mut object_store = FileObjectStore::new(layout.clone());
        let path = match RepoPath::parse("src/main.rs") {
            Ok(path) => path,
            Err(err) => panic!("test path should validate: {err}"),
        };
        let manifest = SnapshotManifest {
            files: vec![SnapshotEntry {
                path,
                bytes: b"fn main() {}\n".to_vec(),
            }],
        };
        let snapshot_bytes = manifest.encode();
        assert!(snapshot_bytes.is_ok());
        let blob = BlobPayload::new(BlobKind::Snapshot, snapshot_bytes.unwrap_or_default());
        let blob_bytes = blob.to_canonical_bytes();
        assert!(blob_bytes.is_ok());
        let mut blob_envelope =
            ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_bytes.unwrap_or_default());
        assert!(blob_envelope.add_signature(maintainer_signature()).is_ok());
        let blob_id = blob_envelope.object_id();
        assert!(object_store.write_object(&blob_envelope).is_ok());

        let block = signed_snapshot_block_envelope(blob_id);
        let block_id = block.object_id();
        assert!(object_store.write_object(&block).is_ok());

        let ref_store = RefStore::new(layout.clone());
        let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
        let ref_state_id = ref_state.object_id();
        let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
        let publication = RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        };
        assert!(ref_store.publish(&publication).is_ok());

        let plan = prepare_snapshot_checkout_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.file_count, 1);
            assert_eq!(plan.total_content_bytes, 13);
            assert_eq!(plan.paths, vec!["src/main.rs".to_string()]);
            assert_eq!(plan.snapshot_blob_id, blob_id);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

fn signed_snapshot_block_envelope(snapshot_blob_ref: prikk_object::ObjectId) -> ObjectEnvelope {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Normal,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([0_u8; 32]),
        snapshot_blob_ref: Some(snapshot_blob_ref),
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

#[test]
fn snapshot_materialization_writes_new_files() {
    let root = unique_temp_dir("snapshot-materialize");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let published = publish_snapshot_block(&layout, "src/main.rs", b"fn main() {}\n");
        assert!(published.is_ok());
        let report = crate::materialize_snapshot_checkout(&layout, "heads/main");
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.planned_files, 1);
            assert_eq!(report.written_files, 1);
            assert_eq!(report.unchanged_files, 0);
        }
        let written = std::fs::read(root.join("src").join("main.rs"));
        assert!(written.is_ok_and(|x| x == b"fn main() {}\n".to_vec()));
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn snapshot_materialization_is_idempotent_for_same_bytes() {
    let root = unique_temp_dir("snapshot-materialize-idempotent");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let published = publish_snapshot_block(&layout, "README.md", b"hello\n");
        assert!(published.is_ok());
        assert!(crate::materialize_snapshot_checkout(&layout, "heads/main").is_ok());
        let second = crate::materialize_snapshot_checkout(&layout, "heads/main");
        assert!(second.is_ok());
        if let Ok(second) = second {
            assert_eq!(second.written_files, 0);
            assert_eq!(second.unchanged_files, 1);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn snapshot_materialization_refuses_conflicting_existing_file() {
    let root = unique_temp_dir("snapshot-materialize-conflict");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let published = publish_snapshot_block(&layout, "README.md", b"snapshot\n");
        assert!(published.is_ok());
        let write = std::fs::write(root.join("README.md"), b"local\n");
        assert!(write.is_ok());
        let report = crate::materialize_snapshot_checkout(&layout, "heads/main");
        assert!(report.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn repo_path_rejects_metadata_directory() {
    assert!(RepoPath::parse(".prikk/FORMAT").is_err());
    assert!(RepoPath::parse(".PRIKK/FORMAT").is_err());
}

fn publish_snapshot_block(
    layout: &RepositoryLayout,
    path: &str,
    bytes: &[u8],
) -> prikk_error::Result<prikk_object::ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let path = RepoPath::parse(path)?;
    let manifest = SnapshotManifest {
        files: vec![SnapshotEntry {
            path,
            bytes: bytes.to_vec(),
        }],
    };
    let snapshot_bytes = manifest.encode()?;
    let blob = BlobPayload::new(BlobKind::Snapshot, snapshot_bytes);
    let blob_bytes = blob.to_canonical_bytes()?;
    let mut blob_envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_bytes);
    blob_envelope.add_signature(maintainer_signature())?;
    let blob_id = blob_envelope.object_id();
    object_store.write_object(&blob_envelope)?;

    let block = signed_snapshot_block_envelope(blob_id);
    let block_id = block.object_id();
    object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    let publication = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    };
    ref_store.publish(&publication)?;
    Ok(block_id)
}
