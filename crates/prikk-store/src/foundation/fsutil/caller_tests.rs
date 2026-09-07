//! Required-open propagation through authoritative storage caller classes.

mod directory_matrix;
mod sync_matrix;
mod validation_matrix;

use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::{
    dummy_signature, signed_empty_block_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::worktree::materialize_manifest_entries;
use crate::{
    DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectWriter, RefPublication, RefStore, RepoPath,
    RepositoryLayout, SnapshotEntry, SnapshotManifest, Wal, add_trusted_maintainer,
    write_active_ref_metadata,
};

#[test]
fn wal_required_open_failure_propagates_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("wal-required-open-caller");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let patch = signed_patch_envelope();
    fail_once_for_test(TestFailPoint::RequiredOpen);
    assert!(wal.append_patch(&patch).is_err());
    assert_eq!(wal.append_patch(&patch)?, 1);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn active_required_open_failure_propagates_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("active-required-open-caller");
    let layout = RepositoryLayout::init(root.clone())?;
    fail_once_for_test(TestFailPoint::RequiredOpen);
    assert!(write_active_ref_metadata(&layout, "heads/main").is_err());
    assert!(write_active_ref_metadata(&layout, "heads/main").is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn object_required_open_failure_propagates_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("object-required-open-caller");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut object = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"payload".to_vec());
    object.add_signature(dummy_signature())?;
    let mut store = FileObjectStore::new(layout);
    fail_once_for_test(TestFailPoint::RequiredOpen);
    assert!(store.write_object(&object).is_err());
    assert_eq!(store.write_object(&object)?, object.object_id());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn trust_required_open_failure_propagates_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("trust-required-open-caller");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = "0707070707070707070707070707070707070707070707070707070707070707";
    fail_once_for_test(TestFailPoint::RequiredOpen);
    assert!(add_trusted_maintainer(&layout, "maintainer", key).is_err());
    assert!(add_trusted_maintainer(&layout, "maintainer", key).is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn ref_required_open_failure_propagates_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("ref-required-open-caller");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let target = objects.write_object(&signed_empty_block_envelope())?;
    let ref_state = signed_ref_state_envelope("heads/main", None, target, 1);
    let ref_state_id = ref_state.object_id();
    let publication = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_update: signed_ref_update_envelope("heads/main", None, ref_state_id, target, 1),
        ref_state,
    };
    let store = RefStore::new(layout);
    fail_once_for_test(TestFailPoint::RequiredOpen);
    assert!(store.publish(&publication).is_err());
    assert_eq!(store.publish(&publication)?, ref_state_id);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn worktree_component_failures_propagate_and_retry() -> prikk_error::Result<()> {
    for point in [TestFailPoint::RequiredOpen, TestFailPoint::DirectoryCreate] {
        let root = unique_temp_dir("worktree-component-caller");
        let layout = RepositoryLayout::init(root.clone())?;
        let manifest = SnapshotManifest {
            files: vec![SnapshotEntry {
                path: RepoPath::parse("nested/file.txt")?,
                bytes: b"content".to_vec(),
            }],
        };
        fail_once_for_test(point);
        assert!(materialize_manifest_entries(&layout, &manifest).is_err());
        assert!(materialize_manifest_entries(&layout, &manifest).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}
