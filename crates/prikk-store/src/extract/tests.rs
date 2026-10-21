//! Unit tests for the upper-layer `extract` API (`prepare_extract_plan`, the no-replace rename).
//! CLI-level, end-to-end coverage lives in `crates/prikk-cli/tests/extract.rs`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose, RecognitionClaimPayload,
};

use super::*;
use crate::point::{Point, PointKind};
use crate::test_gates::test_support::{
    AnchoredHistory, dummy_signature, signed_block, unique_temp_dir, write_blob,
};
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner,
    ObjectReadSnapshot, ObjectWriter, RepositoryLayout, WorktreePatchCommitOptions,
    add_trusted_maintainer, commit_worktree_changes_signed,
    rfc111_seal_simulation::simulate_one_seal,
};

/// A fresh repository with one real, admitted (replay-verified) checkpoint at genesis -- a sealed
/// `README.md` -- plus its own maintainer signer, for `--claim`'s own fixtures (0.54.0 step 3 Part
/// 3): a claim's own parent must be genuinely admitted for `provisional: no`, which needs the real
/// seal pipeline (`simulate_one_seal`), not a hand-built block (`AnchoredHistory`'s own doc: "seals
/// it through the real seal_block path, so it genuinely enters the replay-verified record").
fn admitted_repo_for_claim_fixtures(
    label: &str,
) -> (
    RepositoryLayout,
    Ed25519MaintainerSigner,
    Ed25519AuthorSigner,
    ObjectId,
) {
    let root = unique_temp_dir(label);
    let layout = RepositoryLayout::init(root).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed("extract-claim-maintainer", &[0x71; 32]).unwrap();
    let author = Ed25519AuthorSigner::from_seed("extract-claim-author", &[0x72; 32]).unwrap();
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .unwrap();
    std::fs::write(layout.root().join("README.md"), b"genesis\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "genesis",
        WorktreePatchCommitOptions::default(),
        &author,
    )
    .unwrap();
    simulate_one_seal(&layout, "heads/main", &maintainer).unwrap();
    let genesis_block = crate::refs::read_current_ref_tip_block(
        &layout,
        &ObjectReadSnapshot::open(&layout).unwrap(),
        "heads/main",
    )
    .unwrap();
    (layout, maintainer, author, genesis_block)
}

/// Store `payload` as an unsigned-but-enveloped `RecognitionClaim` (a claim's own signature is not
/// read anywhere on `extract --claim`'s own path -- see the report's own reasoning: the claimed
/// patches must independently exist, decode and apply, which is what actually gates this read, the
/// same way `cat`/`tree`'s own `ReadOnlyReport` anchoring never gates a read behind a signature
/// either).
fn write_claim(store: &mut FileObjectStore, payload: RecognitionClaimPayload) -> ObjectId {
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes().unwrap(),
    );
    envelope.add_signature(dummy_signature()).unwrap();
    store.write_object(&envelope).unwrap()
}

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
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::CaseCollision),
        "{err}"
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

/// `prepare_extract_plan_for_claim` builds the same `ExtractPlan` shape from a pending claim: its
/// own declared parent (a real, admitted checkpoint) plus its own unsealed patch, through
/// `derive_next_state_for_candidate` -- never sealing anything (0.54.0 step 3 Part 3).
#[test]
fn prepare_extract_plan_for_claim_builds_a_plan_from_an_admitted_parent() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-plan");

    // One more, real, *unsealed* patch on top of genesis: a brand-new file, written directly as a
    // Patch object (the shape a received-but-not-yet-sealed patch takes -- individually
    // addressable, never queued through the local active WAL, the same reason the collision and
    // two-parent tests above already write their own claimed patches this way).
    let mut store = FileObjectStore::new(layout.clone());
    let blob = write_blob(&mut store, b"claimed content\n").unwrap();
    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "claimed.txt".to_string(),
                node_id: NodeId::from_bytes([0x33; 32]),
                blob_id: blob,
                mode: 0o100644,
            }),
        }],
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

    let claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: genesis_block, // unread for lineage; only patch_ids/parent_block_ids are.
            patch_ids: vec![patch_id],
            parent_block_ids: vec![genesis_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let plan = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap();

    assert!(!plan.provisional, "genesis is a real, admitted checkpoint");
    assert_eq!(plan.target_block_id, genesis_block);
    // README.md (the admitted anchor's own file, streamed from its blob id, untouched by the
    // claim) plus claimed.txt (the claim's own new file).
    assert_eq!(plan.entries.len(), 2);
    let entry = plan
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "claimed.txt")
        .expect("claimed.txt must be in the plan");
    let bytes = match &entry.content {
        ExtractPlanContent::Bytes(bytes) => bytes.clone(),
        ExtractPlanContent::Blob(blob_id) => read_extract_blob_content(&reader, *blob_id).unwrap(),
    };
    assert_eq!(bytes, b"claimed content\n");
}

/// A claim declaring more than one parent (the shape a claim built from an ancestor `Merge` block
/// carries, `sync_negotiation::sender`'s own `parent_block_ids: block.parent_block_ids.clone()`) is
/// refused by name -- there is no field in the claim naming which of the two is mainline, and
/// guessing would be dishonest (0.54.0 step 3 Part 3).
#[test]
fn prepare_extract_plan_for_claim_refuses_a_claim_with_more_than_one_parent() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-two-parents");
    let mut store = FileObjectStore::new(layout.clone());
    let blob = write_blob(&mut store, b"irrelevant").unwrap();
    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "new.txt".to_string(),
                node_id: NodeId::from_bytes([0x09; 32]),
                blob_id: blob,
                mode: 0o100644,
            }),
        }],
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

    let claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: genesis_block,
            patch_ids: vec![patch_id],
            // Two parents: the shape a claim about a `Merge` block's own ancestor carries.
            parent_block_ids: vec![genesis_block, ObjectId::from_bytes([0x22; 32])],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("2 parents"), "{message}");
    assert!(
        message.contains("does not yet support a claim about a merge block"),
        "{message}"
    );
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimHasSeveralParents { count: 2 }),
        "{err}"
    );
}

/// "The case-collision refusal matters most here" (the handoff's own words): a pending claim's own
/// patch can hold a case-colliding pair that nothing but `extract`'s own check has ever validated --
/// ordinary `commit` lets it into the active WAL without error, and only `seal` would ever catch it,
/// which this claim was never sealed through (0.54.0 step 3 Part 3).
#[test]
fn prepare_extract_plan_for_claim_refuses_case_colliding_paths() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-collision");
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
                    path: "NOTES.md".to_string(),
                    node_id: NodeId::from_bytes([0x11; 32]),
                    blob_id: upper_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "notes.md".to_string(),
                    node_id: NodeId::from_bytes([0x12; 32]),
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

    let claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: genesis_block,
            patch_ids: vec![patch_id],
            parent_block_ids: vec![genesis_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("case-insensitive") || message.contains("collision"),
        "{message}"
    );
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::CaseCollision),
        "{err}"
    );
}

/// 0.54.0 step 3 Part 5d, item 4 (first bullet): an unknown claim id used to read `integrity
/// error`; it is now `Precondition`, classified `claim-not-found` -- a caller precondition (the
/// id was simply never written here), not stored damage.
#[test]
fn prepare_extract_plan_for_claim_refuses_an_unknown_claim_id_as_a_precondition() {
    let (layout, _maintainer, _author, _genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-unknown-id");
    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let unknown = ObjectId::from_bytes([0x99; 32]);
    let err = prepare_extract_plan_for_claim(&layout, &reader, unknown, false).unwrap_err();
    assert!(
        matches!(err.error, prikk_error::PrikkError::Precondition(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("does not exist"), "{err}");
    assert_eq!(err.kind, Some(crate::ExtractRefusalKind::ClaimNotFound));
}

/// 0.54.0 step 3 Part 5d, item 4 (second bullet): a real object of another type (a `Block`) named
/// by `--claim` used to let `PrikkError::ObjectTypeMismatch` bubble straight through
/// `read_typed`'s own shared default; it is now caught at this one call site and reclassified to
/// `Precondition`, `not-a-claim` -- without touching `read_typed` itself (`object_store.rs`'s own
/// default implementation, shared by every other caller that still wants the generic mismatch).
#[test]
fn prepare_extract_plan_for_claim_refuses_a_block_id_as_not_a_claim() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-not-a-claim");
    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, genesis_block, false).unwrap_err();
    assert!(
        matches!(err.error, prikk_error::PrikkError::Precondition(_)),
        "{err:?}"
    );
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::NotAClaim {
            expected: "recognition-claim".to_string(),
            actual: "block".to_string(),
        }),
        "{err}"
    );
}
