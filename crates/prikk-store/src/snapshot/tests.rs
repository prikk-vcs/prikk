//! Snapshot path-safety, v2 manifest, and snapshot-checkout tests.

mod readers;
mod writer;

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose,
};

#[cfg(not(target_os = "windows"))]
use crate::test_gates::test_support::text_replay_manifest;
use crate::test_gates::test_support::{
    dummy_signature, signed_block_with_state_root, signed_ref_state_envelope,
    signed_ref_update_envelope, text_entry, unique_temp_dir, write_blob, write_snapshot,
};
#[cfg(not(target_os = "windows"))]
use crate::worktree::materialize_replay_manifest_entries;
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath, RepositoryLayout,
    SnapshotManifest, prepare_snapshot_checkout_plan,
};

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

        let conflict = text_replay_manifest("conflict.txt", b"replacement")?;
        assert!(materialize_replay_manifest_entries(&layout, &conflict).is_err());
        assert_eq!(std::fs::read(displaced.join("conflict.txt"))?, b"original");
        assert!(!root.join("conflict.txt").exists());

        let new_file = text_replay_manifest("new.txt", b"retained-root")?;
        assert!(materialize_replay_manifest_entries(&layout, &new_file).is_ok());
        assert_eq!(std::fs::read(displaced.join("new.txt"))?, b"retained-root");
        assert!(!root.join("new.txt").exists());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(displaced);
        Ok(())
    }
}

#[test]
fn snapshot_manifest_rejects_case_collisions() -> prikk_error::Result<()> {
    let blob = ObjectId::from_bytes([0x11; 32]);
    let manifest = SnapshotManifest {
        entries: vec![
            text_entry("README.md", 0x01, blob)?,
            text_entry("readme.md", 0x02, blob)?,
        ],
    };
    assert!(manifest.encode().is_err());
    Ok(())
}

#[test]
fn snapshot_manifest_round_trips_through_the_leaf_grammar() -> prikk_error::Result<()> {
    let manifest = SnapshotManifest {
        entries: vec![
            text_entry("README.md", 0x01, ObjectId::from_bytes([0x11; 32]))?,
            text_entry("src/main.rs", 0x02, ObjectId::from_bytes([0x22; 32]))?,
        ],
    };
    let bytes = manifest.encode()?;
    assert!(bytes.starts_with(b"PRIKK-SNAPSHOT-MANIFEST-v2\n"));
    let decoded = SnapshotManifest::decode(&bytes)?;
    assert_eq!(decoded, manifest);
    assert_eq!(
        decoded.recomputed_state_root()?,
        crate::compute_state_root(&manifest.entries)?
    );
    Ok(())
}

#[test]
fn snapshot_checkout_plan_reads_the_blocks_own_state() -> prikk_error::Result<()> {
    let root = unique_temp_dir("snapshot-plan");
    let layout = RepositoryLayout::init(root.clone())?;
    let snapshot_blob_id = publish_snapshot_block(&layout, "src/main.rs", b"fn main() {}\n")?;
    let plan = prepare_snapshot_checkout_plan(&layout, "heads/main")?;
    assert_eq!(plan.file_count, 1);
    assert_eq!(plan.total_content_bytes, 13);
    assert_eq!(plan.paths, vec!["src/main.rs".to_string()]);
    assert_eq!(plan.snapshot_blob_id, snapshot_blob_id);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
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

/// DC-87: the same idempotency, on a platform with **no observable POSIX mode** — where the mode
/// comparison that decides `Unchanged` has nothing to compare.
///
/// This is the Windows failure, reproduced on Linux through the observation seam rather than a `cfg`,
/// so the control exercises the code Windows runs. Before the fix, the second materialization set the
/// mode again and reported `written_files: 1` for a file whose bytes already matched.
#[test]
fn snapshot_materialization_is_idempotent_without_an_observable_mode() {
    let root = unique_temp_dir("snapshot-materialize-idempotent-no-mode");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let published = publish_snapshot_block(&layout, "README.md", b"hello\n");
        assert!(published.is_ok());
        assert!(crate::materialize_snapshot_checkout(&layout, "heads/main").is_ok());
        let second = crate::worktree::with_unobservable_mode_for_test(|| {
            crate::materialize_snapshot_checkout(&layout, "heads/main")
        });
        assert!(second.is_ok());
        if let Ok(second) = second {
            assert_eq!(
                second.written_files, 0,
                "identical bytes are not a write, with or without an observable mode"
            );
            assert_eq!(second.unchanged_files, 1);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// DC-87: a file that disappears between its bytes being read and its mode being stat'd is the
/// changed-during-the-checkout case, not "this platform has no mode". Both arrive as a `None`, and
/// only the inner one may be read as unchanged.
#[cfg(target_os = "linux")]
#[test]
fn a_file_removed_mid_entry_is_refused_not_reported_unchanged() {
    let root = unique_temp_dir("snapshot-materialize-vanished");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let published = publish_snapshot_block(&layout, "README.md", b"hello\n");
        assert!(published.is_ok());
        assert!(crate::materialize_snapshot_checkout(&layout, "heads/main").is_ok());
        let target = root.join("README.md");
        crate::worktree::before_stat_for_test(move || {
            let _ = std::fs::remove_file(&target);
        });
        let second = crate::materialize_snapshot_checkout(&layout, "heads/main");
        match second {
            Err(prikk_error::PrikkError::Precondition(message)) => {
                assert!(message.contains("changed during the checkout"), "{message}")
            }
            other => panic!("expected the changed-during-the-checkout precondition, got {other:?}"),
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

/// A root block whose one patch creates `path`, carrying a snapshot of its own post-patch state
/// (RFC 136 §10.1a). The snapshot block has a patch, so a reader that applied it on top of the
/// snapshot would refuse -- a fixture on which both meanings agree would test neither. Returns the
/// snapshot Blob id.
fn publish_snapshot_block(
    layout: &RepositoryLayout,
    path: &str,
    bytes: &[u8],
) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let blob_id = write_blob(&mut object_store, bytes)?;
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([0x41; 32]),
                blob_id,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state_root = crate::derive_next_state_root(&object_store, None, &[patch_id])?;
    let snapshot_blob_id = write_snapshot(
        &mut object_store,
        vec![text_entry(path, 0x41, blob_id)?],
        state_root,
    )?;
    let block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![patch_id],
        Some(snapshot_blob_id),
        state_root,
    );
    let block_id = object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(snapshot_blob_id)
}

/// RFC 136 §10.3b.2: the provisional marker is durable before the first worktree write. A failure
/// injected at that first write leaves the marker set, so the derivation gate refuses.
#[cfg(target_os = "linux")]
#[test]
fn a_crash_after_the_marker_and_before_the_first_file_leaves_the_marker_set()
-> prikk_error::Result<()> {
    use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};

    let root = unique_temp_dir("snapshot-materialize-crash-after-marker");
    let layout = RepositoryLayout::init(root.clone())?;
    publish_snapshot_block(&layout, "README.md", b"hello\n")?;

    // The marker appends never rename; the first worktree file write does.
    fail_once_for_test(TestFailPoint::MutableRename);
    assert!(
        crate::materialize_snapshot_checkout(&layout, "heads/main").is_err(),
        "fixture sanity: the injected failure must stop materialization"
    );
    assert!(
        !root.join("README.md").exists(),
        "fixture sanity: the failure fired before the first file landed"
    );
    assert!(crate::provisional_worktree(&layout)?.is_some());
    assert!(matches!(
        crate::ensure_worktree_replay_verified(&layout),
        Err(prikk_error::PrikkError::Precondition(_))
    ));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
