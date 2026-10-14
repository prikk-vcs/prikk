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

/// 0.52.0 step 1 Part A, item 3: a frame whose header claims more bytes than remain, with a real
/// sound record written right after it (the store's own append-only shape: a crash truncates the
/// write, the store never notices, and its next write lands right after the torn bytes) -- the exact
/// shape `foundation::frame_resync::partial_before_sound_frame_message`'s own text used to call
/// *"damage, not a torn tail"* here too, contradicting the `InterruptedAppend` classification
/// `verify/objects.rs` already gives it (`complete: false`, a warning, never failed). Shown red on
/// the code before this part (the message said "damage") and green after.
#[test]
#[allow(clippy::expect_used)]
fn an_overrun_torn_frame_reads_as_an_interrupted_append_never_damage() -> Result<()> {
    let magic = container_magic(ObjectType::Blob)?;
    let mut bytes = encode_container_record(ObjectType::Blob, &signed_patch_blob_envelope())?;
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&1u16.to_be_bytes()); // CONTAINER_VERSION
    bytes.extend_from_slice(&999u64.to_be_bytes()); // a claimed body length far longer than what follows
    bytes.extend_from_slice(&[0xABu8; 32]); // a checksum never reached, since the body never finishes
    bytes.extend_from_slice(&[0u8; 10]); // only 10 of the claimed body bytes actually arrive
    bytes.extend_from_slice(&encode_container_record(
        ObjectType::Blob,
        &signed_patch_blob_envelope(),
    )?);

    let replay = decode_container_records(ObjectType::Blob, &bytes)?;
    assert_eq!(
        replay.records.len(),
        2,
        "both sound records must still read"
    );
    let torn_message = replay
        .record_outcomes
        .iter()
        .find_map(|outcome| match &outcome.status {
            ContainerRecordStatus::Failed {
                message,
                complete: false,
            } => Some(message.clone()),
            _ => None,
        })
        .expect("the torn frame must be a non-complete Failed outcome");
    assert!(
        !torn_message.contains("damage, not a torn tail"),
        "an object container's own overrun torn frame must never read as a live repository's \
         damage wording: {torn_message:?}"
    );
    assert!(
        torn_message.contains("an interrupted append, not damage")
            && torn_message.contains("claims more bytes than remain")
            && torn_message.contains("a sound frame starts at byte offset"),
        "the message must read as an interrupted append: {torn_message:?}"
    );
    Ok(())
}

/// 0.52.0 step 2, Part 2B (review `0.52.0-step-2-A-review-v1`, finding 2B-1): once the archive moves
/// onto the identical shared primitive, a differential comparison between the two readers can no
/// longer catch a bug *inside* the primitive -- a bug there moves both readers' answers together.
/// 2A's own perturbation ("treat an overrun as complete") was caught only by the then-still-separate
/// archive twin; this store-side test is the pinned-expectation gap 2B-1 named: a
/// **checksum-mismatched, fully-present** frame (not a short read -- `an_overrun_torn_frame_reads_
/// as_an_interrupted_append_never_damage` above already covers that different shape) whose own
/// claimed body is inflated just enough that a later, genuinely sound frame's own start position
/// falls *inside* the inflated claim -- the one shape that actually exercises `checksum_mismatch_
/// is_complete`'s own `next >= body_end` comparison with a real "no" answer, which this file's own
/// test suite did not previously have. Must read red under 2A's own perturbation (`next >= body_end`
/// forced to unconditional `true`), confirmed in the report.
#[test]
#[allow(clippy::expect_used)]
fn a_checksum_mismatch_whose_sound_frame_starts_inside_the_claimed_range_is_interrupted_not_damage()
-> Result<()> {
    let mut record_a = encode_container_record(ObjectType::Blob, &signed_patch_blob_envelope())?;
    let header_len = super::CONTAINER_HEADER_LEN;
    let real_body_len = (record_a.len() - header_len) as u64;
    // Inflate only the header's own declared body length (bytes 10..18, big-endian u64) by 10 --
    // the checksum bytes and the real body are untouched, so decoding now reads 10 bytes too many
    // (spilling into the next record's own header) and the checksum no longer matches.
    let inflated_len = real_body_len + 10;
    record_a[10..18].copy_from_slice(&inflated_len.to_be_bytes());
    let record_b = encode_container_record(ObjectType::Blob, &signed_patch_blob_envelope())?;
    // `record_b`'s own real magic sits at `record_a.len()` (unchanged -- only a header field was
    // overwritten, not any byte's position), which is `header_len + real_body_len`: strictly inside
    // the inflated claim `header_len + real_body_len + 10`. `record_b` is long enough on its own
    // that the inflated claim still fits entirely within real, present bytes (not a short read).
    let mut bytes = record_a.clone();
    bytes.extend_from_slice(&record_b);

    let replay = decode_container_records(ObjectType::Blob, &bytes)?;
    assert_eq!(
        replay.records.len(),
        1,
        "only the second, genuinely sound record reads -- the first's own claim was both wrong \
         and overlapping"
    );
    let outcome = replay
        .record_outcomes
        .first()
        .expect("the mutated first frame must produce an outcome");
    match &outcome.status {
        ContainerRecordStatus::Failed {
            message,
            complete: false,
        } => {
            assert!(
                message.contains("checksum mismatch"),
                "the base classification is still a checksum mismatch, not a short read: {message:?}"
            );
        }
        other => panic!(
            "expected a non-complete (interrupted append) Failed outcome, got {other:?}"
        ),
    }
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
