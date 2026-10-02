//! RFC 165 R4 K3: one constructed case per rule condition (a)-(e), each refusing and writing
//! nothing, each paired with a control -- the identical construction minus the one fault -- that
//! completes successfully. A control that cannot fail proves nothing (`A control must be able to
//! fail`); pairing fault and control from the same construction is what makes each one's own
//! assertion mean what it claims.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, PatchSetDigest,
    RefKind, RefStatePayload, RefUpdatePayload, TagPayload,
};

use super::super::{CompletionRefusal, plan_ref_completion};
use super::{crash_branch_create, hex, original_signer, root_block, setup};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectWriter, RefPublication, RefStore, Wal,
    add_trusted_maintainer, maintainer_signature as sign_maintainer, remove_trusted_maintainer,
    write_active_ref_metadata,
};

fn revoked_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r4-revoked", &[0x64; 32]).expect("seed")
}

/// Publish `ref_name` for real (no crash): the log keeps the record. Returns the new `RefState` id,
/// usable as a later plant's own `previous_ref_state_id`/baseline log tip.
fn fully_publish(
    layout: &RepositoryLayout,
    ref_name: &str,
    target: ObjectId,
    signer: &impl MaintainerSigner,
    previous: Option<ObjectId>,
    update_seq: u64,
) -> ObjectId {
    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: target,
        update_seq,
        previous_ref_state_id: previous,
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
        old_ref_state_id: previous,
        new_ref_state_id: state_id,
        new_target_object_id: target,
        update_seq,
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
    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: previous,
            ref_state: state_env,
            ref_update: update_env,
        })
        .unwrap();
    state_id
}

/// Plant a `RefState` object (signed by `signer`, whatever shape the caller wants) and point
/// `ref_name`'s pointer directly at it (`write_ref_pointer_candidate_for_test`), bypassing
/// `RefStore::publish`'s own CAS check entirely -- the only way to construct a pointer that
/// disagrees with the log on purpose, which conditions (b) and (c)'s own negative cases need.
/// The ref log is never touched: whatever lead already exists there stays exactly as it was.
#[allow(clippy::too_many_arguments)]
fn plant_lead(
    layout: &RepositoryLayout,
    ref_name: &str,
    kind: RefKind,
    target: ObjectId,
    update_seq: u64,
    previous: Option<ObjectId>,
    signer: &impl MaintainerSigner,
) -> ObjectId {
    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind,
        target_object_id: target,
        update_seq,
        previous_ref_state_id: previous,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(sign_maintainer(signer, ObjectType::RefState, state_id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&state_env)
        .unwrap();
    crate::refs::write_ref_pointer_candidate_for_test(layout, ref_name, state_id).unwrap();
    state_id
}

fn write_tag_object(layout: &RepositoryLayout, target_block_id: ObjectId) -> ObjectId {
    let payload = TagPayload {
        name: "tags/decoy".to_string(),
        target_block_id,
        message: None,
        created_at: 0,
        author_key_id: original_signer().key_id().to_string(),
        patch_set_digest: PatchSetDigest([0u8; 32]),
        patch_count: 0,
    };
    let mut env =
        ObjectEnvelope::unsigned(ObjectType::Tag, 1, payload.to_canonical_bytes().unwrap());
    let id = env.object_id();
    env.add_signature(sign_maintainer(&original_signer(), ObjectType::Tag, id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&env)
        .unwrap()
}

/// Flip one byte well inside the ref log container's first (and here, only) record -- a complete,
/// fully-written record whose checksum no longer verifies, never a tail (RFC 164 §9.2, extended to
/// the ref log in U1). Used by condition (e)'s own case: damage that has nothing to do with the ref
/// under test, elsewhere in the same container.
fn flip_a_byte_in_the_log(layout: &RepositoryLayout) {
    let path = layout.ref_log_container_slot_path(ContainerSlot::A);
    let mut bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 20, "expected at least one real record");
    bytes[10] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();
}

#[test]
fn condition_a_a_lead_signed_by_a_never_adopted_key_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-a-never-adopted");
    let layout = setup(&root);
    let target = root_block(&layout);
    let stranger =
        Ed25519MaintainerSigner::from_seed("rfc165-r4-k3-stranger", &[0x70; 32]).unwrap();

    crash_branch_create(&layout, "heads/topic", target, &stranger);
    assert!(matches!(
        plan_ref_completion(&layout, "heads/topic").unwrap(),
        Err(CompletionRefusal::UntrustedSigner(_))
    ));

    // Control: the identical construction, signed by an adopted key instead -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-a-never-adopted-control");
    let layout2 = setup(&root2);
    let target2 = root_block(&layout2);
    crash_branch_create(&layout2, "heads/topic", target2, &original_signer());
    assert!(
        plan_ref_completion(&layout2, "heads/topic")
            .unwrap()
            .is_ok(),
        "control: an adopted signer must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn condition_a_a_lead_signed_by_a_since_revoked_key_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-a-revoked");
    let layout = setup(&root);
    let revoked = revoked_signer();
    add_trusted_maintainer(&layout, revoked.key_id(), &hex(&revoked.public_key_bytes())).unwrap();
    // Revoke *before* the crash -- `remove_trusted_maintainer` itself refuses while any publication
    // is incomplete, so the only order that works is: adopt, revoke, then crash the lead.
    assert!(remove_trusted_maintainer(&layout, revoked.key_id()).unwrap());

    let target = root_block(&layout);
    crash_branch_create(&layout, "heads/topic", target, &revoked);
    assert!(matches!(
        plan_ref_completion(&layout, "heads/topic").unwrap(),
        Err(CompletionRefusal::UntrustedSigner(_))
    ));

    // Control: the same key, never revoked -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-a-revoked-control");
    let layout2 = setup(&root2);
    let still_trusted = revoked_signer();
    add_trusted_maintainer(
        &layout2,
        still_trusted.key_id(),
        &hex(&still_trusted.public_key_bytes()),
    )
    .unwrap();
    let target2 = root_block(&layout2);
    crash_branch_create(&layout2, "heads/topic", target2, &still_trusted);
    assert!(
        plan_ref_completion(&layout2, "heads/topic")
            .unwrap()
            .is_ok(),
        "control: an un-revoked adopted signer must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn condition_b_a_previous_state_that_is_not_the_log_tip_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-b-previous-mismatch");
    let layout = setup(&root);
    let target = root_block(&layout);
    let _tip = fully_publish(&layout, "heads/main", target, &original_signer(), None, 1);
    // A previous id that is not the real tip -- the target object id, reused only because it is a
    // real, decodable id that is not the tip; its own type is irrelevant to this check.
    plant_lead(
        &layout,
        "heads/main",
        RefKind::Branch,
        target,
        2,
        Some(target),
        &original_signer(),
    );
    assert_eq!(
        plan_ref_completion(&layout, "heads/main").unwrap(),
        Err(CompletionRefusal::NotALead)
    );

    // Control: the same plant, `previous_ref_state_id` corrected to the real tip -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-b-previous-mismatch-control");
    let layout2 = setup(&root2);
    let target2 = root_block(&layout2);
    let tip2 = fully_publish(&layout2, "heads/main", target2, &original_signer(), None, 1);
    plant_lead(
        &layout2,
        "heads/main",
        RefKind::Branch,
        target2,
        2,
        Some(tip2),
        &original_signer(),
    );
    assert!(
        plan_ref_completion(&layout2, "heads/main").unwrap().is_ok(),
        "control: the real previous state must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn condition_b_a_sequence_gap_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-b-sequence-gap");
    let layout = setup(&root);
    let target = root_block(&layout);
    let tip = fully_publish(&layout, "heads/main", target, &original_signer(), None, 1);
    // Correct previous, but the sequence jumps from 1 straight to 3.
    plant_lead(
        &layout,
        "heads/main",
        RefKind::Branch,
        target,
        3,
        Some(tip),
        &original_signer(),
    );
    assert_eq!(
        plan_ref_completion(&layout, "heads/main").unwrap(),
        Err(CompletionRefusal::NotALead)
    );

    // Control: sequence 2 (the real next one) -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-b-sequence-gap-control");
    let layout2 = setup(&root2);
    let target2 = root_block(&layout2);
    let tip2 = fully_publish(&layout2, "heads/main", target2, &original_signer(), None, 1);
    plant_lead(
        &layout2,
        "heads/main",
        RefKind::Branch,
        target2,
        2,
        Some(tip2),
        &original_signer(),
    );
    assert!(
        plan_ref_completion(&layout2, "heads/main").unwrap().is_ok(),
        "control: the real next sequence must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn condition_b_a_repeated_sequence_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-b-sequence-repeat");
    let layout = setup(&root);
    let target = root_block(&layout);
    let tip = fully_publish(&layout, "heads/main", target, &original_signer(), None, 1);
    // Correct previous, but the sequence repeats the one already published (1, not 2).
    plant_lead(
        &layout,
        "heads/main",
        RefKind::Branch,
        target,
        1,
        Some(tip),
        &original_signer(),
    );
    assert_eq!(
        plan_ref_completion(&layout, "heads/main").unwrap(),
        Err(CompletionRefusal::NotALead)
    );

    // Control: sequence 2 -- completable (same construction as the sequence-gap test's own
    // control, proving the fault here is specifically the repeated number, not the plant itself).
    let root2 = unique_temp_dir("rfc165-r4-k3-b-sequence-repeat-control");
    let layout2 = setup(&root2);
    let target2 = root_block(&layout2);
    let tip2 = fully_publish(&layout2, "heads/main", target2, &original_signer(), None, 1);
    plant_lead(
        &layout2,
        "heads/main",
        RefKind::Branch,
        target2,
        2,
        Some(tip2),
        &original_signer(),
    );
    assert!(
        plan_ref_completion(&layout2, "heads/main").unwrap().is_ok(),
        "control: the real next sequence must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn condition_c_a_missing_target_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-c-missing-target");
    let layout = setup(&root);
    let never_written = ObjectId::from_bytes([0x42; 32]);
    plant_lead(
        &layout,
        "heads/topic",
        RefKind::Branch,
        never_written,
        1,
        None,
        &original_signer(),
    );
    assert!(matches!(
        plan_ref_completion(&layout, "heads/topic").unwrap(),
        Err(CompletionRefusal::InvalidTarget(_))
    ));

    // Control: a real Block target -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-c-missing-target-control");
    let layout2 = setup(&root2);
    let target2 = root_block(&layout2);
    plant_lead(
        &layout2,
        "heads/topic",
        RefKind::Branch,
        target2,
        1,
        None,
        &original_signer(),
    );
    assert!(
        plan_ref_completion(&layout2, "heads/topic")
            .unwrap()
            .is_ok(),
        "control: a real target must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn condition_c_a_wrong_kind_target_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-c-wrong-kind");
    let layout = setup(&root);
    let block = root_block(&layout);
    let tag_id = write_tag_object(&layout, block);
    // `kind: Branch` naming a Tag object's id -- `ensure_ref_target_valid` reads it typed as Block
    // and finds a type mismatch, the same "missing" shape a nonexistent id produces.
    plant_lead(
        &layout,
        "heads/topic",
        RefKind::Branch,
        tag_id,
        1,
        None,
        &original_signer(),
    );
    assert!(matches!(
        plan_ref_completion(&layout, "heads/topic").unwrap(),
        Err(CompletionRefusal::InvalidTarget(_))
    ));

    // Control: the Block itself, not the Tag wrapping it -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-c-wrong-kind-control");
    let layout2 = setup(&root2);
    let block2 = root_block(&layout2);
    plant_lead(
        &layout2,
        "heads/topic",
        RefKind::Branch,
        block2,
        1,
        None,
        &original_signer(),
    );
    assert!(
        plan_ref_completion(&layout2, "heads/topic")
            .unwrap()
            .is_ok(),
        "control: a Block target of the right kind must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

/// Shared by both condition (d) cases: an active WAL carrying one patch, active ref metadata
/// naming `heads/topic`, and a sealed-shaped Block whose own `patch_ids` either do or do not match
/// what the WAL holds -- `matching` picks which.
fn seal_shaped_lead(layout: &RepositoryLayout, matching: bool) {
    let patch = crate::test_gates::test_support::signed_patch_envelope();
    let blob = crate::test_gates::test_support::signed_patch_blob_envelope();
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&blob).unwrap();
    let patch_id = objects.write_object(&patch).unwrap();

    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&patch)
        .unwrap();
    write_active_ref_metadata(layout, "heads/topic").unwrap();

    let block_patch_ids = if matching {
        vec![patch_id]
    } else {
        vec![ObjectId::from_bytes([0x77; 32])]
    };
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: block_patch_ids,
        state_merkle_root: crate::compute_state_root(&[]).unwrap(),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block_env = ObjectEnvelope::unsigned(
        ObjectType::Block,
        2,
        block_payload.to_canonical_bytes().unwrap(),
    );
    let block_id = block_env.object_id();
    block_env
        .add_signature(sign_maintainer(&original_signer(), ObjectType::Block, block_id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&block_env)
        .unwrap();

    plant_lead(
        layout,
        "heads/topic",
        RefKind::Branch,
        block_id,
        1,
        None,
        &original_signer(),
    );
}

#[test]
fn condition_d_a_wal_mismatch_for_a_seal_shaped_lead_refuses() {
    let root = unique_temp_dir("rfc165-r4-k3-d-wal-mismatch");
    let layout = setup(&root);
    seal_shaped_lead(&layout, false);
    assert!(matches!(
        plan_ref_completion(&layout, "heads/topic").unwrap(),
        Err(CompletionRefusal::WalEvidenceMismatch(_))
    ));

    // Control: the identical construction with the Block's own `patch_ids` matching the retained
    // WAL -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-d-wal-mismatch-control");
    let layout2 = setup(&root2);
    seal_shaped_lead(&layout2, true);
    assert!(
        plan_ref_completion(&layout2, "heads/topic")
            .unwrap()
            .is_ok(),
        "control: matching retained WAL evidence must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn condition_e_complete_damage_elsewhere_in_the_log_refuses_every_ref() {
    let root = unique_temp_dir("rfc165-r4-k3-e-damage-elsewhere");
    let layout = setup(&root);
    let target = root_block(&layout);
    // `heads/main` is published for real (its own record lands in the log)...
    fully_publish(&layout, "heads/main", target, &original_signer(), None, 1);
    // ...then damaged -- nothing to do with `heads/topic`, the ref actually under test below.
    flip_a_byte_in_the_log(&layout);
    crash_branch_create(&layout, "heads/topic", target, &original_signer());

    assert!(matches!(
        plan_ref_completion(&layout, "heads/topic").unwrap(),
        Err(CompletionRefusal::RefLogDamaged(_))
    ));

    // Control: the identical construction, undamaged -- completable.
    let root2 = unique_temp_dir("rfc165-r4-k3-e-damage-elsewhere-control");
    let layout2 = setup(&root2);
    let target2 = root_block(&layout2);
    fully_publish(&layout2, "heads/main", target2, &original_signer(), None, 1);
    crash_branch_create(&layout2, "heads/topic", target2, &original_signer());
    assert!(
        plan_ref_completion(&layout2, "heads/topic")
            .unwrap()
            .is_ok(),
        "control: an undamaged log must plan cleanly"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

// The completing signer's own gate (`GatedOperation::RefComplete`) already has its fault+control
// pair in `ref_completion::tests::an_untrusted_completer_refuses_and_writes_nothing` (U2) -- not
// duplicated here, since `plan_ref_completion` itself never sees the completing key; this file's
// own sweep covers conditions (a)-(e) of the rule it does evaluate.
