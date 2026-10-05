//! 0.49.0 step 5, round 2 U3 (ruled): every attestation a RefState requires must be present as an Attestation
//! object. The fixture is a RefState payload built directly with a non-empty `required_attestation_ids`, the
//! shape `prikk-object`'s own vectors use, checked against an in-memory store.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload};

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

fn attestation_envelope() -> ObjectEnvelope {
    ObjectEnvelope::unsigned(ObjectType::Attestation, 1, b"an attestation body".to_vec())
}

/// The required attestation is present as an Attestation: the check passes.
#[test]
fn a_required_attestation_that_is_present_passes() {
    let envelope = attestation_envelope();
    let id = envelope.object_id();
    let mut store = MemoryObjectStore::new();
    store.write_object(&envelope).expect("write");
    ensure_required_attestations_present(
        &store,
        &state_requiring(vec![id]),
        ObjectId::from_bytes([9; 32]),
    )
    .expect("present attestation passes");
}

/// **Control: the same RefState with its attestation absent fails, and names it.**
/// Perturb: remove the presence check and this goes green.
#[test]
fn a_required_attestation_that_is_absent_fails_and_is_named() {
    let envelope = attestation_envelope();
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
