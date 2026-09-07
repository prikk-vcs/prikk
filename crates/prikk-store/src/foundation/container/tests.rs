#![allow(clippy::indexing_slicing)]

use prikk_error::Result;
use prikk_object::ObjectType;

use super::{
    ContainerRecordStatus, container_magic, decode_container_records, encode_container_record,
    encode_container_record_for_test,
};
use crate::test_gates::test_support::{signed_patch_blob_envelope, signed_patch_envelope};

#[test]
fn round_trips_a_single_record() -> Result<()> {
    let envelope = signed_patch_envelope();
    let bytes = encode_container_record(ObjectType::Patch, &envelope)?;
    let replay = decode_container_records(ObjectType::Patch, &bytes)?;
    assert!(!replay.has_item_failure());
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert_eq!(replay.records.len(), 1);
    assert_eq!(replay.records[0].envelope, envelope);
    Ok(())
}

#[test]
fn every_persisted_type_has_a_distinct_magic() -> Result<()> {
    let mut magics = Vec::new();
    for object_type in crate::foundation::layout::persisted_object_types() {
        magics.push(container_magic(object_type)?);
    }
    let mut deduped = magics.clone();
    deduped.sort_unstable();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        magics.len(),
        "expected all 6 persisted-type magics to be distinct: {magics:?}"
    );
    Ok(())
}

/// A frame framed under one object type's magic must not decode as a different type's container --
/// this is the whole point of a per-type magic. Framing a Patch envelope, then replaying those bytes
/// as a Blob container, must report an item failure (magic mismatch), not silently accept it.
#[test]
fn a_record_framed_for_one_type_is_rejected_by_another_types_container() -> Result<()> {
    let envelope = signed_patch_envelope();
    let bytes = encode_container_record(ObjectType::Patch, &envelope)?;
    let replay = decode_container_records(ObjectType::Blob, &bytes)?;
    assert!(replay.has_item_failure());
    assert!(replay.records.is_empty());
    Ok(())
}

/// Mirrors `wal_cluster.rs`'s two-damaged-records acceptance evidence, at container scale: three
/// records, the middle one corrupted -- the corruption is named at its own offset, and the sound
/// record after it is still read.
#[test]
fn isolates_a_damaged_record_and_reads_every_sound_record_around_it() -> Result<()> {
    // All three records are Blob-typed -- a container never mixes types in production, and mixing
    // them here would make record 0 fail on a magic mismatch too, muddying this test's actual claim
    // (isolation around a *checksum* failure, not a type-mismatch failure, which is already covered
    // separately above).
    let envelopes = [
        signed_patch_blob_envelope(),
        signed_patch_blob_envelope(),
        signed_patch_blob_envelope(),
    ];
    let mut bytes = Vec::new();
    let mut bounds = Vec::new();
    for envelope in &envelopes {
        let start = bytes.len();
        bytes.extend(encode_container_record(ObjectType::Blob, envelope)?);
        bounds.push((start, bytes.len()));
    }
    // Corrupt the middle record's last byte -- stays inside the body, past the fixed header, so the
    // frame's magic/version/body_len all still parse and decode reaches the checksum compare.
    let (_, middle_stop) = bounds[1];
    bytes[middle_stop - 1] ^= 0x01;

    let replay = decode_container_records(ObjectType::Blob, &bytes)?;
    assert!(replay.has_item_failure());
    let failed_offsets: Vec<usize> = replay
        .record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            ContainerRecordStatus::Failed { .. } => Some(outcome.offset),
            ContainerRecordStatus::Evaluated { .. } => None,
        })
        .collect();
    assert_eq!(failed_offsets, vec![bounds[1].0]);

    // Handoff §5 acceptance criterion 4: "every other record still readable", not just "not
    // failed" -- assert the sound records themselves, matching the bar RFC 102 Stage 2's own WAL
    // proof set (`wal_cluster.rs::wal_replay_isolates_two_damaged_records_and_reads_every_sound_
    // record`'s `sound_seqs` assertion), not merely their count.
    assert_eq!(
        replay.records,
        vec![
            super::ContainerRecord {
                envelope: envelopes[0].clone()
            },
            super::ContainerRecord {
                envelope: envelopes[2].clone()
            },
        ],
        "the two sound records around the damaged one must still be read, in file order"
    );
    Ok(())
}

#[test]
fn trailing_partial_bytes_are_tolerated_not_treated_as_corruption() -> Result<()> {
    let envelope = signed_patch_envelope();
    let mut bytes = encode_container_record(ObjectType::Patch, &envelope)?;
    bytes.extend_from_slice(&[0_u8; 10]);
    let replay = decode_container_records(ObjectType::Patch, &bytes)?;
    assert!(!replay.has_item_failure());
    assert_eq!(replay.trailing_partial_bytes, 10);
    assert_eq!(replay.records.len(), 1);
    Ok(())
}

#[test]
fn non_persisted_type_has_no_container_magic() {
    assert!(container_magic(ObjectType::RefUpdate).is_err());
}

/// A frame's magic proves only which *container* a byte range belongs to -- nothing about the header
/// constrains what `object_type` the body's own envelope claims. A well-formed, correctly checksummed
/// Blob-container frame whose body happens to be a valid Patch envelope must still be rejected, not
/// silently accepted as a Blob (the pre-Stage-3 loose-file `verify_object_file`'s own
/// `envelope.object_type != object_type` check, now enforced here instead). Found and fixed while
/// investigating a re-targeted `verify` test, not assumed correct from the type-per-magic design
/// alone.
#[test]
fn envelope_type_disagreeing_with_its_own_containers_type_is_rejected() -> Result<()> {
    let patch_envelope = signed_patch_envelope();
    assert_eq!(patch_envelope.object_type, ObjectType::Patch);
    let bytes = encode_container_record(ObjectType::Blob, &patch_envelope)?;
    let replay = decode_container_records(ObjectType::Blob, &bytes)?;
    assert!(replay.has_item_failure());
    assert!(replay.records.is_empty());
    Ok(())
}

/// `encode_container_record_for_test` bypasses the strict-signature validation
/// `encode_container_record` applies (`envelope.validate_strict()`, inside `encode_envelope_file`),
/// needed by later Stage 3 fixtures whose whole point is a malformed shape `encode_container_record`
/// itself would refuse to produce -- mirrors `wal::encode_record_for_test`'s exact role. Confirms
/// here that a structurally-valid-but-otherwise-unsigned envelope still frames and decodes at the
/// container level: envelope decode succeeding is a different concern from schema/signature policy
/// acceptance, which is a later, separate check (RFC 102 Stage 2's own established distinction).
#[test]
fn encode_for_test_bypasses_strict_validation_but_still_decodes() -> Result<()> {
    let mut envelope = signed_patch_blob_envelope();
    envelope.signatures.clear();
    let bytes = encode_container_record_for_test(ObjectType::Blob, &envelope)?;
    let replay = decode_container_records(ObjectType::Blob, &bytes)?;
    assert!(!replay.has_item_failure());
    assert_eq!(replay.records.len(), 1);
    assert!(replay.records[0].envelope.signatures.is_empty());
    Ok(())
}
