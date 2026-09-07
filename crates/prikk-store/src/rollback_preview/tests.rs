//! Rollback preview tests.

use crate::{RepositoryLayout, RollbackPreviewChangeKind, prepare_rollback_preview};

use crate::test_gates::test_support::unique_temp_dir;
use crate::test_gates::test_support::{
    publish_snapshot_then_patch_block, publish_text_create_then_edit_block,
    publish_text_edit_then_unsupported_rename_path_block,
};

#[test]
fn rollback_preview_reports_file_level_changes_to_snapshot_baseline() {
    let root = unique_temp_dir("rollback-preview-file-ops");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_snapshot_then_patch_block(&layout);
        assert!(result.is_ok());
        let preview = prepare_rollback_preview(&layout, "heads/main");
        assert!(preview.is_ok());
        if let Ok(preview) = preview {
            assert_eq!(preview.block_count, 2);
            assert_eq!(preview.patch_count, 1);
            assert_eq!(preview.inverse_operation_count, 2);
            assert_eq!(preview.current_file_count, 2);
            assert_eq!(preview.preview_file_count, 2);
            assert_eq!(preview.change_count, 2);
            assert_eq!(preview.would_create_files, 1);
            assert_eq!(preview.would_delete_files, 1);
            assert_eq!(preview.would_replace_files, 0);
            let changes: Vec<(&str, RollbackPreviewChangeKind)> = preview
                .changes
                .iter()
                .map(|change| (change.path.as_str(), change.kind))
                .collect();
            assert_eq!(
                changes,
                vec![
                    ("extra.txt", RollbackPreviewChangeKind::WouldDelete),
                    ("old.txt", RollbackPreviewChangeKind::WouldCreate),
                ]
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rollback_preview_accepts_arbitrary_span_text_inverse_plan() {
    let root = unique_temp_dir("rollback-preview-edit-text");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_text_create_then_edit_block(&layout, b"alpha beta\n", b"alpha BETA\n");
        assert!(result.is_ok());
        let preview = prepare_rollback_preview(&layout, "heads/main");
        assert!(preview.is_ok());
        if let Ok(preview) = preview {
            assert_eq!(preview.inverse_operation_count, 2);
            assert_eq!(preview.current_file_count, 1);
            assert_eq!(preview.preview_file_count, 0);
            assert_eq!(preview.change_count, 1);
            assert_eq!(preview.would_delete_files, 1);
            assert_eq!(
                preview.changes.first().map(|change| change.kind),
                Some(RollbackPreviewChangeKind::WouldDelete)
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rollback_preview_fails_closed_on_supported_text_plus_unsupported_operation() {
    let root = unique_temp_dir("rollback-preview-edit-text-unsupported");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_text_edit_then_unsupported_rename_path_block(&layout);
        assert!(result.is_ok());
        let preview = prepare_rollback_preview(&layout, "heads/main");
        assert!(preview.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}
