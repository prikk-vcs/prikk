//! Retained-format mutation-boundary tests.

use crate::test_gates::test_support::{signed_patch_envelope, unique_temp_dir};
use crate::{
    ActiveLock, DEFAULT_ACTIVE_NAME, RepositoryLayout, Wal, finish_active_publication_cleanup,
    remove_active_ref_metadata, write_active_ref_metadata,
};

#[test]
fn changed_or_missing_retained_format_marker_blocks_active_and_wal_mutation() {
    for (name, marker) in [
        ("legacy", Some(b"1\n".as_slice())),
        ("legacy-v2", Some(b"2\n".as_slice())),
        ("legacy-v3", Some(b"3\n".as_slice())),
        ("legacy-v4", Some(b"4\n".as_slice())),
        ("legacy-v5", Some(b"5\n".as_slice())),
        // RFC 102 Stage 6: format 6 is now current, so "unknown" needs a value past it to still be
        // genuinely unrecognized rather than accidentally matching.
        ("unknown", Some(b"7\n".as_slice())),
        ("malformed", Some(b"2".as_slice())),
        ("missing", None),
    ] {
        let root = unique_temp_dir(&format!("retained-format-{name}"));
        let layout = RepositoryLayout::init(root.clone()).unwrap();
        write_active_ref_metadata(&layout, "heads/main").unwrap();
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        wal.append_patch(&signed_patch_envelope()).unwrap();
        let wal_before = std::fs::read(wal.path()).unwrap();
        let metadata_before = std::fs::read(layout.default_active_ref_name_path()).unwrap();
        let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();

        match marker {
            Some(bytes) => std::fs::write(layout.format_path(), bytes).unwrap(),
            None => std::fs::remove_file(layout.format_path()).unwrap(),
        }

        assert!(layout.require_current_format().is_err());
        assert!(wal.append_patch(&signed_patch_envelope()).is_err());
        assert!(wal.truncate_empty().is_err());
        assert!(wal.truncate_trailing_partial().is_err());
        assert!(write_active_ref_metadata(&layout, "heads/other").is_err());
        assert!(remove_active_ref_metadata(&layout).is_err());
        assert!(finish_active_publication_cleanup(&layout, &active_lock).is_err());
        assert_eq!(std::fs::read(wal.path()).unwrap(), wal_before);
        assert_eq!(
            std::fs::read(layout.default_active_ref_name_path()).unwrap(),
            metadata_before
        );

        drop(active_lock);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn public_cleanup_primitives_refuse_legacy_bytes_unchanged() {
    let root = unique_temp_dir("legacy-public-cleanup-refusal");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&signed_patch_envelope()).unwrap();
    let wal_before = std::fs::read(wal.path()).unwrap();
    let metadata_before = std::fs::read(layout.default_active_ref_name_path()).unwrap();
    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    std::fs::write(layout.format_path(), b"1\n").unwrap();

    let bare_wal = Wal::new(wal.path());
    assert!(bare_wal.append_patch(&signed_patch_envelope()).is_err());
    assert!(bare_wal.truncate_empty().is_err());
    assert!(bare_wal.truncate_trailing_partial().is_err());
    assert!(wal.truncate_empty().is_err());
    assert!(wal.truncate_trailing_partial().is_err());
    assert!(write_active_ref_metadata(&layout, "heads/other").is_err());
    assert!(remove_active_ref_metadata(&layout).is_err());
    assert!(finish_active_publication_cleanup(&layout, &active_lock).is_err());
    assert_eq!(std::fs::read(wal.path()).unwrap(), wal_before);
    assert_eq!(
        std::fs::read(layout.default_active_ref_name_path()).unwrap(),
        metadata_before
    );

    drop(active_lock);
    let _ = std::fs::remove_dir_all(root);
}
