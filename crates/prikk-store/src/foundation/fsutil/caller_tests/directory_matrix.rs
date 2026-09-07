//! Per-caller directory component failure and retry matrix.

use std::path::Path;

use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::unique_temp_dir;
use crate::worktree::materialize_manifest_entries;
use crate::{RepoPath, RepositoryLayout, SnapshotEntry, SnapshotManifest};

const COMPONENT_POINTS: [TestFailPoint; 2] = [
    TestFailPoint::DirectoryCreate,
    TestFailPoint::CreatedDirectoryParentSync,
];

fn assert_retained_component(point: TestFailPoint, path: &Path) {
    match point {
        TestFailPoint::DirectoryCreate => assert!(!path.exists()),
        TestFailPoint::CreatedDirectoryParentSync => assert!(path.is_dir()),
        _ => panic!("not a component-creation point"),
    }
}

fn remove_empty_directory(path: &Path) -> prikk_error::Result<()> {
    std::fs::remove_dir(path)?;
    Ok(())
}

#[test]
fn repository_initialization_component_matrix() -> prikk_error::Result<()> {
    for point in COMPONENT_POINTS {
        let root = unique_temp_dir("repository-init-component-matrix");
        fail_once_for_test(point);
        assert!(RepositoryLayout::init(root.clone()).is_err());
        assert_retained_component(point, &root.join(".prikk"));
        assert!(RepositoryLayout::init(root.clone()).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

// RFC 102 Stage 3: `object_directory_component_matrix` (proving a first-object-write directory
// component failure is retryable) has no container-era equivalent and was removed rather than
// retargeted. Every container and index file is allocated once, at `init`, per
// `layout/tests.rs::init_allocates_every_container_index_and_generation_log_name_once` -- an
// object write never creates a directory component at all anymore, so this matrix's own
// `DirectoryCreate`/`CreatedDirectoryParentSync` failpoints are simply never reached by
// `FileObjectStore::write_object`, and there is no analogous scenario left to prove here.

// RFC 102 Stage 5, design-v1.md §14.3/§14.5: `wal_directory_component_matrix` (proving a
// missing-directory-plus-missing-WAL-file failure during append was retryable) has no
// strict-`durable_append`-era equivalent, and was removed rather than retargeted -- the same
// disposition Stage 3 and Stage 4 gave the two retirement notes above, for the same underlying
// reason arriving through a third door. Its setup deleted `queue.wal` and then its now-empty
// parent directory to force what was then `wal.rs:194`'s defensive `ensure_directory_required`
// to recreate a directory from nothing; under the old lenient `durable_append`, the subsequent
// append also transparently recreated the deleted file. Strict `durable_append` closes exactly
// that transparent recreation (design-v1.md §14.3's own point: it "silently repairs" an
// interrupted-init-shaped state into an undetectable one), so the fixture's constructed state --
// directory absent, file absent -- is no longer one `wal.append_patch` can recover from, and is
// not a state a correctly-`init`ed repository can reach in the first place:
// `default_active_dir()` is in `required_directories()` (`layout.rs:378`) and nothing in this
// codebase ever removes a required directory. This was flagged as a finding rather than retired
// silently -- the same disposition Stage 3 and Stage 4 gave their own analogous calls -- and the
// dead-surface consolidation handoff (`rfcs/handoffs/consolidation/`) ruled it dead by the same
// reasoning and removed the `wal.rs` call outright.

// RFC 102 Stage 5, design-v1.md §14.8: `active_metadata_directory_component_matrix` (proving a
// missing-`active/default/`-directory failure during `write_active_ref_metadata` was retryable) has
// no strict-`durable_truncate_to_empty`-era equivalent, and is retired the same way and for the same
// reason `wal_directory_component_matrix` was in round 2 -- the asymmetry that kept this one alive a
// round longer (`durable_truncate_to_empty` tolerated a missing directory/file when strict
// `durable_append` already did not) is exactly what §14.8 closed. `default_active_dir()` is
// permanent from `init` (`layout.rs:389`) and nothing removes it, so the fixture's constructed
// state -- directory absent -- is no longer one `write_active_ref_metadata` can recover from, and is
// not reachable from a correctly-`init`ed repository in the first place.

// RFC 102 Stage 4: `ref_log_directory_component_matrix` (proving a first-ref-publish directory
// component failure on `refs/logs/` is retryable) has no container-era equivalent, retired the
// same way and for the same reason `object_directory_component_matrix` was above. The ref-log
// container is allocated once, at `init`, per `layout/tests.rs::init_allocates_every_ref_
// container_name_once` -- a ref publish never creates a directory component at all anymore, so
// this matrix's own `DirectoryCreate`/`CreatedDirectoryParentSync` failpoints are simply never
// reached by `RefStore::publish`, and there is no analogous scenario left to prove here.

// RFC 102 Stage 5, design-v1.md §14.9: `trust_directory_component_matrix` (proving a missing-
// `trust/keys/maintainer/`-directory failure during `add_trusted_maintainer` was retryable) has no
// container-era equivalent, retired the same way and for the same reason `active_metadata_directory_
// component_matrix` was. `maintainer_trust_keys_dir()` doesn't exist as a concept anymore --
// `add_trusted_maintainer` now appends into `trust_dir()` directly, which is permanent from `init`
// (`layout.rs`'s own `required_directories()`) via strict `durable_append`, so there is no nested
// directory left to go missing in the first place.

#[test]
fn lock_directory_component_matrix() -> prikk_error::Result<()> {
    for point in COMPONENT_POINTS {
        let root = unique_temp_dir("lock-component-matrix");
        let layout = RepositoryLayout::init(root.clone())?;
        let locks_dir = layout.refs_dir().join("locks");
        remove_empty_directory(&locks_dir)?;
        fail_once_for_test(point);
        assert!(crate::RefLock::acquire(&layout, "heads/main").is_err());
        assert_retained_component(point, &locks_dir);
        assert!(crate::RefLock::acquire(&layout, "heads/main").is_ok());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn worktree_directory_component_matrix() -> prikk_error::Result<()> {
    for point in COMPONENT_POINTS {
        let root = unique_temp_dir("worktree-component-matrix");
        let layout = RepositoryLayout::init(root.clone())?;
        let nested = root.join("nested");
        let manifest = manifest("nested/file.txt")?;
        fail_once_for_test(point);
        assert!(materialize_manifest_entries(&layout, &manifest).is_err());
        assert_retained_component(point, &nested);
        assert!(materialize_manifest_entries(&layout, &manifest).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

fn manifest(path: &str) -> prikk_error::Result<SnapshotManifest> {
    Ok(SnapshotManifest {
        files: vec![SnapshotEntry {
            path: RepoPath::parse(path)?,
            bytes: b"content".to_vec(),
        }],
    })
}
