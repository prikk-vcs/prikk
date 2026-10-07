//! RFC 165 R5: `plan_pointer_index_rebuild`/`rebuild_pointer_index` tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind,
    RefStatePayload, RefUpdatePayload,
};

use super::{plan_pointer_index_rebuild, rebuild_pointer_index};
use crate::foundation::generation;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, add_trusted_maintainer,
    maintainer_signature as sign_maintainer, remove_trusted_maintainer,
};

fn original_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r5-original", &[0x91; 32]).expect("seed")
}

fn revoked_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r5-revoked", &[0x92; 32]).expect("seed")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn setup(root: &std::path::Path) -> RepositoryLayout {
    let layout = RepositoryLayout::init(root.to_path_buf()).expect("init");
    for signer_key in [original_signer(), revoked_signer()] {
        add_trusted_maintainer(
            &layout,
            signer_key.key_id(),
            &hex(&signer_key.public_key_bytes()),
        )
        .expect("adopt maintainer");
    }
    layout
}

/// `_seed` is unused beyond distinguishing call sites in the test source -- two blocks with the same
/// `parent` and otherwise-empty content are legitimately the same content-addressed object (a root
/// block, say, shared by two refs' own first publish), not a problem this helper needs to avoid; a
/// non-root block's own `parent_block_ids` already makes distinct chains naturally distinct.
fn new_block(layout: &RepositoryLayout, parent: Option<ObjectId>, _seed: u8) -> ObjectId {
    let payload = BlockPayload {
        parent_block_ids: parent.into_iter().collect(),
        kind: if parent.is_some() {
            BlockKind::Normal
        } else {
            BlockKind::Root
        },
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).unwrap(),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let bytes = payload.to_canonical_bytes().unwrap();
    let mut env = ObjectEnvelope::unsigned(ObjectType::Block, 2, bytes);
    let id = env.object_id();
    env.add_signature(sign_maintainer(&original_signer(), ObjectType::Block, id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&env)
        .unwrap()
}

/// Publish `ref_name` for real, signed by `signer`: both the pointer and the log gain a record.
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

/// Crash a publication (pointer committed, log not yet appended) for `ref_name`, signed by `signer`.
fn crash_publish(
    layout: &RepositoryLayout,
    ref_name: &str,
    target: ObjectId,
    signer: &impl MaintainerSigner,
    previous: Option<ObjectId>,
    update_seq: u64,
) -> ObjectId {
    let log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    let before_len = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
    let state_id = fully_publish(layout, ref_name, target, signer, previous, update_seq);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&log_path)
        .unwrap();
    file.set_len(before_len).unwrap();
    state_id
}

fn live_pointer_index_bytes(layout: &RepositoryLayout) -> Vec<u8> {
    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let (live_slot, _, _) =
        generation::resolve_live_slot_with_tail(layout, &generation_log_path).unwrap();
    std::fs::read(layout.ref_pointer_index_slot_path(live_slot)).unwrap()
}

#[test]
fn byte_identical_to_the_live_index_at_two_depths() {
    for depth in [1usize, 5usize] {
        let root = unique_temp_dir(&format!("rfc165-r5-byte-identical-{depth}"));
        let layout = setup(&root);
        let mut tip_main: Option<ObjectId> = None;
        let mut tip_other: Option<ObjectId> = None;
        for seq in 1..=depth {
            let target = new_block(&layout, tip_main, seq as u8);
            tip_main = Some(fully_publish(
                &layout,
                "heads/main",
                target,
                &original_signer(),
                tip_main,
                seq as u64,
            ));
            let target2 = new_block(&layout, tip_other, (seq + 100) as u8);
            tip_other = Some(fully_publish(
                &layout,
                "heads/other",
                target2,
                &original_signer(),
                tip_other,
                seq as u64,
            ));
        }

        let before = live_pointer_index_bytes(&layout);
        rebuild_pointer_index(&layout).unwrap();
        let after = live_pointer_index_bytes(&layout);
        assert_eq!(
            before, after,
            "depth {depth}: rebuilt index must be byte-identical to the original, in its \
             uncompacted form"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn a_revoked_key_does_not_move_a_ref() {
    let root = unique_temp_dir("rfc165-r5-revoked-key");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    let sound_tip = fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);

    // Revoke before crashing -- `remove_trusted_maintainer` itself refuses while a publication is
    // incomplete.
    assert!(remove_trusted_maintainer(&layout, revoked_signer().key_id()).unwrap());
    let target2 = new_block(&layout, Some(target1), 2);
    let revoked_lead = crash_publish(
        &layout,
        "heads/main",
        target2,
        &revoked_signer(),
        Some(sound_tip),
        2,
    );

    let plan = rebuild_pointer_index(&layout).unwrap();
    let entry = plan
        .per_ref
        .iter()
        .find(|entry| entry.ref_name == "heads/main")
        .expect("heads/main must appear in the plan");
    assert_eq!(entry.before, Some(revoked_lead));
    assert_eq!(
        entry.after,
        Some(sound_tip),
        "the revoked key must not move heads/main past the last sound log record"
    );
    assert_eq!(plan.dropped_leads.len(), 1);
    let dropped = &plan.dropped_leads[0];
    assert_eq!(dropped.ref_name, "heads/main");
    assert_eq!(dropped.lead_ref_state_id, revoked_lead);
    assert!(
        matches!(dropped.reason, crate::CompletionRefusal::UntrustedSigner(_)),
        "the dropped lead must name the untrusted-signer reason, got {:?}",
        dropped.reason
    );

    let store = RefStore::new(layout.clone());
    assert_eq!(
        store.read_current_ref_state_id("heads/main").unwrap(),
        Some(sound_tip),
        "heads/main's own live pointer must now read the sound tip, not the revoked lead"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_damaged_pointer_index_record_is_served_from_the_log() {
    let root = unique_temp_dir("rfc165-r5-damaged-pointer");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    let sound_tip = fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);

    // Damage heads/main's own pointer-index record directly (flip a byte in its first record).
    let pointer_path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let mut bytes = std::fs::read(&pointer_path).unwrap();
    assert!(bytes.len() > 10, "expected at least one real record");
    bytes[5] ^= 0xFF;
    std::fs::write(&pointer_path, bytes).unwrap();

    // `verify`/`ref complete` would both refuse over this; the rebuild serves it instead.
    let plan = rebuild_pointer_index(&layout).unwrap();
    let entry = plan
        .per_ref
        .iter()
        .find(|entry| entry.ref_name == "heads/main")
        .expect("heads/main must appear in the plan");
    assert_eq!(
        entry.before, None,
        "the damaged pointer record itself must read as unavailable, not a lead"
    );
    assert_eq!(entry.after, Some(sound_tip));
    assert!(plan.dropped_leads.is_empty());

    let store = RefStore::new(layout.clone());
    assert_eq!(
        store.read_current_ref_state_id("heads/main").unwrap(),
        Some(sound_tip),
        "heads/main must now read cleanly from the rebuilt index"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// U4 review v1's own finding: when the pointer index's *newest* record for a ref is damaged, the
/// damage-tolerant reader falls back to an *older*, still-sound entry -- stale, behind the log, never
/// a lead. The write is still correct (the log's own tip), but the plan must say "restored," never
/// "dropped lead": nothing authorized is being discarded.
#[test]
fn a_damaged_newest_pointer_record_falls_back_to_a_stale_one_and_is_restored_not_dropped() {
    let root = unique_temp_dir("rfc165-r5-stale-fallback-restored");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    let seq1 = fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);
    let target2 = new_block(&layout, Some(target1), 2);
    let seq2 = fully_publish(
        &layout,
        "heads/main",
        target2,
        &original_signer(),
        Some(seq1),
        2,
    );

    // Damage only the newest (second) pointer-index record, leaving the first one sound.
    let pointer_path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let mut bytes = std::fs::read(&pointer_path).unwrap();
    let first_record_len = bytes.len() / 2;
    assert!(
        first_record_len > 10,
        "expected two roughly-equal-size records, got {} total bytes",
        bytes.len()
    );
    let flip_at = first_record_len + 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&pointer_path, bytes).unwrap();

    let plan = rebuild_pointer_index(&layout).unwrap();
    let entry = plan
        .per_ref
        .iter()
        .find(|entry| entry.ref_name == "heads/main")
        .expect("heads/main must appear in the plan");
    assert_eq!(
        entry.before,
        Some(seq1),
        "the damage-tolerant reader must fall back to the older, sound record"
    );
    assert_eq!(
        entry.after,
        Some(seq2),
        "the write is still the log's own tip"
    );
    assert!(
        plan.dropped_leads.is_empty(),
        "a stale pointer is not a dropped lead: {:?}",
        plan.dropped_leads
    );
    assert_eq!(
        plan.restored,
        vec![super::RestoredRef {
            ref_name: "heads/main".to_string(),
            stale_ref_state_id: seq1,
        }],
        "the stale fallback must be reported as restored, naming the exact stale id"
    );

    let store = RefStore::new(layout.clone());
    assert_eq!(
        store.read_current_ref_state_id("heads/main").unwrap(),
        Some(seq2),
        "heads/main must read the log's own tip after the rebuild"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_completable_lead_refuses_the_whole_rebuild() {
    let root = unique_temp_dir("rfc165-r5-completable-lead-refuses");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    let sound_tip = fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);
    let target2 = new_block(&layout, Some(target1), 2);
    crash_publish(
        &layout,
        "heads/main",
        target2,
        &original_signer(),
        Some(sound_tip),
        2,
    );

    let before = live_pointer_index_bytes(&layout);
    let error = rebuild_pointer_index(&layout).unwrap_err();
    assert!(
        error.to_string().contains("heads/main"),
        "the refusal must name the blocking ref: {error}"
    );
    let after = live_pointer_index_bytes(&layout);
    assert_eq!(before, after, "a refusal must write nothing");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ref_log_damage_refuses_the_whole_rebuild() {
    let root = unique_temp_dir("rfc165-r5-ref-log-damage-refuses");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);

    let log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    let mut bytes = std::fs::read(&log_path).unwrap();
    bytes[5] ^= 0xFF;
    std::fs::write(&log_path, bytes).unwrap();

    let before = live_pointer_index_bytes(&layout);
    let error = rebuild_pointer_index(&layout).unwrap_err();
    assert!(
        error.to_string().contains("damaged record"),
        "unexpected error: {error}"
    );
    let after = live_pointer_index_bytes(&layout);
    assert_eq!(before, after, "a refusal must write nothing");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn plan_only_is_read_only_and_equals_the_real_runs_plan() {
    let root = unique_temp_dir("rfc165-r5-plan-only");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);

    let before = live_pointer_index_bytes(&layout);
    let planned = plan_pointer_index_rebuild(&layout).unwrap();
    let after_plan = live_pointer_index_bytes(&layout);
    assert_eq!(before, after_plan, "--plan-only must write nothing");

    let executed = rebuild_pointer_index(&layout).unwrap();
    assert_eq!(
        planned, executed,
        "--plan-only's own plan must equal the real run's plan"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// K3 control: remove the completable-lead refusal, and the rebuild drops an authorized transition.
#[test]
fn control_without_the_completable_lead_refusal_the_rebuild_would_drop_an_authorized_transition() {
    let root = unique_temp_dir("rfc165-r5-control-completable-lead");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    let sound_tip = fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);
    let target2 = new_block(&layout, Some(target1), 2);
    let completable_lead = crash_publish(
        &layout,
        "heads/main",
        target2,
        &original_signer(),
        Some(sound_tip),
        2,
    );

    // The real rule refuses.
    assert!(rebuild_pointer_index(&layout).is_err());

    // The control: what the rebuild's *content* would have been had it not refused -- the log alone,
    // without the still-pending, fully authorized `completable_lead` transition. This demonstrates
    // the refusal is load-bearing: without it, a rebuild silently drops an authorized transition no
    // one asked to undo.
    let store = RefStore::new(layout.clone());
    assert_eq!(
        store.read_current_ref_state_id("heads/main").unwrap(),
        Some(completable_lead),
        "the live pointer still (correctly) carries the authorized lead"
    );
    let discovery = crate::refs::decode_ref_log_for_rebuild(&layout).unwrap();
    let log_only_tip = discovery
        .records
        .iter()
        .rfind(|record| record.ref_name == "heads/main")
        .map(|record| record.new_ref_state_id);
    assert_eq!(
        log_only_tip,
        Some(sound_tip),
        "the log alone (what a rebuild would write, absent the refusal) stops one transition short \
         of the authorized lead -- exactly the drop the refusal exists to prevent"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// K3 control: add a trust filter (re-verify every log record's own signature while building the
/// base reconstruction), and the revoked-key row goes red -- i.e. confirms the rebuild's own "never
/// trust-filtered" base reconstruction is load-bearing: if condition (a) were (wrongly) re-applied to
/// every *historical* log record (not just the current lead), a ref whose entire history happens to
/// include a since-revoked signer would go empty/short even for transitions nothing is wrong with.
#[test]
fn control_a_trust_filter_over_historical_log_records_would_wrongly_empty_a_sound_ref() {
    let root = unique_temp_dir("rfc165-r5-control-trust-filter");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    // Published by a key that is about to be revoked -- but *fully*, landing in the log, not left
    // as a lead. A trust-filtered rebuild would wrongly drop this sound, log-confirmed transition.
    let tip = fully_publish(&layout, "heads/main", target1, &revoked_signer(), None, 1);
    assert!(remove_trusted_maintainer(&layout, revoked_signer().key_id()).unwrap());

    // The real rebuild (never trust-filtered) keeps it.
    let plan = rebuild_pointer_index(&layout).unwrap();
    let entry = plan
        .per_ref
        .iter()
        .find(|entry| entry.ref_name == "heads/main")
        .unwrap();
    assert_eq!(
        entry.after,
        Some(tip),
        "a sound, log-confirmed transition must survive even though its signer is now revoked"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn k5_rebuild_is_never_run_implicitly() {
    let root = unique_temp_dir("rfc165-r5-k5-never-implicit");
    let layout = setup(&root);
    let target1 = new_block(&layout, None, 1);
    fully_publish(&layout, "heads/main", target1, &original_signer(), None, 1);

    let before = live_pointer_index_bytes(&layout);
    let _ = crate::doctor_repository(&layout);
    let _ = crate::repair_tails(&layout);
    let _ = crate::repair_object_index(&layout);
    let _ = crate::repair_pointer_index_tail(&layout);
    let _ = crate::verify_repository(&layout);
    let after = live_pointer_index_bytes(&layout);
    assert_eq!(
        before, after,
        "no other verb may switch the pointer-index generation log -- only the named rebuild does"
    );

    let generation_log_path = layout.ref_pointer_index_generation_log_path();
    let (live_slot, _, _) =
        generation::resolve_live_slot_with_tail(&layout, &generation_log_path).unwrap();
    assert_eq!(
        live_slot,
        ContainerSlot::A,
        "the live slot must still be the original -- nothing switched it implicitly"
    );

    let _ = std::fs::remove_dir_all(&root);
}

mod k4_failpoints_and_race;

/// Review v1 (a plan exits as the real run would): a torn generation-log tail refuses the plan and the real
/// run alike, with the same text, and the plan writes nothing. The control: the same log without its tail
/// plans cleanly.
#[test]
fn a_plan_refuses_over_a_torn_generation_log_tail_as_the_real_run_does() {
    let root = unique_temp_dir("rfc165-r5-plan-torn-generation-tail");
    let layout = setup(&root);
    let target = new_block(&layout, None, 1);
    fully_publish(&layout, "heads/main", target, &original_signer(), None, 1);
    let log = layout.ref_pointer_index_generation_log_path();
    let clean = std::fs::read(&log).expect("generation log");
    plan_pointer_index_rebuild(&layout).expect("control: a clean log plans");

    let mut torn = clean.clone();
    torn.extend_from_slice(&[0u8; 100]);
    std::fs::write(&log, &torn).expect("torn tail");
    let plan_refusal = match plan_pointer_index_rebuild(&layout) {
        Ok(_) => panic!("the plan must refuse a torn generation-log tail"),
        Err(error) => error.to_string(),
    };
    let run_refusal = match rebuild_pointer_index(&layout) {
        Ok(_) => panic!("the real run must refuse a torn generation-log tail"),
        Err(error) => error.to_string(),
    };
    assert_eq!(plan_refusal, run_refusal);
    assert!(plan_refusal.contains("--repair-tails"), "{plan_refusal}");
    assert_eq!(
        std::fs::read(&log).expect("generation log"),
        torn,
        "the plan writes nothing"
    );

    std::fs::write(&log, &clean).expect("restore the clean log");
    plan_pointer_index_rebuild(&layout).expect("control: without the tail the plan is clean again");
}
