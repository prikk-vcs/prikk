#![allow(clippy::indexing_slicing)]

mod proptest_decode;

use prikk_error::Result;

use super::{
    ReceivedIndexEntry, ReceivedIndexRecordStatus, decode_received_index_records,
    encode_received_index_record,
};
use crate::foundation::layout::ref_name_key_bytes;
use crate::test_gates::test_support::sample_object_id;

#[test]
fn a_single_record_round_trips_through_decode() -> Result<()> {
    let entry = ReceivedIndexEntry {
        ref_name_key: ref_name_key_bytes("remotes/heads/main"),
        ref_name: "remotes/heads/main".to_string(),
        ref_state_id: sample_object_id("state"),
    };
    let bytes = encode_received_index_record(&entry)?;
    let replay = decode_received_index_records(&bytes)?;
    assert_eq!(replay.entries, vec![entry]);
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert!(
        replay
            .record_outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, ReceivedIndexRecordStatus::Evaluated))
    );
    Ok(())
}

/// Mirrors `pointer_index/tests.rs::trailing_partial_bytes_are_tolerated_not_treated_as_corruption`:
/// same three-shape frame structure (`parse_frame_at`'s `TrailingPartial`/`Invalid`/`Record`),
/// confirmed by construction here rather than assumed from the parallel design.
#[test]
fn trailing_partial_bytes_are_tolerated_not_treated_as_corruption() -> Result<()> {
    let entry = ReceivedIndexEntry {
        ref_name_key: ref_name_key_bytes("remotes/heads/main"),
        ref_name: "remotes/heads/main".to_string(),
        ref_state_id: sample_object_id("state"),
    };
    let mut bytes = encode_received_index_record(&entry)?;
    bytes.extend_from_slice(&[0_u8; 10]);
    let replay = decode_received_index_records(&bytes)?;
    assert_eq!(replay.entries, vec![entry]);
    assert_eq!(replay.trailing_partial_bytes, 10);
    Ok(())
}

/// Decode-level isolation: a damaged entry is named at its own offset and the scan resyncs past it,
/// leaving every sound entry on either side intact -- mirrors `pointer_index/tests.rs::isolates_a_
/// damaged_entry_at_decode_level_and_reads_sound_entries_around_it`.
#[test]
fn isolates_a_damaged_entry_at_decode_level_and_reads_sound_entries_around_it() -> Result<()> {
    let main_entry = ReceivedIndexEntry {
        ref_name_key: ref_name_key_bytes("remotes/heads/main"),
        ref_name: "remotes/heads/main".to_string(),
        ref_state_id: sample_object_id("main-state"),
    };
    let topic_entry = ReceivedIndexEntry {
        ref_name_key: ref_name_key_bytes("remotes/heads/topic"),
        ref_name: "remotes/heads/topic".to_string(),
        ref_state_id: sample_object_id("topic-state"),
    };
    let other_entry = ReceivedIndexEntry {
        ref_name_key: ref_name_key_bytes("remotes/heads/other"),
        ref_name: "remotes/heads/other".to_string(),
        ref_state_id: sample_object_id("other-state"),
    };
    let mut bytes = encode_received_index_record(&main_entry)?;
    let damaged_start = bytes.len();
    let damaged_record = encode_received_index_record(&topic_entry)?;
    let damaged_end = damaged_start + damaged_record.len();
    bytes.extend_from_slice(&damaged_record);
    bytes.extend_from_slice(&encode_received_index_record(&other_entry)?);
    // Corrupt the middle (remotes/heads/topic) record's last byte -- inside its body, not its header,
    // so decode reaches the checksum comparison rather than failing to parse the header at all.
    bytes[damaged_end - 1] ^= 0x01;

    let replay = decode_received_index_records(&bytes)?;
    let failed_offsets: Vec<_> = replay
        .record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            ReceivedIndexRecordStatus::Failed { .. } => Some(outcome.offset),
            ReceivedIndexRecordStatus::Evaluated => None,
        })
        .collect();
    assert_eq!(
        failed_offsets,
        vec![damaged_start],
        "the damaged entry must be named at its own offset"
    );
    assert_eq!(
        replay.entries,
        vec![main_entry, other_entry],
        "both sound entries, on either side of the damaged one, must still be read"
    );
    Ok(())
}
