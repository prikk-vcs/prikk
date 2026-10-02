//! RFC 165 R4: `plan_ref_completion`/`complete_ref_publication` tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType,
};

use super::{CompletionRefusal, complete_ref_publication, plan_ref_completion};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::object_store::ObjectWriteSession;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectWriter, RefPublication, RefStore,
    add_trusted_maintainer, maintainer_signature as sign_maintainer,
};

fn original_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r4-original", &[0x61; 32]).expect("seed")
}

fn completing_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r4-completer", &[0x62; 32]).expect("seed")
}

fn untrusted_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r4-untrusted", &[0x63; 32]).expect("seed")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A repository with two adopted maintainers (the original signer and a different completer),
/// matching owner decision 2 (RFC 165 §6): any adopted key may complete another's interrupted
/// publication.
fn setup(root: &std::path::Path) -> RepositoryLayout {
    let layout = RepositoryLayout::init(root.to_path_buf()).expect("init");
    for signer_key in [original_signer(), completing_signer()] {
        add_trusted_maintainer(
            &layout,
            signer_key.key_id(),
            &hex(&signer_key.public_key_bytes()),
        )
        .expect("adopt maintainer");
    }
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
    env.add_signature(sign_maintainer(&original_signer(), ObjectType::Block, id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&env)
        .unwrap()
}

/// Crash a `branch create`-shaped publication for `ref_name` between its pointer write and its ref-log
/// write, through the real `RefStore::publish` path, then truncate the log back to its own exact
/// pre-publish byte length -- a genuine `PointerLeading` state, matching the construction already
/// proven in `refs::tests::every_publication_refuses_first`. `signer` signs both the `RefState` and
/// `RefUpdate` -- `RefStore::publish` itself does not gate on trust policy membership (the caller
/// does, before ever reaching it), so an untrusted or since-revoked signer crashes exactly the same
/// way a trusted one does, which is what condition (a)'s own negative cases need.
fn crash_branch_create(
    layout: &RepositoryLayout,
    ref_name: &str,
    target: ObjectId,
    signer: &impl MaintainerSigner,
) -> ObjectId {
    use prikk_object::{RefKind, RefStatePayload, RefUpdatePayload};

    let log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    let before_len = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: target,
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
        new_target_object_id: target,
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
    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: None,
            ref_state: state_env,
            ref_update: update_env,
        })
        .unwrap();

    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&log_path)
        .unwrap();
    file.set_len(before_len).unwrap();
    state_id
}

#[test]
fn a_crashed_branch_create_is_planned_and_completed_by_a_different_adopted_key() {
    let root = unique_temp_dir("rfc165-r4-plan-and-complete");
    let layout = setup(&root);
    let target = root_block(&layout);
    let leading_state_id = crash_branch_create(&layout, "heads/topic", target, &original_signer());

    let plan = plan_ref_completion(&layout, "heads/topic")
        .unwrap()
        .expect("heads/topic must be a completable lead");
    assert_eq!(plan.ref_name, "heads/topic");
    assert_eq!(plan.leading_ref_state_id, leading_state_id);
    assert_eq!(plan.original_signer_key_id, original_signer().key_id());
    assert_eq!(plan.target_object_id, target);
    assert_eq!(plan.log_tip, None);
    assert_eq!(plan.next_sequence, 1);
    assert_eq!(plan.removes_partial_tail_bytes, 0);

    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let mut object_store = ObjectWriteSession::open(&layout).unwrap();
    let completer = completing_signer();
    let completed_id =
        complete_ref_publication(&layout, &mut object_store, &active_lock, &plan, &completer)
            .unwrap();
    drop(object_store);
    drop(active_lock);
    assert_eq!(completed_id, leading_state_id);

    let store = RefStore::new(layout.clone());
    assert_eq!(
        store.read_current_ref_state_id("heads/topic").unwrap(),
        Some(leading_state_id)
    );
    let replay = store.replay_log("heads/topic").unwrap();
    assert_eq!(replay.records.len(), 1, "the log must now have one record");
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert!(
        replay.records[0].envelope.object_id() != leading_state_id,
        "the completing RefUpdate is its own object, not the RefState"
    );

    // Completion is idempotent against a second, identical attempt (the same shape DC-38's own
    // seal retry already has): planning now says there is nothing left to complete.
    assert_eq!(
        plan_ref_completion(&layout, "heads/topic").unwrap(),
        Err(CompletionRefusal::NotALead),
        "a ref already caught up is not a pending completion"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `complete_ref_publication` gates the *completing* signer itself, separate from
/// `plan_ref_completion`'s own condition (a) (the *original* signer, already verified before this
/// plan was ever offered) -- an otherwise-valid plan, completed by a key this repository never
/// adopted, refuses and writes nothing.
#[test]
fn an_untrusted_completer_refuses_and_writes_nothing() {
    let root = unique_temp_dir("rfc165-r4-untrusted-completer");
    let layout = setup(&root);
    let target = root_block(&layout);
    crash_branch_create(&layout, "heads/topic", target, &original_signer());

    let plan = plan_ref_completion(&layout, "heads/topic")
        .unwrap()
        .expect("heads/topic must be a completable lead");

    let log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    let before = std::fs::read(&log_path).unwrap();

    let active_lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let mut object_store = ObjectWriteSession::open(&layout).unwrap();
    let outsider = untrusted_signer();
    let result =
        complete_ref_publication(&layout, &mut object_store, &active_lock, &plan, &outsider);
    drop(object_store);
    drop(active_lock);
    assert!(
        result.is_err(),
        "an unadopted completer must be refused, got {result:?}"
    );

    assert_eq!(
        std::fs::read(&log_path).unwrap(),
        before,
        "a refused completion must write nothing to the ref log"
    );
    let store = RefStore::new(layout.clone());
    assert!(
        store
            .read_current_ref_state_id("heads/topic")
            .unwrap()
            .is_some(),
        "the pointer itself is untouched by a refused completion"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// K1: `--plan-only` and a real run share the same `plan_ref_completion` call -- calling it alone,
/// any number of times, is read-only (what `--plan-only` does), and every call returns the identical
/// plan a real run would go on to print before its own write.
#[test]
fn planning_alone_is_read_only_and_repeatable() {
    let root = unique_temp_dir("rfc165-r4-plan-only-read-only");
    let layout = setup(&root);
    let target = root_block(&layout);
    crash_branch_create(&layout, "heads/topic", target, &original_signer());

    let snapshot = |layout: &RepositoryLayout| {
        [
            layout.ref_log_container_slot_path(ContainerSlot::A),
            layout.ref_log_container_slot_path(ContainerSlot::B),
            layout.ref_pointer_index_slot_path(ContainerSlot::A),
            layout.ref_pointer_index_slot_path(ContainerSlot::B),
        ]
        .map(|path| std::fs::read(path).unwrap_or_default())
    };

    let before = snapshot(&layout);
    let first_plan = plan_ref_completion(&layout, "heads/topic").unwrap();
    let after_first = snapshot(&layout);
    let second_plan = plan_ref_completion(&layout, "heads/topic").unwrap();
    let after_second = snapshot(&layout);

    assert_eq!(
        before, after_first,
        "planning alone must not change the ref log or pointer index"
    );
    assert_eq!(
        after_first, after_second,
        "a second planning call must not change anything either"
    );
    assert_eq!(
        first_plan, second_plan,
        "planning the same state twice must return the identical plan a real run would print"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_ref_with_no_pointer_is_not_completable() {
    let root = unique_temp_dir("rfc165-r4-no-pointer");
    let layout = setup(&root);
    assert_eq!(
        plan_ref_completion(&layout, "heads/never-published").unwrap(),
        Err(CompletionRefusal::NoPointer)
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_fully_published_ref_is_not_completable() {
    let root = unique_temp_dir("rfc165-r4-already-published");
    let layout = setup(&root);
    let target = root_block(&layout);
    use prikk_object::{RefKind, RefStatePayload, RefUpdatePayload};
    let state = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: target,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(sign_maintainer(&original_signer(), ObjectType::RefState, state_id).unwrap())
        .unwrap();
    let update = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: target,
        update_seq: 1,
        created_at: 0,
        author_key_id: original_signer().key_id().to_string(),
    };
    let mut update_env = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        update.to_canonical_bytes().unwrap(),
    );
    let update_id = update_env.object_id();
    update_env
        .add_signature(
            sign_maintainer(&original_signer(), ObjectType::RefUpdate, update_id).unwrap(),
        )
        .unwrap();
    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: state_env,
            ref_update: update_env,
        })
        .unwrap();

    assert_eq!(
        plan_ref_completion(&layout, "heads/main").unwrap(),
        Err(CompletionRefusal::NotALead)
    );
    let _ = std::fs::remove_dir_all(&root);
}

mod k4_failpoints_and_race;
mod negative_conditions;
