//! Unit tests for the upper-layer `extract` API (`prepare_extract_plan`, the no-replace rename).
//! CLI-level, end-to-end coverage lives in `crates/prikk-cli/tests/extract.rs`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, ChangePerm, CreateFile, NodeId, ObjectEnvelope, ObjectId,
    ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, RecognitionClaimPayload,
};

use super::*;
use crate::point::{Point, PointKind};
use crate::test_gates::test_support::{
    AnchoredHistory, Signing, dummy_signature, maintainer_signature, signed_block,
    unique_temp_dir, write_blob,
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

/// Store `payload` as a `RecognitionClaim` carrying a real MAINTAINER-role signature, unadopted by
/// this fixture's own repository (0.56.0 step 3e, D2: `extract --claim` now checks every claim's own
/// signature, and an unadopted-but-present MAINTAINER signature reads `Unverifiable`, which D2 says
/// proceeds -- the same bar `show`/`sync pending` already apply). Before this round, `extract
/// --claim` checked no signature at all, so this fixture used an AUTHOR-role dummy signature
/// (`dummy_signature()`); that shape is now `Invalid` (no MAINTAINER signature present at all), so
/// every test here that expects the walk to proceed past a claim uses this instead.
fn write_claim(store: &mut FileObjectStore, payload: RecognitionClaimPayload) -> ObjectId {
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes().unwrap(),
    );
    envelope.add_signature(maintainer_signature()).unwrap();
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

/// 0.55.0 step 1 item 2 (external review 035 §3.3): a claim whose own declared parent was never
/// sealed or received -- this repository is not damaged, the parent simply has not arrived yet.
/// Classified structurally, by `read_typed`'s own `Ok(None)` for the parent id specifically, never
/// by matching a later "missing Block" error's message text.
#[test]
fn prepare_extract_plan_for_claim_refuses_an_unsealed_parent_as_claim_parent_not_sealed() {
    let (layout, _maintainer, _author, _genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-parent-not-sealed");
    let mut store = FileObjectStore::new(layout.clone());
    let blob = write_blob(&mut store, b"claimed content\n").unwrap();
    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "claimed.txt".to_string(),
                node_id: NodeId::from_bytes([0x44; 32]),
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

    // The declared parent: a well-formed id, never written to this store at all -- exactly the
    // shape a claim whose parent has not yet been sealed/received takes. Distinct from the claim's
    // own recognized block (0x11): the chain walk's own cycle detection (0.56.0 step 3e, D3) tracks
    // every block the walk has already visited, and this claim's own `block_id` is visited before
    // its `parent_block_ids` is ever resolved -- using the same id for both would make this fixture
    // a genuine (if accidental) self-referencing claim, caught as `claim-chain-cycle` rather than
    // the `claim-parent-not-sealed` case this test means to exercise.
    let recognized_block = ObjectId::from_bytes([0x11; 32]);
    let unsealed_parent = ObjectId::from_bytes([0x55; 32]);
    let claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: recognized_block,
            patch_ids: vec![patch_id],
            parent_block_ids: vec![unsealed_parent],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap_err();
    assert!(
        matches!(err.error, prikk_error::PrikkError::Precondition(_)),
        "{err:?}"
    );
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimParentNotSealed(Box::new(
            crate::ClaimParentNotSealedInfo {
                claim: claim_id,
                parent: unsealed_parent,
            }
        ))),
        "{err}"
    );
    assert!(err.to_string().contains(&claim_id.to_string()), "{err}");
    assert!(
        err.to_string().contains(&unsealed_parent.to_string()),
        "{err}"
    );
}

/// The narrow-classification sibling: a claim whose declared parent genuinely *is* sealed, but
/// whose own patch references a Patch object never written anywhere -- a different, deeper missing
/// object than the parent itself. This must stay unclassified (`kind: None`), falling through to
/// the ordinary `damaged-repository` fallback exactly as before 0.55.0 step 1 item 2 -- proving the
/// new classification above is narrow, not a blanket reclassification of every "missing object"
/// shape on this path.
#[test]
fn prepare_extract_plan_for_claim_still_falls_back_when_a_different_object_is_missing() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-other-object-missing");
    let mut store = FileObjectStore::new(layout.clone());

    // A well-formed Patch id, never written to this store -- a genuinely missing object that is
    // not the claim's own declared parent.
    let missing_patch = ObjectId::from_bytes([0x66; 32]);
    let claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: genesis_block,
            patch_ids: vec![missing_patch],
            parent_block_ids: vec![genesis_block], // genuinely sealed -- not the missing object.
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap_err();
    assert_eq!(
        err.kind, None,
        "a missing object other than the claim's own parent must stay unclassified, not \
         {:?}",
        err.kind
    );
}

// ---------------------------------------------------------------------------------------------
// 0.56.0 step 3e: the claim chain walk (D1-D4) -- every row of 2B's case table, plus the three
// single-claim output changes the ruling names directly.
// ---------------------------------------------------------------------------------------------

/// Store `payload` as a `RecognitionClaim` carrying no MAINTAINER signature at all (an AUTHOR-role
/// dummy signature only) -- `verify_claim_signature`'s own `Err` case, classified `Invalid` by
/// [`crate::recognition_claim::classify_claim_signature`]. The structural twin of a forged claim
/// (D2: "no signature at all is invalid too").
fn write_claim_unsigned(store: &mut FileObjectStore, payload: RecognitionClaimPayload) -> ObjectId {
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes().unwrap(),
    );
    envelope.add_signature(dummy_signature()).unwrap();
    store.write_object(&envelope).unwrap()
}

/// Store `payload` as a `RecognitionClaim` signed for real by `signer` -- `Sound` when `signer`'s
/// own key is adopted by `store`'s repository, `Unverifiable` otherwise (classified the same way
/// any other real signature is).
fn write_claim_signed_by(
    store: &mut FileObjectStore,
    payload: RecognitionClaimPayload,
    signer: &impl MaintainerSigner,
) -> ObjectId {
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes().unwrap(),
    );
    let claim_id = envelope.object_id();
    let signature =
        crate::maintainer_signing::maintainer_signature(signer, ObjectType::RecognitionClaim, claim_id)
            .unwrap();
    envelope.add_signature(signature).unwrap();
    store.write_object(&envelope).unwrap()
}

/// A single `CreateFile` patch at `path`/`node_byte`, author-signed with the ordinary dummy (patch
/// signatures are never read on this path -- only the claim's own MAINTAINER signature is new scope
/// this round).
fn write_simple_create_file_patch(
    store: &mut FileObjectStore,
    path: &str,
    node_byte: u8,
    content: &[u8],
) -> ObjectId {
    let blob = write_blob(store, content).unwrap();
    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_byte; 32]),
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
    store.write_object(&patch).unwrap()
}

/// A single `ChangePerm` patch naming a node id that is never live in any state this fold would
/// reach -- a genuine fold mismatch ([`crate::lifecycle_cache::replay::LifecycleReplayError::InconsistentLifecycleEffect`]),
/// not a missing object (0.56.0 step 3e, D4: this must read `claim-does-not-fold`, never
/// `damaged-repository`).
fn write_non_folding_change_perm_patch(store: &mut FileObjectStore) -> ObjectId {
    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id: NodeId::from_bytes([0xFE; 32]),
                old_mode: 0o100644,
                new_mode: 0o100755,
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
    store.write_object(&patch).unwrap()
}

/// Adopt a maintainer key under the literal id `"maintainer-key"` -- the key id
/// `crate::test_gates::test_support::maintainer_signature()`'s own fixed, fake signature always
/// names -- with a bogus (but shape-valid) public key, so that fixed fake signature now names an
/// *adopted* key whose signature bytes do not actually verify against it: a forged signature under
/// a trusted identity, `Invalid` via a genuine crypto failure rather than via "not adopted at all".
fn adopt_bogus_maintainer_key_named_maintainer_key(layout: &RepositoryLayout) {
    add_trusted_maintainer(
        layout,
        "maintainer-key",
        &prikk_hash::to_hex(&[0x42_u8; 32]),
    )
    .unwrap();
}

/// Land `block_id` as a received pointer, then remove it again: the `Block` object stays in the
/// store (still readable by its own bare id) but is no longer reachable from any tip -- the same
/// fixture shape 0.56.0 step 1b2 used to fix `show`'s own identical reachability bug.
fn orphan_block_via_received_remove(
    layout: &RepositoryLayout,
    store: &mut FileObjectStore,
    block_id: ObjectId,
) {
    let ref_state = crate::test_gates::test_support::signed_ref_state_envelope(
        "remotes/origin/heads/main",
        None,
        block_id,
        1,
    );
    let ref_state_id = store.write_object(&ref_state).unwrap();
    crate::received::write_received_pointer_for_test(
        layout,
        "remotes/origin/heads/main",
        ref_state_id,
    )
    .unwrap();
    crate::remove_received_pointer(layout, "remotes/origin/heads/main").unwrap();
}

/// **Single-claim output change 1** (the ruling's own CHANGELOG framing): a forged claim -- a
/// MAINTAINER signature naming an *adopted* key, whose bytes do not verify against it -- used to
/// extract with exit 0 (no signature check existed at all); it now refuses `claim-signature-invalid`.
#[test]
fn prepare_extract_plan_for_claim_refuses_a_forged_signature_as_claim_signature_invalid() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-forged-signature");
    adopt_bogus_maintainer_key_named_maintainer_key(&layout);
    let mut store = FileObjectStore::new(layout.clone());
    let patch_id = write_simple_create_file_patch(&mut store, "forged.txt", 0x61, b"forged\n");
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
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimSignatureInvalid(Box::new(
            crate::ClaimSignatureInvalidInfo { claim_id }
        ))),
        "{err}"
    );
}

/// **Single-claim output change 1, the structural twin**: an unsigned claim (no MAINTAINER
/// signature at all) refuses the identical way.
#[test]
fn prepare_extract_plan_for_claim_refuses_an_unsigned_claim_as_claim_signature_invalid() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-unsigned");
    let mut store = FileObjectStore::new(layout.clone());
    let patch_id = write_simple_create_file_patch(&mut store, "unsigned.txt", 0x62, b"unsigned\n");
    let claim_id = write_claim_unsigned(
        &mut store,
        RecognitionClaimPayload {
            block_id: genesis_block,
            patch_ids: vec![patch_id],
            parent_block_ids: vec![genesis_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap_err();
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimSignatureInvalid(Box::new(
            crate::ClaimSignatureInvalidInfo { claim_id }
        ))),
        "{err}"
    );
}

/// **Single-claim output change 2**: a parent `Block` that genuinely exists in the store (orphaned
/// by a `received remove`) but is reachable from no tip used to wrongly anchor `extract --claim`
/// (the old check was existence, `read_typed(...).Ok(Some(_))`); it now refuses
/// `claim-parent-not-sealed` exactly as an absent parent already did (D4: the anchor is
/// reachability, not existence).
#[test]
fn prepare_extract_plan_for_claim_refuses_an_unreachable_but_existing_parent_as_claim_parent_not_sealed()
{
    let root = unique_temp_dir("extract-claim-unreachable-parent");
    let layout = RepositoryLayout::init(root).unwrap();
    let mut store = FileObjectStore::new(layout.clone());
    let orphan_root = store
        .write_object(&signed_block(BlockKind::Root, Vec::new(), Vec::new(), None))
        .unwrap();
    orphan_block_via_received_remove(&layout, &mut store, orphan_root);

    let patch_id = write_simple_create_file_patch(&mut store, "orphan.txt", 0x63, b"orphan\n");
    let claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0x64; 32]),
            patch_ids: vec![patch_id],
            parent_block_ids: vec![orphan_root],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap_err();
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimParentNotSealed(Box::new(
            crate::ClaimParentNotSealedInfo {
                claim: claim_id,
                parent: orphan_root,
            }
        ))),
        "{err}"
    );
}

/// **Single-claim output change 3**: a claim whose own patch does not fold onto its declared
/// parent's state now reads `claim-does-not-fold`, never `damaged-repository` (D4).
#[test]
fn prepare_extract_plan_for_claim_reads_claim_does_not_fold_not_damaged_repository() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-does-not-fold");
    let mut store = FileObjectStore::new(layout.clone());
    let patch_id = write_non_folding_change_perm_patch(&mut store);
    let claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0x65; 32]),
            patch_ids: vec![patch_id],
            parent_block_ids: vec![genesis_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_id, false).unwrap_err();
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimDoesNotFold(Box::new(
            crate::ClaimDoesNotFoldInfo { claim_id }
        ))),
        "{err}"
    );
}

/// **D4's own core proof**: a two-link chain -- the named claim's own parent is not a real block,
/// but a held claim recognizes it, and *that* claim's own parent genuinely is sealed. This used to
/// refuse outright at the first hop (`claim-parent-not-sealed`); it now walks through and succeeds.
#[test]
fn prepare_extract_plan_for_claim_follows_a_two_link_chain_to_a_sealed_root() {
    let (layout, maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-two-link-chain");
    let mut store = FileObjectStore::new(layout.clone());

    let root_patch_id = write_simple_create_file_patch(&mut store, "root-link.txt", 0x71, b"root link\n");
    let intermediate_block = ObjectId::from_bytes([0x72; 32]);
    let root_claim_id = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: intermediate_block,
            patch_ids: vec![root_patch_id],
            parent_block_ids: vec![genesis_block],
        },
        &maintainer,
    );

    let foot_patch_id = write_simple_create_file_patch(&mut store, "foot-link.txt", 0x73, b"foot link\n");
    let foot_target_block = ObjectId::from_bytes([0x74; 32]);
    let foot_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: foot_target_block,
            patch_ids: vec![foot_patch_id],
            parent_block_ids: vec![intermediate_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let plan = prepare_extract_plan_for_claim(&layout, &reader, foot_claim_id, false).unwrap();

    assert!(!plan.provisional, "genesis is a real, admitted checkpoint");
    assert_eq!(plan.target_block_id, foot_target_block);
    assert_eq!(plan.claim_parent_block_id, Some(intermediate_block));
    assert_eq!(
        plan.chain.iter().map(|link| link.claim_id).collect::<Vec<_>>(),
        vec![root_claim_id, foot_claim_id],
        "root to foot order"
    );
    assert_eq!(
        plan.chain_signature,
        Some(crate::ChainSignatureOutcomeKind::Unverifiable),
        "the foot claim's own fake signature is unadopted -- the weakest outcome in the chain"
    );
    let paths: std::collections::BTreeSet<&str> =
        plan.entries.iter().map(|entry| entry.path.as_str()).collect();
    assert!(paths.contains("README.md"));
    assert!(paths.contains("root-link.txt"));
    assert!(paths.contains("foot-link.txt"));
}

/// **D1, the default**: two held claims recognize the same block, both `Unverifiable` -- refused,
/// naming both candidates, rather than silently picking one.
#[test]
fn prepare_extract_plan_for_claim_refuses_a_shared_block_with_no_sound_candidate() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-shared-block-no-sound");
    let mut store = FileObjectStore::new(layout.clone());
    let shared_block = ObjectId::from_bytes([0x81; 32]);

    let patch_a = write_simple_create_file_patch(&mut store, "a-candidate.txt", 0x82, b"a\n");
    let candidate_a = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![patch_a],
            parent_block_ids: vec![genesis_block],
        },
    );
    let patch_b = write_simple_create_file_patch(&mut store, "b-candidate.txt", 0x83, b"b\n");
    let candidate_b = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![patch_b],
            parent_block_ids: vec![genesis_block],
        },
    );

    let foot_patch_id = write_simple_create_file_patch(&mut store, "foot.txt", 0x84, b"foot\n");
    let foot_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0x85; 32]),
            patch_ids: vec![foot_patch_id],
            parent_block_ids: vec![shared_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, foot_claim_id, false).unwrap_err();
    let message = err.to_string();
    assert!(message.contains(&candidate_a.to_string()), "{message}");
    assert!(message.contains(&candidate_b.to_string()), "{message}");
    let Some(crate::ExtractRefusalKind::ClaimChainSharedBlock(info)) = &err.kind else {
        panic!("expected ClaimChainSharedBlock, got {:?}", err.kind);
    };
    assert_eq!(info.block_id, shared_block);
    assert_eq!(info.candidates.len(), 2);
    assert!(
        info.candidates
            .iter()
            .all(|candidate| candidate.outcome == crate::ChainSignatureOutcomeKind::Unverifiable)
    );
}

/// **D1, the exception**: exactly one of two candidates recognizing the same block is `Sound` --
/// used, with the other named in the report rather than silently dropped.
#[test]
fn prepare_extract_plan_for_claim_uses_the_one_sound_candidate_of_a_shared_block() {
    let (layout, maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-shared-block-one-sound");
    let mut store = FileObjectStore::new(layout.clone());
    let shared_block = ObjectId::from_bytes([0x91; 32]);

    let sound_patch = write_simple_create_file_patch(&mut store, "sound.txt", 0x92, b"sound\n");
    let sound_candidate = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![sound_patch],
            parent_block_ids: vec![genesis_block],
        },
        &maintainer,
    );
    let unverifiable_patch =
        write_simple_create_file_patch(&mut store, "unverifiable.txt", 0x93, b"unverifiable\n");
    let unverifiable_candidate = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![unverifiable_patch],
            parent_block_ids: vec![genesis_block],
        },
    );

    let foot_patch_id = write_simple_create_file_patch(&mut store, "foot.txt", 0x94, b"foot\n");
    let foot_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0x95; 32]),
            patch_ids: vec![foot_patch_id],
            parent_block_ids: vec![shared_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let plan = prepare_extract_plan_for_claim(&layout, &reader, foot_claim_id, false).unwrap();
    assert_eq!(
        plan.chain.iter().map(|link| link.claim_id).collect::<Vec<_>>(),
        vec![sound_candidate, foot_claim_id]
    );
    let chosen = &plan.chain[0];
    assert_eq!(chosen.claim_id, sound_candidate);
    assert_eq!(chosen.signature_outcome, crate::ChainSignatureOutcomeKind::Sound);
    assert_eq!(
        chosen.other_candidates.iter().map(|c| c.claim_id).collect::<Vec<_>>(),
        vec![unverifiable_candidate],
        "the losing candidate is still named, not silently dropped"
    );
}

/// **D1, two sound candidates**: two trusted maintainers contradicting each other about which claim
/// recognizes a block is refused, not resolved by picking one.
#[test]
fn prepare_extract_plan_for_claim_refuses_a_shared_block_with_two_sound_candidates() {
    let (layout, maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-shared-block-two-sound");
    let second_maintainer =
        Ed25519MaintainerSigner::from_seed("extract-claim-second-maintainer", &[0x96; 32]).unwrap();
    add_trusted_maintainer(
        &layout,
        second_maintainer.key_id(),
        &prikk_hash::to_hex(&second_maintainer.public_key_bytes()),
    )
    .unwrap();
    let mut store = FileObjectStore::new(layout.clone());
    let shared_block = ObjectId::from_bytes([0x97; 32]);

    let patch_a = write_simple_create_file_patch(&mut store, "sound-a.txt", 0x98, b"a\n");
    let candidate_a = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![patch_a],
            parent_block_ids: vec![genesis_block],
        },
        &maintainer,
    );
    let patch_b = write_simple_create_file_patch(&mut store, "sound-b.txt", 0x99, b"b\n");
    let candidate_b = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![patch_b],
            parent_block_ids: vec![genesis_block],
        },
        &second_maintainer,
    );

    let foot_patch_id = write_simple_create_file_patch(&mut store, "foot.txt", 0x9A, b"foot\n");
    let foot_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0x9B; 32]),
            patch_ids: vec![foot_patch_id],
            parent_block_ids: vec![shared_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, foot_claim_id, false).unwrap_err();
    let Some(crate::ExtractRefusalKind::ClaimChainSharedBlock(info)) = &err.kind else {
        panic!("expected ClaimChainSharedBlock, got {:?}", err.kind);
    };
    assert_eq!(info.candidates.len(), 2);
    assert!(
        info.candidates
            .iter()
            .all(|candidate| candidate.outcome == crate::ChainSignatureOutcomeKind::Sound)
    );
    let message = err.to_string();
    assert!(message.contains(&candidate_a.to_string()), "{message}");
    assert!(message.contains(&candidate_b.to_string()), "{message}");
}

/// **D3's cycle refusal**: two claims whose declared parents loop back on each other -- content
/// -addressed claims cannot form this honestly, but a hostile sender can still assert one.
#[test]
fn prepare_extract_plan_for_claim_refuses_a_cycle() {
    let (layout, _maintainer, _author, _genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-cycle");
    let mut store = FileObjectStore::new(layout.clone());
    let block_x = ObjectId::from_bytes([0xB1; 32]);
    let block_y = ObjectId::from_bytes([0xB2; 32]);

    let patch_g = write_simple_create_file_patch(&mut store, "g.txt", 0xB3, b"g\n");
    let claim_g = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: block_x,
            patch_ids: vec![patch_g],
            parent_block_ids: vec![block_y],
        },
    );
    let patch_f = write_simple_create_file_patch(&mut store, "f.txt", 0xB4, b"f\n");
    let claim_f = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: block_y,
            patch_ids: vec![patch_f],
            parent_block_ids: vec![block_x],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, claim_f, false).unwrap_err();
    let Some(crate::ExtractRefusalKind::ClaimChainCycle(info)) = &err.kind else {
        panic!("expected ClaimChainCycle, got {:?}", err.kind);
    };
    assert!(info.claim_ids.contains(&claim_f), "{info:?}");
    assert!(info.claim_ids.contains(&claim_g), "{info:?}");
}

/// **D4's merge extension**: a merge-shaped claim (more than one declared parent) mid-chain, not at
/// the foot -- refused the identical way a foot-level merge claim always was.
#[test]
fn prepare_extract_plan_for_claim_refuses_a_merge_claim_mid_chain() {
    let (layout, _maintainer, _author, _genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-merge-mid-chain");
    let mut store = FileObjectStore::new(layout.clone());
    let merge_block = ObjectId::from_bytes([0xC1; 32]);

    let patch_merge = write_simple_create_file_patch(&mut store, "merge.txt", 0xC2, b"merge\n");
    let merge_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: merge_block,
            patch_ids: vec![patch_merge],
            parent_block_ids: vec![
                ObjectId::from_bytes([0xC3; 32]),
                ObjectId::from_bytes([0xC4; 32]),
            ],
        },
    );
    let patch_foot = write_simple_create_file_patch(&mut store, "foot.txt", 0xC5, b"foot\n");
    let foot_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0xC6; 32]),
            patch_ids: vec![patch_foot],
            parent_block_ids: vec![merge_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, foot_claim_id, false).unwrap_err();
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimHasSeveralParents { count: 2 }),
        "{err}"
    );
    assert!(err.to_string().contains(&merge_claim_id.to_string()), "{err}");
}

/// **D2's "any link" framing**: an invalid MAINTAINER signature mid-chain, not on the claim named on
/// the command line, still refuses the whole chain.
#[test]
fn prepare_extract_plan_for_claim_refuses_an_invalid_signature_mid_chain() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-invalid-mid-chain");
    let mut store = FileObjectStore::new(layout.clone());
    let intermediate_block = ObjectId::from_bytes([0xD1; 32]);

    let patch_root = write_simple_create_file_patch(&mut store, "root.txt", 0xD2, b"root\n");
    let root_claim_id = write_claim_unsigned(
        &mut store,
        RecognitionClaimPayload {
            block_id: intermediate_block,
            patch_ids: vec![patch_root],
            parent_block_ids: vec![genesis_block],
        },
    );
    let patch_foot = write_simple_create_file_patch(&mut store, "foot.txt", 0xD3, b"foot\n");
    let foot_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0xD4; 32]),
            patch_ids: vec![patch_foot],
            parent_block_ids: vec![intermediate_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, foot_claim_id, false).unwrap_err();
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimSignatureInvalid(Box::new(
            crate::ClaimSignatureInvalidInfo {
                claim_id: root_claim_id,
            }
        ))),
        "the mid-chain link's own id must be named, not the claim given on the command line: {err}"
    );
}

/// **D4's "any link" framing, for fold**: a link's own patch does not fold, mid-chain, not at the
/// foot -- still reads `claim-does-not-fold`, naming that link.
#[test]
fn prepare_extract_plan_for_claim_refuses_a_fold_failure_mid_chain() {
    let (layout, _maintainer, _author, genesis_block) =
        admitted_repo_for_claim_fixtures("extract-claim-fold-failure-mid-chain");
    let mut store = FileObjectStore::new(layout.clone());
    let intermediate_block = ObjectId::from_bytes([0xE1; 32]);

    let non_folding_patch = write_non_folding_change_perm_patch(&mut store);
    let root_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: intermediate_block,
            patch_ids: vec![non_folding_patch],
            parent_block_ids: vec![genesis_block],
        },
    );
    let patch_foot = write_simple_create_file_patch(&mut store, "foot.txt", 0xE2, b"foot\n");
    let foot_claim_id = write_claim(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0xE3; 32]),
            patch_ids: vec![patch_foot],
            parent_block_ids: vec![intermediate_block],
        },
    );

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let err = prepare_extract_plan_for_claim(&layout, &reader, foot_claim_id, false).unwrap_err();
    assert_eq!(
        err.kind,
        Some(crate::ExtractRefusalKind::ClaimDoesNotFold(Box::new(
            crate::ClaimDoesNotFoldInfo {
                claim_id: root_claim_id,
            }
        ))),
        "the mid-chain link's own id must be named, not the foot claim: {err}"
    );
}

/// **D3's own bound**: the anchor's own real-block distance plus the chain's own hop count,
/// together, must not exceed `MAX_ANCHOR_DISTANCE` -- a chain long enough combined with a real
/// anchor distance close to the bound crosses it even though neither alone would.
/// `--allow-full-replay` overrides it exactly as it already does for the ref/block-id path.
#[test]
fn prepare_extract_plan_for_claim_refuses_past_the_bound_combining_anchor_distance_and_hops() {
    let mut history =
        AnchoredHistory::standard_signed("extract-claim-chain-bound", 63, Signing::Unadopted);
    let tip = *history.blocks.last().unwrap();

    let patch_g2 = write_simple_create_file_patch(&mut history.store, "chain-g2.txt", 0xE2, b"g2\n");
    let block_g2 = ObjectId::from_bytes([0xF1; 32]);
    let claim_g2 = write_claim(
        &mut history.store,
        RecognitionClaimPayload {
            block_id: block_g2,
            patch_ids: vec![patch_g2],
            parent_block_ids: vec![tip],
        },
    );
    let patch_g1 = write_simple_create_file_patch(&mut history.store, "chain-g1.txt", 0xE1, b"g1\n");
    let block_g1 = ObjectId::from_bytes([0xF2; 32]);
    let claim_g1 = write_claim(
        &mut history.store,
        RecognitionClaimPayload {
            block_id: block_g1,
            patch_ids: vec![patch_g1],
            parent_block_ids: vec![block_g2],
        },
    );
    let _ = claim_g1;
    let patch_f = write_simple_create_file_patch(&mut history.store, "chain-f.txt", 0xE0, b"f\n");
    let foot_claim_id = write_claim(
        &mut history.store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0xF3; 32]),
            patch_ids: vec![patch_f],
            parent_block_ids: vec![block_g1],
        },
    );
    let _ = claim_g2;

    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
    let refused =
        prepare_extract_plan_for_claim(&history.layout, &reader, foot_claim_id, false).unwrap_err();
    assert_eq!(
        refused.kind,
        Some(crate::ExtractRefusalKind::NoCheckpointInReach(
            crate::NoCheckpointReason::SignerNotAdopted
        )),
        "{refused}"
    );
    let message = refused.to_string();
    assert!(message.contains("63 blocks"), "{message}");
    assert!(message.contains("2 further"), "{message}");

    let allowed =
        prepare_extract_plan_for_claim(&history.layout, &reader, foot_claim_id, true).unwrap();
    assert!(allowed.provisional);
    assert_eq!(allowed.chain.len(), 3);
}
