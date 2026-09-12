//! Final containing-directory sync propagation and retry classification.

use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test, fail_once_for_test};
use crate::test_gates::test_support::{
    dummy_signature, signed_empty_block_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::worktree::materialize_manifest_entries;
use crate::{
    ActiveLock, DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectWriter, RefPublication, RefStore,
    RepoPath, RepositoryLayout, SnapshotEntry, SnapshotManifest, Wal, write_active_ref_metadata,
};

/// RFC 102 Stage 5, design-v1.md §14.10: `FORMAT` now goes through `create_new_file_required`
/// (`create_exclusive`), the same primitive every other `init`-time name uses -- `MutableParentSync`
/// (`atomic_replace`-only) no longer fires anywhere in `init` at all. `create_exclusive` shares
/// `RequiredDirectorySync` with every other name's own creation, so landing on `FORMAT`'s specific
/// occurrence (the last one, since §14.2 orders it last) needs a skip count derived from the exact
/// sequence, not assumed. RFC 102 Stage 6 Step 1 (design-v1.md §15.6) gives each of the three genuine
/// compaction targets (ref pointer index, received index, trust policy container) its own A/B slot
/// pair and generation log -- 3 names apiece instead of the 1 each carried before this stage. RFC 115
/// Stage 2 added `ObjectType::RecognitionClaim` as a seventh persisted object type (two more slot
/// creations, A and B).
/// Recomputed directly from `init`'s own call sequence: 4 fixed names (worktree marker, WAL,
/// active-ref metadata, RFC 144 §4o.2's rename-declaration store) + 7 persisted object types × 2
/// slots (14) + the object index + generation log (2) + 2 ref-log slots + 3 ref-pointer-index
/// (A/B/genlog) + 3 received-index (A/B/genlog) + 1 trust key + 3 trust-policy (A/B/genlog) = 32
/// names before `FORMAT` itself, so skip 32 to land on the 33rd occurrence. If a future stage adds
/// another `init`-time name before `FORMAT`, this count needs updating -- the same maintenance
/// `ref_log_parent_sync_failure_retains_one_update_and_retries` below already carries for the same
/// reason.
#[test]
fn repository_format_create_sync_failure_retains_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("repository-sync-matrix");
    fail_after_for_test(TestFailPoint::RequiredDirectorySync, 33);
    assert!(RepositoryLayout::init(root.clone()).is_err());
    assert!(
        root.join(".prikk/FORMAT").is_file(),
        "the create already landed before the directory sync failed"
    );
    assert!(RepositoryLayout::init(root.clone()).is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-98 has a Windows twin of this exact body, same `skip` values, in
/// `fsutil::anchored::windows::tests::object_write_sync_failure_retains_and_classifies_windows`
/// (duplicated rather than freed from this module's shared per-file `use` statements -- see that
/// test's own doc comment). **Mirror any edit to this test there too**, or the two silently
/// diverge.
#[test]
fn object_write_sync_failure_retains_and_classifies() -> prikk_error::Result<()> {
    // RFC 102 Stage 3: an object write durably appends to its container, then to the index, both
    // through `RequiredFileSync`. RFC 102's 2026-09-12 object-store lock adds one `RequiredFileSync`
    // *before* both -- the lock file's own creation -- so every index here shifted by one, and skip
    // 0 now names an operation that did not exist before.
    //
    // The three rows were measured, not reasoned: skip 0 leaves the lock behind and blocks the
    // retry; skip 1 writes nothing and retries clean; skip 2 leaves the object readable in-process
    // and retries clean; skip 3 does not fail at all, which is what says there are exactly three
    // syncs on this path.
    for (skip, indexed_after_error, retry_succeeds) in [
        // The lock file's own creation sync. Nothing is written -- and the lock file is **left
        // behind**, so the retry meets `LockConflict` and the repository needs `prikk unlock`.
        // That is prikk's documented posture for every lock ("the tool never decides a lock is
        // stale on its own", `concurrency-locking.md`), applied for the first time to the object
        // write path: an operator-visible refusal rather than a silent unlock. Pinned here so the
        // cost of the ruling is a tested fact rather than a paragraph.
        (0, false, false),
        // The container append itself: the object is not durably indexed at all.
        (1, false, true),
        // The index append: the container record and the index entry's bytes are both already on
        // disk and only the index sync is interrupted, so the object is already visible to a
        // same-process read.
        (2, true, true),
    ] {
        let root = unique_temp_dir("object-sync-matrix");
        let layout = RepositoryLayout::init(root.clone())?;
        let mut object = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"sync".to_vec());
        object.add_signature(dummy_signature())?;
        let object_id = object.object_id();
        let lock_path = layout.containers_dir().join("objects.lock");
        let mut store = FileObjectStore::new(layout);
        fail_after_for_test(TestFailPoint::RequiredFileSync, skip);
        assert!(store.write_object(&object).is_err(), "skip {skip}");
        assert_eq!(
            store.contains_object(ObjectType::Blob, object_id),
            indexed_after_error,
            "skip {skip}"
        );
        // The lock is released on every failure *after* acquisition, and only retained when the
        // acquisition itself failed -- the two halves of the posture, asserted together.
        assert_eq!(
            lock_path.is_file(),
            !retry_succeeds,
            "skip {skip}: stale lock"
        );
        let retry = store.write_object(&object);
        assert_eq!(retry.is_ok(), retry_succeeds, "skip {skip}: {retry:?}");
        if retry_succeeds {
            assert_eq!(retry?, object_id);
            assert!(store.contains_object(ObjectType::Blob, object_id));
        }
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn wal_parent_sync_failure_retains_one_record_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("wal-sync-matrix");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let patch = signed_patch_envelope();
    fail_once_for_test(TestFailPoint::RequiredDirectorySync);
    assert!(wal.append_patch(&patch).is_err());
    assert_eq!(wal.replay()?.records.len(), 1);
    assert_eq!(wal.append_patch(&patch)?, 1);
    assert_eq!(wal.replay()?.records.len(), 1);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 102 Stage 5, design-v1.md §14.6: `write_active_ref_metadata` no longer goes through
/// `atomic_replace` -- it truncates, then appends, each through `RequiredDirectorySync`. `is_file()`
/// stopped being a meaningful postcondition the moment the file became permanent from `init` onward
/// (it is always true); asserting the actual content survived an error is what still distinguishes
/// "the write landed, only its confirmation failed" from "the write never happened." Skip 1 to land the
/// injected failure on the append's own directory sync, not the truncate's -- skip 0 would fail before
/// "heads/main" is ever written at all, proving a different (and less interesting) property.
#[test]
fn active_metadata_parent_sync_failure_retains_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("active-sync-matrix");
    let layout = RepositoryLayout::init(root.clone())?;
    fail_after_for_test(TestFailPoint::RequiredDirectorySync, 1);
    assert!(write_active_ref_metadata(&layout, "heads/main").is_err());
    assert_eq!(
        std::fs::read(layout.default_active_ref_name_path())?,
        b"heads/main"
    );
    assert!(write_active_ref_metadata(&layout, "heads/main").is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn ref_log_parent_sync_failure_retains_one_update_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("ref-sync-matrix");
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
    // RFC 102 Stage 3: skip past the ref lock's own creation and the ref-state object's own
    // container and index appends (each fires `RequiredDirectorySync` too). RFC 102 Stage 4 adds
    // one more before the log append: the pointer-index append. RFC 102 Stage 6 Step 2,
    // design-v1.md §15.8, adds two more before that -- `acquire_container_locks`'s own
    // `RefPointerIndex` and `RefLog` lock file creations, hoisted to right after `RefLock::acquire`
    // and each firing this same failpoint via `create_new_file_required` the same way `RefLock`'s
    // own creation always has. RFC 102's 2026-09-12 object-store lock adds one more
    // before the object write -- `append_object_under_lock`'s own lock file creation, which
    // fires this point through `create_new_file_required` exactly as every other lock creation
    // here does. Skip 7, not 6, to land the
    // injected failure on the log append's own directory sync -- see `refs::tests`'s equivalent fix
    // for the full accounting.
    fail_after_for_test(TestFailPoint::RequiredDirectorySync, 7);
    assert!(store.publish(&publication).is_err());
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    assert_eq!(store.publish(&publication)?, ref_state_id);
    assert_eq!(store.replay_log("heads/main")?.records.len(), 1);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn lock_parent_sync_failure_retains_stale_state_until_explicit_cleanup() -> prikk_error::Result<()>
{
    let root = unique_temp_dir("lock-sync-matrix");
    let layout = RepositoryLayout::init(root.clone())?;
    let lock_path = layout.default_active_lock_path();
    fail_once_for_test(TestFailPoint::RequiredDirectorySync);
    assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_err());
    assert!(lock_path.is_file());
    assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_err());
    std::fs::remove_file(&lock_path)?;
    assert!(ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn worktree_parent_sync_failure_is_repaired_before_unchanged_success() -> prikk_error::Result<()> {
    let root = unique_temp_dir("worktree-sync-matrix");
    let layout = RepositoryLayout::init(root.clone())?;
    let manifest = SnapshotManifest {
        files: vec![SnapshotEntry {
            path: RepoPath::parse("file.txt")?,
            bytes: b"content".to_vec(),
        }],
    };
    fail_once_for_test(TestFailPoint::MutableParentSync);
    assert!(materialize_manifest_entries(&layout, &manifest).is_err());
    assert_eq!(std::fs::read(root.join("file.txt"))?, b"content");
    fail_once_for_test(TestFailPoint::RequiredDirectorySync);
    assert!(materialize_manifest_entries(&layout, &manifest).is_err());
    assert!(materialize_manifest_entries(&layout, &manifest).is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
