//! RFC 151 increment 2: `switch_branch`, including the crash postures the failpoints can reach.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use super::switch_branch;
use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test, fail_once_for_test};
use crate::test_gates::test_support::{
    dummy_signature, signed_block_with_state_root, signed_patch_envelope,
    signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir, write_blob,
};
use crate::worktree_marker::worktree_is_dirty;
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout, Wal,
    materialize_patch_checkout, write_active_ref_metadata,
};

type Files<'a> = &'a [(&'a str, &'a [u8])];

/// Publish one root block on `ref_name` creating exactly `files`.
fn publish_branch(
    layout: &RepositoryLayout,
    ref_name: &str,
    seed: u8,
    files: Files<'_>,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let mut operations = Vec::new();
    for (index, (path, bytes)) in files.iter().enumerate() {
        let blob_id = write_blob(&mut object_store, bytes)?;
        let mut node = [seed; 32];
        node[31] = u8::try_from(index).unwrap_or(u8::MAX);
        operations.push(Operation {
            op_seq: u32::try_from(index + 1).unwrap_or(u32::MAX),
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: (*path).to_string(),
                node_id: NodeId::from_bytes(node),
                blob_id,
                mode: 0o100644,
            }),
        });
    }
    let payload = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state = crate::derive_next_state_root(&object_store, None, &[patch_id])?;
    let block =
        signed_block_with_state_root(BlockKind::Root, Vec::new(), vec![patch_id], None, state);
    let block_id = object_store.write_object(&block)?;
    let ref_state = signed_ref_state_envelope(ref_name, None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(ref_name, None, ref_state_id, block_id, 1);
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

const MAIN: Files<'static> = &[("a.txt", b"a\n"), ("shared.txt", b"main\n")];
const OTHER: Files<'static> = &[
    ("b.txt", b"b\n"),
    ("dir/shared-deep.txt", b"deep\n"),
    ("shared.txt", b"other\n"),
];

/// A repository with `heads/main` and `heads/other` published and `heads/main` materialized.
fn two_branches(name: &str) -> (std::path::PathBuf, RepositoryLayout) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    publish_branch(&layout, "heads/main", 0x51, MAIN).expect("publish main");
    publish_branch(&layout, "heads/other", 0x52, OTHER).expect("publish other");
    materialize_patch_checkout(&layout, "heads/main").expect("materialize main");
    (root, layout)
}

fn pointer(root: &std::path::Path) -> String {
    std::fs::read_to_string(root.join(".prikk/current-branch")).expect("pointer")
}

fn read(root: &std::path::Path, path: &str) -> Option<Vec<u8>> {
    std::fs::read(root.join(path)).ok()
}

fn assert_on_other(root: &std::path::Path) {
    assert_eq!(read(root, "a.txt"), None);
    assert_eq!(read(root, "b.txt").as_deref(), Some(&b"b\n"[..]));
    assert_eq!(
        read(root, "dir/shared-deep.txt").as_deref(),
        Some(&b"deep\n"[..])
    );
    assert_eq!(read(root, "shared.txt").as_deref(), Some(&b"other\n"[..]));
}

fn assert_precondition(result: prikk_error::Result<super::BranchSwitchReport>, needles: &[&str]) {
    match result {
        Err(prikk_error::PrikkError::Precondition(message)) => {
            for needle in needles {
                assert!(message.contains(needle), "{needle:?} not in {message:?}");
            }
        }
        other => panic!("expected a Precondition, got {other:?}"),
    }
}

#[test]
fn a_switch_creates_replaces_and_deletes_then_writes_the_pointer() {
    let (root, layout) = two_branches("switch-moves");
    std::fs::write(root.join("notes.txt"), b"mine\n").unwrap();

    let report = switch_branch(&layout, Some("heads/main"), "heads/other").unwrap();
    assert_eq!(
        (
            report.written_files,
            report.unchanged_files,
            report.deleted_files
        ),
        (3, 0, 1)
    );
    assert_on_other(&root);
    assert_eq!(
        read(&root, "notes.txt").as_deref(),
        Some(&b"mine\n"[..]),
        "untracked is untouched"
    );
    assert_eq!(pointer(&root), "heads/other\n");
    assert!(!worktree_is_dirty(&layout).unwrap());

    let back = switch_branch(&layout, Some("heads/other"), "heads/main").unwrap();
    assert_eq!(back.deleted_files, 2);
    assert_eq!(read(&root, "a.txt").as_deref(), Some(&b"a\n"[..]));
    assert_eq!(read(&root, "b.txt"), None);
    assert_eq!(read(&root, "shared.txt").as_deref(), Some(&b"main\n"[..]));
    assert_eq!(pointer(&root), "heads/main\n");

    let again = switch_branch(&layout, Some("heads/main"), "heads/main").unwrap();
    assert!(again.already_current);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn refusals_come_in_order_and_write_nothing() {
    let (root, layout) = two_branches("switch-refusals");

    assert_precondition(
        switch_branch(&layout, Some("heads/main"), "heads/nowhere"),
        &["heads/nowhere does not exist", "prikk branch create"],
    );

    // A tracked file modified on disk that the target lacks: the dirty refusal, not a deletion one.
    std::fs::write(root.join("a.txt"), b"edited\n").unwrap();
    assert_precondition(
        switch_branch(&layout, Some("heads/main"), "heads/other"),
        &["not clean against heads/main", "a.txt (modified)"],
    );
    std::fs::write(root.join("a.txt"), b"a\n").unwrap();

    // An untracked file where the target writes: refused before any write.
    std::fs::write(root.join("b.txt"), b"someone else's\n").unwrap();
    assert_precondition(
        switch_branch(&layout, Some("heads/main"), "heads/other"),
        &["in the way", "b.txt", "nothing was written"],
    );
    assert_eq!(
        read(&root, "b.txt").as_deref(),
        Some(&b"someone else's\n"[..])
    );
    assert_eq!(read(&root, "shared.txt").as_deref(), Some(&b"main\n"[..]));
    std::fs::remove_file(root.join("b.txt")).unwrap();

    // Unsealed work owned by a ref other than the target -- checked before cleanliness.
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    Wal::for_layout(&layout, crate::DEFAULT_ACTIVE_NAME)
        .append_patch(&signed_patch_envelope())
        .unwrap();
    std::fs::write(root.join("a.txt"), b"edited\n").unwrap();
    assert_precondition(
        switch_branch(&layout, Some("heads/main"), "heads/other"),
        &["unsealed work for heads/main", "prikk seal"],
    );

    assert_eq!(pointer(&root), "heads/main\n");
    assert!(!worktree_is_dirty(&layout).unwrap());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_unresolvable_pointer_switches_without_replacing_or_deleting() {
    let (root, layout) = two_branches("switch-no-from");
    std::fs::remove_file(root.join("shared.txt")).unwrap();

    let report = switch_branch(&layout, None, "heads/other").unwrap();
    assert_eq!(report.deleted_files, 0);
    assert_eq!(
        read(&root, "a.txt").as_deref(),
        Some(&b"a\n"[..]),
        "nothing is deleted"
    );
    assert_eq!(read(&root, "shared.txt").as_deref(), Some(&b"other\n"[..]));
    assert_eq!(pointer(&root), "heads/other\n");

    std::fs::write(root.join("shared.txt"), b"neither\n").unwrap();
    assert_precondition(
        switch_branch(&layout, None, "heads/main"),
        &["in the way", "shared.txt"],
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Crash in the delete phase (the first unlink): no torn file, every target file already in place,
/// the old file still present, the pointer unchanged, the marker set -- and the same switch run again
/// completes it.
#[test]
fn a_switch_interrupted_while_deleting_leaves_the_old_pointer_and_resumes() {
    let (root, layout) = two_branches("switch-crash-unlink");
    fail_once_for_test(TestFailPoint::Unlink);
    assert!(switch_branch(&layout, Some("heads/main"), "heads/other").is_err());

    assert_eq!(pointer(&root), "heads/main\n");
    assert!(worktree_is_dirty(&layout).unwrap());
    assert_eq!(read(&root, "a.txt").as_deref(), Some(&b"a\n"[..]));
    assert_eq!(read(&root, "b.txt").as_deref(), Some(&b"b\n"[..]));
    assert_eq!(read(&root, "shared.txt").as_deref(), Some(&b"other\n"[..]));

    let report = switch_branch(&layout, Some("heads/main"), "heads/other").unwrap();
    assert_eq!(report.deleted_files, 1);
    assert_on_other(&root);
    assert_eq!(pointer(&root), "heads/other\n");
    assert!(!worktree_is_dirty(&layout).unwrap());
    let _ = std::fs::remove_dir_all(root);
}

/// Crash at the pointer write. Rename ordinals measured on this fixture: 0 and 1 are the lifecycle
/// cache's own best-effort writes during baseline resolution, 2..=4 the three target files in path
/// order, 5 the pointer. The worktree is fully on the target, the pointer still names the old branch,
/// the marker is still set -- and the same switch run again completes it.
#[test]
fn a_switch_interrupted_at_the_pointer_write_leaves_the_old_pointer_and_resumes() {
    let (root, layout) = two_branches("switch-crash-pointer");
    fail_after_for_test(TestFailPoint::MutableRename, 5);
    assert!(switch_branch(&layout, Some("heads/main"), "heads/other").is_err());

    assert_eq!(pointer(&root), "heads/main\n");
    assert!(worktree_is_dirty(&layout).unwrap());
    assert_on_other(&root);

    let report = switch_branch(&layout, Some("heads/main"), "heads/other").unwrap();
    assert_eq!((report.written_files, report.deleted_files), (0, 0));
    assert_eq!(pointer(&root), "heads/other\n");
    assert!(!worktree_is_dirty(&layout).unwrap());
    let _ = std::fs::remove_dir_all(root);
}

/// Crash at the marker clear, after the pointer: the pointer already names the target and the
/// marker is still set. "Already on the target" must then finish the job, not return early.
#[test]
fn a_switch_interrupted_at_the_marker_clear_is_finished_by_switching_again() {
    let (root, layout) = two_branches("switch-crash-marker");
    fail_once_for_test(TestFailPoint::Truncate);
    assert!(switch_branch(&layout, Some("heads/main"), "heads/other").is_err());

    assert_eq!(pointer(&root), "heads/other\n");
    assert!(worktree_is_dirty(&layout).unwrap());
    assert_on_other(&root);

    let report = switch_branch(&layout, Some("heads/other"), "heads/other").unwrap();
    assert!(!report.already_current);
    assert!(!worktree_is_dirty(&layout).unwrap());
    let _ = std::fs::remove_dir_all(root);
}

/// Crash in the write phase, at the second target file (rename ordinal 3, see above): `b.txt` is in
/// place, `dir/shared-deep.txt` is not, `shared.txt` still holds `heads/main`'s bytes, every old file
/// is present and the pointer unchanged; the same switch run again completes it.
#[test]
fn a_switch_interrupted_while_writing_leaves_every_old_file_and_resumes() {
    let (root, layout) = two_branches("switch-crash-write");
    fail_after_for_test(TestFailPoint::MutableRename, 3);
    assert!(switch_branch(&layout, Some("heads/main"), "heads/other").is_err());

    assert_eq!(pointer(&root), "heads/main\n");
    assert!(worktree_is_dirty(&layout).unwrap());
    assert_eq!(read(&root, "a.txt").as_deref(), Some(&b"a\n"[..]));
    assert_eq!(read(&root, "b.txt").as_deref(), Some(&b"b\n"[..]));
    assert_eq!(read(&root, "dir/shared-deep.txt"), None);
    assert_eq!(read(&root, "shared.txt").as_deref(), Some(&b"main\n"[..]));

    switch_branch(&layout, Some("heads/main"), "heads/other").unwrap();
    assert_on_other(&root);
    assert_eq!(pointer(&root), "heads/other\n");
    assert!(!worktree_is_dirty(&layout).unwrap());
    let _ = std::fs::remove_dir_all(root);
}
