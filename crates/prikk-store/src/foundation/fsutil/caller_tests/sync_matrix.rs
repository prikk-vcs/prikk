//! Final containing-directory sync propagation and retry classification.

use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test, fail_once_for_test};
use crate::test_gates::test_support::{dummy_signature, text_replay_manifest, unique_temp_dir};
use crate::worktree::materialize_replay_manifest_entries;
use crate::{
    ActiveLock, DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectWriter, RepositoryLayout,
    write_active_ref_metadata,
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
/// Recomputed directly from `init`'s own call sequence: 5 fixed names (worktree marker, RFC 136
/// §10.3b's provisional-worktree marker, WAL, active-ref metadata, RFC 144 §4o.2's rename-declaration
/// store) + 7 persisted object types × 2 slots (14) + the object index + generation log (2) + 2
/// ref-log slots + 3 ref-pointer-index (A/B/genlog) + 3 received-index (A/B/genlog) + 1 trust key + 3
/// trust-policy (A/B/genlog) = 33 names before `FORMAT` itself. The skip below is one more than the
/// name count, as it was before the provisional marker (32 names, skip 33). If a future stage adds
/// another `init`-time name before `FORMAT`, this count needs updating -- the same maintenance
/// `ref_log_parent_sync_failure_retains_one_update_and_retries` below already carries for the same
/// reason.
#[test]
fn repository_format_create_sync_failure_retains_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("repository-sync-matrix");
    // RFC 168 §3.1 and §3.3 added the recovery log and the default witness to `init` before `FORMAT`: two names, and the
    // `recovery/` directory's own create and sync. The directory sync that lands on `FORMAT`'s create is the 37th.
    fail_after_for_test(TestFailPoint::RequiredDirectorySync, 37);
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

// `wal_parent_sync_failure_retains_one_record_and_retries` removed, 0.55.0 step 3 Part 2:
// `durable_append_reporting_offset` no longer syncs the directory at all (the WAL file is
// permanent from `init`, so its own append never creates a directory entry -- RFC 102 §3). There
// is no longer a `RequiredDirectorySync` point on this call path; `wal::tests::wal_file_sync_
// failure_retains_replayable_record` already covers the sibling file-sync failure, unchanged.

/// 0.55.0 step 3 Part 3a (G1): retargeted from `RequiredDirectorySync` to `RequiredFileSync`.
/// `write_active_ref_metadata` truncates (`RequiredFileSync` occurrence 1, still directory-synced
/// afterward too -- `durable_truncate_to_empty` is a different primitive, unchanged by Part 2),
/// then appends through `durable_append_reporting_offset` (`RequiredFileSync` occurrence 2, and no
/// directory sync at all any more -- RFC 102 §3). Skip 1 lands on the append's own file sync, not
/// the truncate's -- verified empirically (skip 0 fails before any content is written at all, a
/// red control this report cites), not merely reasoned, the same way the original directory-sync
/// skip of 1 was.
#[test]
fn active_metadata_file_sync_failure_retains_and_retries() -> prikk_error::Result<()> {
    let root = unique_temp_dir("active-sync-matrix");
    let layout = RepositoryLayout::init(root.clone())?;
    fail_after_for_test(TestFailPoint::RequiredFileSync, 1);
    assert!(write_active_ref_metadata(&layout, "heads/main").is_err());
    assert_eq!(
        std::fs::read(layout.default_active_ref_name_path())?,
        b"heads/main"
    );
    assert!(write_active_ref_metadata(&layout, "heads/main").is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// `ref_log_parent_sync_failure_retains_one_update_and_retries` removed, 0.55.0 step 3 Part 2:
// every append on this path (the object container, the object index, the pointer index, and the
// ref log container itself) goes through `durable_append_reporting_offset`, which no longer
// syncs the directory at all (RFC 102 §3) -- only the lock-file creations still do, and none of
// them is "the log append's own directory sync" this test meant to land on, so it no longer has
// an injection point. The identical property for the file-sync case is unchanged and covered by
// `refs::tests::first_publication_retries_completed_log_sync_without_duplicate`.

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
    let manifest = text_replay_manifest("file.txt", b"content")?;
    fail_once_for_test(TestFailPoint::MutableParentSync);
    assert!(materialize_replay_manifest_entries(&layout, &manifest).is_err());
    assert_eq!(std::fs::read(root.join("file.txt"))?, b"content");
    fail_once_for_test(TestFailPoint::RequiredDirectorySync);
    assert!(materialize_replay_manifest_entries(&layout, &manifest).is_err());
    assert!(materialize_replay_manifest_entries(&layout, &manifest).is_ok());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
