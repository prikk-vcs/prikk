#![allow(clippy::indexing_slicing)]

use prikk_error::Result;

use super::{
    GenerationRecord, GenerationRecordStatus, decode_generation_records, encode_generation_record,
    resolve_live_slot,
};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::test_gates::test_support::unique_temp_dir;

#[test]
fn a_single_record_round_trips_through_decode() -> Result<()> {
    let record = GenerationRecord {
        live_slot: ContainerSlot::B,
    };
    let bytes = encode_generation_record(&record);
    let replay = decode_generation_records(&bytes)?;
    assert_eq!(replay.records, vec![record]);
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert!(
        replay
            .record_outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, GenerationRecordStatus::Evaluated))
    );
    Ok(())
}

/// Mirrors `received_index/tests.rs`'s own version of this test: trailing bytes shorter than one
/// complete frame are a tolerated in-progress append, not corruption.
#[test]
fn trailing_partial_bytes_are_tolerated_not_treated_as_corruption() -> Result<()> {
    let record = GenerationRecord {
        live_slot: ContainerSlot::A,
    };
    let mut bytes = encode_generation_record(&record);
    bytes.extend_from_slice(&[0_u8; 10]);
    let replay = decode_generation_records(&bytes)?;
    assert_eq!(replay.records, vec![record]);
    assert_eq!(replay.trailing_partial_bytes, 10);
    Ok(())
}

/// Decode-level isolation, mirroring `received_index/tests.rs`'s own version: a damaged record is
/// named at its own offset and the scan resyncs past it, leaving sound records on either side intact.
#[test]
fn isolates_a_damaged_record_at_decode_level_and_reads_sound_records_around_it() -> Result<()> {
    let first = GenerationRecord {
        live_slot: ContainerSlot::A,
    };
    let damaged = GenerationRecord {
        live_slot: ContainerSlot::B,
    };
    let last = GenerationRecord {
        live_slot: ContainerSlot::A,
    };
    let mut bytes = encode_generation_record(&first);
    let damaged_start = bytes.len();
    let damaged_record = encode_generation_record(&damaged);
    let damaged_end = damaged_start + damaged_record.len();
    bytes.extend_from_slice(&damaged_record);
    bytes.extend_from_slice(&encode_generation_record(&last));
    bytes[damaged_end - 1] ^= 0x01;

    let replay = decode_generation_records(&bytes)?;
    let failed_offsets: Vec<_> = replay
        .record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            GenerationRecordStatus::Failed { .. } => Some(outcome.offset),
            GenerationRecordStatus::Evaluated => None,
        })
        .collect();
    assert_eq!(
        failed_offsets,
        vec![damaged_start],
        "the damaged record must be named at its own offset"
    );
    assert_eq!(
        replay.records,
        vec![first, last],
        "both sound records, on either side of the damaged one, must still be read"
    );
    Ok(())
}

/// Step 1's only reachable outcome in production: an empty generation log resolves to `A`, since
/// nothing has ever appended a record.
#[test]
fn an_empty_or_missing_generation_log_resolves_to_slot_a() -> Result<()> {
    let root = unique_temp_dir("generation-empty-resolves-a");
    let layout = RepositoryLayout::init(root.clone())?;
    let resolved = resolve_live_slot(&layout, &layout.ref_pointer_index_generation_log_path())?;
    assert_eq!(resolved, ContainerSlot::A);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The last complete record wins, matching every other last-entry-wins container's own reader
/// contract in this codebase.
#[test]
fn resolver_takes_the_last_complete_record() -> Result<()> {
    let root = unique_temp_dir("generation-resolver-last-wins");
    let layout = RepositoryLayout::init(root.clone())?;
    let path = layout.received_index_generation_log_path();
    let mut bytes = encode_generation_record(&GenerationRecord {
        live_slot: ContainerSlot::A,
    });
    bytes.extend_from_slice(&encode_generation_record(&GenerationRecord {
        live_slot: ContainerSlot::B,
    }));
    std::fs::write(&path, bytes)?;

    let resolved = resolve_live_slot(&layout, &path)?;
    assert_eq!(resolved, ContainerSlot::B);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The corruption-blast-radius ruling (design-v1.md §15.6): a damaged generation record must not
/// resolve to a plausible-but-stale slot -- it fails closed instead.
#[test]
fn a_damaged_generation_record_fails_closed_rather_than_resolving_silently() -> Result<()> {
    let root = unique_temp_dir("generation-resolver-fails-closed");
    let layout = RepositoryLayout::init(root.clone())?;
    let path = layout.trust_policy_generation_log_path();
    let mut bytes = encode_generation_record(&GenerationRecord {
        live_slot: ContainerSlot::B,
    });
    let last_index = bytes.len() - 1;
    bytes[last_index] ^= 0x01;
    std::fs::write(&path, bytes)?;

    assert!(resolve_live_slot(&layout, &path).is_err());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
