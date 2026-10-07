#![allow(clippy::indexing_slicing)]

use prikk_error::Result;

use super::{
    PointerIndexEntry, PointerIndexRecordStatus, append_ref_pointer_entry,
    decode_pointer_index_records, encode_pointer_index_record, lookup_ref_pointer,
    replay_pointer_index, truncate_pointer_index_trailing_partial,
};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, ref_name_key_bytes};
use crate::test_gates::test_support::{sample_object_id, unique_temp_dir};

#[test]
fn write_then_lookup_round_trips() -> Result<()> {
    let root = unique_temp_dir("pointer-index-write-lookup");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = ref_name_key_bytes("heads/main");
    let entry = PointerIndexEntry {
        ref_name_key: key,
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("state"),
    };
    append_ref_pointer_entry(&layout, &entry)?;

    let (found, _tail) = lookup_ref_pointer(&layout, key)?;
    assert_eq!(found, Some(entry));
    assert_eq!(
        lookup_ref_pointer(&layout, ref_name_key_bytes("heads/other"))?.0,
        None
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 164 §9.2, Addendum 2 item 3's whole-record sweep, closing the exact gap the review's own
/// control found (nothing in the suite held §9 for the pointer index): flip every byte of a single
/// complete pointer-index record in turn and assert none of them decodes to a tail.
#[test]
fn no_offset_of_a_single_pointer_index_record_decodes_to_a_tail() -> Result<()> {
    let entry = PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/main"),
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("state"),
    };
    let sound = encode_pointer_index_record(&entry)?;
    for offset in 0..sound.len() {
        let mut flipped = sound.clone();
        flipped[offset] ^= 0xFF;
        let replay = decode_pointer_index_records(&flipped)?;
        assert_eq!(
            replay.trailing_partial_bytes, 0,
            "offset {offset}: a flipped byte in the only (complete) record must never read as a tail"
        );
        // Addendum 3 item 2: the review's own control stopped recording the failure for a complete
        // damaged record here -- the record was silently dropped, and this sweep stayed green because
        // it asserted only `trailing_partial_bytes == 0`. Caught now.
        assert!(
            replay
                .record_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, PointerIndexRecordStatus::Failed { .. })),
            "offset {offset}: a flipped byte must be reported as damage, not silently dropped"
        );
    }
    Ok(())
}

/// "Last entry wins" (Step 0 §13.4's own words, mirroring `index.rs::lookup_object_location`): a
/// second publish for the same ref supersedes the first at lookup time, without needing the first
/// entry removed or rewritten -- append-only, never overwritten in place.
#[test]
fn a_second_publish_supersedes_the_first_at_lookup() -> Result<()> {
    let root = unique_temp_dir("pointer-index-last-wins");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = ref_name_key_bytes("heads/main");
    let first = PointerIndexEntry {
        ref_name_key: key,
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("state-1"),
    };
    let second = PointerIndexEntry {
        ref_name_key: key,
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("state-2"),
    };
    append_ref_pointer_entry(&layout, &first)?;
    append_ref_pointer_entry(&layout, &second)?;

    assert_eq!(lookup_ref_pointer(&layout, key)?.0, Some(second));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 102 Stage 4 checkpoint review, design-v1.md §13.13: the pointer index had no corruption-
/// isolation coverage of its own before this -- the old, now-removed "non-canonical ref pointer path"
/// test's `by-id/` sub-case was, unintentionally, the only thing exercising this behavior at all.
/// Mirrors `container/tests.rs::trailing_partial_bytes_are_tolerated_not_treated_as_corruption`:
/// `decode_pointer_index_records` has the identical three-shape frame structure `container.rs` does
/// (`parse_frame_at`'s `TrailingPartial`/`Invalid`/`Record`), confirmed by reading both side by side
/// before writing this, not assumed from the parallel design.
#[test]
fn trailing_partial_bytes_are_tolerated_not_treated_as_corruption() -> Result<()> {
    let entry = PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/main"),
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("state"),
    };
    let mut bytes = encode_pointer_index_record(&entry)?;
    bytes.extend_from_slice(&[0_u8; 10]);
    let replay = decode_pointer_index_records(&bytes)?;
    assert_eq!(replay.entries, vec![entry]);
    assert_eq!(replay.trailing_partial_bytes, 10);
    assert!(
        replay
            .record_outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, PointerIndexRecordStatus::Evaluated))
    );
    Ok(())
}

/// Decode-level isolation: `decode_pointer_index_records` names a damaged entry at its own offset and
/// resyncs to keep reading every sound entry around it, the same discipline `container.rs` uses. **No
/// per-entry `claimed_ref_name_key` to assert here, unlike the container side** -- `pointer_index.rs`'s
/// own `FrameAttempt::Invalid` carries only a message, not a claimed key (confirmed by reading the
/// type, not assumed symmetric with `container.rs`'s `Invalid { message, claimed_ref_name_key }`).
/// Offset attribution and around-the-damage survival are still proven; see the next test for why that
/// asymmetry doesn't leave a safety gap at the layer that actually matters for lookups.
#[test]
fn isolates_a_damaged_entry_at_decode_level_and_reads_sound_entries_around_it() -> Result<()> {
    let main_entry = PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/main"),
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("main-state"),
    };
    let topic_entry = PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/topic"),
        ref_name: "heads/topic".to_string(),
        ref_state_id: sample_object_id("topic-state"),
    };
    let other_entry = PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/other"),
        ref_name: "heads/other".to_string(),
        ref_state_id: sample_object_id("other-state"),
    };
    let mut bytes = encode_pointer_index_record(&main_entry)?;
    let damaged_start = bytes.len();
    let damaged_record = encode_pointer_index_record(&topic_entry)?;
    let damaged_end = damaged_start + damaged_record.len();
    bytes.extend_from_slice(&damaged_record);
    bytes.extend_from_slice(&encode_pointer_index_record(&other_entry)?);
    // Corrupt the middle (heads/topic) record's last byte -- inside its body, not its header, so
    // decode reaches the checksum comparison rather than failing to parse the header at all.
    bytes[damaged_end - 1] ^= 0x01;

    let replay = decode_pointer_index_records(&bytes)?;
    let failed_offsets: Vec<_> = replay
        .record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            PointerIndexRecordStatus::Failed { .. } => Some(outcome.offset),
            PointerIndexRecordStatus::Evaluated => None,
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

/// The safety property the checkpoint review's own reasoning turned on: unlike the log container
/// (item-contained, per-ref isolation), the pointer index's "last entry wins" lookup semantics make
/// silently skipping a damaged entry actively dangerous -- an unreadable *latest* entry for some ref
/// must never let an *older* entry for that ref be silently treated as current. `read_pointers`
/// (`refs/verify/scan.rs`) and `lookup_ref_pointer` both refuse outright on any damaged entry
/// (`replay.has_item_failure()`), rather than resolving to whatever the isolate-and-continue decoder
/// still parsed. Proven directly: a repository with one genuinely resolvable ref and one damaged entry
/// for a different ref must fail closed for *both*, not silently serve the sound one while hiding the
/// damage.
#[test]
fn damaged_entry_fails_closed_rather_than_silently_resolving_a_stale_entry() -> Result<()> {
    let root = unique_temp_dir("pointer-index-fail-closed");
    let layout = RepositoryLayout::init(root.clone())?;
    let main_key = ref_name_key_bytes("heads/main");
    let topic_key = ref_name_key_bytes("heads/topic");
    let main_entry = PointerIndexEntry {
        ref_name_key: main_key,
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("main-state"),
    };
    append_ref_pointer_entry(&layout, &main_entry)?;

    // A damaged entry for a second, unrelated ref -- constructed directly (not via a real append)
    // so its checksum is wrong but its shape is otherwise valid.
    let topic_entry = PointerIndexEntry {
        ref_name_key: topic_key,
        ref_name: "heads/topic".to_string(),
        ref_state_id: sample_object_id("topic-state"),
    };
    let mut damaged = encode_pointer_index_record(&topic_entry)?;
    let last = damaged
        .last_mut()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("expected a record".to_string()))?;
    *last ^= 0x01;
    let path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let mut existing = std::fs::read(&path)?;
    existing.extend_from_slice(&damaged);
    // RFC 162 rule 3: a corrupted entry with nothing sound after it is now a repairable tail, not
    // damage -- so a third, genuinely sound entry follows the damaged one, keeping this fixture
    // interior damage (a sound entry follows it), which is what this test is actually about.
    let other_key = ref_name_key_bytes("heads/other");
    let other_entry = PointerIndexEntry {
        ref_name_key: other_key,
        ref_name: "heads/other".to_string(),
        ref_state_id: sample_object_id("other-state"),
    };
    existing.extend_from_slice(&encode_pointer_index_record(&other_entry)?);
    std::fs::write(&path, existing)?;

    assert!(replay_pointer_index(&layout)?.has_item_failure());
    assert!(lookup_ref_pointer(&layout, main_key).is_err());
    assert!(lookup_ref_pointer(&layout, topic_key).is_err());
    assert!(lookup_ref_pointer(&layout, other_key).is_err());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 162 rule 3: a genuinely torn pointer-index tail (fewer bytes than one header) is truncated,
/// the removed bytes are saved, and every sound entry survives -- the pointer-index analogue of
/// `wal.rs::wal_truncate_preserves_all_complete_records_in_a_torn_queue_and_reports_their_ids`.
#[test]
#[allow(clippy::expect_used)]
fn truncate_pointer_index_trailing_partial_truncates_a_torn_tail_and_saves_the_bytes() -> Result<()>
{
    let root = unique_temp_dir("pointer-index-repair-torn-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let main_key = ref_name_key_bytes("heads/main");
    let entry = PointerIndexEntry {
        ref_name_key: main_key,
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("main-state"),
    };
    append_ref_pointer_entry(&layout, &entry)?;

    let path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let mut bytes = std::fs::read(&path)?;
    let torn = vec![0xAB_u8; 7];
    bytes.extend_from_slice(&torn);
    std::fs::write(&path, &bytes)?;

    assert!(!replay_pointer_index(&layout)?.has_item_failure());
    assert_eq!(replay_pointer_index(&layout)?.trailing_partial_bytes, 7);

    let repair = truncate_pointer_index_trailing_partial(&layout)?;
    assert_eq!(repair.preserved_entries, 1);
    assert_eq!(repair.truncated_bytes, 7);
    let recovery_id = &repair
        .recovery
        .as_ref()
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity(
                "a repair that truncated bytes names its recovery entry".to_string(),
            )
        })?
        .id;
    assert_eq!(
        crate::recovery_log::removed_bytes(&layout, recovery_id)?.expect("the log holds the entry"),
        torn,
        "the recovery log holds exactly the removed bytes"
    );

    let replay = replay_pointer_index(&layout)?;
    assert_eq!(replay.trailing_partial_bytes, 0);
    assert_eq!(replay.entries, vec![entry]);

    // Idempotent: a second run finds nothing left to truncate.
    let second = truncate_pointer_index_trailing_partial(&layout)?;
    assert_eq!(second.truncated_bytes, 0);
    assert_eq!(second.recovery, None);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 162 rule 3: genuine interior damage (a sound entry follows a damaged one) is not a tail, and
/// this repair refuses it rather than truncating past it -- the pointer-index analogue of
/// `wal.rs::wal_truncate_failure_retains_partial_tail_and_retry_repairs_it`'s refusal half.
#[test]
fn truncate_pointer_index_trailing_partial_refuses_on_interior_damage() -> Result<()> {
    let root = unique_temp_dir("pointer-index-repair-refuses-damage");
    let layout = RepositoryLayout::init(root.clone())?;
    let main_key = ref_name_key_bytes("heads/main");
    append_ref_pointer_entry(
        &layout,
        &PointerIndexEntry {
            ref_name_key: main_key,
            ref_name: "heads/main".to_string(),
            ref_state_id: sample_object_id("main-state"),
        },
    )?;

    let path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let mut bytes = std::fs::read(&path)?;
    let mut damaged = encode_pointer_index_record(&PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/topic"),
        ref_name: "heads/topic".to_string(),
        ref_state_id: sample_object_id("topic-state"),
    })?;
    let last = damaged
        .last_mut()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("expected a record".to_string()))?;
    *last ^= 0x01;
    bytes.extend_from_slice(&damaged);
    bytes.extend_from_slice(&encode_pointer_index_record(&PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/other"),
        ref_name: "heads/other".to_string(),
        ref_state_id: sample_object_id("other-state"),
    })?);
    std::fs::write(&path, &bytes)?;

    let Err(error) = truncate_pointer_index_trailing_partial(&layout) else {
        panic!("interior damage must refuse");
    };
    let error = error.to_string();
    // 019 §5.3 (A3): names the way out that re-derives the whole container, since this repair
    // cannot modify a damaged entry.
    assert!(
        error.contains("run `prikk doctor --rebuild-pointer-index` instead"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(&path)?,
        bytes,
        "a refused repair must leave the file byte for byte as it was"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// **RFC 168 §7 rehearsal, the pointer index:** repair, list, restore, then a slot byte-identical to its pre-repair state.
/// The meaning file is the ref log's live slot, and the restore holds while it is unchanged.
#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn a_pointer_index_repair_is_restored_byte_for_byte_from_the_log() -> Result<()> {
    let root = unique_temp_dir("pointer-index-rehearsal");
    let layout = RepositoryLayout::init(root.clone())?;
    let entry = PointerIndexEntry {
        ref_name_key: ref_name_key_bytes("heads/main"),
        ref_name: "heads/main".to_string(),
        ref_state_id: sample_object_id("main-state"),
    };
    append_ref_pointer_entry(&layout, &entry)?;
    let path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let before = std::fs::read(&path)?;
    let mut with_tail = before.clone();
    with_tail.extend_from_slice(&[0xCD_u8; 7]);
    std::fs::write(&path, &with_tail)?;

    let repair = truncate_pointer_index_trailing_partial(&layout)?;
    let id = repair
        .recovery
        .as_ref()
        .expect("the repair names its entry")
        .id
        .clone();
    assert_eq!(
        std::fs::read(&path)?,
        before,
        "truncated to the sound prefix"
    );

    let plan = crate::recovery_log::restore(&layout, &id, false)?;
    assert!(plan.written, "every condition holds: {plan:?}");
    assert_eq!(
        std::fs::read(&path)?,
        with_tail,
        "byte-identical to before the repair"
    );

    // Control: the ref log's live slot changed after the repair: the restore is refused and the slot is left alone.
    let repaired_again = truncate_pointer_index_trailing_partial(&layout)?;
    let id = repaired_again.recovery.expect("named").id;
    std::fs::write(
        layout.ref_log_container_slot_path(ContainerSlot::A),
        b"changed",
    )?;
    let refused = crate::recovery_log::restore(&layout, &id, false)?;
    assert!(!refused.written);
    assert!(
        refused
            .conditions
            .iter()
            .any(|condition| !condition.holds && condition.text.contains("log-a")),
        "{refused:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
