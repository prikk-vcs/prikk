//! Repository verification tests.

mod local_tag_trust;
mod received_refs;
mod ref_cluster;
mod root_authority;
mod stage_containment;
mod trust;
mod wal_cluster;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, MerkleRoot,
    NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose,
};

use crate::maintainer_signing::MaintainerSigner;
use crate::wal::{WalRecord, encode_record_for_test};
use crate::{
    ActiveWalMetadataStatus, AuthorSignatureVerification, AuthorSigner, BlockStateStatus,
    DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, FileObjectStore, ObjectItemStatus,
    ObjectVerification, ObjectWriter, RepositoryLayout, RepositoryVerification, StageStatus,
    VerificationStage, Wal, author_signature, derive_next_state_root, verify_repository,
    write_active_ref_metadata,
};

use crate::test_gates::test_support::{
    dummy_signature, maintainer_signature, rollback_patch_envelope, sample_object_id,
    signed_patch_envelope, signed_ref_state_envelope, unique_temp_dir,
};

/// DC-95 Stage 2 Level 1: assert that `stage` resolved to `Failed` with a message containing
/// `expected_substring`. Every fixture that used to assert `verify_repository(&layout).is_err()` for
/// a specific check now asserts this instead -- the check's own reachability/probe reasoning
/// (recorded in each fixture's own doc comment) is unchanged; only the shape of "did the defect get
/// caught" moved from a bare `Result` to a per-stage outcome (DC-95 Stage 2 design).
pub(super) fn assert_stage_failed(
    report: &RepositoryVerification,
    stage: VerificationStage,
    expected_substring: &str,
) {
    assert!(
        report.has_stage_failure(),
        "expected at least one stage to fail, got: {report:?}"
    );
    let Some(outcome) = report
        .stage_outcomes
        .iter()
        .find(|outcome| outcome.stage == stage)
    else {
        panic!("expected a StageOutcome for stage {stage}, found none in: {report:?}");
    };
    match &outcome.status {
        StageStatus::Failed { message } => assert!(
            message.contains(expected_substring),
            "stage {stage} failed, but message {message:?} does not contain {expected_substring:?}"
        ),
        other => panic!("expected stage {stage} to be Failed, got: {other:?}"),
    }
}

/// DC-95 Stage 2 Level 2: assert that at least one Phase A object outcome (`report.object_outcomes`)
/// is `Failed` with a message containing `expected_substring`. Every fixture that used to reach this
/// defect via a whole-stage `assert_stage_failed(..., VerificationStage::Objects, ...)` now asserts
/// this instead -- item containment means a single bad object no longer fails the `Objects` stage
/// itself (`StageOutcome` for `Objects` reads `Evaluated`); the same defect now surfaces as one
/// `ObjectItemOutcome::Failed` entry among the others, which may all still be `Evaluated`.
pub(super) fn assert_object_item_failed(report: &RepositoryVerification, expected_substring: &str) {
    assert!(
        report.has_item_failure(),
        "expected at least one item to fail, got: {report:?}"
    );
    let found = report.object_outcomes.iter().any(|outcome| {
        matches!(&outcome.status, ObjectItemStatus::Failed { message } if message.contains(expected_substring))
    });
    assert!(
        found,
        "expected an object item Failed with a message containing {expected_substring:?}, got: {:?}",
        report.object_outcomes
    );
}

/// DC-95 Stage 2 Level 2: the Phase B analogue of [`assert_object_item_failed`] -- asserts at least
/// one block-state outcome (`report.block_state_outcomes`) is `Failed` with a message containing
/// `expected_substring`. `validate_block_v2_shape` and the state-root comparison both run inside
/// `verify_block_v2_state` (Phase B, `block_state.rs`), not Phase A -- a shape violation or state-root
/// mismatch was never a Phase A defect even before Level 2, it just used to fail the whole `Objects`
/// stage the same way every other defect in it did.
pub(super) fn assert_block_state_failed(report: &RepositoryVerification, expected_substring: &str) {
    assert!(
        report.has_item_failure(),
        "expected at least one item to fail, got: {report:?}"
    );
    let found = report.block_state_outcomes.iter().any(|outcome| {
        matches!(&outcome.status, BlockStateStatus::Failed { message } if message.contains(expected_substring))
    });
    assert!(
        found,
        "expected a block-state outcome Failed with a message containing {expected_substring:?}, got: {:?}",
        report.block_state_outcomes
    );
}

/// DC-95 Stage 2 Level 2 (refs half): the `verify_refs` analogue of [`assert_object_item_failed`].
/// A ref's own defect can surface in any of three places depending on where the check that catches
/// it actually lives -- a pointer file's own read (`pointer_outcomes`), a log file's own read
/// (`log_outcomes`), or `classify_ref_state` itself (`ref_item_outcomes`) -- and which one applies
/// is an implementation detail of *where* a given check runs, not something each fixture's own
/// intent needs to pin down (unlike the Phase A/Phase B split for objects, which is a load-bearing
/// distinction `stage-2-level-2-step0-v1.md` §1.2 derived). This checks all three.
pub(super) fn assert_ref_failed(report: &RepositoryVerification, expected_substring: &str) {
    assert!(
        report.has_item_failure(),
        "expected at least one item to fail, got: {report:?}"
    );
    let found = report
        .pointer_outcomes
        .iter()
        .chain(&report.log_outcomes)
        .any(|outcome| {
            matches!(&outcome.status, crate::refs::RefFileStatus::Failed { message } if message.contains(expected_substring))
        })
        || report.ref_item_outcomes.iter().any(|outcome| {
            matches!(&outcome.status, crate::refs::RefItemStatus::Failed { message } if message.contains(expected_substring))
        })
        || report.received_ref_item_outcomes.iter().any(|outcome| {
            matches!(&outcome.status, crate::refs::RefItemStatus::Failed { message } if message.contains(expected_substring))
        });
    assert!(
        found,
        "expected a ref-related outcome Failed with a message containing {expected_substring:?}, got: pointer_outcomes={:?} log_outcomes={:?} ref_item_outcomes={:?} received_ref_item_outcomes={:?}",
        report.pointer_outcomes,
        report.log_outcomes,
        report.ref_item_outcomes,
        report.received_ref_item_outcomes
    );
}

/// RFC 102 Stage 2: isolate-and-continue reading means a damaged WAL record is an item finding, not
/// a `WalReplay` stage failure -- the stage itself now evaluates cleanly around it. Mirrors
/// `assert_ref_failed`'s shape for `wal_record_outcomes`.
pub(super) fn assert_wal_item_failed(report: &RepositoryVerification, expected_substring: &str) {
    assert!(
        report.has_item_failure(),
        "expected at least one item to fail, got: {report:?}"
    );
    let found = report.wal_record_outcomes.iter().any(|outcome| {
        matches!(&outcome.status, crate::wal::WalRecordStatus::Failed { message } if message.contains(expected_substring))
    });
    assert!(
        found,
        "expected a wal_record_outcomes entry Failed with a message containing {expected_substring:?}, got: {:?}",
        report.wal_record_outcomes
    );
}

/// DC-95 Stage 1, round 2: the three "referenced object is missing" checks in `verify_block_payload`
/// (`verify.rs`) -- parent block, patch, and snapshot blob. Supersedes the older, weaker `verify_
/// repository_detects_block_with_missing_patch` (asserted only `.is_err()`) with a table asserting
/// each check's own specific message, per the round-1 condition's standard. Each fixture is shape-valid
/// (isolating the existence check from `validate_block_v2_shape`) -- a `Normal` block referencing one
/// nonexistent parent for the parent-existence row, `Root` blocks otherwise.
///
/// **The `missing-snapshot-blob` row uses a replay-correct root** (`snapshot_blob_ref` is metadata only
/// -- never read by state derivation, confirmed by tracing `derive_next_state_root`/`apply_candidate_
/// patches`, neither of which touch it -- so a correct root is always computable regardless of whether
/// the snapshot blob exists). Disabling the snapshot-blob check on this row was confirmed, by an actual
/// probe, to let `verify_repository` return `Ok` -- a clean pass, not a differently-worded rejection --
/// so this row genuinely demonstrates Stage 1's own rule. **Re-verified against a genuinely clean
/// baseline** (DC-95-stage-1-round-5-review-v1 §2-4): the original probe's fixture used a fake,
/// unadopted signer, so its `Ok` result always carried `PRIKK-TRUST-POLICY-INVALID` regardless of
/// the check's own state. Re-probed with a real, adopted signer behind the Block: disabling the
/// check now returns `Ok` with every issue vector empty. Classification unchanged: load-bearing.
///
/// **`missing-parent` and `missing-patch` use arbitrary roots, and cannot do otherwise: reported, not
/// silently inconsistent with round 1's standard.** Computing a replay-correct root for either requires
/// *reading* the referenced object to derive from it -- exactly what "missing" makes impossible. Probed
/// anyway, to learn what disabling each check actually does rather than assume it would be confounded
/// the same way an arbitrary root was in round 1: disabling the parent-existence loop still rejects the
/// block, via `validate_v2_lineage`'s own independent "format-2 parent Block {id} is missing" read in
/// Phase B; disabling the patch-existence check still rejects it, via the lifecycle-replay layer's own
/// "patch {id} is malformed (patch object is missing)" when Phase B tries to replay it. **Both are
/// redundant with a downstream read for `CurrentV6` blocks specifically** -- disabling `verify_block_
/// payload`'s own explicit check does not let a bad repository verify clean, because something else
/// already reads the same reference and fails closed too. That is a real property of the current design,
/// not a gap this round's test can paper over with a placeholder root, and it is why these two rows are
/// regression guards on `verify_block_payload`'s own message (useful for diagnostics -- "which check
/// said so" matters to an operator) rather than the "silent absence" demonstration `missing-snapshot-
/// blob` gives directly.
#[test]
fn verify_repository_detects_every_missing_referenced_object() -> Result<()> {
    type CaseFn = fn(&FileObjectStore, ObjectId) -> Result<(BlockPayload, &'static str)>;
    let cases: Vec<(&str, CaseFn)> = vec![
        ("missing-parent", |_store, missing| {
            Ok((
                BlockPayload {
                    parent_block_ids: vec![missing],
                    kind: BlockKind::Normal,
                    patch_ids: Vec::new(),
                    state_merkle_root: MerkleRoot([0xC0_u8; 32]),
                    snapshot_blob_ref: None,
                    mainline_parent_id: None,
                    merge_baseline_block_id: None,
                },
                "references missing parent block",
            ))
        }),
        ("missing-patch", |_store, missing| {
            Ok((
                BlockPayload {
                    parent_block_ids: Vec::new(),
                    kind: BlockKind::Root,
                    patch_ids: vec![missing],
                    state_merkle_root: MerkleRoot([0xC1_u8; 32]),
                    snapshot_blob_ref: None,
                    mainline_parent_id: None,
                    merge_baseline_block_id: None,
                },
                "references missing block patch",
            ))
        }),
        ("missing-snapshot-blob", |store, missing| {
            let state_merkle_root = derive_next_state_root(store, None, &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: Vec::new(),
                    kind: BlockKind::Root,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: Some(missing),
                    mainline_parent_id: None,
                    merge_baseline_block_id: None,
                },
                "references missing snapshot blob",
            ))
        }),
    ];

    for (name, case_fn) in cases {
        let root = unique_temp_dir(&format!("missing-referenced-{name}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let mut store = FileObjectStore::new(layout.clone());
        let missing = sample_object_id(&format!("{name}-target"));
        let (payload, expected_substring) = case_fn(&store, missing)?;
        write_signed_block(&mut store, &payload)?;

        let report = verify_repository(&layout)?;
        assert_object_item_failed(&report, expected_substring);
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

/// DC-92 implementation review §4: an end-to-end control that `verify` actually performs block state
/// verification through the real `verify_objects` wiring — Phase A's collection into
/// `pending_v2_blocks`, then Phase B's `verify_blocks_topological` — not merely in the unit-level
/// `verify_block_v2_state`/`verify_blocks_topological` calls `block_state::tests` exercises directly
/// against a `MemoryObjectStore`. The review found that removing the inline state-check call entirely
/// (on `main`, pre-DC-92, and again after DC-92's restructuring) left the whole workspace suite
/// green — nothing wired the two together. Built, not byte-corrupted, matching this module's own
/// `verify_repository_detects_every_missing_referenced_object`: content addressing means a post-hoc-
/// corrupted object is just a different, self-consistent valid object, never a mismatch.
#[test]
fn verify_repository_detects_block_with_state_root_mismatch() -> Result<()> {
    let root = unique_temp_dir("block-state-root-mismatch");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    // A Root block over empty history, claiming a state root that is not the true empty-state root
    // (`derive_next_state_root(&store, None, &[])`) -- wrong only in what it claims, otherwise
    // shape-valid and schema-valid.
    write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: MerkleRoot([0xEE_u8; 32]),
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )?;

    let report = verify_repository(&layout)?;
    assert_block_state_failed(&report, "state root does not match authoritative replay");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, §5.2: the 8 `validate_block_v2_shape` error arms, proven through `verify_repository`
/// rather than only at the unit level (`block_state/tests.rs`'s own `format2_parent_and_kind_matrix_
/// is_closed`/`format2_merge_shape_matrix`). The review's own probe found that disabling shape
/// validation entirely left every existing test passing -- including DC-92's own lineage-member shape
/// violation test, which calls `verify_blocks_topological` directly -- because none of them reach the
/// check through `verify_repository`. One fresh repository per row (not one growing repository across
/// rows: `verify_repository` stops at the first hard error, so a shared repository would only ever
/// prove whichever row's block sorts first by `ObjectId`, not the row under test). Each row's payload
/// is built from a shared set of real parent blocks so it's wrong only in the one field under test.
///
/// **Every row's `state_merkle_root` is the replay-*correct* root for what `state_derivation_parent`
/// would resolve if shape validation did not run first** -- computed via `derive_next_state_root`
/// against exactly that parent, never an arbitrary placeholder. This is DC-92's own isolation
/// discipline (`naive_continue`'s doc comment, `block_state/tests.rs`), required here for the same
/// reason a first review round found missing: an arbitrary root lets the *state-root* check catch a
/// shape-invalid fixture instead, so disabling shape validation alone would not make the row pass and
/// the test would prove nothing about shape specifically. Confirmed by re-deriving `state_derivation_
/// parent`'s own match arms for each row: non-`Merge` kinds resolve to `parent_block_ids.first()`
/// (`None` when empty, regardless of shape validity), `Merge` resolves to `mainline_parent_id` directly
/// (`None` when absent, unchecked against `parent_block_ids` when present) -- so every row here has a
/// well-defined resolved parent, and none needed `naive_continue`'s from-scratch-continuation trick,
/// since no row builds on an already-corrupted ancestor.
///
/// **Load-bearing classification re-verified against a genuinely clean baseline** (DC-95-stage-1-
/// round-5-review-v1 §2-4): the original disable-and-restore probe used these fixtures' own fake,
/// unadopted signer, so every probe's `Ok` result always carried a `PRIKK-TRUST-POLICY-INVALID`
/// finding regardless of the shape check's own state -- the repository could never have "verified
/// clean" either way, making the original probe's result unable to distinguish load-bearing from
/// downstream-redundant. Re-probed all 8 rows with a real, adopted `Ed25519MaintainerSigner` behind
/// every Block: with the check enabled, all 8 reject with their original messages unchanged; with
/// `validate_block_v2_shape` disabled, all 8 return `Ok` with `publication_trust_issues`,
/// `ref_publication_issues`, and `signature_envelope_issues` all empty -- genuinely clean, not
/// merely `Ok`. Classification unchanged: all 8 confirmed load-bearing.
#[test]
fn verify_repository_detects_every_block_shape_violation() -> Result<()> {
    type CaseFn =
        fn(&FileObjectStore, ObjectId, ObjectId, ObjectId) -> Result<(BlockPayload, &'static str)>;
    let cases: Vec<(&str, CaseFn)> = vec![
        ("root-with-parent", |store, genesis, _a, _b| {
            // state_derivation_parent(Root, ..) = parent_block_ids.first() = Some(genesis).
            let state_merkle_root = derive_next_state_root(store, Some(genesis), &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: vec![genesis],
                    kind: BlockKind::Root,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: None,
                    merge_baseline_block_id: None,
                },
                "Root Block must have zero parents",
            ))
        }),
        ("normal-with-zero-parents", |store, _genesis, _a, _b| {
            // state_derivation_parent(Normal, []) = parent_block_ids.first() = None.
            let state_merkle_root = derive_next_state_root(store, None, &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: Vec::new(),
                    kind: BlockKind::Normal,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: None,
                    merge_baseline_block_id: None,
                },
                "Normal Block must have exactly one parent",
            ))
        }),
        ("merge-with-one-parent", |store, genesis, _a, _b| {
            // state_derivation_parent(Merge, ..) = mainline_parent_id = Some(genesis).
            let state_merkle_root = derive_next_state_root(store, Some(genesis), &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: vec![genesis],
                    kind: BlockKind::Merge,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: Some(genesis),
                    merge_baseline_block_id: Some(genesis),
                },
                "Merge Block must have exactly two parents",
            ))
        }),
        ("repair-kind-unauthorized", |store, _genesis, _a, _b| {
            // state_derivation_parent(Repair, []) = parent_block_ids.first() = None.
            let state_merkle_root = derive_next_state_root(store, None, &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: Vec::new(),
                    kind: BlockKind::Repair,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: None,
                    merge_baseline_block_id: None,
                },
                "Block kind is not authorized",
            ))
        }),
        ("root-with-mainline-field", |store, genesis, _a, _b| {
            // state_derivation_parent ignores mainline_parent_id for non-Merge kinds:
            // parent_block_ids.first() = None (parent_block_ids is empty here).
            let state_merkle_root = derive_next_state_root(store, None, &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: Vec::new(),
                    kind: BlockKind::Root,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: Some(genesis),
                    merge_baseline_block_id: None,
                },
                "must not carry a mainline parent or merge baseline",
            ))
        }),
        ("merge-without-mainline", |store, genesis, a, b| {
            // state_derivation_parent(Merge, ..) = mainline_parent_id = None here, regardless of
            // parent_block_ids -- so the resolved parent is genesis-equivalent (empty), not a or b.
            let mut parents = vec![a, b];
            parents.sort();
            let state_merkle_root = derive_next_state_root(store, None, &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: parents,
                    kind: BlockKind::Merge,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: None,
                    merge_baseline_block_id: Some(genesis),
                },
                "Merge Block must name a mainline parent",
            ))
        }),
        ("merge-mainline-not-a-parent", |store, genesis, a, b| {
            // state_derivation_parent(Merge, ..) = mainline_parent_id = Some(genesis) directly --
            // never checked against parent_block_ids at this stage, that's exactly the shape rule
            // being bypassed.
            let mut parents = vec![a, b];
            parents.sort();
            let state_merkle_root = derive_next_state_root(store, Some(genesis), &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: parents,
                    kind: BlockKind::Merge,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: Some(genesis),
                    merge_baseline_block_id: Some(genesis),
                },
                "mainline parent must be one of its own parents",
            ))
        }),
        ("merge-without-baseline", |store, _genesis, a, b| {
            // state_derivation_parent(Merge, ..) = mainline_parent_id = Some(a).
            let mut parents = vec![a, b];
            parents.sort();
            let state_merkle_root = derive_next_state_root(store, Some(a), &[])?;
            Ok((
                BlockPayload {
                    parent_block_ids: parents,
                    kind: BlockKind::Merge,
                    patch_ids: Vec::new(),
                    state_merkle_root,
                    snapshot_blob_ref: None,
                    mainline_parent_id: Some(a),
                    merge_baseline_block_id: None,
                },
                "must record the baseline confluence was proven against",
            ))
        }),
    ];

    for (name, case_fn) in cases {
        let root = unique_temp_dir(&format!("block-shape-{name}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let mut store = FileObjectStore::new(layout.clone());

        let genesis_root = derive_next_state_root(&store, None, &[])?;
        let genesis = write_signed_block(
            &mut store,
            &BlockPayload {
                parent_block_ids: Vec::new(),
                kind: BlockKind::Root,
                patch_ids: Vec::new(),
                state_merkle_root: genesis_root,
                snapshot_blob_ref: None,
                mainline_parent_id: None,
                merge_baseline_block_id: None,
            },
        )?;
        let parent_a = write_create_child(&mut store, genesis, "a.txt", 0x51)?;
        let parent_b = write_create_child(&mut store, genesis, "b.txt", 0x52)?;

        let (payload, expected_substring) = case_fn(&store, genesis, parent_a, parent_b)?;
        write_signed_block(&mut store, &payload)?;

        let report = verify_repository(&layout)?;
        assert_block_state_failed(&report, expected_substring);
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

fn write_signed_block(store: &mut FileObjectStore, payload: &BlockPayload) -> Result<ObjectId> {
    let payload_bytes = payload.to_canonical_bytes()?;
    let mut block = ObjectEnvelope::unsigned(ObjectType::Block, 2, payload_bytes);
    block.add_signature(maintainer_signature())?;
    store.write_object(&block)
}

/// Seals a child block over `parent` with one `CreateFile` patch at `path` — real, replayable
/// content, so two children of the same parent are distinguishable (a content-addressed patch-free
/// `Normal` child of a given parent is otherwise a single, unique object; there is only one way to
/// say "nothing changed").
fn write_create_child(
    store: &mut FileObjectStore,
    parent: ObjectId,
    path: &str,
    node_byte: u8,
) -> Result<ObjectId> {
    let blob = BlobPayload::new(BlobKind::Text, format!("{path}\n").into_bytes());
    let mut blob_env = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
    blob_env.add_signature(maintainer_signature())?;
    let blob_id = store.write_object(&blob_env)?;

    let patch = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_byte; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch_env = ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch.to_canonical_bytes()?);
    patch_env.add_signature(maintainer_signature())?;
    let patch_id = store.write_object(&patch_env)?;

    let state_merkle_root = derive_next_state_root(store, Some(parent), &[patch_id])?;
    write_signed_block(
        store,
        &BlockPayload {
            parent_block_ids: vec![parent],
            kind: BlockKind::Normal,
            patch_ids: vec![patch_id],
            state_merkle_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )
}

/// DC-75: a `Merge` block's `merge_baseline_block_id` is a claim `verify` independently re-derives,
/// not trusts. This constructs a `Merge` block whose recorded baseline is not reachable from either
/// parent at all — forged, distinct from `genesis`, the actual common ancestor — and confirms
/// `verify` still passes structurally (shape and state root are both genuinely valid) but reports
/// the divergence rather than silently accepting the claim.
#[test]
fn verify_repository_flags_merge_block_with_baseline_not_a_common_ancestor() -> Result<()> {
    let root = unique_temp_dir("merge-baseline-forged");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let genesis_root = derive_next_state_root(&store, None, &[])?;
    let genesis = write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: genesis_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )?;
    let mainline_parent = write_create_child(&mut store, genesis, "mainline.txt", 0x10)?;
    let secondary_parent = write_create_child(&mut store, genesis, "secondary.txt", 0x20)?;
    let mut parents = vec![mainline_parent, secondary_parent];
    parents.sort();
    let forged_baseline = sample_object_id("forged-baseline-not-an-ancestor");
    let merge_root = derive_next_state_root(&store, Some(mainline_parent), &[])?;
    let merge_block = write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: parents,
            kind: BlockKind::Merge,
            patch_ids: Vec::new(),
            state_merkle_root: merge_root,
            snapshot_blob_ref: None,
            mainline_parent_id: Some(mainline_parent),
            merge_baseline_block_id: Some(forged_baseline),
        },
    )?;

    let report = verify_repository(&layout)?;
    assert!(report.has_merge_baseline_divergence());
    assert_eq!(report.merge_baseline_divergences.len(), 1);
    let Some(divergence) = report.merge_baseline_divergences.first() else {
        panic!("expected exactly one merge-baseline divergence");
    };
    assert_eq!(divergence.block_id, merge_block);
    assert_eq!(divergence.recorded_baseline, forged_baseline);
    assert_eq!(divergence.mainline_parent_id, mainline_parent);
    assert_eq!(divergence.secondary_parent_id, secondary_parent);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The positive-path counterpart: a `Merge` block recording its true, genuine common ancestor as the
/// baseline reports no divergence.
#[test]
fn verify_repository_accepts_merge_block_with_genuine_common_ancestor_baseline() -> Result<()> {
    let root = unique_temp_dir("merge-baseline-genuine");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let genesis_root = derive_next_state_root(&store, None, &[])?;
    let genesis = write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: genesis_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )?;
    let mainline_parent = write_create_child(&mut store, genesis, "mainline.txt", 0x30)?;
    let secondary_parent = write_create_child(&mut store, genesis, "secondary.txt", 0x40)?;
    let mut parents = vec![mainline_parent, secondary_parent];
    parents.sort();
    let merge_root = derive_next_state_root(&store, Some(mainline_parent), &[])?;
    write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: parents,
            kind: BlockKind::Merge,
            patch_ids: Vec::new(),
            state_merkle_root: merge_root,
            snapshot_blob_ref: None,
            mainline_parent_id: Some(mainline_parent),
            merge_baseline_block_id: Some(genesis),
        },
    )?;

    let report = verify_repository(&layout)?;
    assert!(!report.has_merge_baseline_divergence());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 §D3.4, increment 3b, control 1 (single active) and control 2 (a hand-planted second):
/// `active_session_count` is sourced from `RepositoryLayout::active_session_names`, so a fresh
/// repository reports `1` and gains one per session planted directly -- nothing in the codebase
/// creates a second active yet, so this test constructs `active/second/` itself, the same technique
/// increment 2's own controls use. Every other field on the report is untouched by this: this test
/// does not assert on them, but the full-suite gate run for this increment separately confirmed zero
/// existing assertions moved.
#[test]
fn verify_repository_reports_active_session_count() -> Result<()> {
    let root = unique_temp_dir("verify-active-session-count");
    let layout = RepositoryLayout::init(root.clone())?;

    assert_eq!(verify_repository(&layout)?.active_session_count, 1);

    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    std::fs::write(layout.active_queue_wal_path("second"), b"")?;
    assert_eq!(verify_repository(&layout)?.active_session_count, 2);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn verify_repository_counts_objects_and_wal_records() {
    let root = unique_temp_dir("verify");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let mut store = FileObjectStore::new(layout.clone());
        let mut blob = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"payload".to_vec());
        assert!(blob.add_signature(dummy_signature()).is_ok());
        assert!(store.write_object(&blob).is_ok());

        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        assert!(write_active_ref_metadata(&layout, "heads/main").is_ok());
        assert!(wal.append_patch(&signed_patch_envelope()).is_ok());

        let report = verify_repository(&layout);
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.checked_objects, Some(1));
            assert_eq!(report.checked_blocks, Some(0));
            assert_eq!(report.checked_wal_records, Some(1));
            assert_eq!(report.persisted_wal_patches, Some(0));
            assert_eq!(report.checked_refs, Some(0));
            assert_eq!(report.checked_ref_log_records, Some(0));
            assert_eq!(report.trailing_partial_wal_bytes, Some(0));
            assert_eq!(
                report.active_wal_metadata_status,
                Some(ActiveWalMetadataStatus::ValidForNonEmptyWal {
                    ref_name: "heads/main".to_string()
                })
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// DC-66 criterion 6: `verify` reports queue ordering explicitly. Reachable only by direct file
/// tampering — `Wal::append_patch` always assigns `previous.seq + 1` — but a queue of N gives
/// "ordering" a meaning worth verifying rather than assuming from successful structural decode.
#[test]
fn verify_repository_reports_active_wal_ordering_violation() {
    let root = unique_temp_dir("verify-wal-ordering");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        // Ensure the WAL file and its parent directory exist, then overwrite with two hand-crafted
        // records sharing sequence 1 — an ordering violation no append path can produce.
        assert!(wal.append_patch(&signed_patch_envelope()).is_ok());
        let first = WalRecord {
            seq: 1,
            envelope: signed_patch_envelope(),
        };
        let second = WalRecord {
            seq: 1,
            envelope: rollback_patch_envelope(),
        };
        let mut bytes = Vec::new();
        let first_encoded = encode_record_for_test(&first);
        assert!(first_encoded.is_ok());
        if let Ok(first_encoded) = first_encoded {
            bytes.extend(first_encoded);
        }
        let second_encoded = encode_record_for_test(&second);
        assert!(second_encoded.is_ok());
        if let Ok(second_encoded) = second_encoded {
            bytes.extend(second_encoded);
        }
        assert!(std::fs::write(wal.path(), &bytes).is_ok());
        assert!(write_active_ref_metadata(&layout, "heads/main").is_ok());

        let report = verify_repository(&layout);
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.checked_wal_records, Some(2));
            assert!(report.has_active_wal_ordering_issue());
            assert_eq!(
                report.active_wal_ordering_issues,
                vec![crate::ActiveWalOrderingIssue {
                    index: 1,
                    previous_seq: 1,
                    seq: 1,
                }]
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn verify_repository_reports_missing_active_metadata_for_non_empty_wal() {
    let root = unique_temp_dir("verify-active-metadata-missing");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        assert!(wal.append_patch(&signed_patch_envelope()).is_ok());

        let report = verify_repository(&layout);
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(
                report.active_wal_metadata_status,
                Some(ActiveWalMetadataStatus::MissingForNonEmptyWal)
            );
            assert!(report.has_active_wal_metadata_integrity_issue());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn verify_repository_reports_malformed_empty_active_metadata_as_warning_state() {
    let root = unique_temp_dir("verify-active-metadata-debris");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(std::fs::write(layout.default_active_ref_name_path(), b"tags/v1").is_ok());

        let report = verify_repository(&layout);
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert!(matches!(
                report.active_wal_metadata_status,
                Some(ActiveWalMetadataStatus::InvalidForEmptyWal { .. })
            ));
            assert!(report.has_active_wal_metadata_warning());
            assert!(!report.has_active_wal_metadata_integrity_issue());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// DC-95 Stage 1, round 12: `ActiveWalMetadataStatus::InvalidForNonEmptyWal` -- the last row in §5.
/// The sibling empty-WAL case (`InvalidForEmptyWal`, above) is a non-blocking warning; this is its
/// non-empty counterpart, which `has_active_wal_metadata_integrity_issue()` treats as blocking
/// (`main.rs`'s `run_verify` priority chain). Construction mirrors the warning-state test exactly,
/// with one real WAL record appended first so the malformed metadata is read against a non-empty
/// WAL instead of an empty one -- the same malformed bytes (`"tags/v1"`, a reserved namespace
/// `validate_local_branch_ref` rejects), reaching a different arm of `classify_active_wal_metadata`
/// purely because of the WAL's own emptiness at read time.
#[test]
fn verify_repository_reports_malformed_active_metadata_for_non_empty_wal_as_integrity_issue() {
    let root = unique_temp_dir("verify-active-metadata-invalid-nonempty");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        assert!(wal.append_patch(&signed_patch_envelope()).is_ok());
        assert!(std::fs::write(layout.default_active_ref_name_path(), b"tags/v1").is_ok());

        let report = verify_repository(&layout);
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert!(matches!(
                report.active_wal_metadata_status,
                Some(ActiveWalMetadataStatus::InvalidForNonEmptyWal { .. })
            ));
            assert!(report.has_active_wal_metadata_integrity_issue());
            assert!(!report.has_active_wal_metadata_warning());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 102 Stage 3 re-target: under the pre-Stage-3 loose-file layout, "an object stored under the
/// wrong path" meant a file's location disagreed with its own content-derived id. Containers have no
/// per-object path, so the equivalent defect is an **index entry naming a location whose bytes
/// decode to a different object entirely** -- caught by `verify_objects`'s own index cross-validation
/// pass (`verify/objects.rs`: "the bytes found are validated by recomputing the content hash",
/// design-v1.md §12/§10.2), constructed here by pointing a synthetic index entry at a real, correctly
/// framed record for a *different* object.
#[test]
fn verify_repository_detects_index_entry_resolving_to_a_different_object() -> Result<()> {
    let root = unique_temp_dir("verify-index-wrong-location");
    let layout = RepositoryLayout::init(root.clone())?;
    let real_envelope = signed_patch_envelope();
    let real_id = real_envelope.object_id();
    let record_bytes = crate::foundation::container::encode_container_record_for_test(
        ObjectType::Patch,
        &real_envelope,
    )?;
    std::fs::write(
        layout.container_slot_path(
            ObjectType::Patch,
            crate::foundation::layout::ContainerSlot::A,
        ),
        &record_bytes,
    )?;
    let wrong_id = sample_object_id("not-this-patch-s-real-id");
    let bad_entry = crate::foundation::index::IndexEntry {
        object_id: wrong_id,
        object_type: ObjectType::Patch,
        slot: crate::foundation::layout::ContainerSlot::A,
        offset: 0,
        length: record_bytes.len() as u64,
        container_checksum: [0_u8; 32],
    };
    std::fs::write(
        layout.container_index_path(),
        crate::foundation::index::encode_index_record(&bad_entry)?,
    )?;
    assert_ne!(wrong_id, real_id);

    let report = verify_repository(&layout)?;
    assert_stage_failed(
        &report,
        VerificationStage::Objects,
        "resolves to an envelope with computed id",
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 3, re-targeted for RFC 102 Stage 3: `verify_object_file`'s envelope-type-
/// mismatch check moved into `container::parse_frame_at` itself (found and fixed during this very
/// re-target -- the container magic alone does not constrain what `object_type` the body's own
/// envelope claims, a real gap the original loose-file check closed and this container rewrite had
/// silently dropped until this test's own migration surfaced it). Constructed by framing a `Patch`
/// envelope under the `Blob` container's magic -- correct checksum, correct framing, wrong type. See
/// `container::tests::envelope_type_disagreeing_with_its_own_containers_type_is_rejected` for the
/// same defect proven at the container level directly; this is the same defect proven reachable end
/// to end through `verify_repository`, DC-95's own established standard for this file.
///
/// **Doubles as this round's re-target of the retired `verify_repository_detects_object_id_mismatch`**
/// (DC-95 Stage 1 round 3): that test's own defect -- a stored id disagreeing with its content -- has
/// no loose-file-shaped equivalent under containers (there is no filename to disagree with), and its
/// closest container-native analog (an index entry naming a location that decodes to a different id)
/// is now proven by `verify_repository_detects_index_entry_resolving_to_a_different_object` above,
/// not duplicated here.
#[test]
fn verify_repository_detects_envelope_type_mismatch() -> Result<()> {
    let root = unique_temp_dir("verify-envelope-type-mismatch");
    let layout = RepositoryLayout::init(root.clone())?;

    let patch_envelope = signed_patch_envelope();
    assert_eq!(patch_envelope.object_type, ObjectType::Patch);
    let record_bytes =
        crate::foundation::container::encode_container_record(ObjectType::Blob, &patch_envelope)?;
    std::fs::write(
        layout.container_slot_path(
            ObjectType::Blob,
            crate::foundation::layout::ContainerSlot::A,
        ),
        &record_bytes,
    )?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(&report, "is under type");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-53 Stage 1: a minimal single-operation Patch payload, distinguished only by `discriminant` so
/// each of the three `verify_author_signature` tests below gets its own object id.
fn author_verification_test_patch_payload(discriminant: u8) -> PatchPayload {
    PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: format!("dc53-author-verify-{discriminant}.txt"),
                node_id: NodeId::from_bytes([discriminant; 32]),
                blob_id: ObjectId::from_bytes([discriminant.wrapping_add(1); 32]),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    }
}

/// Writes a real-Ed25519-signed Patch envelope directly into the Patch container's slot A (no index
/// entry -- `Unindexed` is a valid non-failure classification, and every other object-item test in
/// this file that doesn't need indexedness skips it the same way, e.g.
/// `verify_repository_detects_envelope_type_mismatch` above). Returns the signer's `key_id` and the
/// written object's id.
fn write_author_signed_patch(
    layout: &RepositoryLayout,
    discriminant: u8,
    key_id: &str,
    corrupt_signature: bool,
) -> Result<(String, ObjectId)> {
    let payload = author_verification_test_patch_payload(discriminant);
    let canonical = payload.to_canonical_bytes()?;
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, canonical);
    let object_id = envelope.object_id();
    let signer = Ed25519AuthorSigner::from_seed(key_id, &[discriminant; 32])?;
    let mut signature = author_signature(&signer, object_id)?;
    if corrupt_signature {
        if let Some(byte) = signature.signature_bytes.get_mut(0) {
            *byte ^= 1;
        }
    }
    envelope.add_signature(signature)?;
    let record_bytes =
        crate::foundation::container::encode_container_record(ObjectType::Patch, &envelope)?;
    std::fs::write(
        layout.container_slot_path(
            ObjectType::Patch,
            crate::foundation::layout::ContainerSlot::A,
        ),
        &record_bytes,
    )?;
    Ok((signer.key_id().to_string(), object_id))
}

fn find_object_verification(
    report: &RepositoryVerification,
    object_id: ObjectId,
) -> &ObjectVerification {
    report
        .object_outcomes
        .iter()
        .find_map(|outcome| match &outcome.status {
            ObjectItemStatus::Evaluated(verification)
            | ObjectItemStatus::Unindexed(verification)
                if verification.object_id == object_id =>
            {
                Some(verification)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected an evaluated outcome for {object_id}, got: {report:?}"))
}

/// DC-53 Stage 1 implementation review v1, B2: a forged AUTHOR signature must not silently pass.
/// Reproduces the review's own negative control in reverse -- with `verify_author_signature`'s real
/// body, a tampered signature against *recorded* key material must surface as an object item
/// `Failed`, reached through `verify_repository` end to end (not asserted only against the module
/// function directly), so `verify/objects.rs:298`'s `?` propagation is what this test actually
/// exercises.
#[test]
fn verify_repository_fails_a_tampered_author_signature_against_recorded_key_material() -> Result<()>
{
    let root = unique_temp_dir("verify-author-signature-fails");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = Ed25519AuthorSigner::from_seed("fails-author", &[0x61; 32])?;
    let active_lock = crate::lock::ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    crate::author::author_key_index::record_author_key_material(
        &layout,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    write_author_signed_patch(&layout, 0x61, "fails-author", true)?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(
        &report,
        "AUTHOR signature does not verify against recorded key material",
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-53 Stage 2, D3's fourth row: a `key_id` whose recorded material already disagrees with itself
/// fails `verify` even though the Patch's own signature genuinely verifies against the *first*
/// recorded key -- D8's rule is an invariant on the container, not a claim about any one signature.
/// The conflict is planted directly (`force_conflicting_author_key_entry_for_test`), bypassing
/// `record_author_key_material`'s own Stage 2 rejection, because that rejection is exactly what
/// makes this state unreachable through normal operation -- this test exercises `verify`'s
/// defense-in-depth for a state that predates the rule or arrived some other way, reached through
/// `verify_repository` end to end, matching this file's own established discipline for every other
/// D3 row (Stage 1 implementation review v1, B2).
#[test]
fn verify_repository_fails_for_a_key_id_with_conflicting_recorded_keys() -> Result<()> {
    let root = unique_temp_dir("verify-author-signature-conflict");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = Ed25519AuthorSigner::from_seed("conflict-author", &[0x91; 32])?;
    let active_lock = crate::lock::ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    crate::author::author_key_index::record_author_key_material(
        &layout,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    crate::author::author_key_index::force_conflicting_author_key_entry_for_test(
        &layout,
        signer.key_id(),
        [0xee; 32],
    )?;
    write_author_signed_patch(&layout, 0x91, "conflict-author", false)?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(&report, "has more than one distinct recorded public key");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-53 Stage 1 implementation review v1, B2: the production authoring path and production
/// verification must agree. Records material the same way `author_signature()`'s real callers do
/// (`crate::author::author_key_index::record_author_key_material`, not a shortcut), signs through
/// `author_signature()` itself, and asserts `verify_repository` reports `Sound` -- not merely that it
/// passes, which `Unverifiable` would also do.
#[test]
fn verify_repository_reports_sound_for_a_signature_verifying_against_recorded_material()
-> Result<()> {
    let root = unique_temp_dir("verify-author-signature-sound");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = Ed25519AuthorSigner::from_seed("sound-author", &[0x71; 32])?;
    let active_lock = crate::lock::ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    crate::author::author_key_index::record_author_key_material(
        &layout,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    let (key_id, object_id) = write_author_signed_patch(&layout, 0x71, "sound-author", false)?;

    let report = verify_repository(&layout)?;
    assert!(
        !report.has_item_failure(),
        "expected no item failure, got: {report:?}"
    );
    let verification = find_object_verification(&report, object_id);
    assert_eq!(
        verification.author_verification,
        Some(AuthorSignatureVerification::Sound { key_id })
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-53 Stage 1 implementation review v1, B2: D3's second row must be asserted deliberately, not
/// inferred from other tests that would pass either way regardless of whether it fires. No call to
/// `record_author_key_material` at all -- a genuinely valid signature naming a `key_id` this
/// repository has never recorded material for.
#[test]
fn verify_repository_reports_unverifiable_for_a_key_id_with_no_recorded_material() -> Result<()> {
    let root = unique_temp_dir("verify-author-signature-unverifiable");
    let layout = RepositoryLayout::init(root.clone())?;
    let (key_id, object_id) =
        write_author_signed_patch(&layout, 0x81, "unverifiable-author", false)?;

    let report = verify_repository(&layout)?;
    assert!(
        !report.has_item_failure(),
        "expected no item failure, got: {report:?}"
    );
    let verification = find_object_verification(&report, object_id);
    assert_eq!(
        verification.author_verification,
        Some(AuthorSignatureVerification::Unverifiable { key_id })
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 3: the two "unexpected entry kind" structural checks in `verify_object_type`/
/// `verify_prefix_dir` (`verify/objects.rs`) -- a plain file sitting directly under an object-type
/// directory (where a two-character hex prefix *directory* is expected), and a directory sitting
/// inside a prefix directory (where an object *file* is expected). **Both probed, both
/// downstream-redundant** -- neither disabling arm lets `verify_repository` return `Ok`; each is
/// independently caught one layer further in, with a different, less specific message. Disabling the
/// type-directory check: `list_directory` itself rejects treating the stray file as a directory (a
/// plain filesystem `i/o error: Not a directory (os error 20)`, not an integrity error at all).
/// Disabling the prefix-directory check: `object_id_from_path` (`verify/objects.rs:291`) rejects the
/// stray directory's name for lacking a `.pobj` extension, before the entry is ever read as an object
/// file. Both rows are regression guards on `verify_object_type`'s/`verify_prefix_dir`'s own friendlier,
/// more specific messages -- worth keeping for that diagnostic value -- not demonstrations of Stage 1's
/// silent-absence rule; today's code catches both defects some other way even without these two arms.
#[test]
fn verify_repository_detects_every_directory_shape_violation() -> Result<()> {
    let non_directory_root = unique_temp_dir("verify-non-directory-in-type-dir");
    let layout = RepositoryLayout::init(non_directory_root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());
    store.write_object(&ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        b"payload".to_vec(),
    ))?;
    // Dead-surface consolidation: `objects/` + its type subdirectories are no longer in
    // `required_directories()` (nothing in a format-3 repository writes loose object files there
    // anymore), so this fixture -- deliberately probing the dormant `scan_loose_file_temp_debris`
    // check that still reads this tree -- must create it itself rather than relying on `init`.
    let object_type_dir = layout.object_type_dir(ObjectType::Blob);
    std::fs::create_dir_all(&object_type_dir)?;
    let stray_file = object_type_dir.join("zz");
    std::fs::write(&stray_file, b"not a prefix directory")?;
    let report = verify_repository(&layout)?;
    assert_stage_failed(
        &report,
        VerificationStage::Objects,
        "unexpected non-directory in object type directory",
    );
    let _ = std::fs::remove_dir_all(non_directory_root);

    let non_file_root = unique_temp_dir("verify-non-file-in-prefix-dir");
    let layout = RepositoryLayout::init(non_file_root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());
    let id = store.write_object(&ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        b"payload".to_vec(),
    ))?;
    let prefix_dir = layout
        .object_path(ObjectType::Blob, id)
        .parent()
        .ok_or_else(|| PrikkError::Io {
            kind: None,
            context: "object path has no parent".to_string(),
        })?
        .to_path_buf();
    std::fs::create_dir_all(prefix_dir.join("stray-directory"))?;
    let report = verify_repository(&layout)?;
    assert_stage_failed(
        &report,
        VerificationStage::Objects,
        "unexpected non-file in object prefix directory",
    );
    let _ = std::fs::remove_dir_all(non_file_root);
    Ok(())
}

/// DC-95 Stage 1, round 3: publication-trust failure proven for a `Block` specifically through
/// `verify_repository`, not only for a `Blob` at the unit level (`verify/tests/trust.rs`, which calls
/// `PublicationTrustVerifier` directly). Every existing Block-fixture test in this file signs with
/// `test_support::maintainer_signature()` -- a fixed, non-cryptographic placeholder signature (`key_id
/// "maintainer-key"`, `signature_bytes: vec![5; 64]`) -- and, until this test, none of them ever
/// established a trust policy at all. **That distinction matters and was learned by getting it wrong
/// first**: an absent trust policy produces `PRIKK-TRUST-POLICY-INVALID` (`verify/trust.rs:39-50`), not
/// `PRIKK-TRUST-PUBLICATION-UNTRUSTED` -- confirmed by an initial version of this test asserting the
/// wrong code and failing. The fixture below establishes a *valid* policy (via `add_trusted_maintainer`,
/// trusting a different, genuinely keyed signer) before writing the untrusted block, so the untrusted
/// signer's own key is checked against a real policy that legitimately doesn't name it, not against a
/// missing one. The trusted contrast then uses *genuinely, cryptographically* signed material --
/// `crate::Ed25519MaintainerSigner` and the real, argument-taking `crate::maintainer_signature`
/// (distinct from this file's already-imported `test_support::maintainer_signature`; the placeholder's
/// fixed bytes are not a real signature under any keypair, so trusting its literal key id would not make
/// it verify). **Not independently probed by disabling production code** -- `publication_trust_issues`
/// is accumulated, never a hard `Err` (confirmed in `verify/tests/trust.rs`'s own tests), so "disable the
/// check" would need to suppress the whole `PublicationTrustVerifier::verify` call rather than one arm;
/// the untrusted-vs-trusted contrast within this one test, checked in a single `verify_repository` call,
/// is the isolation instead.
#[test]
fn verify_repository_flags_untrusted_block_signer_and_clears_once_trusted() -> Result<()> {
    let root = unique_temp_dir("verify-untrusted-block-signer");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    // Establish a real, valid trust policy first, naming only `trusted_signer` -- so the untrusted
    // block below is checked against a policy that legitimately excludes it, not a missing one.
    let trusted_signer =
        crate::Ed25519MaintainerSigner::from_seed("verify-trust-maintainer", &[0x63; 32])?;
    let trusted_public_key_hex: String = trusted_signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(&layout, trusted_signer.key_id(), &trusted_public_key_hex)?;

    let untrusted_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: derive_next_state_root(&store, None, &[])?,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let untrusted_block_id = write_signed_block(&mut store, &untrusted_payload)?;

    // A second, independent Root block (no relationship to the first needed -- verify_objects scans
    // every persisted object regardless of any ref pointing to it, matching every other block-only
    // fixture in this file, none of which create a ref/pointer either), signed for real by the
    // already-trusted key. It names a real, existing snapshot blob -- distinguishing its payload bytes
    // (and so its content-addressed id) from the untrusted block above, which is otherwise identical --
    // without tripping the unrelated missing-snapshot-blob check.
    let snapshot_blob = BlobPayload::new(BlobKind::Text, b"trusted-block-snapshot".to_vec());
    let mut snapshot_envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, snapshot_blob.to_canonical_bytes()?);
    snapshot_envelope.add_signature(maintainer_signature())?;
    let snapshot_blob_id = snapshot_envelope.object_id();
    store.write_object(&snapshot_envelope)?;

    let trusted_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: derive_next_state_root(&store, None, &[])?,
        snapshot_blob_ref: Some(snapshot_blob_id),
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let trusted_payload_bytes = trusted_payload.to_canonical_bytes()?;
    let mut trusted_envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, trusted_payload_bytes);
    let trusted_id = trusted_envelope.object_id();
    trusted_envelope.add_signature(crate::maintainer_signature(
        &trusted_signer,
        ObjectType::Block,
        trusted_id,
    )?)?;
    store.write_object(&trusted_envelope)?;

    let report = verify_repository(&layout)?;
    assert!(report.has_publication_trust_issues());
    assert!(report.publication_trust_issues.iter().any(|issue| {
        issue.code == "PRIKK-TRUST-PUBLICATION-UNTRUSTED"
            && issue.message.contains(&untrusted_block_id.to_string())
    }));
    assert!(
        !report
            .publication_trust_issues
            .iter()
            .any(|issue| issue.message.contains(&trusted_id.to_string())),
        "the trusted block must carry no publication-trust issue of its own: {:?}",
        report.publication_trust_issues
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 4: the `RefState` half of the publication-trust check, mirroring round 3's
/// `Block` test above. `verify_object_file` (`verify/objects.rs:255`) routes `Block` and `RefState`
/// through `PublicationTrustVerifier::verify` identically (`matches!(object_type, ObjectType::Block |
/// ObjectType::RefState)`); nothing else about the check's own arm needs re-probing, but this codebase's
/// DC-95 bar is per-object-type end-to-end proof, not "the same `matches!` arm should behave the same".
/// A `RefState` object is written raw via `store.write_object`, orphaned (no ref pointer created,
/// matching the Block test's own precedent): `verify_refs`/`verify_ref_publication` only resolve
/// `RefState` objects reachable through an actual ref pointer, never by scanning the `ref-state`
/// object-type directory, so an orphan is invisible to every check except this one and the general
/// object-count scan. Two distinct `ref_name`s keep the two payloads' canonical bytes apart, unlike the
/// Block test, which needed a distinguishing snapshot-blob reference since an empty Root block has no
/// other field to vary.
#[test]
fn verify_repository_flags_untrusted_ref_state_signer_and_clears_once_trusted() -> Result<()> {
    let root = unique_temp_dir("verify-untrusted-refstate-signer");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let trusted_signer =
        crate::Ed25519MaintainerSigner::from_seed("verify-trust-maintainer-rs", &[0x64; 32])?;
    let trusted_public_key_hex: String = trusted_signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(&layout, trusted_signer.key_id(), &trusted_public_key_hex)?;

    let untrusted_envelope = signed_ref_state_envelope(
        "heads/untrusted",
        None,
        sample_object_id("untrusted-target"),
        1,
    );
    let untrusted_id = store.write_object(&untrusted_envelope)?;

    let trusted_payload = prikk_object::RefStatePayload {
        ref_name: "heads/trusted".to_string(),
        kind: prikk_object::RefKind::Branch,
        target_object_id: sample_object_id("trusted-target"),
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let trusted_bytes = trusted_payload.to_canonical_bytes()?;
    let mut trusted_envelope = ObjectEnvelope::unsigned(ObjectType::RefState, 1, trusted_bytes);
    let trusted_id = trusted_envelope.object_id();
    trusted_envelope.add_signature(crate::maintainer_signature(
        &trusted_signer,
        ObjectType::RefState,
        trusted_id,
    )?)?;
    store.write_object(&trusted_envelope)?;

    let report = verify_repository(&layout)?;
    assert!(report.has_publication_trust_issues());
    assert!(report.publication_trust_issues.iter().any(|issue| {
        issue.code == "PRIKK-TRUST-PUBLICATION-UNTRUSTED"
            && issue.message.contains(&untrusted_id.to_string())
    }));
    assert!(
        !report
            .publication_trust_issues
            .iter()
            .any(|issue| issue.message.contains(&trusted_id.to_string())),
        "the trusted ref-state must carry no publication-trust issue of its own: {:?}",
        report.publication_trust_issues
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 4: `format::validate_read_schema`'s strict-signature-shape branch
/// (`ObjectEnvelope::validate_strict`, `prikk-object/src/envelope.rs:98-117`) -- an Ed25519 signature
/// whose `signature_bytes` is not exactly 64 bytes. Previously only "Partial" coverage: `signature_
/// contract_tests/read_admission.rs`'s `format2_object_reads_reject_every_strict_envelope_failure`
/// already runs this through `verify_repository` end to end, but only asserts `.is_err()` over three
/// bundled variants (malformed shape, duplicate signature, non-canonical order) without pinning the
/// rejection to this specific one's message -- so a regression that swapped which variant fired first,
/// or which of the three rejected at all, could pass silently. This isolates the shape variant alone
/// with a specific-message assertion, matching this file's own bar for every other round.
///
/// **Must bypass both `write_object` and `verify/tests.rs`'s own established `encode_envelope_file`
/// helper** (used by every other raw-placement test in this file, e.g. the type/id-mismatch tests
/// above): both enforce `validate_strict()` at encode time and would reject a malformed-shape signature
/// before any bytes could be written. Only the `#[cfg(test)]`-only `encode_envelope_file_structural`
/// (which validates shape loosely, not strictly) permits constructing this fixture at all -- confirming
/// this rule is enforced exactly once, at read time in `verify_object_file`, with production write paths
/// closed to it entirely.
///
/// **Probed, load-bearing, confirmed -- but not the way the type/id-mismatch checks were.** Disabling
/// `validate_strict`'s `malformed_shape` arm does not produce a clean `Ok` with zero issues: the
/// downstream, independent `classify_signature_envelope` (`verify/objects.rs`) still records the same
/// defect, as a `SignatureEnvelopeIssue` with code `PRIKK-VERIFY-SIGNATURE-MALFORMED`, in `report.
/// signature_envelope_issues`. The reason this still counts as load-bearing under Stage 1's rule: unlike
/// `publication_trust_issues` or `merge_baseline_divergences`, `signature_envelope_issues` backs none of
/// `RepositoryVerification`'s eight `has_*` blocking predicates (`verify.rs:153-212`) -- the exact set
/// `run_verify`'s priority chain (`prikk-cli/src/main.rs:530-544`) reads to decide pass/fail. So a
/// malformed-shape signature caught only by the downstream classifier, with this hard check removed,
/// would report as an informational note while `prikk verify` still exits clean: precisely the "silent
/// absence lets a repository verify clean" scenario the rule is about, just realized through a
/// non-blocking sibling finding rather than through total silence.
///
/// **Re-verified against a genuinely clean baseline** (DC-95-stage-1-round-5-review-v1 §2-4): this
/// fixture, like the type/id-mismatch tests, writes only a `Blob`, never checked by `Publication
/// TrustVerifier` -- so the original probe's report (`publication_trust_issues: []`, `checked_
/// publication_trust_records: 0`, already printed at the time this classification was first made)
/// was never confounded by an absent trust policy. Re-confirmed by re-running the probe: unchanged.
#[test]
fn verify_repository_rejects_malformed_signature_shape() -> Result<()> {
    let root = unique_temp_dir("verify-malformed-signature-shape");
    let layout = RepositoryLayout::init(root.clone())?;

    let blob = BlobPayload::new(BlobKind::Text, b"strict-shape fixture\n".to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
    // Bypasses add_signature's own shape gate by setting the field directly -- Ed25519 requires
    // exactly 64 bytes; this is 63.
    envelope.signatures = vec![prikk_object::Signature {
        algorithm: prikk_object::SignatureAlgorithm::Ed25519,
        key_id: "maintainer-key".to_string(),
        signature_bytes: vec![5_u8; 63],
        created_at: 8,
        signer_role: prikk_object::SignerRole::Maintainer,
    }];

    let record_bytes = crate::foundation::container::encode_container_record_for_test(
        ObjectType::Blob,
        &envelope,
    )?;
    std::fs::write(
        layout.container_slot_path(
            ObjectType::Blob,
            crate::foundation::layout::ContainerSlot::A,
        ),
        &record_bytes,
    )?;

    let report = verify_repository(&layout)?;
    assert_object_item_failed(&report, "malformed algorithm shape");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 8: `PRIKK-TRUST-POLICY-INVALID` (`verify/trust.rs`'s `PublicationTrust
/// Verifier::verify`, backed by `load_maintainer_trust_policy`, `trust.rs`) -- proven through
/// `verify_repository` end to end, for both ways loading a policy can fail (no policy file at all;
/// a policy file present but malformed), not only at the unit level `verify/tests/trust.rs`'s own
/// `missing_policy_is_reported_once_while_count_advances`/`malformed_policy_is_reported_once_
/// while_count_advances` already cover by instantiating `PublicationTrustVerifier` directly.
///
/// **This is the check that confounded rounds 1, 2, and two of round 5's probes before the
/// classification pass** (`DC-95-stage-1-round-5-review-v1` §2; `DC-95-stage-1-classified-
/// inventory-ruling-v1.md` §3): every fixture in this file that never calls `add_trusted_maintainer`
/// -- which was every fixture, before round 3 introduced the technique -- has been silently
/// producing exactly this finding as baseline noise, without it ever being the deliberate subject
/// of its own test. This one is that test.
///
/// **No disable-and-restore probe applies here, unlike every other round-1-7 check.** There is no
/// validation branch to toggle off: `PRIKK-TRUST-POLICY-INVALID` is the direct, sole consequence of
/// `load_maintainer_trust_policy` failing, not a separate guard *against* something else that could
/// independently catch the same defect. Classified load-bearing by construction -- confirmed instead
/// by asserting the specific code reaches `report.publication_trust_issues` *and* that `report.
/// has_publication_trust_issues()` is true, i.e. it genuinely reaches the blocking `has_*` surface
/// `run_verify` decides pass/fail from, not merely that `verify_repository` returns some `Ok`.
#[test]
fn verify_repository_detects_invalid_trust_policy() -> Result<()> {
    let missing_root = unique_temp_dir("verify-trust-policy-missing");
    let layout = RepositoryLayout::init(missing_root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());
    let state_merkle_root = derive_next_state_root(&store, None, &[])?;
    write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )?;
    let report = verify_repository(&layout)?;
    assert!(report.has_publication_trust_issues());
    assert!(
        report
            .publication_trust_issues
            .iter()
            .any(|issue| issue.code == "PRIKK-TRUST-POLICY-INVALID"),
        "expected a missing-policy PRIKK-TRUST-POLICY-INVALID issue, got: {:?}",
        report.publication_trust_issues
    );
    let _ = std::fs::remove_dir_all(missing_root);

    let malformed_root = unique_temp_dir("verify-trust-policy-malformed");
    let layout = RepositoryLayout::init(malformed_root.clone())?;
    std::fs::write(
        layout.trust_policy_container_slot_path(crate::foundation::layout::ContainerSlot::A),
        b"not a valid trust policy container at all",
    )?;
    let mut store = FileObjectStore::new(layout.clone());
    let state_merkle_root = derive_next_state_root(&store, None, &[])?;
    write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )?;
    let report = verify_repository(&layout)?;
    assert!(report.has_publication_trust_issues());
    assert!(
        report
            .publication_trust_issues
            .iter()
            .any(|issue| issue.code == "PRIKK-TRUST-POLICY-INVALID"),
        "expected a malformed-policy PRIKK-TRUST-POLICY-INVALID issue, got: {:?}",
        report.publication_trust_issues
    );
    let _ = std::fs::remove_dir_all(malformed_root);
    Ok(())
}
