//! 0.49.0 step 5, round 2 U3 (ruled): every attestation a RefState requires must be present as an Attestation
//! object. The fixture is a RefState payload built directly with a non-empty `required_attestation_ids`, the
//! shape `prikk-object`'s own vectors use, checked against an in-memory store.
//!
//! 0.50.0 step 2 Part B: the attestation's own `target_block_id` is now checked too, once the
//! attestation itself is present -- the fixture attestation is a genuinely canonical-encoded
//! `AttestationPayload` from here on (it has to decode for the new check to run at all), naming a
//! block the fixture either writes (present) or does not (missing).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_object::{
    AttestationPayload, AttestationStatus, CanonicalEncode, CanonicalWriter, ObjectEnvelope,
    ObjectId, ObjectType, RefKind, RefStatePayload,
};

use crate::memory_store::MemoryObjectStore;
use crate::object_store::ObjectWriter;
use crate::refs::ensure_required_attestations_present;

fn state_requiring(attestations: Vec<ObjectId>) -> RefStatePayload {
    RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: ObjectId::from_bytes([1; 32]),
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: attestations,
        closed: false,
    }
}

fn block_envelope(seed: u8) -> ObjectEnvelope {
    ObjectEnvelope::unsigned(ObjectType::Block, 1, vec![seed; 4])
}

fn attestation_envelope(target_block_id: ObjectId) -> ObjectEnvelope {
    let payload = AttestationPayload {
        target_block_id,
        policy_version: "v1".to_string(),
        plugin_set_hash: vec![1, 2, 3],
        results: Vec::new(),
        status: AttestationStatus::Pass,
        created_at: 0,
        is_reproducible_offline: true,
    };
    let mut writer = CanonicalWriter::new();
    payload.encode_canonical(&mut writer).expect("encode");
    ObjectEnvelope::unsigned(ObjectType::Attestation, 1, writer.finish())
}

/// The required attestation is present as an Attestation, and its own target block is present too:
/// the check passes.
#[test]
fn a_required_attestation_that_is_present_passes() {
    let block = block_envelope(7);
    let block_id = block.object_id();
    let envelope = attestation_envelope(block_id);
    let id = envelope.object_id();
    let mut store = MemoryObjectStore::new();
    store.write_object(&block).expect("write");
    store.write_object(&envelope).expect("write");
    ensure_required_attestations_present(
        &store,
        &state_requiring(vec![id]),
        ObjectId::from_bytes([9; 32]),
    )
    .expect("present attestation passes");
}

/// **Control: the attestation's own target block must be present too.** Perturb: an attestation
/// present, naming a target block the store never wrote. The case table's "missing" row.
#[test]
fn a_present_attestation_whose_target_block_is_missing_fails_and_is_named() {
    let missing_block_id = ObjectId::from_bytes([2; 32]);
    let envelope = attestation_envelope(missing_block_id);
    let id = envelope.object_id();
    let mut store = MemoryObjectStore::new();
    store.write_object(&envelope).expect("write");
    let error = ensure_required_attestations_present(
        &store,
        &state_requiring(vec![id]),
        ObjectId::from_bytes([9; 32]),
    )
    .expect_err("a missing target block is damage");
    assert!(
        error.to_string().contains(&missing_block_id.to_string()),
        "{error}"
    );
}

/// **Control: a present but undecodable attestation is damage, not silently skipped.** The case
/// table's "damaged" row.
#[test]
fn a_present_but_undecodable_attestation_fails() {
    let envelope = ObjectEnvelope::unsigned(ObjectType::Attestation, 1, b"not canonical".to_vec());
    let id = envelope.object_id();
    let mut store = MemoryObjectStore::new();
    store.write_object(&envelope).expect("write");
    ensure_required_attestations_present(
        &store,
        &state_requiring(vec![id]),
        ObjectId::from_bytes([9; 32]),
    )
    .expect_err("an undecodable attestation must not pass silently");
}

/// **Control: the same RefState with its attestation absent fails, and names it.**
/// Perturb: remove the presence check and this goes green.
#[test]
fn a_required_attestation_that_is_absent_fails_and_is_named() {
    let envelope = attestation_envelope(ObjectId::from_bytes([3; 32]));
    let id = envelope.object_id();
    let store = MemoryObjectStore::new();
    let error = ensure_required_attestations_present(
        &store,
        &state_requiring(vec![id]),
        ObjectId::from_bytes([9; 32]),
    )
    .expect_err("an absent attestation is damage");
    assert!(error.to_string().contains(&id.to_string()), "{error}");
}

/// **Control: the read is typed.** A non-Attestation object stored under the required id is not an
/// attestation, so the requirement is not met. Perturb: read untyped and this goes green.
#[test]
fn an_object_of_another_type_under_the_required_id_does_not_satisfy_it() {
    let blob = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"a blob body".to_vec());
    let id = blob.object_id();
    let mut store = MemoryObjectStore::new();
    store.write_object(&blob).expect("write");
    ensure_required_attestations_present(
        &store,
        &state_requiring(vec![id]),
        ObjectId::from_bytes([9; 32]),
    )
    .expect_err("a blob is not an attestation");
}

/// No required attestations: nothing to check, and the check passes (every honest RefState today).
#[test]
fn an_empty_requirement_list_passes() {
    let store = MemoryObjectStore::new();
    ensure_required_attestations_present(
        &store,
        &state_requiring(vec![]),
        ObjectId::from_bytes([9; 32]),
    )
    .expect("empty list passes");
}
