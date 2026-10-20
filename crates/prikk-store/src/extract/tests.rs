//! Unit tests for the upper-layer `extract` API (`prepare_extract_plan`, the no-replace rename).
//! CLI-level, end-to-end coverage lives in `crates/prikk-cli/tests/extract.rs`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use super::*;
use crate::point::{Point, PointKind};
use crate::test_gates::test_support::{
    AnchoredHistory, dummy_signature, signed_block, unique_temp_dir, write_blob,
};
use crate::{FileObjectStore, ObjectReadSnapshot, ObjectWriter, RepositoryLayout};

/// `prepare_extract_plan` refuses the whole point, before any write, when two of its own paths
/// collide under ASCII case folding -- the shape `commit` lets into its own active WAL today without
/// error (`path-safety.md`), which received history can carry too.
#[test]
fn prepare_extract_plan_refuses_case_colliding_paths() {
    let root = unique_temp_dir("extract-plan-collision");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let mut store = FileObjectStore::new(layout.clone());
    let upper_blob = write_blob(&mut store, b"upper").unwrap();
    let lower_blob = write_blob(&mut store, b"lower").unwrap();
    let patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id: NodeId::from_bytes([0x01; 32]),
                    blob_id: upper_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "readme.md".to_string(),
                    node_id: NodeId::from_bytes([0x02; 32]),
                    blob_id: lower_blob,
                    mode: 0o100644,
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        patch_payload.to_canonical_bytes().unwrap(),
    );
    patch.add_signature(dummy_signature()).unwrap();
    let patch_id = store.write_object(&patch).unwrap();
    // No real state root is needed: this crafted block is never admitted as an anchor either way
    // (it carries no snapshot), so `prepare_extract_plan` replays its one patch directly from
    // genesis and must refuse on the collision it finds there, regardless of what the block's own
    // (here, unchecked) root claims.
    let block = signed_block(BlockKind::Root, Vec::new(), vec![patch_id], None);
    let block_id = store.write_object(&block).unwrap();

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let point = Point {
        name: block_id.to_string(),
        kind: PointKind::Block,
        block_id,
    };
    let err = prepare_extract_plan(&layout, &reader, &point, false).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("case-insensitive") || message.contains("collision"),
        "{message}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// The whole plan, end to end: one admitted anchor, one entry, right content, `provisional: no`.
#[test]
fn prepare_extract_plan_builds_a_plan_from_an_admitted_anchor() {
    let mut history = AnchoredHistory::standard("extract-plan-smoke", 9);
    let tip = *history.blocks.last().unwrap();
    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
    let point = Point {
        name: "heads/main".to_string(),
        kind: PointKind::LocalBranch,
        block_id: tip,
    };
    let plan = prepare_extract_plan(&history.layout, &reader, &point, false).unwrap();
    assert!(!plan.provisional);
    assert_eq!(plan.target_block_id, tip);
    let paths: Vec<&str> = plan.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"a.txt"));
    assert!(paths.contains(&"b.txt"));
    assert!(paths.contains(&"d.txt"));
    assert!(!paths.contains(&"c.txt"), "c.txt was deleted");
    let _ = &mut history;
}

/// The no-replace rename control itself (0.54.0 step 3 Part 2b R2): the real, shipped shape, on
/// every platform including Windows. `std::fs::rename`'s own cross-platform behavior for an
/// existing directory target differs (Unix silently clobbers an empty one; Windows refuses one
/// regardless of emptiness -- see the Linux/macOS-only demonstration test below for that risk
/// stated directly), but `rename_new_directory_no_replace`'s own contract does not: it refuses an
/// existing target everywhere, which is the one fact this test pins across platforms.
#[test]
fn rename_new_directory_no_replace_refuses_an_existing_target() {
    let root = unique_temp_dir("extract-rename-control");
    let source = root.join("source");
    let target = root.join("target");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(source.join("marker.txt"), b"from source").unwrap();

    let result = rename_new_directory_no_replace(&source, &target);
    assert!(
        result.is_err(),
        "rename_new_directory_no_replace must refuse an existing target"
    );
    assert!(
        source.join("marker.txt").exists(),
        "the source must be untouched after a refused rename"
    );
    assert!(
        !target.join("marker.txt").exists(),
        "the target must never pick up the source's content on a refused rename"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// The real risk `rename_new_directory_no_replace` exists to close (R-A4), stated directly: on
/// Linux and macOS, a *plain* `rename(2)` silently replaces an existing **empty** target directory
/// (a non-empty one already refuses with `DirectoryNotEmpty`, which would make a plain rename look
/// safe when it is not -- the whole risk is specific to the empty case, which is exactly the state
/// a freshly-created, still-empty extract target is in before its first file is written).
///
/// **Windows-gated out, not merely skipped silently**: a plain `std::fs::rename` there refuses
/// whenever the destination is an existing directory at all (Rust's own documentation: the
/// destination must not be a directory), empty or not -- a different fact from the POSIX one this
/// test demonstrates, and one that would make this exact demonstration panic in CI's own
/// `windows-mutation` job rather than show anything real about this platform's own risk. The
/// cross-platform pin above (`rename_new_directory_no_replace_refuses_an_existing_target`) is what
/// actually matters on every platform; this test exists only to show *why* that pin matters on
/// Linux/macOS specifically.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_plain_rename_would_have_silently_clobbered_an_empty_target() {
    let root = unique_temp_dir("extract-rename-control-posix-risk");
    let source = root.join("source");
    let target = root.join("target");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(source.join("marker.txt"), b"from source").unwrap();

    std::fs::rename(&source, &target).expect("a plain rename over an empty directory succeeds");
    assert!(
        target.join("marker.txt").exists(),
        "fixture sanity: the plain rename must have actually replaced the target"
    );

    let _ = std::fs::remove_dir_all(root);
}
