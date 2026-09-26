//! RFC 160 P4 for the two record formats whose replay types are private to `refs`: the **ref log container** and the **pointer index**.
//! The same cases as `test_gates/hostile_lengths.rs` (which holds the reasoning, the helpers and the list that names these two): every
//! hostile frame length, and every hostile body field with a matching checksum, refused -- in a child process, so a regression fails
//! the case and does not abort the suite.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use prikk_error::Result;
use prikk_object::ObjectId;

use crate::refs::container::{
    RefContainerRecordStatus, decode_ref_container_records, encode_ref_container_record_for_test,
};
use crate::refs::pointer_index::{
    PointerIndexEntry, PointerIndexRecordStatus, decode_pointer_index_records,
    encode_pointer_index_record,
};
use crate::test_gates::hostile_length_support::{Seen, hostile_case};
use crate::test_gates::hostile_lengths::{
    Format, check, check_partial_frames, envelope_bodies, ref_name_length_bodies,
};
use crate::test_gates::test_support::signed_ref_update_envelope;

fn ref_container_valid() -> Vec<u8> {
    let id = ObjectId::from_bytes([8; 32]);
    encode_ref_container_record_for_test(
        [5; 32],
        &signed_ref_update_envelope("heads/main", None, id, id, 1),
    )
    .expect("encoding")
}

fn ref_container_decode(bytes: &[u8]) -> Result<Seen> {
    let replay = decode_ref_container_records(bytes)?;
    Ok(Seen {
        records: replay.records.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, RefContainerRecordStatus::Failed { .. }))
            .count(),
    })
}

fn pointer_index_valid() -> Vec<u8> {
    encode_pointer_index_record(&PointerIndexEntry {
        ref_name_key: [5; 32],
        ref_name: "heads/main".to_string(),
        ref_state_id: ObjectId::from_bytes([6; 32]),
    })
    .expect("encoding")
}

fn pointer_index_decode(bytes: &[u8]) -> Result<Seen> {
    let replay = decode_pointer_index_records(bytes)?;
    Ok(Seen {
        records: replay.entries.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, PointerIndexRecordStatus::Failed { .. }))
            .count(),
    })
}

fn ref_container() {
    check(&Format {
        name: "ref container",
        pre: 32,
        valid: ref_container_valid,
        decode: ref_container_decode,
        hostile_bodies: envelope_bodies,
    });
}

fn pointer_index() {
    check(&Format {
        name: "pointer index",
        pre: 0,
        valid: pointer_index_valid,
        decode: pointer_index_decode,
        hostile_bodies: ref_name_length_bodies,
    });
}

hostile_case!(hostile_length_ref_container, ref_container);
hostile_case!(hostile_length_pointer_index, pointer_index);

/// RFC 160 F3 for the two `refs` readers (the rule, and its controls, are documented on `check_partial_frames`): a partial frame before
/// a sound one is damage (in the ref log, attributed to the ref its header names); a prefix of the last frame is still a tail.
#[test]
fn the_ref_readers_treat_a_partial_frame_before_a_sound_frame_as_damage() {
    for (name, pre, valid, decode, bodies) in [
        (
            "ref container",
            32,
            ref_container_valid as fn() -> Vec<u8>,
            ref_container_decode as fn(&[u8]) -> Result<Seen>,
            envelope_bodies as fn(&[u8]) -> Vec<crate::test_gates::hostile_lengths::HostileBody>,
        ),
        (
            "pointer index",
            0,
            pointer_index_valid,
            pointer_index_decode,
            ref_name_length_bodies,
        ),
    ] {
        check_partial_frames(&Format {
            name,
            pre,
            valid,
            decode,
            hostile_bodies: bodies,
        });
    }
}
