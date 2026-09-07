//! Unclean-shutdown worktree marker tests (RFC 102 Stage 1).

use crate::RepositoryLayout;
use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::unique_temp_dir;
use crate::worktree_marker::{clear_worktree_dirty, mark_worktree_dirty, worktree_is_dirty};

#[test]
fn marker_is_created_clean_at_init_and_round_trips_dirty_and_clean() {
    let root = unique_temp_dir("worktree-marker-round-trip");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(layout.worktree_unclean_shutdown_marker_path().exists());
        assert!(worktree_is_dirty(&layout).is_ok_and(|dirty| !dirty));

        assert!(mark_worktree_dirty(&layout).is_ok());
        assert!(worktree_is_dirty(&layout).is_ok_and(|dirty| dirty));

        assert!(clear_worktree_dirty(&layout).is_ok());
        assert!(worktree_is_dirty(&layout).is_ok_and(|dirty| !dirty));
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Acceptance criterion 1, demonstrated rather than merely inspected: `atomic_replace` would
/// overwrite the marker's content with exactly one sentinel on every dirty-set, regardless of what
/// was there before. Two dirty-sets with no clear between them growing the file to two sentinels'
/// worth of bytes is only possible if the marker is genuinely appended to, not replaced -- confirming
/// by observed behavior what `worktree_marker.rs`'s own imports (only `append_file_required`/
/// `truncate_file_empty_required`, never `write_file_atomically`) already show by inspection.
#[test]
fn marker_dirty_set_appends_rather_than_replaces() {
    let root = unique_temp_dir("worktree-marker-append-not-replace");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(mark_worktree_dirty(&layout).is_ok());
        let after_first = std::fs::read(layout.worktree_unclean_shutdown_marker_path());
        assert!(after_first.is_ok());

        assert!(mark_worktree_dirty(&layout).is_ok());
        let after_second = std::fs::read(layout.worktree_unclean_shutdown_marker_path());

        if let (Ok(first), Ok(second)) = (after_first, after_second) {
            assert!(!first.is_empty());
            assert_eq!(second.len(), first.len() * 2);
            assert!(second.starts_with(&first));
        }
        assert!(worktree_is_dirty(&layout).is_ok_and(|dirty| dirty));
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Acceptance criterion 3: a crash during the clear must leave the marker dirty, never clean --
/// append/truncate fails toward "still has bytes," never toward a corrupted intermediate value, the
/// safe direction (a spurious refusal on the next commit, never a missed dirty state).
#[test]
fn crash_during_clear_leaves_the_marker_dirty_not_clean() {
    let root = unique_temp_dir("worktree-marker-crash-during-clear");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(mark_worktree_dirty(&layout).is_ok());
        assert!(worktree_is_dirty(&layout).is_ok_and(|dirty| dirty));

        fail_once_for_test(TestFailPoint::Truncate);
        assert!(clear_worktree_dirty(&layout).is_err());
        assert!(
            worktree_is_dirty(&layout).is_ok_and(|dirty| dirty),
            "a failed clear must leave the marker dirty, not clean"
        );

        assert!(clear_worktree_dirty(&layout).is_ok());
        assert!(worktree_is_dirty(&layout).is_ok_and(|dirty| !dirty));
    }
    let _ = std::fs::remove_dir_all(root);
}

/// A repository initialized before this marker existed has no marker file. Must read as clean, not
/// error and not dirty -- this project's standing direction is that migration for existing
/// repositories is not required.
#[test]
fn missing_marker_file_reads_as_clean() {
    let root = unique_temp_dir("worktree-marker-missing-reads-clean");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(std::fs::remove_file(layout.worktree_unclean_shutdown_marker_path()).is_ok());
        assert!(worktree_is_dirty(&layout).is_ok_and(|dirty| !dirty));
    }
    let _ = std::fs::remove_dir_all(root);
}
