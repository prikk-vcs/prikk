//! Active-session tests.

#![allow(clippy::unwrap_used)]

use crate::{
    ActiveLock, ActiveRefMetadata, ActiveSession, DEFAULT_ACTIVE_NAME, DEFAULT_ACTIVE_PATCH_LIMIT,
    RepositoryLayout, Wal, finish_active_publication_cleanup, read_active_ref_metadata,
    remove_active_ref_metadata, write_active_ref_metadata,
};

use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test, fail_once_for_test};
use crate::test_gates::test_support::{
    rollback_patch_envelope, signed_patch_envelope, unique_temp_dir,
};

mod format_transition;

#[test]
fn active_lock_rejects_second_writer() {
    let root = unique_temp_dir("lock");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let first = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME);
        assert!(first.is_ok());
        let second = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME);
        assert!(second.is_err());
        drop(first);
        let third = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME);
        assert!(third.is_ok());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_session_appends_signed_patch_under_lock() {
    let root = unique_temp_dir("active-session");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let session = ActiveSession::new(layout.clone());
        let envelope = signed_patch_envelope();
        let result = session.append_patch(&envelope, DEFAULT_ACTIVE_PATCH_LIMIT);
        assert!(result.is_ok());
        if let Ok(result) = result {
            assert_eq!(result.wal_sequence, 1);
        }
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        let replay = wal.replay();
        assert!(replay.is_ok());
        if let Ok(replay) = replay {
            assert_eq!(replay.records.len(), 1);
            assert_eq!(
                replay.records.first().map(|record| &record.envelope),
                Some(&envelope)
            );
        }
        assert_eq!(
            read_active_ref_metadata(&layout).unwrap(),
            ActiveRefMetadata::Valid("heads/main".to_string())
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 132 part 2: a full active-patch queue is a caller precondition here too, not a lock --
/// exercised directly against `ActiveSession::append_patch`, since this method (unlike
/// `node_authoring.rs`'s own identical check, covered at the compiled-binary level by
/// `dc57_active_patch_thresholds.rs`) has no current production caller through the CLI to drive it
/// with (see this file's own note on the next test).
#[test]
fn active_session_append_rejects_at_the_configured_limit_as_a_precondition() {
    let root = unique_temp_dir("active-session-limit");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let session = ActiveSession::new(layout.clone());

    let err = session
        .append_patch(&signed_patch_envelope(), 0)
        .unwrap_err();
    assert!(
        matches!(err, prikk_error::PrikkError::Precondition(_)),
        "unexpected error variant: {err:?}"
    );
    assert!(
        err.to_string().starts_with("precondition not met:"),
        "unexpected error: {err}"
    );
    assert!(
        err.to_string()
            .contains("queued patches, at or above the configured limit"),
        "unexpected error: {err}"
    );

    let _ = std::fs::remove_dir_all(root);
}

// DC-66: a non-empty active WAL now queues a distinct patch rather than refusing it — this test
// previously asserted the pre-DC-66 reject behavior; updated per the RFC's raised cap (1 -> N).
// Ref-ownership-mismatch coverage for a non-empty queue lives at the production `worktree_patch`
// layer (`node_authoring.rs`'s `author_inner`), which this narrow, currently uncalled-in-production
// helper mirrors but does not itself need to re-cover.
#[test]
fn active_session_append_queues_distinct_patch_onto_non_empty_wal() {
    let root = unique_temp_dir("active-session-nonempty");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let session = ActiveSession::new(layout.clone());
    let first = session
        .append_patch(&signed_patch_envelope(), DEFAULT_ACTIVE_PATCH_LIMIT)
        .unwrap();
    assert_eq!(first.wal_sequence, 1);

    let second = session
        .append_patch(&rollback_patch_envelope(), DEFAULT_ACTIVE_PATCH_LIMIT)
        .unwrap();
    assert_eq!(second.wal_sequence, 2);

    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay.records.len(), 2);
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/main".to_string())
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_session_append_rejects_trailing_partial_wal() {
    let root = unique_temp_dir("active-session-partial");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(layout.default_queue_wal_path(), b"partial").unwrap();
    let session = ActiveSession::new(layout.clone());

    let err = session
        .append_patch(&signed_patch_envelope(), DEFAULT_ACTIVE_PATCH_LIMIT)
        .unwrap_err();
    assert!(
        err.to_string().contains("trailing partial bytes"),
        "unexpected error: {err}"
    );
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Missing
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_session_append_does_not_overwrite_other_ref_metadata() {
    let root = unique_temp_dir("active-session-owned");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    write_active_ref_metadata(&layout, "heads/topic").unwrap();
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&signed_patch_envelope())
        .unwrap();
    let session = ActiveSession::new(layout.clone());

    let err = session
        .append_patch(&signed_patch_envelope(), DEFAULT_ACTIVE_PATCH_LIMIT)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("active WAL is owned by heads/topic"),
        "unexpected error: {err}"
    );
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/topic".to_string())
    );
    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay.records.len(), 1);

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 132 part 1: `active_ref_ownership` is the question form
/// `require_active_ref_for_non_empty_wal` asserts an answer to -- both cases, exercised directly
/// rather than only through `ActiveSession::append_patch`'s own coverage above.
#[test]
fn active_ref_ownership_answers_owned_and_owned_by_other_as_values_not_errors() {
    let root = unique_temp_dir("active-ref-ownership");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();

    assert_eq!(
        crate::active_ref_ownership(&layout, "heads/main").unwrap(),
        crate::ActiveRefOwnership::Owned
    );
    assert_eq!(
        crate::active_ref_ownership(&layout, "heads/topic").unwrap(),
        crate::ActiveRefOwnership::OwnedByOther("heads/main".to_string())
    );

    // The assertion form is written in terms of the question form, not a second, independent
    // comparison -- so its two outcomes must match exactly what the question form just answered.
    assert!(
        crate::require_active_ref_for_non_empty_wal(&layout, "heads/main").is_ok(),
        "assertion form must agree with Owned"
    );
    let err = crate::require_active_ref_for_non_empty_wal(&layout, "heads/topic").unwrap_err();
    assert!(
        err.to_string()
            .contains("active WAL is owned by heads/main"),
        "assertion form must agree with OwnedByOther: {err}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_ref_metadata_round_trips_and_removes() {
    let root = unique_temp_dir("active-ref");
    let layout = RepositoryLayout::init(root.clone()).unwrap();

    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Missing
    );
    assert_eq!(
        write_active_ref_metadata(&layout, "heads/topic").unwrap(),
        "heads/topic"
    );
    assert_eq!(
        std::fs::read(layout.default_active_ref_name_path()).unwrap(),
        b"heads/topic"
    );
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/topic".to_string())
    );
    assert!(remove_active_ref_metadata(&layout).unwrap());
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Missing
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_ref_metadata_reports_malformed_content() {
    let root = unique_temp_dir("active-ref-bad");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(layout.default_active_ref_name_path(), b"heads//bad").unwrap();

    assert!(matches!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Invalid(_)
    ));

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 102 Stage 5, design-v1.md §14.6: redesigned onto the new truncate-then-append write and
/// truncate-only clear, replacing the retired `write_file_atomically`/`remove_file_if_present_required`
/// failpoints (`MutableParentSync`/`CleanupDirectorySync`) with the ones the new primitives actually
/// have. `write_active_ref_metadata`'s two-step shape introduces a state the old single-`atomic_replace`
/// write never had: a truncate that lands but an append that fails leaves the file genuinely empty
/// (`Missing`), not the old value or the new one -- exactly the crash window the §14.6 condition moved
/// inside this function rather than eliminating.
#[test]
fn active_metadata_write_and_removal_failures_pin_retained_state() {
    let root = unique_temp_dir("active-ref-failpoints");
    let layout = RepositoryLayout::init(root.clone()).unwrap();

    // Truncate fails before any append: nothing has changed yet.
    fail_once_for_test(TestFailPoint::Truncate);
    assert!(write_active_ref_metadata(&layout, "heads/topic").is_err());
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Missing
    );

    // Append fails after a successful truncate: the file is left genuinely empty, not the requested
    // value.
    fail_once_for_test(TestFailPoint::AppendWrite);
    assert!(write_active_ref_metadata(&layout, "heads/topic").is_err());
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Missing
    );

    assert!(write_active_ref_metadata(&layout, "heads/topic").is_ok());
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/topic".to_string())
    );

    // Clearing fails at the directory-sync confirmation, after the physical truncate already landed:
    // the same "operation happened, confirmation failed" shape the old atomic-replace test proved for
    // `MutableParentSync`, now via `durable_truncate_to_empty`'s own last step.
    fail_once_for_test(TestFailPoint::RequiredDirectorySync);
    assert!(remove_active_ref_metadata(&layout).is_err());
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Missing
    );

    // A further clear is now a genuine no-op: nothing left to clear, reported as such.
    assert!(!remove_active_ref_metadata(&layout).unwrap());

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 102 Stage 5, design-v1.md §14.6: `finish_active_publication_cleanup` truncates the WAL, then
/// truncates the metadata -- both now the same `durable_truncate_to_empty` primitive, so a one-shot
/// `Truncate` failpoint always hits the WAL's own call first. Reaching the metadata's call needs
/// `fail_after_for_test` to skip the WAL's. Retired the old `Unlink`/`CleanupDirectorySync` cases
/// entirely: nothing in this function unlinks anything anymore, and `!layout.default_active_ref_name_
/// path().exists()` is no longer a reachable postcondition -- the file is permanent from `init` onward.
#[test]
fn active_publication_cleanup_failures_preserve_retryable_states() {
    // The WAL's own truncate (the first of the two calls) fails: neither the WAL nor the metadata is
    // touched.
    let root = unique_temp_dir("active-publication-cleanup-failpoint-wal");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&signed_patch_envelope())
        .unwrap();
    let wal_before = std::fs::read(layout.default_queue_wal_path()).unwrap();
    let metadata_before = std::fs::read(layout.default_active_ref_name_path()).unwrap();
    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();

    fail_once_for_test(TestFailPoint::Truncate);
    assert!(finish_active_publication_cleanup(&layout, &active_lock).is_err());
    assert_eq!(
        std::fs::read(layout.default_queue_wal_path()).unwrap(),
        wal_before
    );
    assert_eq!(
        std::fs::read(layout.default_active_ref_name_path()).unwrap(),
        metadata_before
    );

    finish_active_publication_cleanup(&layout, &active_lock).unwrap();
    assert!(
        std::fs::read(layout.default_queue_wal_path())
            .unwrap()
            .is_empty()
    );
    assert!(
        std::fs::read(layout.default_active_ref_name_path())
            .unwrap()
            .is_empty()
    );
    drop(active_lock);
    let _ = std::fs::remove_dir_all(root);

    // The metadata's own truncate (the second of the two calls) fails: the WAL's truncate already
    // landed, and the metadata's prior content is retained.
    let root = unique_temp_dir("active-publication-cleanup-failpoint-metadata");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&signed_patch_envelope())
        .unwrap();
    let metadata_before = std::fs::read(layout.default_active_ref_name_path()).unwrap();
    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();

    fail_after_for_test(TestFailPoint::Truncate, 1);
    assert!(finish_active_publication_cleanup(&layout, &active_lock).is_err());
    assert!(
        std::fs::read(layout.default_queue_wal_path())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read(layout.default_active_ref_name_path()).unwrap(),
        metadata_before
    );

    finish_active_publication_cleanup(&layout, &active_lock).unwrap();
    assert!(
        std::fs::read(layout.default_queue_wal_path())
            .unwrap()
            .is_empty()
    );
    assert!(
        std::fs::read(layout.default_active_ref_name_path())
            .unwrap()
            .is_empty()
    );
    drop(active_lock);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_metadata_read_and_write_remain_on_retained_repository_root() {
    let root = unique_temp_dir("active-ref-root-replacement");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    write_active_ref_metadata(&layout, "heads/original").unwrap();
    let displaced = root.join(".prikk-displaced");
    std::fs::rename(layout.prikk_dir(), &displaced).unwrap();
    std::fs::create_dir_all(root.join(".prikk/active/default")).unwrap();
    std::fs::write(
        root.join(".prikk/active/default/ref-name"),
        b"heads/replacement",
    )
    .unwrap();

    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/original".to_string())
    );
    write_active_ref_metadata(&layout, "heads/updated").unwrap();
    assert_eq!(
        std::fs::read(displaced.join("active/default/ref-name")).unwrap(),
        b"heads/updated"
    );
    assert_eq!(
        std::fs::read(root.join(".prikk/active/default/ref-name")).unwrap(),
        b"heads/replacement"
    );

    let _ = std::fs::remove_dir_all(root);
}
