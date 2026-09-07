//! Patch replay planning tests.

use crate::test_gates::test_support::{
    publish_snapshot_then_patch_block, publish_text_create_then_edit_block, unique_temp_dir,
};
use crate::{RepositoryLayout, prepare_patch_replay_plan};

#[test]
fn patch_replay_applies_create_delete_and_replace() {
    let root = unique_temp_dir("patch-replay");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_snapshot_then_patch_block(&layout);
        assert!(result.is_ok());
        let plan = prepare_patch_replay_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.block_count, 2);
            assert_eq!(plan.patch_count, 2);
            assert_eq!(plan.applied_operation_count, 4);
            assert_eq!(plan.file_count, 2);
            assert!(plan.paths.contains(&"README.md".to_string()));
            assert!(plan.paths.contains(&"extra.txt".to_string()));
            assert!(!plan.paths.contains(&"old.txt".to_string()));
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_replay_applies_arbitrary_span_edit_text() {
    let root = unique_temp_dir("patch-replay-edit-text");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(
            publish_text_create_then_edit_block(&layout, b"hello world\n", b"hello prikk\n")
                .is_ok()
        );
        let plan = prepare_patch_replay_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.block_count, 1);
            assert_eq!(plan.patch_count, 1);
            assert_eq!(plan.applied_operation_count, 2);
            assert_eq!(plan.file_count, 1);
            assert_eq!(plan.total_content_bytes, b"hello prikk\n".len() as u64);
            assert_eq!(plan.paths, vec!["README.md".to_string()]);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_replay_applies_subcharacter_widened_edit_text() {
    let root = unique_temp_dir("patch-replay-edit-text-subchar");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(
            publish_text_create_then_edit_block(&layout, "é\n".as_bytes(), "è\n".as_bytes())
                .is_ok()
        );
        let plan = prepare_patch_replay_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert_eq!(plan.applied_operation_count, 2);
            assert_eq!(plan.total_content_bytes, "è\n".len() as u64);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
