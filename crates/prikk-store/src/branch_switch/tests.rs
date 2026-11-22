//! RFC 151 increment 2: `switch_branch`, including the crash postures the failpoints can reach.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use super::switch_branch;
use crate::foundation::fsutil::{
    TestFailPoint, clear_failpoint_for_test, fail_after_for_test, fail_once_for_test,
};
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
    publish_branch_claiming_state(layout, ref_name, seed, files, None)
}

/// [`publish_branch`], optionally with the block claiming `claimed_state` instead of the state root its patch derives. Needed to
/// publish a history whose state cannot be derived (it holds two names for one file), which `seal` would never author.
fn publish_branch_claiming_state(
    layout: &RepositoryLayout,
    ref_name: &str,
    seed: u8,
    files: Files<'_>,
    claimed_state: Option<prikk_object::MerkleRoot>,
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
    let state = match claimed_state {
        Some(claimed) => claimed,
        None => crate::derive_next_state_root(&object_store, None, &[patch_id])?,
    };
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

/// Crash at the pointer write, before its truncate: the pointer keeps its old value, the marker is set, and the same switch run
/// again completes it (the pre-RFC 168 behaviour, unchanged: the worktree is already on the target).
#[test]
fn a_switch_interrupted_before_the_pointer_write_leaves_the_old_pointer_and_resumes() {
    let (root, layout) = two_branches("switch-crash-pointer");
    fail_once_for_test(TestFailPoint::Truncate);
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

/// Crash between the pointer's truncate and its append (RFC 168 §3.3): the pointer is empty in place, the marker is set and
/// names its target. The refusal names the exact command, and running that command completes the switch. The marker's own
/// append is the first append of a switch, so skip it.
#[test]
fn a_switch_interrupted_between_the_pointer_truncate_and_append_leaves_a_torn_pointer_and_the_named_switch_completes_it()
 {
    let (root, layout) = two_branches("switch-crash-pointer-torn");
    fail_after_for_test(TestFailPoint::AppendWrite, 1);
    assert!(switch_branch(&layout, Some("heads/main"), "heads/other").is_err());
    clear_failpoint_for_test();

    assert_eq!(
        pointer(&root),
        "",
        "the pointer was emptied in place and not yet appended"
    );
    assert!(worktree_is_dirty(&layout).unwrap());
    assert_on_other(&root);
    let refusal = crate::refs::current_branch(&layout)
        .expect_err("a torn pointer is refused")
        .to_string();
    assert!(
        refusal.contains("a branch switch to heads/other was interrupted; run `prikk branch switch heads/other` to finish it"),
        "the refusal names the exact command: {refusal}"
    );

    // The command the refusal names: the CLI reads the pointer as `from`, and a torn one reads as none.
    let report = switch_branch(&layout, None, "heads/other").unwrap();
    assert_eq!((report.written_files, report.deleted_files), (0, 0));
    assert_eq!(pointer(&root), "heads/other\n");
    assert!(!worktree_is_dirty(&layout).unwrap());
    let status = crate::worktree_status::worktree_status(&layout, "heads/other").unwrap();
    assert!(
        status.changes.is_empty(),
        "status is clean after the named switch: {status:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Crash at the marker clear, after the pointer: the pointer already names the target and the marker is still set. "Already
/// on the target" must then finish the job, not return early. The pointer's own truncate is the first truncate of a switch, so
/// skip it: the marker clear is the second.
#[test]
fn a_failpoint_at_the_marker_clear_leaves_the_new_pointer_in_place() {
    let (root, layout) = two_branches("switch-crash-marker");
    fail_after_for_test(TestFailPoint::Truncate, 1);
    assert!(switch_branch(&layout, Some("heads/main"), "heads/other").is_err());
    clear_failpoint_for_test();

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

/// 0.57.0 U3: a switch to a branch whose history holds two names for one file (`Straße.txt` and `STRASSE.txt`) is refused before it
/// writes anything. `seal` would never author that state; a history sealed elsewhere or imported can hold it.
#[test]
fn a_switch_to_a_history_with_two_names_for_one_file_is_refused_before_any_write() {
    let root = unique_temp_dir("switch-collision");
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    publish_branch(&layout, "heads/main", 0x51, MAIN).expect("publish main");
    publish_branch_claiming_state(
        &layout,
        "heads/other",
        0x52,
        &[("Stra\u{df}e.txt", b"upper\n"), ("STRASSE.txt", b"lower\n")],
        Some(crate::compute_state_root(&[]).expect("the empty tree's root")),
    )
    .expect("publish other");
    materialize_patch_checkout(&layout, "heads/main").expect("materialize main");
    let before = pointer(&root);
    let result = switch_branch(&layout, Some("heads/main"), "heads/other");
    let message = match &result {
        Err(error) => error.to_string(),
        Ok(_) => panic!("the switch must be refused"),
    };
    assert!(
        message.contains("case-insensitive path collision") || message.contains("collision"),
        "{message}"
    );
    assert_eq!(pointer(&root), before, "the pointer did not move");
    assert_eq!(
        read(&root, "a.txt").as_deref(),
        Some(&b"a\n"[..]),
        "main's files are untouched"
    );
    assert_eq!(
        read(&root, "Stra\u{df}e.txt"),
        None,
        "nothing of the target was written"
    );
    let _ = std::fs::remove_dir_all(root);
}

// ---- 0.59.0 step W2: a switch never changes, on disk, a path the target's queue holds ---------------------------------------

mod queue_held {
    use std::collections::BTreeSet;

    use super::*;
    use crate::commit_boundary::worktree_patch::{
        WorktreePatchCommitOptions, commit_worktree_changes_signed,
    };
    use crate::object_store::ObjectReader;
    use crate::{Ed25519AuthorSigner, active_ref_ownership};

    fn author() -> Ed25519AuthorSigner {
        Ed25519AuthorSigner::from_seed("w2-author", &[0x71; 32]).expect("signer")
    }

    /// `heads/main` and `heads/feature` published with the same files, `heads/main` materialized, the worktree's record naming main.
    fn main_and_feature(name: &str) -> (std::path::PathBuf, RepositoryLayout) {
        let root = unique_temp_dir(name);
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let files: Files<'_> = &[("a.txt", b"base\n"), ("k.txt", b"keep\n")];
        publish_branch(&layout, "heads/main", 0x51, files).expect("publish main");
        // `branch create heads/feature --from heads/main`: the same Block, so the worktree's record is current for both.
        let main_state = RefStore::new(layout.clone())
            .read_current_ref_state_id("heads/main")
            .expect("read")
            .expect("main published");
        let envelope = crate::ObjectReadSnapshot::open(&layout)
            .expect("snapshot")
            .read_typed(main_state, ObjectType::RefState)
            .expect("read")
            .expect("main's RefState");
        let tip = prikk_object::RefStatePayload::decode_canonical(
            &envelope.canonical_payload,
            envelope.schema_version,
        )
        .expect("decode")
        .target_object_id;
        let ref_state = signed_ref_state_envelope("heads/feature", None, tip, 1);
        let ref_state_id = ref_state.object_id();
        let ref_update = signed_ref_update_envelope("heads/feature", None, ref_state_id, tip, 1);
        RefStore::new(layout.clone())
            .publish(&RefPublication {
                ref_name: "heads/feature".to_string(),
                expected_previous_ref_state_id: None,
                ref_state,
                ref_update,
            })
            .expect("publish feature");
        materialize_patch_checkout(&layout, "heads/main").expect("materialize main");
        (root, layout)
    }

    fn commit_onto_feature(layout: &RepositoryLayout, message: &str) {
        commit_worktree_changes_signed(
            layout,
            "heads/feature",
            message,
            WorktreePatchCommitOptions::default(),
            &author(),
        )
        .expect("commit onto feature");
    }

    /// Every path the switch's two derivations disagree on: the record-folded map the switch judges against, and `main`'s plain
    /// sealed baseline. Exactly the paths the queue added, changed or removed.
    fn record_folded_diff(layout: &RepositoryLayout) -> BTreeSet<String> {
        let replay = Wal::for_layout(layout, crate::DEFAULT_ACTIVE_NAME)
            .replay()
            .expect("replay");
        let folded = crate::worktree_materialize::baseline_files_against_record_or_live(
            layout,
            "heads/main",
            &replay,
        )
        .expect("record-folded");
        let sealed = crate::worktree_materialize::baseline_files(layout, "heads/main", &replay)
            .expect("sealed");
        folded
            .keys()
            .chain(sealed.keys())
            .filter(|path| folded.get(*path) != sealed.get(*path))
            .cloned()
            .collect()
    }

    fn queue_paths(layout: &RepositoryLayout) -> BTreeSet<String> {
        crate::enumerate_queued_patches(layout)
            .expect("queue")
            .iter()
            .flat_map(|patch| &patch.operations)
            .flat_map(|operation| &operation.paths)
            .filter_map(|path| match path {
                crate::QueuedPathResolution::Path(path) => Some(path.clone()),
                crate::QueuedPathResolution::Unresolved { .. } => None,
            })
            .collect()
    }

    /// One defect case: queue the work for `feature`, then `switch_branch(main -> feature)` must refuse before any write, the
    /// two derivations of "the queue's paths" must agree, and nothing on disk or in the pointer may have moved.
    fn assert_refused(
        name: &str,
        work: impl FnOnce(&std::path::Path),
        expected_paths: &[&str],
        disk: &[(&str, Option<&[u8]>)],
    ) {
        let (root, layout) = main_and_feature(name);
        work(&root);
        commit_onto_feature(&layout, "queued");
        assert_eq!(
            record_folded_diff(&layout),
            queue_paths(&layout),
            "the record-folded diff and the queue's own paths agree"
        );
        let record_before = std::fs::read(root.join(".prikk/worktree.baseline")).unwrap();
        let result = switch_branch(&layout, Some("heads/main"), "heads/feature");
        match &result {
            Err(prikk_error::PrikkError::Precondition(message)) => {
                assert!(
                    message.starts_with("refusing to switch to heads/feature: its queue holds 1 commit(s) that change "),
                    "{message}"
                );
                for path in expected_paths {
                    assert!(message.contains(path), "{path} not in {message}");
                }
                assert!(
                    message.contains("(nothing was written)")
                        && message.contains("`prikk seal --allow-no-audit --ref heads/feature` publishes them and keeps your files")
                        && message.contains("`prikk uncommit ")
                        && message.contains("takes the newest commit back and leaves your files"),
                    "{message}"
                );
            }
            other => panic!("expected the queue-held refusal, got {other:?}"),
        }
        for (path, expected) in disk {
            assert_eq!(read(&root, path).as_deref(), *expected, "{path} on disk");
        }
        assert_eq!(pointer(&root), "heads/main\n");
        assert!(
            !worktree_is_dirty(&layout).unwrap(),
            "no marker: nothing was written"
        );
        assert_eq!(
            std::fs::read(root.join(".prikk/worktree.baseline")).unwrap(),
            record_before,
            "the record is untouched"
        );
        assert!(matches!(
            active_ref_ownership(&layout, "heads/feature").unwrap(),
            crate::ActiveRefOwnership::Owned
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_queued_new_file_is_never_deleted_by_the_switch() {
        assert_refused(
            "w2-new",
            |root| std::fs::write(root.join("queued.txt"), b"q\n").unwrap(),
            &["queued.txt"],
            &[("queued.txt", Some(b"q\n")), ("a.txt", Some(b"base\n"))],
        );
    }

    #[test]
    fn a_queued_edit_is_never_reverted_by_the_switch() {
        assert_refused(
            "w2-edit",
            |root| std::fs::write(root.join("a.txt"), b"base\nedit\n").unwrap(),
            &["a.txt"],
            &[("a.txt", Some(b"base\nedit\n"))],
        );
    }

    #[test]
    fn a_queued_deletion_is_never_written_back_by_the_switch() {
        assert_refused(
            "w2-delete",
            |root| std::fs::remove_file(root.join("k.txt")).unwrap(),
            &["k.txt"],
            &[("k.txt", None), ("a.txt", Some(b"base\n"))],
        );
    }

    #[test]
    fn a_queued_rename_is_never_undone_by_the_switch() {
        assert_refused(
            "w2-rename",
            |root| {
                std::fs::rename(root.join("a.txt"), root.join("r.txt")).unwrap();
            },
            &["a.txt", "r.txt"],
            &[("a.txt", None), ("r.txt", Some(b"base\n"))],
        );
    }

    /// The clean row: the queue creates a file and deletes it again, so the switch changes nothing the queue holds.
    #[test]
    fn a_queue_that_changes_nothing_the_switch_touches_switches_as_before() {
        let (root, layout) = main_and_feature("w2-clean");
        std::fs::write(root.join("queued.txt"), b"q\n").unwrap();
        commit_onto_feature(&layout, "add");
        std::fs::remove_file(root.join("queued.txt")).unwrap();
        commit_onto_feature(&layout, "remove");
        let report = switch_branch(&layout, Some("heads/main"), "heads/feature").unwrap();
        assert_eq!((report.written_files, report.deleted_files), (0, 0));
        assert_eq!(pointer(&root), "heads/feature\n");
        let _ = std::fs::remove_dir_all(root);
    }

    /// The third-branch row: a queue owned by another ref is refusal 2's, with its own sentence.
    #[test]
    fn a_queue_for_another_branch_is_still_refusal_two() {
        let (root, layout) = main_and_feature("w2-third");
        std::fs::write(root.join("queued.txt"), b"q\n").unwrap();
        commit_onto_feature(&layout, "queued");
        publish_branch(
            &layout,
            "heads/third",
            0x53,
            &[("a.txt", b"base\n"), ("k.txt", b"keep\n")],
        )
        .unwrap();
        assert_precondition(
            switch_branch(&layout, Some("heads/main"), "heads/third"),
            &[
                "the active WAL holds unsealed work for heads/feature",
                "prikk seal --allow-no-audit --ref heads/feature",
            ],
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_sentence_names_five_paths_and_counts_the_rest_and_the_unnameable_variant_says_so() {
        use crate::queue_held::{Who, refusal_sentence_for_test};
        let id = prikk_object::ObjectId::from_bytes([7; 32]);
        let sw = Who::Switch { target: "heads/x" };
        let many = ["a", "b", "c", "d", "e", "f", "g"];
        let sentence = refusal_sentence_for_test(&sw, "heads/x", 3, id, &many, false, false);
        assert!(sentence.contains("change a, b, c, d, e and 2 more, which the switch would overwrite or delete on disk"), "{sentence}");
        assert!(sentence.contains("holds 3 commit(s)"), "{sentence}");
        let unnameable = refusal_sentence_for_test(&sw, "heads/x", 1, id, &["a"], true, false);
        let marked = refusal_sentence_for_test(&sw, "heads/x", 1, id, &["a"], false, true);
        assert!(
            marked.contains("publishes them and keeps your files; run the switch again after it")
                && !marked.contains("uncommit"),
            "an interrupted worktree: `uncommit` refuses, so it is not named: {marked}"
        );
        assert!(unnameable.contains("with changes to paths it cannot name, and the switch would overwrite or delete a on disk"), "{unnameable}");
    }

    #[test]
    fn a_producers_sentence_names_the_queues_owner_and_its_own_action() {
        use crate::queue_held::{Who, refusal_sentence_for_test};
        let id = prikk_object::ObjectId::from_bytes([7; 32]);
        let producer = Who::Producer {
            action: "merging heads/topic into heads/main",
            already_written: None,
        };
        let sentence =
            refusal_sentence_for_test(&producer, "heads/feature", 2, id, &["a.txt"], false, false);
        assert!(
        sentence.starts_with("refusing: heads/feature's queue holds 2 commit(s) that change a.txt, which merging heads/topic into heads/main would overwrite or delete on disk (nothing was written); `prikk seal --allow-no-audit --ref heads/feature` publishes them to heads/feature, or `prikk uncommit "),
        "{sentence}"
    );
        assert!(
            sentence.ends_with("takes the newest commit back and leaves your files"),
            "{sentence}"
        );
        let backstop = Who::Producer {
            action: "adopting heads/main",
            already_written: Some("the archive's objects are already stored"),
        };
        let sentence =
            refusal_sentence_for_test(&backstop, "heads/feature", 1, id, &["a.txt"], false, true);
        assert!(
            sentence.contains("(the archive's objects are already stored)")
                && sentence
                    .ends_with("publishes them to heads/feature; run this command again after it"),
            "{sentence}"
        );
    }
}
