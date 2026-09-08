//! RFC 143: `prepare_patch_plan_content_report` -- content at a replayed point, for exactly the
//! requested paths.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::test_gates::test_support::{
    publish_binary_create_then_replace, publish_snapshot_then_patch_block,
    publish_text_create_then_edit_block, publish_text_edit_then_rename_path_block,
    publish_text_edit_then_unsupported_create_symlink_block, unique_temp_dir,
};
use crate::{PatchPlanContent, RepositoryLayout, prepare_patch_plan_content_report};

#[test]
fn text_content_is_the_replayed_bytes_verbatim() {
    let root = unique_temp_dir("content-report-text");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_text_create_then_edit_block(&layout, b"hello world\n", b"hello prikk\n").unwrap();

    let report =
        prepare_patch_plan_content_report(&layout, "heads/main", &["README.md".to_string()])
            .unwrap();
    assert!(report.not_found.is_empty());
    let [entry] = report.entries.as_slice() else {
        panic!("expected exactly one entry, got {:?}", report.entries);
    };
    assert_eq!(entry.path, "README.md");
    match &entry.content {
        PatchPlanContent::Text(bytes) => assert_eq!(bytes, b"hello prikk\n"),
        other => panic!("expected Text, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 143 §5: binary content reports id and declared size, never the bytes -- this is the test
/// that would fail if a future change accidentally started rendering binary content.
#[test]
fn binary_content_reports_id_and_size_not_bytes() {
    let root = unique_temp_dir("content-report-binary");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let old = b"\x00\x01old-binary-content";
    let new = b"\x00\x01new-binary-content-longer";
    let new_blob_id = publish_binary_create_then_replace(&layout, old, new).unwrap();

    let report =
        prepare_patch_plan_content_report(&layout, "heads/main", &["asset.bin".to_string()])
            .unwrap();
    assert!(report.not_found.is_empty());
    let [entry] = report.entries.as_slice() else {
        panic!("expected exactly one entry, got {:?}", report.entries);
    };
    match &entry.content {
        PatchPlanContent::Binary { blob_id, size } => {
            assert_eq!(
                *blob_id, new_blob_id,
                "must report the CURRENT blob, not the replaced one"
            );
            assert_eq!(*size, new.len() as u64);
        }
        other => panic!("expected Binary, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

/// A requested path that never existed, and one that existed but was deleted before the target
/// point, both degrade to `not_found` -- never an error (RFC 140 §7b / RFC 142 §6b: absence
/// degrades).
#[test]
fn absent_paths_degrade_to_not_found_not_error() {
    let root = unique_temp_dir("content-report-not-found");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_snapshot_then_patch_block(&layout).unwrap();

    let report = prepare_patch_plan_content_report(
        &layout,
        "heads/main",
        &[
            "old.txt".to_string(),           // existed, deleted by the patch block
            "never-existed.txt".to_string(), // never referenced at all
        ],
    )
    .unwrap();
    assert!(report.entries.is_empty(), "{:?}", report.entries);
    assert_eq!(
        report.not_found,
        vec!["old.txt".to_string(), "never-existed.txt".to_string()]
    );

    let _ = std::fs::remove_dir_all(root);
}

/// A path seeded by a snapshot boundary and never subsequently touched carries no live-node
/// entry, so its kind cannot be determined from the replayed window -- `Opaque`, not a guess.
#[test]
fn snapshot_seeded_untouched_path_is_opaque() {
    let root = unique_temp_dir("content-report-opaque");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_snapshot_then_patch_block(&layout).unwrap();

    let report =
        prepare_patch_plan_content_report(&layout, "heads/main", &["README.md".to_string()])
            .unwrap();
    assert!(report.not_found.is_empty());
    let [entry] = report.entries.as_slice() else {
        panic!("expected exactly one entry, got {:?}", report.entries);
    };
    match &entry.content {
        PatchPlanContent::Opaque { size } => assert_eq!(*size, b"hello\n".len() as u64),
        other => panic!("expected Opaque, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 143 §6b: an unsupported operation anywhere in the walked chain still fails the whole call
/// -- never folded into a degraded field alongside otherwise-successful content. `CreateSymlink`
/// remains apply-unsupported after RFC 144 increment 1 (unlike `RenamePath`, which this fixture
/// used to cover before that increment made it apply-supported -- see
/// `publish_text_edit_then_unsupported_create_symlink_block`'s own doc comment).
#[test]
fn unsupported_operation_still_propagates_as_an_error() {
    let root = unique_temp_dir("content-report-unsupported");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_text_edit_then_unsupported_create_symlink_block(&layout).unwrap();

    let result =
        prepare_patch_plan_content_report(&layout, "heads/main", &["README.md".to_string()]);
    assert!(
        result.is_err(),
        "an unsupported CreateSymlink in the chain must error, not degrade: {result:?}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 144 increment 1, control 4: content at a point over rename-containing history reports the
/// node's content at its *new* path, and the old path is no longer live.
#[test]
fn content_report_reflects_renamed_path() {
    let root = unique_temp_dir("content-report-rename");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_text_edit_then_rename_path_block(&layout).unwrap();

    let report = prepare_patch_plan_content_report(
        &layout,
        "heads/main",
        &["README2.md".to_string(), "README.md".to_string()],
    )
    .unwrap();
    assert_eq!(
        report.not_found,
        vec!["README.md".to_string()],
        "the old path must no longer be live after the rename"
    );
    let [entry] = report.entries.as_slice() else {
        panic!("expected exactly one entry, got {:?}", report.entries);
    };
    assert_eq!(entry.path, "README2.md");
    match &entry.content {
        PatchPlanContent::Text(bytes) => assert_eq!(bytes, b"alpha BETA\n"),
        other => panic!("expected Text, got {other:?}"),
    }
    assert_eq!(
        report.coverage.applied_operation_kinds,
        vec!["create-file", "edit-text", "rename-path"]
    );

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 143 §6: the coverage field states which operation kinds were actually applied and that the
/// walk is single-parent -- machine-branchable, not prose.
#[test]
fn coverage_reports_applied_kinds_and_single_parent_walk() {
    let root = unique_temp_dir("content-report-coverage");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_text_create_then_edit_block(&layout, b"hello world\n", b"hello prikk\n").unwrap();

    let report = prepare_patch_plan_content_report(&layout, "heads/main", &[]).unwrap();
    assert!(report.entries.is_empty());
    assert!(report.not_found.is_empty());
    assert_eq!(report.coverage.walk, "single-parent");
    assert_eq!(
        report.coverage.applied_operation_kinds,
        vec!["create-file", "edit-text"]
    );

    let _ = std::fs::remove_dir_all(root);
}

/// A bare request (no paths) still resolves and still reports coverage -- metadata only, per §5's
/// own "reporting metadata without content" answer for the no-path case.
#[test]
fn no_requested_paths_yields_metadata_only() {
    let root = unique_temp_dir("content-report-bare");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_text_create_then_edit_block(&layout, b"hello world\n", b"hello prikk\n").unwrap();

    let report = prepare_patch_plan_content_report(&layout, "heads/main", &[]).unwrap();
    assert!(report.entries.is_empty());
    assert!(report.not_found.is_empty());
    assert!(!report.coverage.applied_operation_kinds.is_empty());

    let _ = std::fs::remove_dir_all(root);
}
