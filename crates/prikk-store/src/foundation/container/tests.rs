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
        torn_message.contains("claims more bytes than remain")
            && torn_message.contains("a sound frame starts at byte offset"),
        "the message must name both offsets, the evidence a later write finished past this one \
         (W1, 0.52.0 release prep: the base message describes the frame only, with no verdict \
         clause of its own -- the caller's own wrapper gives the verdict): {torn_message:?}"
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
    let sound_frame_offset = record_a.len();
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
            // 2B-2: the message for this shape no longer says "checksum mismatch" (that text is
            // reserved for the *complete*, genuinely-damage case) -- it says what this shape
            // actually is, naming both the frame's own offset and the later sound frame's own
            // offset as the evidence a later write finished past it. W1 (0.52.0 release prep): the
            // message itself carries no verdict word of its own any more (the caller's own wrapper
            // supplies "interrupted append"/"not damage") -- this assertion checks the fact, not the
            // verdict wording.
            assert!(
                !message.contains("checksum mismatch"),
                "the damage-case wording must not leak into the interrupted-append case: {message:?}"
            );
            assert!(
                message.contains("already uses"),
                "must read as a later sound frame already using the claimed bytes: {message:?}"
            );
            assert!(
                message.contains("byte offset 0"),
                "must name the frame's own offset: {message:?}"
            );
            assert!(
                message.contains(&format!("byte offset {sound_frame_offset}")),
                "must name the later sound frame's own offset, the evidence this is harmless: \
                 {message:?}"
            );
        }
        other => {
            panic!("expected a non-complete (interrupted append) Failed outcome, got {other:?}")
        }
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

// ---- 0.59.0 P5b1a: a record is framed in two pieces, and the pieces are the bytes the single buffer was ---------------------------

/// The framing as it was before the record was held in two pieces, written out here independently of `record_checksum` and
/// `frame_pieces`: magic, version, body length, the sha256 of those three and the body concatenated, then the body.
fn single_buffer_framing(object_type: ObjectType, body: &[u8]) -> Result<Vec<u8>> {
    let magic = container_magic(object_type)?;
    let mut preimage = magic.to_vec();
    preimage.extend_from_slice(&super::CONTAINER_VERSION.to_be_bytes());
    preimage.extend_from_slice(&(body.len() as u64).to_be_bytes());
    preimage.extend_from_slice(body);
    let mut out = magic.to_vec();
    out.extend_from_slice(&super::CONTAINER_VERSION.to_be_bytes());
    out.extend_from_slice(&(body.len() as u64).to_be_bytes());
    out.extend_from_slice(&prikk_hash::sha256(&preimage));
    out.extend_from_slice(body);
    Ok(out)
}

/// **The pieces are the old bytes.** Every persisted type, bodies of every size around the hash's block boundaries and one large:
/// the header and the body, concatenated, are the single-buffer framing; the record's own length and checksum are the file's.
/// **Perturb:** a header with its checksum left zero, the body counted twice in the length, or the checksum taken over the body alone
/// (not the header's first fields with it) goes red here.
#[test]
fn a_record_in_two_pieces_is_the_bytes_the_single_buffer_was() -> Result<()> {
    let sizes = [
        0_usize, 1, 31, 32, 33, 55, 56, 63, 64, 65, 119, 120, 1000, 100_000,
    ];
    for object_type in crate::foundation::layout::persisted_object_types() {
        for size in sizes {
            let body: Vec<u8> = (0..size).map(|n| (n * 7 + size) as u8).collect();
            let record = super::frame_pieces(object_type, std::borrow::Cow::Borrowed(&body))?;
            let reference = single_buffer_framing(object_type, &body)?;
            assert_eq!(
                record.header(),
                &reference[..super::CONTAINER_HEADER_LEN],
                "{object_type} {size}: the header"
            );
            assert_eq!(
                record.body(),
                body.as_slice(),
                "{object_type} {size}: the body"
            );
            assert_eq!(
                record.len(),
                reference.len(),
                "{object_type} {size}: the length"
            );
            assert_eq!(
                record.checksum().as_slice(),
                &reference[18..50],
                "{object_type} {size}: the checksum"
            );
            assert_eq!(
                record.to_vec(),
                reference,
                "{object_type} {size}: the whole"
            );
            // The reader's own checksum function agrees with the writer's, on the header's own fields.
            let magic = container_magic(object_type)?;
            assert_eq!(
                super::record_checksum(magic, size as u64, &body),
                record.checksum()
            );
        }
    }
    Ok(())
}

/// A record framed from bytes the caller already holds (a received body) is the record framed from its own encoding: the same two
/// pieces. (The `cfg(test)` guard in the framing function also asserts the hint equals a fresh encode.)
#[test]
fn a_record_framed_from_a_known_body_is_the_record_framed_from_the_envelope() -> Result<()> {
    let envelope = signed_patch_envelope();
    let body = crate::foundation::file_codec::encode_envelope_file(&envelope)?;
    let owned = super::frame_container_record_with_known_body(ObjectType::Patch, &envelope, None)?;
    let known =
        super::frame_container_record_with_known_body(ObjectType::Patch, &envelope, Some(&body))?;
    assert_eq!(owned.header(), known.header());
    assert_eq!(owned.body(), known.body());
    assert_eq!(
        owned.to_vec(),
        encode_container_record(ObjectType::Patch, &envelope)?
    );
    Ok(())
}

/// **A record cut at every byte is a torn tail, as it was.** The third record of a container is appended through the real two-piece
/// loop, stopped after `cut` bytes (a power loss part way through the one write), for every cut from nothing to the whole record,
/// and with one-byte and larger writes: replay reads the two sound records before it, reports exactly `cut` bytes as the incomplete
/// final record, names no damage; at the whole record, three records and no tail.
/// **Perturb:** the loop writing the body before the header, or skipping a byte at the join, goes red here.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_two_piece_record_cut_at_every_byte_is_a_torn_tail() -> Result<()> {
    let envelopes = [
        signed_patch_blob_envelope(),
        signed_patch_blob_envelope(),
        signed_patch_blob_envelope(),
    ];
    let mut prior = Vec::new();
    for envelope in &envelopes[..2] {
        prior.extend(encode_container_record(ObjectType::Blob, envelope)?);
    }
    let record =
        super::frame_container_record_with_known_body(ObjectType::Blob, &envelopes[2], None)?;
    for most in [1_usize, 7, 50, 51, usize::MAX] {
        for cut in 0..=record.len() {
            let mut file = prior.clone();
            let mut written = 0_usize;
            let result = crate::foundation::fsutil::append_pieces(
                record.header(),
                record.body(),
                |pieces| {
                    let mut room = (cut - written).min(most);
                    if room == 0 {
                        return Err(rustix::io::Errno::IO);
                    }
                    let before = room;
                    for piece in pieces {
                        let n = piece.len().min(room);
                        file.extend_from_slice(&piece[..n]);
                        room -= n;
                        if n < piece.len() {
                            break;
                        }
                    }
                    written += before - room;
                    Ok(before - room)
                },
            );
            assert_eq!(
                result.is_ok(),
                cut == record.len(),
                "cut {cut}, {most} a call"
            );
            assert_eq!(file.len(), prior.len() + cut);
            let replay = decode_container_records(ObjectType::Blob, &file)?;
            assert!(
                !replay.has_item_failure(),
                "cut {cut}: a torn tail is not damage"
            );
            if cut == record.len() {
                assert_eq!(replay.records.len(), 3);
                assert_eq!(replay.trailing_partial_bytes, 0);
            } else {
                assert_eq!(
                    replay.records.len(),
                    2,
                    "cut {cut}: the sound records before it"
                );
                assert_eq!(
                    replay.trailing_partial_bytes, cut,
                    "cut {cut}: the torn tail"
                );
            }
        }
    }
    Ok(())
}
