//! Patch checkout materialization tests.

use crate::{
    RepositoryLayout, materialize_patch_checkout, materialize_patch_checkout_with_deletions,
    plan_patch_checkout_deletions,
};

use crate::foundation::fsutil::{TestFailPoint, fail_once_for_test};
use crate::test_gates::test_support::{
    publish_snapshot_then_patch_block, publish_text_create_then_edit_block, unique_temp_dir,
};
use crate::worktree::between_plan_and_write_for_test;
use crate::worktree_marker::worktree_is_dirty;

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
            // RFC 136 §10.3a: every block is replayed, the root's creates included.
            assert_eq!(report.patch_count, 2);
            assert_eq!(report.applied_operation_count, 4);
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
        // Checkout-refusal round §2.3: `Precondition`, naming the path.
        let message = precondition_message(report).unwrap_or_default();
        assert!(
            message.contains(
                "refusing checkout deletion because 1 candidate(s) are unsafe: old.txt ("
            ),
            "{message}"
        );
        assert!(!root.join("README.md").exists());
        assert!(std::fs::read(root.join("old.txt")).is_ok_and(|x| x == b"local edit\n".to_vec()));
    }
    let _ = std::fs::remove_dir_all(root);
}

// ---- Checkout-refusal round §3: a refused checkout writes nothing -------------------------------------

/// Every worktree file outside `.prikk`, with its bytes.
fn worktree_files(
    root: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".prikk") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                files.insert(
                    path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    bytes,
                );
            }
        }
    }
    files
}

/// The message of a `Precondition` refusal; `None` for success or any other class.
fn precondition_message<T>(result: prikk_error::Result<T>) -> Option<String> {
    match result {
        Err(prikk_error::PrikkError::Precondition(message)) => Some(message),
        _ => None,
    }
}

/// stikk's letter 012 in the store: `README.md` sorts before the conflicting `extra.txt`, so before the
/// planner it was written before the refusal. Now both writers refuse with nothing written, no marker
/// set, and a `Precondition` naming the path.
#[test]
fn a_refused_patch_materialization_writes_nothing_and_sets_no_marker() -> prikk_error::Result<()> {
    let root = unique_temp_dir("checkout-refusal-writes-nothing");
    let layout = RepositoryLayout::init(root.clone())?;
    publish_snapshot_then_patch_block(&layout)?;
    std::fs::write(root.join("extra.txt"), b"mine\n")?;
    let before = worktree_files(&root);

    let message =
        precondition_message(materialize_patch_checkout(&layout, "heads/main")).unwrap_or_default();
    assert!(
        message
            .contains("1 path(s) in the way: extra.txt (an existing file with different content)"),
        "{message}"
    );
    assert!(message.contains("nothing was written"), "{message}");
    assert_eq!(worktree_files(&root), before, "the refusal wrote README.md");
    assert!(
        !worktree_is_dirty(&layout)?,
        "the refusal set the dirty marker"
    );

    let deleting = precondition_message(materialize_patch_checkout_with_deletions(
        &layout,
        "heads/main",
    ))
    .unwrap_or_default();
    assert!(
        deleting.contains("extra.txt (an existing file"),
        "{deleting}"
    );
    assert_eq!(
        worktree_files(&root),
        before,
        "the deleting variant wrote README.md"
    );
    assert!(!worktree_is_dirty(&layout)?);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Two conflicting paths of different kinds are both named, not the first.
#[test]
fn every_conflicting_path_is_named() -> prikk_error::Result<()> {
    let root = unique_temp_dir("checkout-refusal-two-conflicts");
    let layout = RepositoryLayout::init(root.clone())?;
    publish_snapshot_then_patch_block(&layout)?;
    std::fs::write(root.join("README.md"), b"local\n")?;
    std::fs::create_dir(root.join("extra.txt"))?;
    let message =
        precondition_message(materialize_patch_checkout(&layout, "heads/main")).unwrap_or_default();
    assert!(message.contains("2 path(s) in the way"), "{message}");
    assert!(
        message.contains("README.md (an existing file with different content)"),
        "{message}"
    );
    assert!(
        message.contains("extra.txt (not a regular file)"),
        "{message}"
    );
    assert!(!worktree_is_dirty(&layout)?);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The second guard (§2.2): a file that changes between the plan and the write refuses at write time,
/// the dirty marker stays set, `doctor` reports it, and the route clears it once the file is moved aside.
#[test]
fn a_file_changed_during_the_checkout_keeps_the_marker_and_names_the_route()
-> prikk_error::Result<()> {
    const CODE: &str = "PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION";
    let root = unique_temp_dir("checkout-refusal-concurrent-change");
    let layout = RepositoryLayout::init(root.clone())?;
    publish_snapshot_then_patch_block(&layout)?;
    let changed = root.join("extra.txt");
    between_plan_and_write_for_test(move || {
        let _ = std::fs::write(&changed, b"concurrent\n");
    });

    let message =
        precondition_message(materialize_patch_checkout(&layout, "heads/main")).unwrap_or_default();
    assert!(
        message.contains("extra.txt changed during the checkout"),
        "{message}"
    );
    assert!(
        message.contains("`prikk checkout --patch-materialize --ref <the current branch>`")
            && message.contains("`prikk branch switch <the current branch>`"),
        "{message}"
    );
    assert!(
        worktree_is_dirty(&layout)?,
        "the partly written worktree keeps the marker"
    );
    assert!(format!("{:?}", crate::doctor_repository(&layout)).contains(CODE));

    std::fs::remove_file(root.join("extra.txt"))?;
    materialize_patch_checkout(&layout, "heads/main")?;
    assert!(!worktree_is_dirty(&layout)?, "the route clears the marker");
    assert!(!format!("{:?}", crate::doctor_repository(&layout)).contains(CODE));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
