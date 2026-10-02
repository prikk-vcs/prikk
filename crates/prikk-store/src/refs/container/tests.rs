#![allow(clippy::indexing_slicing)]

use prikk_error::Result;

use super::{
    RefContainerRecordStatus, append_ref_container_record, decode_ref_container_records,
    encode_ref_container_record, incomplete_tail_matches, replay_ref_subsequence,
    truncate_incomplete_tail,
};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, ref_name_key_bytes};
use crate::test_gates::test_support::{
    sample_object_id, signed_ref_update_envelope, unique_temp_dir,
};

#[test]
fn round_trips_a_single_record() -> Result<()> {
    let key = ref_name_key_bytes("heads/main");
    let envelope = signed_ref_update_envelope(
        "heads/main",
        None,
        sample_object_id("state"),
        sample_object_id("target"),
        1,
    );
    let bytes = encode_ref_container_record(key, &envelope)?;
    let replay = decode_ref_container_records(&bytes)?;
    assert_eq!(replay.records.len(), 1);
    assert_eq!(replay.records[0].ref_name_key, key);
    assert_eq!(replay.records[0].envelope, envelope);
    assert_eq!(replay.trailing_partial_bytes, 0);
    Ok(())
}

/// RFC 165 R5 (§9.2 for the ref log): flip every byte of a single complete ref-log record in turn --
/// magic, version, `ref_name_key`, length, checksum, and body alike -- and assert none of the offsets
/// decodes to a tail. Mirrors `trust_index::tests::no_offset_of_a_single_trust_key_record_decodes_to_a_tail`
/// exactly; decode only, no I/O, cheap enough to cover every offset, not a sample.
///
/// **Manual control (handoff §1 item 3, "§9.2 bypassed at the ref log"), not an automated toggle:**
/// every `never_a_tail` assignment in `parse_frame_at`/`decode_ref_container_records` was forced to
/// `false` (via `sed`, `container.rs` restored from a backup afterward), reproducing "§9.2 does not
/// apply here" directly. Rerunning this exact test then failed at offset 0 (a magic-byte flip):
/// `left: 374, right: 0` -- the flipped record was misread as a 374-byte tail instead of damage.
/// Restored and reverified green before this file was committed.
#[test]
fn no_offset_of_a_single_ref_log_record_decodes_to_a_tail() -> Result<()> {
    let key = ref_name_key_bytes("heads/main");
    let envelope = signed_ref_update_envelope(
        "heads/main",
        None,
        sample_object_id("state"),
        sample_object_id("target"),
        1,
    );
    let sound = encode_ref_container_record(key, &envelope)?;
    for offset in 0..sound.len() {
        let mut flipped = sound.clone();
        flipped[offset] ^= 0xFF;
        let replay = decode_ref_container_records(&flipped)?;
        assert_eq!(
            replay.trailing_partial_bytes, 0,
            "offset {offset}: a flipped byte in the only (complete) record must never read as a tail"
        );
        assert!(
            replay.record_outcomes.iter().any(|outcome| matches!(
                outcome.status,
                RefContainerRecordStatus::Failed {
                    never_a_tail: true,
                    ..
                }
            )),
            "offset {offset}: a flipped byte must be reported as damage (never_a_tail), not silently \
             dropped or read as a tail"
        );
        assert!(
            replay.records.is_empty(),
            "offset {offset}: a complete damaged record must not be returned as a sound one"
        );
    }
    Ok(())
}

#[test]
fn trailing_partial_bytes_are_tolerated_not_treated_as_corruption() -> Result<()> {
    let key = ref_name_key_bytes("heads/main");
    let envelope = signed_ref_update_envelope(
        "heads/main",
        None,
        sample_object_id("state"),
        sample_object_id("target"),
        1,
    );
    let mut bytes = encode_ref_container_record(key, &envelope)?;
    bytes.extend_from_slice(&[0_u8; 10]);
    let replay = decode_ref_container_records(&bytes)?;
    assert_eq!(replay.records.len(), 1);
    assert_eq!(replay.trailing_partial_bytes, 10);
    assert!(
        replay
            .record_outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, RefContainerRecordStatus::Evaluated))
    );
    Ok(())
}

/// Handoff acceptance criterion 4 / Step 0 §13.5 (corruption isolation, promoted to an acceptance
/// criterion): three refs' own records, interleaved -- one damaged -- and every sound record around
/// it, for every ref, is still read. Mirrors the top-level `container.rs`'s own
/// `isolates_a_damaged_record_and_reads_every_sound_record_around_it`, strengthened the same way
/// (asserting survived content, not just offsets).
#[test]
fn isolates_a_damaged_record_and_reads_every_sound_record_around_it_across_refs() -> Result<()> {
    let refs = ["heads/main", "heads/topic", "heads/main"];
    let envelopes: Vec<_> = refs
        .iter()
        .enumerate()
        .map(|(index, ref_name)| {
            let seq = if *ref_name == "heads/topic" {
                1
            } else {
                index as u64 + 1
            };
            signed_ref_update_envelope(
                ref_name,
                None,
                sample_object_id(&format!("state-{index}")),
                sample_object_id(&format!("target-{index}")),
                seq,
            )
        })
        .collect();
    let mut bytes = Vec::new();
    let mut bounds = Vec::new();
    for (ref_name, envelope) in refs.iter().zip(&envelopes) {
        let start = bytes.len();
        bytes.extend(encode_ref_container_record(
            ref_name_key_bytes(ref_name),
            envelope,
        )?);
        bounds.push((start, bytes.len()));
    }
    // Corrupt the middle record's (heads/topic) last byte.
    let (_, middle_stop) = bounds[1];
    bytes[middle_stop - 1] ^= 0x01;

    let replay = decode_ref_container_records(&bytes)?;
    let failed: Vec<_> = replay
        .record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            RefContainerRecordStatus::Failed {
                claimed_ref_name_key,
                ..
            } => Some((outcome.offset, *claimed_ref_name_key)),
            RefContainerRecordStatus::Evaluated => None,
        })
        .collect();
    assert_eq!(
        failed,
        vec![(bounds[1].0, Some(ref_name_key_bytes("heads/topic")))],
        "the damaged record must be named at its own offset, attributed to its own ref"
    );
    assert_eq!(
        replay
            .records
            .iter()
            .map(|r| &r.envelope)
            .collect::<Vec<_>>(),
        vec![&envelopes[0], &envelopes[2]],
        "both sound records -- for heads/main, on either side of the damaged heads/topic record -- \
         must still be read"
    );
    Ok(())
}

#[test]
fn append_and_replay_round_trips_one_ref() -> Result<()> {
    let root = unique_temp_dir("ref-container-append-replay");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = ref_name_key_bytes("heads/main");
    let envelope = signed_ref_update_envelope(
        "heads/main",
        None,
        sample_object_id("state"),
        sample_object_id("target"),
        1,
    );
    append_ref_container_record(&layout, key, &envelope)?;

    let replay = replay_ref_subsequence(&layout, key)?;
    assert_eq!(replay.records.len(), 1);
    assert_eq!(replay.records[0].envelope, envelope);
    assert_eq!(replay.trailing_partial_bytes, 0);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Step 0 §13.1's own ruling, proven against the real production append path (not hand-framed
/// bytes): each ref's own subsequence is exactly its own records, in relative order, unaffected by
/// interleaving with other refs' own appends in between.
#[test]
fn replay_ref_subsequence_filters_correctly_across_interleaved_refs() -> Result<()> {
    let root = unique_temp_dir("ref-container-interleaved-replay");
    let layout = RepositoryLayout::init(root.clone())?;
    let main_key = ref_name_key_bytes("heads/main");
    let topic_key = ref_name_key_bytes("heads/topic");

    let main_1 = signed_ref_update_envelope(
        "heads/main",
        None,
        sample_object_id("main-state-1"),
        sample_object_id("target"),
        1,
    );
    let topic_1 = signed_ref_update_envelope(
        "heads/topic",
        None,
        sample_object_id("topic-state-1"),
        sample_object_id("target"),
        1,
    );
    let main_2 = signed_ref_update_envelope(
        "heads/main",
        Some(sample_object_id("main-state-1")),
        sample_object_id("main-state-2"),
        sample_object_id("target"),
        2,
    );
    append_ref_container_record(&layout, main_key, &main_1)?;
    append_ref_container_record(&layout, topic_key, &topic_1)?;
    append_ref_container_record(&layout, main_key, &main_2)?;

    let main_replay = replay_ref_subsequence(&layout, main_key)?;
    assert_eq!(
        main_replay
            .records
            .iter()
            .map(|r| &r.envelope)
            .collect::<Vec<_>>(),
        vec![&main_1, &main_2]
    );
    let topic_replay = replay_ref_subsequence(&layout, topic_key)?;
    assert_eq!(
        topic_replay
            .records
            .iter()
            .map(|r| &r.envelope)
            .collect::<Vec<_>>(),
        vec![&topic_1]
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Design-v1.md §13.6 point 2: a ref's own torn tail is correctly attributed (via the header's
/// `ref_name_key`), reported through this ref's own `trailing_partial_bytes`, confirmed by
/// `incomplete_tail_matches`, and cleanly repairable by `truncate_incomplete_tail` -- the exact
/// recovery shape `refs/log.rs`'s per-file mechanism already proved, re-earned at container scale.
#[test]
fn own_torn_tail_is_attributed_and_repairable() -> Result<()> {
    let root = unique_temp_dir("ref-container-own-torn-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = ref_name_key_bytes("heads/main");
    let envelope = signed_ref_update_envelope(
        "heads/main",
        None,
        sample_object_id("state"),
        sample_object_id("target"),
        1,
    );
    let record = encode_ref_container_record(key, &envelope)?;
    let torn = record.get(..record.len() - 3).unwrap_or_default();
    std::fs::write(layout.ref_log_container_slot_path(ContainerSlot::A), torn)?;

    let replay = replay_ref_subsequence(&layout, key)?;
    assert_eq!(replay.records.len(), 0);
    assert_eq!(replay.trailing_partial_bytes, torn.len());
    assert!(incomplete_tail_matches(&layout, key, &envelope)?);

    let removed = truncate_incomplete_tail(&layout)?;
    assert_eq!(removed, torn.len());
    let after_truncate = replay_ref_subsequence(&layout, key)?;
    assert_eq!(after_truncate.trailing_partial_bytes, 0);

    append_ref_container_record(&layout, key, &envelope)?;
    let after_retry = replay_ref_subsequence(&layout, key)?;
    assert_eq!(after_retry.records, vec![super::RefLogRecord { envelope }]);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Design-v1.md §13.6 points 1 and 2, the ruling's own load-bearing safety argument, proven directly:
/// one ref's torn tail must never block, or misattribute to, a different ref's own publish. Ref B's
/// ordinary append succeeds and lands correctly even while ref A's own torn tail sits, unresolved, at
/// the container's physical end.
#[test]
fn foreign_torn_tail_does_not_block_or_misattribute_an_unrelated_ref() -> Result<()> {
    let root = unique_temp_dir("ref-container-foreign-torn-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    let a_key = ref_name_key_bytes("heads/a");
    let b_key = ref_name_key_bytes("heads/b");
    let a_envelope = signed_ref_update_envelope(
        "heads/a",
        None,
        sample_object_id("a-state"),
        sample_object_id("target"),
        1,
    );
    let record = encode_ref_container_record(a_key, &a_envelope)?;
    let torn = record.get(..record.len() - 3).unwrap_or_default();
    std::fs::write(layout.ref_log_container_slot_path(ContainerSlot::A), torn)?;

    // Ref B is not the torn tail's owner: its own replay must see no trailing partial at all.
    let b_replay_before = replay_ref_subsequence(&layout, b_key)?;
    assert_eq!(b_replay_before.trailing_partial_bytes, 0);

    // Ref B's ordinary publish must not be refused by A's unresolved debris.
    let b_envelope = signed_ref_update_envelope(
        "heads/b",
        None,
        sample_object_id("b-state"),
        sample_object_id("target"),
        1,
    );
    append_ref_container_record(&layout, b_key, &b_envelope)?;
    let b_replay_after = replay_ref_subsequence(&layout, b_key)?;
    assert_eq!(
        b_replay_after.records,
        vec![super::RefLogRecord {
            envelope: b_envelope
        }]
    );
    assert_eq!(b_replay_after.trailing_partial_bytes, 0);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
