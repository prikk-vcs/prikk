//! 0.59.0 P5a2: the ref log's summary against the whole walk, over every state the case tables name.
//!
//! The oracle is the walk every caller has always made (`with_whole_walk_for_test` switches the summary off). Each state is observed twice, by
//! the summary (under a hold that has already kept it) and by the whole walk: the **precondition** (`ensure_no_incomplete_publication`,
//! `ensure_may_publish`) and the **classification** of the next publication of the ref (`classify_state`: its state, its last record, its
//! damage and tail), and the two must agree **in the decision and in the refusal text**. Each case also pins which fallback, if any, the
//! summary took, so a case that falls back is not mistaken for one the summary answered. **Linux and macOS only** (the hold's identity).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::{CanonicalEncode as _, ObjectEnvelope, ObjectId, ObjectType, RefUpdatePayload};

use super::{Fallback, take_fallbacks_for_test, with_whole_walk_for_test};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, ref_name_key_bytes};
use crate::lock::acquire_container_locks;
use crate::refs::container::encode_ref_container_record_for_test;
use crate::refs::kept_pointer_index::tests::publish_ref;
use crate::refs::kept_ref_log::load_ref_log_view;
use crate::refs::pointer_index::{PointerIndexEntry, append_ref_pointer_entry};
use crate::refs::publication::classify_state;
use crate::test_gates::test_support::{
    maintainer_signature, sample_object_id, signed_ref_state_envelope, unique_temp_dir,
};
use crate::{LockableContainer, RefPublication, RefStore};

const MAIN: &str = "heads/main";

fn repository(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    (RepositoryLayout::init(root.clone()).expect("init"), root)
}

fn another_process(layout: &RepositoryLayout) -> RepositoryLayout {
    RepositoryLayout::open(layout.root().to_path_buf()).expect("reopen")
}

fn log_path(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.ref_log_container_slot_path(ContainerSlot::A)
}

/// A RefUpdate envelope with every field the chain checks, as a test plants it (a record the real writer would refuse to produce).
fn update(
    ref_name: &str,
    old: Option<ObjectId>,
    new: ObjectId,
    seq: u64,
    created_at: u64,
) -> ObjectEnvelope {
    let payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: old,
        new_ref_state_id: new,
        new_target_object_id: sample_object_id(&format!("{ref_name} target {seq}")),
        update_seq: seq,
        created_at,
        author_key_id: "maintainer-key".to_string(),
    };
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        payload.to_canonical_bytes().expect("encode"),
    );
    envelope
        .add_signature(maintainer_signature())
        .expect("sign");
    envelope
}

/// Append one record's frame to the log as it is (the key is the header's).
fn append_record(layout: &RepositoryLayout, key_of: &str, envelope: &ObjectEnvelope) {
    let frame =
        encode_ref_container_record_for_test(ref_name_key_bytes(key_of), envelope).expect("frame");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path(layout))
        .expect("open");
    std::io::Write::write_all(&mut file, &frame).expect("append");
}

/// The next publication of `MAIN` from what the pointer says now.
fn next_publication(layout: &RepositoryLayout) -> RefPublication {
    let store = RefStore::new(layout.clone());
    let previous = store.read_current_ref_state_id(MAIN).expect("pointer");
    let seq = 1 + store
        .replay_log(MAIN)
        .map(|replay| replay.records.len() as u64)
        .unwrap_or(0);
    let target = sample_object_id(&format!("{MAIN} next {seq}"));
    let ref_state = signed_ref_state_envelope(MAIN, previous, target, seq);
    let ref_state_id = ref_state.object_id();
    RefPublication {
        ref_name: MAIN.to_string(),
        expected_previous_ref_state_id: previous,
        ref_state,
        ref_update: update(MAIN, previous, ref_state_id, seq, 0),
    }
}

/// What a use of the ref log decides, as text: both preconditions, and the classification of the next publication of `MAIN` (its state, its last
/// record, its tail and damage). Compared between the two ways of reading, so it includes the refusal text of each.
fn observe(layout: &RepositoryLayout) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "no-incomplete: {:?}",
        crate::refs::ensure_no_incomplete_publication(layout).map_err(|e| e.to_string())
    ));
    out.push(format!(
        "may-publish: {:?}",
        crate::refs::ensure_may_publish(layout, MAIN).map_err(|e| e.to_string())
    ));
    let store = RefStore::new(layout.clone());
    let publication = next_publication(layout);
    let update = RefUpdatePayload::decode_canonical(&publication.ref_update.canonical_payload)
        .expect("decode");
    out.push(match classify_state(&store, &publication, &update) {
        Ok((state, replay)) => format!(
            "classify: {state:?} last={:?} trailing={} failure={}",
            replay
                .records
                .last()
                .map(|record| record.envelope.object_id()),
            replay.trailing_partial_bytes,
            replay.has_item_failure()
        ),
        Err(error) => format!("classify: error {error}"),
    });
    // The classification of a publication that is already complete (a retry after the record was written): the log's last record **is** the
    // publication's `RefUpdate`, which only the summary's kept last envelope can say.
    if let Some(last) = store
        .replay_log(MAIN)
        .ok()
        .and_then(|replay| replay.records.last().map(|record| record.envelope.clone()))
    {
        let recorded = RefUpdatePayload::decode_canonical(&last.canonical_payload).expect("decode");
        let publication = RefPublication {
            ref_name: MAIN.to_string(),
            expected_previous_ref_state_id: recorded.old_ref_state_id,
            ref_state: signed_ref_state_envelope(
                MAIN,
                recorded.old_ref_state_id,
                recorded.new_target_object_id,
                recorded.update_seq,
            ),
            ref_update: last,
        };
        out.push(match classify_state(&store, &publication, &recorded) {
            Ok((state, replay)) => format!(
                "classify-last: {state:?} last={:?}",
                replay
                    .records
                    .last()
                    .map(|record| record.envelope.object_id())
            ),
            Err(error) => format!("classify-last: error {error}"),
        });
    }
    out
}

/// The summary's observation (under a hold that already kept the summary of the log as it stands) and the whole walk's, with the fallbacks the
/// summary took on the way. The two observations must be equal.
fn both_ways(layout: &RepositoryLayout, label: &str) -> (Vec<String>, Vec<Fallback>) {
    let _hold = acquire_container_locks(layout, &[LockableContainer::TrustPolicy]).expect("hold");
    let _ = load_ref_log_view(layout);
    let _ = take_fallbacks_for_test();
    let summarised = observe(layout);
    let fallbacks = take_fallbacks_for_test();
    let whole = with_whole_walk_for_test(|| observe(layout));
    assert_eq!(
        summarised, whole,
        "{label}: the summary and the whole walk decided differently"
    );
    (summarised, fallbacks)
}

fn publish_main_and_other(layout: &RepositoryLayout, main: u64, other: u64) -> Result<()> {
    // Interleaved: another ref's records between this ref's.
    let (mut m, mut o) = (0, 0);
    while m < main || o < other {
        if m < main {
            m += 1;
            publish_ref(layout, MAIN, m)?;
        }
        if o < other {
            o += 1;
            publish_ref(layout, "heads/other", o)?;
        }
    }
    Ok(())
}

#[test]
fn a_sound_log_with_another_refs_records_between_is_answered_by_the_summary() -> Result<()> {
    let (layout, root) = repository("p5a2-sound");
    publish_main_and_other(&layout, 6, 4)?;
    let (observed, fallbacks) = both_ways(&layout, "sound");
    assert!(
        fallbacks.is_empty(),
        "no fallback on a sound log: {fallbacks:?}"
    );
    assert!(observed[0].starts_with("no-incomplete: Ok"), "{observed:?}");
    assert!(observed[2].contains("Ready"), "{observed:?}");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn an_empty_log_and_no_log_are_the_same_as_the_whole_walk() -> Result<()> {
    let (layout, root) = repository("p5a2-empty");
    let (observed, fallbacks) = both_ways(&layout, "empty");
    assert!(observed[2].contains("Ready"), "{observed:?}");
    assert!(
        fallbacks.is_empty() || fallbacks == [Fallback::NothingKept],
        "{fallbacks:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A lead: the pointer one transition ahead of the log (a publication killed between its pointer and its record).
#[test]
fn a_pointer_lead_is_decided_as_the_whole_walk_decides_it() -> Result<()> {
    let (layout, root) = repository("p5a2-lead");
    publish_main_and_other(&layout, 3, 2)?;
    let next = next_publication(&layout);
    append_ref_pointer_entry(
        &layout,
        &PointerIndexEntry {
            ref_name_key: ref_name_key_bytes(MAIN),
            ref_name: MAIN.to_string(),
            ref_state_id: next.ref_state.object_id(),
        },
    )?;
    let (observed, fallbacks) = both_ways(&layout, "lead");
    assert!(fallbacks.is_empty(), "{fallbacks:?}");
    assert!(
        observed[0].contains("Err") || observed[0].contains("IncompletePublication"),
        "the precondition refuses a lead: {observed:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_torn_tail_and_a_damaged_interior_record_are_read_whole() -> Result<()> {
    for torn in [true, false] {
        let (layout, root) = repository(if torn { "p5a2-torn" } else { "p5a2-damaged" });
        publish_main_and_other(&layout, 4, 3)?;
        let path = log_path(&layout);
        if torn {
            let file = std::fs::OpenOptions::new().write(true).open(&path)?;
            file.set_len(std::fs::metadata(&path)?.len() - 9)?;
        } else {
            let mut bytes = std::fs::read(&path)?;
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0xff;
            std::fs::write(&path, &bytes)?;
        }
        let label = if torn {
            "torn tail"
        } else {
            "damaged interior"
        };
        let (_, fallbacks) = both_ways(&layout, label);
        // Never kept, so every use is the whole read: nothing for the summary to answer, nothing it may get wrong.
        assert!(
            fallbacks.iter().all(|f| *f == Fallback::NothingKept),
            "{label}: {fallbacks:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

/// A record of one id twice (format 7 wrote them), an out-of-sequence record, and a record whose payload names another ref than its header: each
/// makes the chain unsound, so the classification of that ref is the whole walk's, with its refusal; and a record with `created_at != 0` is the
/// precondition's refusal. The summary says each by its fallback, and never decides one itself.
#[test]
fn an_unsound_chain_falls_back_and_is_refused_as_the_whole_walk_refuses() -> Result<()> {
    type Plant = fn(&RepositoryLayout);
    let cases: [(&str, Plant, Option<Fallback>); 4] = [
        (
            "a duplicate record of one id",
            |layout| {
                let log = std::fs::read(log_path(layout)).unwrap();
                let replay = crate::refs::container::decode_ref_container_records(&log).unwrap();
                let last = replay.records.last().unwrap().clone();
                append_record(layout, MAIN, &last.envelope);
            },
            Some(Fallback::ChainUnsound),
        ),
        (
            "a record out of sequence",
            |layout| {
                let store = RefStore::new(layout.clone());
                let current = store.read_current_ref_state_id(MAIN).unwrap();
                append_record(
                    layout,
                    MAIN,
                    &update(MAIN, current, sample_object_id("skip"), 9, 0),
                );
            },
            Some(Fallback::ChainUnsound),
        ),
        (
            "a payload that names another ref than its header",
            |layout| {
                let store = RefStore::new(layout.clone());
                let current = store.read_current_ref_state_id(MAIN).unwrap();
                let seq = store.replay_log(MAIN).unwrap().records.len() as u64 + 1;
                append_record(
                    layout,
                    MAIN,
                    &update("heads/zzzz", current, sample_object_id("name"), seq, 0),
                );
            },
            Some(Fallback::ChainUnsound),
        ),
        (
            "a record with created_at != 0",
            |layout| {
                let store = RefStore::new(layout.clone());
                let current = store.read_current_ref_state_id(MAIN).unwrap();
                let seq = store.replay_log(MAIN).unwrap().records.len() as u64 + 1;
                append_record(
                    layout,
                    "heads/other",
                    &update("heads/other", None, sample_object_id("late"), 1, 7),
                );
                let _ = (current, seq);
            },
            None,
        ),
    ];
    for (label, plant, expected_fallback) in cases {
        let (layout, root) = repository("p5a2-unsound");
        publish_main_and_other(&layout, 3, 2)?;
        plant(&layout);
        let (observed, fallbacks) = both_ways(&layout, label);
        match expected_fallback {
            Some(reason) => assert!(
                fallbacks.contains(&reason),
                "{label}: expected the fallback {reason:?}, got {fallbacks:?} for {observed:?}"
            ),
            None => assert!(
                observed[0].contains("Err"),
                "{label}: the precondition refuses a created_at: {observed:?}"
            ),
        }
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

/// Another process extends the log between two uses: the kept summary reads the tail and answers as a fresh walk does.
#[test]
fn a_log_another_process_extends_between_two_uses_is_read_to_its_tail() -> Result<()> {
    let (layout, root) = repository("p5a2-extended");
    publish_main_and_other(&layout, 3, 2)?;
    let other = another_process(&layout);
    let _hold = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy]).expect("hold");
    let _ = load_ref_log_view(&layout)?;
    for step in 4..=9_u64 {
        publish_ref(&other, MAIN, step)?;
        publish_ref(&other, "heads/other", step - 1)?;
        let _ = take_fallbacks_for_test();
        let summarised = observe(&layout);
        assert!(
            take_fallbacks_for_test().is_empty(),
            "step {step}: the tail was read, the summary answered"
        );
        let whole = with_whole_walk_for_test(|| observe(&layout));
        assert_eq!(summarised, whole, "step {step}");
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The summary never answers a use the whole walk would refuse: over every prefix of a log with a duplicate planted at the end, and a lead.
#[test]
fn the_precondition_decides_the_same_at_every_length_of_a_growing_log() -> Result<()> {
    let (layout, root) = repository("p5a2-growing");
    for step in 1..=12_u64 {
        publish_ref(&layout, MAIN, step)?;
        if step % 3 == 0 {
            publish_ref(&layout, "heads/other", step / 3)?;
        }
        let _ = both_ways(&layout, &format!("length {step}"));
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A hold's publications read the log whole **once**: after the first use, each publication reads the canary and what was appended (a record or
/// two), however long the log is. (Before P5a2 each decoded and copied the whole subsequence: the bytes read were the same, the walk was not, so
/// this pins the walk by the fallbacks and the summary's count as well.)
#[test]
fn a_holds_publications_read_the_log_whole_once_and_never_fall_back() -> Result<()> {
    use crate::foundation::fsutil::read_tally;
    let (layout, root) = repository("p5a2-reads");
    for step in 1..=40_u64 {
        publish_ref(&layout, MAIN, step)?;
    }
    let relative = layout
        .repository_relative(&log_path(&layout))
        .expect("relative");
    let whole = std::fs::metadata(log_path(&layout))?.len();
    let _hold = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy]).expect("hold");
    let _ = take_fallbacks_for_test();
    read_tally::reset();
    for step in 41..=100_u64 {
        publish_ref(&layout, MAIN, step)?;
    }
    let read = read_tally::bytes_read(&relative);
    let grown = std::fs::metadata(log_path(&layout))?.len();
    // The first publication under the hold reads the log whole; sixty publications of one record each read at most a few records apiece after it.
    let record = (grown - whole) / 60;
    assert!(
        read <= whole + 60 * 4 * record,
        "{read} bytes read for 60 publications over a {whole}-byte log of {record}-byte records"
    );
    assert_eq!(
        take_fallbacks_for_test(),
        vec![Fallback::NothingKept],
        "the only fallback is the first use, which had nothing kept"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
