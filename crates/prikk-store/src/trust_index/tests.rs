#![allow(clippy::indexing_slicing)]

use prikk_error::Result;

use super::{
    TrustKeyEntry, TrustKeyRecordStatus, TrustPolicyRecordStatus, TrustPolicySnapshotEntry,
    decode_trust_key_records, decode_trust_policy_records, encode_trust_key_record,
    encode_trust_policy_record,
};

fn sample_public_key(seed: u8) -> [u8; 32] {
    [seed; 32]
}

#[test]
fn a_single_key_entry_round_trips_through_decode() -> Result<()> {
    let entry = TrustKeyEntry {
        key_id: "maintainer".to_string(),
        public_key: sample_public_key(1),
    };
    let bytes = encode_trust_key_record(&entry)?;
    let replay = decode_trust_key_records(&bytes)?;
    assert_eq!(replay.entries, vec![entry]);
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert!(
        replay
            .record_outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, TrustKeyRecordStatus::Evaluated))
    );
    Ok(())
}

/// RFC 164 §9.2, Addendum 2 item 3's whole-record sweep: flip every byte of a single complete trust-key
/// record in turn -- magic, version, length, checksum, and body alike -- and assert none of the 170
/// offsets decodes to a tail. Decode only, no I/O: cheap enough to cover every offset, not a sample.
#[test]
fn no_offset_of_a_single_trust_key_record_decodes_to_a_tail() -> Result<()> {
    let entry = TrustKeyEntry {
        key_id: "maintainer".to_string(),
        public_key: sample_public_key(1),
    };
    let sound = encode_trust_key_record(&entry)?;
    for offset in 0..sound.len() {
        let mut flipped = sound.clone();
        flipped[offset] ^= 0xFF;
        let replay = decode_trust_key_records(&flipped)?;
        assert_eq!(
            replay.trailing_partial_bytes, 0,
            "offset {offset}: a flipped byte in the only (complete) record must never read as a tail"
        );
        // Addendum 3 item 2: a cut that stops recording the failure (silently dropping the record
        // instead of reporting it as damage) would still pass the assertion above -- caught here.
        assert!(
            replay
                .record_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, TrustKeyRecordStatus::Failed { .. })),
            "offset {offset}: a flipped byte must be reported as damage, not silently dropped"
        );
    }
    Ok(())
}

#[test]
fn trailing_partial_key_bytes_are_tolerated_not_treated_as_corruption() -> Result<()> {
    let entry = TrustKeyEntry {
        key_id: "maintainer".to_string(),
        public_key: sample_public_key(1),
    };
    let mut bytes = encode_trust_key_record(&entry)?;
    bytes.extend_from_slice(&[0_u8; 10]);
    let replay = decode_trust_key_records(&bytes)?;
    assert_eq!(replay.entries, vec![entry]);
    assert_eq!(replay.trailing_partial_bytes, 10);
    Ok(())
}

/// Decode-level isolation: a damaged entry is named at its own offset and the scan resyncs past it,
/// leaving every sound entry on either side intact -- the same discipline `pointer_index.rs`/
/// `received_index.rs` use.
#[test]
fn isolates_a_damaged_key_entry_and_reads_sound_entries_around_it() -> Result<()> {
    let first = TrustKeyEntry {
        key_id: "alice".to_string(),
        public_key: sample_public_key(1),
    };
    let damaged_source = TrustKeyEntry {
        key_id: "bob".to_string(),
        public_key: sample_public_key(2),
    };
    let third = TrustKeyEntry {
        key_id: "carol".to_string(),
        public_key: sample_public_key(3),
    };
    let mut bytes = encode_trust_key_record(&first)?;
    let damaged_start = bytes.len();
    let damaged_record = encode_trust_key_record(&damaged_source)?;
    let damaged_end = damaged_start + damaged_record.len();
    bytes.extend_from_slice(&damaged_record);
    bytes.extend_from_slice(&encode_trust_key_record(&third)?);
    bytes[damaged_end - 1] ^= 0x01;

    let replay = decode_trust_key_records(&bytes)?;
    let failed_offsets: Vec<_> = replay
        .record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            TrustKeyRecordStatus::Failed { .. } => Some(outcome.offset),
            TrustKeyRecordStatus::Evaluated => None,
        })
        .collect();
    assert_eq!(failed_offsets, vec![damaged_start]);
    assert_eq!(replay.entries, vec![first, third]);
    Ok(())
}

/// RFC 164 §9.2, Addendum 2 item 3's whole-record sweep, for trust policy.
#[test]
fn no_offset_of_a_single_trust_policy_record_decodes_to_a_tail() -> Result<()> {
    let entry = TrustPolicySnapshotEntry {
        key_ids: vec!["alice".to_string(), "bob".to_string()],
    };
    let sound = encode_trust_policy_record(&entry)?;
    for offset in 0..sound.len() {
        let mut flipped = sound.clone();
        flipped[offset] ^= 0xFF;
        let replay = decode_trust_policy_records(&flipped)?;
        assert_eq!(
            replay.trailing_partial_bytes, 0,
            "offset {offset}: a flipped byte in the only (complete) record must never read as a tail"
        );
        assert!(
            replay
                .record_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, TrustPolicyRecordStatus::Failed { .. })),
            "offset {offset}: a flipped byte must be reported as damage, not silently dropped"
        );
    }
    Ok(())
}

#[test]
fn a_single_policy_snapshot_round_trips_through_decode() -> Result<()> {
    let entry = TrustPolicySnapshotEntry {
        key_ids: vec!["alice".to_string(), "bob".to_string()],
    };
    let bytes = encode_trust_policy_record(&entry)?;
    let replay = decode_trust_policy_records(&bytes)?;
    assert_eq!(replay.entries, vec![entry]);
    assert_eq!(replay.trailing_partial_bytes, 0);
    Ok(())
}

/// The property that makes revocation representable: a second, shorter snapshot is a wholly
/// independent record, not a diff against the first -- "last entry wins" resolves the whole list at
/// once, matching `trust_index.rs`'s own module doc.
#[test]
fn a_second_snapshot_supersedes_the_first_at_replay() -> Result<()> {
    let first = TrustPolicySnapshotEntry {
        key_ids: vec!["alice".to_string(), "bob".to_string()],
    };
    let second = TrustPolicySnapshotEntry {
        key_ids: vec!["alice".to_string()],
    };
    let mut bytes = encode_trust_policy_record(&first)?;
    bytes.extend_from_slice(&encode_trust_policy_record(&second)?);
    let replay = decode_trust_policy_records(&bytes)?;
    assert_eq!(replay.entries, vec![first, second.clone()]);
    assert_eq!(replay.entries.last(), Some(&second));
    Ok(())
}

#[test]
fn decode_rejects_a_snapshot_listing_the_same_key_id_twice() -> Result<()> {
    let entry = TrustPolicySnapshotEntry {
        key_ids: vec!["alice".to_string(), "alice".to_string()],
    };
    let mut bytes = encode_trust_policy_record(&entry)?;
    // RFC 164 Rule A: an undecodable record with nothing sound after it is now a tail, not damage --
    // so a genuinely sound record follows here, keeping this fixture interior damage (a sound record
    // follows), which Rule A explicitly keeps as a `Failed` outcome.
    bytes.extend(encode_trust_policy_record(&TrustPolicySnapshotEntry {
        key_ids: Vec::new(),
    })?);
    let replay = decode_trust_policy_records(&bytes)?;
    assert!(replay.has_item_failure());
    assert!(replay.record_outcomes.iter().any(
        |outcome| matches!(&outcome.status, TrustPolicyRecordStatus::Failed { message }
                if message.contains("more than once"))
    ));
    Ok(())
}
