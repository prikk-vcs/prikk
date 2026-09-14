//! Patch checkout materialization tests.

use crate::{
    RepositoryLayout, materialize_patch_checkout, materialize_patch_checkout_with_deletions,
    plan_patch_checkout_deletions,
};

use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::{
    publish_snapshot_then_patch_block, publish_text_create_then_edit_block, unique_temp_dir,
};

#[test]
fn patch_materialization_writes_replayed_files() {
    let root = unique_temp_dir("patch-materialize");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        let report = materialize_patch_checkout(&layout, "heads/main");
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.block_count, 2);
            assert_eq!(report.patch_count, 1);
            assert_eq!(report.applied_operation_count, 2);
            assert_eq!(report.planned_files, 2);
            assert_eq!(report.written_files, 2);
            assert_eq!(report.unchanged_files, 0);
        }
        assert!(std::fs::read(root.join("README.md")).is_ok_and(|x| x == b"hello\n".to_vec()));
        assert!(std::fs::read(root.join("extra.txt")).is_ok_and(|x| x == b"extra\n".to_vec()));
        assert!(!root.join("old.txt").exists());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_writes_arbitrary_span_edit_text_result() {
    let root = unique_temp_dir("patch-materialize-edit-text");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(
            publish_text_create_then_edit_block(&layout, b"hello world\n", b"hello prikk\n")
                .is_ok()
        );
        let report = materialize_patch_checkout(&layout, "heads/main");
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.applied_operation_count, 2);
            assert_eq!(report.planned_files, 1);
            assert_eq!(report.written_files, 1);
        }
        assert!(
            std::fs::read(root.join("README.md")).is_ok_and(|x| x == b"hello prikk\n".to_vec())
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_is_idempotent_for_same_bytes() {
    let root = unique_temp_dir("patch-materialize-idempotent");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(materialize_patch_checkout(&layout, "heads/main").is_ok());
        let second = materialize_patch_checkout(&layout, "heads/main");
        assert!(second.is_ok());
        if let Ok(second) = second {
            assert_eq!(second.written_files, 0);
            assert_eq!(second.unchanged_files, 2);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_refuses_conflicting_existing_file() {
    let root = unique_temp_dir("patch-materialize-conflict");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("README.md"), b"local\n").is_ok());
        let report = materialize_patch_checkout(&layout, "heads/main");
        assert!(report.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_deletion_plan_reports_safe_removed_files() {
    let root = unique_temp_dir("patch-delete-plan");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("old.txt"), b"old\n").is_ok());
        let plan = plan_patch_checkout_deletions(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.planned_deletions, 1);
            assert_eq!(plan.deletable_files, 1);
            assert_eq!(plan.already_absent_files, 0);
            assert!(plan.conflicts.is_empty());
            assert_eq!(plan.deletable_paths, vec!["old.txt".to_string()]);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_materialization_with_deletions_removes_matching_old_file() {
    let root = unique_temp_dir("patch-materialize-delete");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("old.txt"), b"old\n").is_ok());
        let report = materialize_patch_checkout_with_deletions(&layout, "heads/main");
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.deleted_files, 1);
            assert_eq!(report.already_absent_deleted_files, 0);
            assert_eq!(report.deletion_conflicts, 0);
        }
        assert!(std::fs::read(root.join("README.md")).is_ok_and(|x| x == b"hello\n".to_vec()));
        assert!(std::fs::read(root.join("extra.txt")).is_ok_and(|x| x == b"extra\n".to_vec()));
        assert!(!root.join("old.txt").exists());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_deletion_retry_resyncs_observed_absent_parent() -> prikk_error::Result<()> {
    let root = unique_temp_dir("patch-delete-cleanup-retry");
    let layout = RepositoryLayout::init(root.clone())?;
    publish_snapshot_then_patch_block(&layout)?;
    std::fs::write(root.join("old.txt"), b"old\n")?;

    fail_once_for_test(TestFailPoint::CleanupDirectorySync);
    assert!(materialize_patch_checkout_with_deletions(&layout, "heads/main").is_err());
    assert!(!root.join("old.txt").exists());
    fail_once_for_test(TestFailPoint::CleanupDirectorySync);
    assert!(materialize_patch_checkout_with_deletions(&layout, "heads/main").is_err());
    let report = materialize_patch_checkout_with_deletions(&layout, "heads/main")?;
    assert_eq!(report.deleted_files, 0);
    assert_eq!(report.already_absent_deleted_files, 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn patch_materialization_with_deletions_refuses_modified_removed_file() {
    let root = unique_temp_dir("patch-materialize-delete-conflict");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(publish_snapshot_then_patch_block(&layout).is_ok());
        assert!(std::fs::write(root.join("old.txt"), b"local edit\n").is_ok());
        let report = materialize_patch_checkout_with_deletions(&layout, "heads/main");
        assert!(report.is_err());
        assert!(!root.join("README.md").exists());
        assert!(std::fs::read(root.join("old.txt")).is_ok_and(|x| x == b"local edit\n".to_vec()));
    }
    let _ = std::fs::remove_dir_all(root);
}
