//! RFC 165 R2: `ensure_no_incomplete_publication` answers its own question in one pass, not
//! `verify_refs`'s refs × log scan. `ensure_no_incomplete_publication_via_verify_refs_for_test` is
//! the pre-R2 implementation, kept as a test oracle (handoff §2 item 1).
//!
//! Every sweep below covers **every** `AppendWrite` and `RequiredFileSync` ordinal, with 0 and with 3
//! settled refs beside the one crashed -- never a bare ordinal (review v2 §2 item 3).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, NodeId,
    ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
    RefKind, RefStatePayload, RefUpdatePayload,
};

use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::object_store::ObjectWriteSession;
use crate::refs::{
    ensure_no_incomplete_publication, ensure_no_incomplete_publication_via_verify_refs_for_test,
};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, FileObjectStore, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, Wal, add_trusted_maintainer, author_signature,
    maintainer_signature as sign_maintainer,
};

fn maintainer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r2-maintainer", &[0x61; 32]).expect("seed")
}

fn author() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc165-r2-author", &[0x62; 32]).expect("seed")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn setup(root: &std::path::Path) -> RepositoryLayout {
    let layout = RepositoryLayout::init(root.to_path_buf()).expect("init");
    let signer = maintainer();
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &hex_encode(&signer.public_key_bytes()),
    )
    .expect("trust");
    layout
}

fn root_block(layout: &RepositoryLayout) -> ObjectId {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).unwrap(),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut env =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes().unwrap());
    let id = env.object_id();
    env.add_signature(sign_maintainer(&maintainer(), ObjectType::Block, id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&env)
        .unwrap()
}

fn build_branch_create_publication(
    ref_name: &str,
    target_block: ObjectId,
    signer: &impl MaintainerSigner,
) -> RefPublication {
    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: target_block,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(sign_maintainer(signer, ObjectType::RefState, state_id).unwrap())
        .unwrap();
    let update = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: target_block,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut update_env = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        update.to_canonical_bytes().unwrap(),
    );
    let update_id = update_env.object_id();
    update_env
        .add_signature(sign_maintainer(signer, ObjectType::RefUpdate, update_id).unwrap())
        .unwrap();
    RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state: state_env,
        ref_update: update_env,
    }
}

fn build_branch_close_publication(
    ref_name: &str,
    previous_ref_state_id: ObjectId,
    target_block: ObjectId,
    signer: &impl MaintainerSigner,
) -> RefPublication {
    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: target_block,
        update_seq: 2,
        previous_ref_state_id: Some(previous_ref_state_id),
        required_attestation_ids: Vec::new(),
        closed: true,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 2, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(sign_maintainer(signer, ObjectType::RefState, state_id).unwrap())
        .unwrap();
    let update = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: Some(previous_ref_state_id),
        new_ref_state_id: state_id,
        new_target_object_id: target_block,
        update_seq: 2,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut update_env = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        update.to_canonical_bytes().unwrap(),
    );
    let update_id = update_env.object_id();
    update_env
        .add_signature(sign_maintainer(signer, ObjectType::RefUpdate, update_id).unwrap())
        .unwrap();
    RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: Some(previous_ref_state_id),
        ref_state: state_env,
        ref_update: update_env,
    }
}

fn queue_one_patch(layout: &RepositoryLayout, ref_name: &str, label: &str) {
    let blob = BlobPayload::new(BlobKind::Text, format!("{label}\n").into_bytes());
    let blob_envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes().unwrap());
    let blob_id = blob_envelope.object_id();
    FileObjectStore::new(layout.clone())
        .write_object(&blob_envelope)
        .unwrap();
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: format!("{label}.txt"),
                node_id: NodeId::from_bytes(*blob_id.as_bytes()),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    envelope
        .add_signature(author_signature(&author(), envelope.object_id()).unwrap())
        .unwrap();
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
    crate::write_active_ref_metadata(layout, ref_name).unwrap();
}

/// Publish `count` fully-settled, healthy refs -- "3 settled refs beside it."
fn publish_settled_refs(layout: &RepositoryLayout, count: u32) {
    let ref_store = RefStore::new(layout.clone());
    for i in 0..count {
        let name = format!("heads/settled{i}");
        let target = root_block(layout);
        let publication = build_branch_create_publication(&name, target, &maintainer());
        let mut object_store = ObjectWriteSession::open(layout).unwrap();
        ref_store
            .publish_with_object_store(&mut object_store, &publication)
            .unwrap();
    }
}

/// Sweep every `AppendWrite` and `RequiredFileSync` ordinal of `attempt` (a closure building a fresh
/// repository via `setup`, running one publication attempt against it, and returning whether it
/// succeeded), with 0 and with 3 settled refs beside it. At every ordinal: `ensure_no_incomplete_
/// publication` and the pre-R2 oracle must agree. Returns whether at least one ordinal refused (so a
/// caller can assert the sweep did not pass by exercising nothing).
fn sweep_equivalence(
    label: &str,
    attempt: impl Fn(&RepositoryLayout, TestFailPoint, usize) -> bool,
) -> bool {
    let mut any_refused = false;
    for settled in [0u32, 3] {
        for point in [TestFailPoint::AppendWrite, TestFailPoint::RequiredFileSync] {
            let mut skip = 0usize;
            loop {
                let root = unique_temp_dir(&format!(
                    "rfc165-r2-sweep-{label}-{settled}-{point:?}-{skip}"
                ));
                let layout = setup(&root);
                publish_settled_refs(&layout, settled);
                let ok = attempt(&layout, point, skip);

                let original = ensure_no_incomplete_publication_via_verify_refs_for_test(&layout);
                let fast = ensure_no_incomplete_publication(&layout);
                assert_eq!(
                    original.is_err(),
                    fast.is_err(),
                    "{label} settled={settled} {point:?} skip={skip}: original={original:?} fast={fast:?}"
                );
                if original.is_err() {
                    any_refused = true;
                }

                let _ = std::fs::remove_dir_all(&root);
                // Stop once the publication itself stops crashing (every later ordinal is past the
                // end of this publication's own write sequence) -- covers every ordinal up to and
                // including the first full success, never a bare single ordinal.
                if ok {
                    break;
                }
                skip += 1;
                assert!(
                    skip < 64,
                    "{label} {point:?}: swept past 64 ordinals without succeeding"
                );
            }
        }
    }
    any_refused
}

#[test]
fn equivalence_at_every_ordinal_branch_create() {
    let refused = sweep_equivalence("branch-create", |layout, point, skip| {
        let target = root_block(layout);
        let publication = build_branch_create_publication("heads/main", target, &maintainer());
        let ref_store = RefStore::new(layout.clone());
        let mut object_store = ObjectWriteSession::open(layout).unwrap();
        fail_after_for_test(point, skip);
        let result = ref_store.publish_with_object_store(&mut object_store, &publication);
        drop(object_store);
        clear_failpoint_for_test();
        result.is_ok()
    });
    assert!(refused, "sweep must reach at least one refusing ordinal");
}

#[test]
fn equivalence_at_every_ordinal_branch_close() {
    let refused = sweep_equivalence("branch-close", |layout, point, skip| {
        let target = root_block(layout);
        let open = build_branch_create_publication("heads/main", target, &maintainer());
        let ref_store = RefStore::new(layout.clone());
        {
            let mut object_store = ObjectWriteSession::open(layout).unwrap();
            ref_store
                .publish_with_object_store(&mut object_store, &open)
                .unwrap();
        }
        let close = build_branch_close_publication(
            "heads/main",
            open.ref_state.object_id(),
            target,
            &maintainer(),
        );
        let mut object_store = ObjectWriteSession::open(layout).unwrap();
        fail_after_for_test(point, skip);
        let result = ref_store.publish_with_object_store(&mut object_store, &close);
        drop(object_store);
        clear_failpoint_for_test();
        result.is_ok()
    });
    assert!(refused, "sweep must reach at least one refusing ordinal");
}

#[test]
fn equivalence_at_every_ordinal_tag_create() {
    use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
    use crate::tag_travel::create_local_tag;

    let refused = sweep_equivalence("tag-create", |layout, point, skip| {
        let target = root_block(layout);
        let (digest, count) = {
            let store = FileObjectStore::new(layout.clone());
            compute_patch_set_digest_and_count_from_block(&store, target).unwrap()
        };
        let mut session = ObjectWriteSession::open(layout).unwrap();
        fail_after_for_test(point, skip);
        let result = create_local_tag(
            layout,
            &mut session,
            "tags/v1",
            target,
            None,
            digest,
            count,
            &maintainer(),
        );
        drop(session);
        clear_failpoint_for_test();
        result.is_ok()
    });
    assert!(refused, "sweep must reach at least one refusing ordinal");
}

#[test]
fn equivalence_at_every_ordinal_merge() {
    use crate::merge::execute::execute_merge;

    let refused = sweep_equivalence("merge", |layout, point, skip| {
        let signer = maintainer();
        let main_target = root_block(layout);
        let main_pub = build_branch_create_publication("heads/main", main_target, &signer);
        let ref_store = RefStore::new(layout.clone());
        {
            let mut object_store = ObjectWriteSession::open(layout).unwrap();
            ref_store
                .publish_with_object_store(&mut object_store, &main_pub)
                .unwrap();
        }
        let topic_pub = build_branch_create_publication("heads/topic", main_target, &signer);
        {
            let mut object_store = ObjectWriteSession::open(layout).unwrap();
            ref_store
                .publish_with_object_store(&mut object_store, &topic_pub)
                .unwrap();
        }
        queue_one_patch(layout, "heads/topic", "m");
        crate::simulate_one_seal_for_test_support(layout, "heads/topic", &signer).unwrap();

        fail_after_for_test(point, skip);
        let result = execute_merge(layout, main_target, "heads/main", "heads/topic", &signer);
        clear_failpoint_for_test();
        result.is_ok()
    });
    assert!(refused, "sweep must reach at least one refusing ordinal");
}

#[test]
fn equivalence_at_every_ordinal_seal() {
    let refused = sweep_equivalence("seal", |layout, point, skip| {
        let signer = maintainer();
        queue_one_patch(layout, "heads/main", "s");
        fail_after_for_test(point, skip);
        let result = crate::simulate_one_seal_for_test_support(layout, "heads/main", &signer);
        clear_failpoint_for_test();
        result.is_ok()
    });
    assert!(refused, "sweep must reach at least one refusing ordinal");
}

#[test]
fn equivalence_at_every_ordinal_sync_seal() {
    use crate::seal_from_accepted::seal_from_accepted_claim;

    let refused = sweep_equivalence("sync-seal", |layout, point, skip| {
        let signer = maintainer();
        let blob = BlobPayload::new(BlobKind::Text, b"claimable\n".to_vec());
        let blob_env =
            ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes().unwrap());
        let blob_id = blob_env.object_id();
        FileObjectStore::new(layout.clone())
            .write_object(&blob_env)
            .unwrap();
        let patch = PatchPayload {
            operations: vec![Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "claimable.txt".to_string(),
                    node_id: NodeId::from_bytes(*blob_id.as_bytes()),
                    blob_id,
                    mode: 0o100_644,
                }),
            }],
            intent: None,
            preconditions: Vec::new(),
            purpose: PatchPurpose::Normal,
            message: None,
        };
        let mut patch_env =
            ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch.to_canonical_bytes().unwrap());
        patch_env
            .add_signature(author_signature(&author(), patch_env.object_id()).unwrap())
            .unwrap();
        let patch_id = FileObjectStore::new(layout.clone())
            .write_object(&patch_env)
            .unwrap();
        let claim = prikk_object::RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0x99; 32]),
            patch_ids: vec![patch_id],
            parent_block_ids: Vec::new(),
        };
        let mut claim_env = ObjectEnvelope::unsigned(
            ObjectType::RecognitionClaim,
            1,
            claim.to_canonical_bytes().unwrap(),
        );
        let claim_id = claim_env.object_id();
        claim_env
            .add_signature(
                sign_maintainer(&signer, ObjectType::RecognitionClaim, claim_id).unwrap(),
            )
            .unwrap();
        let claim_id = FileObjectStore::new(layout.clone())
            .write_object(&claim_env)
            .unwrap();

        fail_after_for_test(point, skip);
        let result = seal_from_accepted_claim(layout, "heads/main", claim_id, &signer);
        clear_failpoint_for_test();
        result.is_ok()
    });
    assert!(refused, "sweep must reach at least one refusing ordinal");
}

/// A damaged (checksum-failed) ref-log record refuses -- for both the original and the new
/// implementation.
#[test]
fn a_damaged_ref_log_record_refuses() {
    // Two generations, and the EARLIER record is the one damaged -- the later, sound record still
    // leaves the pointer and the log's newest record agreeing, so this specifically isolates the
    // damaged-record check from the pointer/log agreement check (a damaged *last* record would also
    // be caught by the agreement check alone, proving nothing about this check in particular).
    let root = unique_temp_dir("rfc165-r2-damaged-record");
    let layout = setup(&root);
    let signer = maintainer();
    let target = root_block(&layout);
    let open = build_branch_create_publication("heads/main", target, &signer);
    let ref_store = RefStore::new(layout.clone());
    {
        let mut object_store = ObjectWriteSession::open(&layout).unwrap();
        ref_store
            .publish_with_object_store(&mut object_store, &open)
            .unwrap();
    }
    let close =
        build_branch_close_publication("heads/main", open.ref_state.object_id(), target, &signer);
    {
        let mut object_store = ObjectWriteSession::open(&layout).unwrap();
        ref_store
            .publish_with_object_store(&mut object_store, &close)
            .unwrap();
    }

    let path = layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
    let bytes = std::fs::read(&path).unwrap();
    let discovery = crate::refs::decode_ref_container_records(&bytes).unwrap();
    let second_offset = discovery
        .record_outcomes
        .get(1)
        .expect("two records")
        .offset;
    assert!(second_offset > 0, "first record must have nonzero length");
    let mut corrupted = bytes.clone();
    corrupted[second_offset - 1] ^= 0xff;
    std::fs::write(&path, &corrupted).unwrap();

    // Confirm the fixture: the second (newest) record is still sound, and the pointer still agrees
    // with it -- so a pass here would not, on its own, prove the damaged-record check fired.
    let replay = ref_store.replay_log("heads/main").unwrap();
    assert!(
        replay.has_item_failure(),
        "fixture bug: the earlier record should now be damaged"
    );
    assert_eq!(
        replay.records.last().map(|record| &record.envelope),
        Some(&close.ref_update)
    );

    assert!(ensure_no_incomplete_publication_via_verify_refs_for_test(&layout).is_err());
    assert!(ensure_no_incomplete_publication(&layout).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// A settled publication whose active WAL has not drained (`has_incomplete_active_cleanup`) refuses
/// -- for both implementations.
#[test]
fn a_pending_active_cleanup_refuses() {
    let root = unique_temp_dir("rfc165-r2-pending-cleanup");
    let layout = setup(&root);
    let signer = maintainer();
    queue_one_patch(&layout, "heads/main", "p");
    // A real seal, then re-queue the exact same WAL content directly (bypassing the drain step
    // `finish_active_publication_cleanup` would otherwise run), so the active WAL's own metadata
    // still names a ref whose patch_ids now match the sealed Block's own -- exactly
    // `has_incomplete_active_cleanup`'s own fixture shape.
    crate::simulate_one_seal_for_test_support(&layout, "heads/main", &signer).unwrap();
    queue_one_patch(&layout, "heads/main", "p");

    assert!(ensure_no_incomplete_publication_via_verify_refs_for_test(&layout).is_err());
    assert!(ensure_no_incomplete_publication(&layout).is_err());
    let _ = std::fs::remove_dir_all(&root);
}
