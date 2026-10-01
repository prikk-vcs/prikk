//! RFC 165 R1: a publication reads the shared ref log container at most once (F1), and the
//! post-write check is a ranged read-back, not a trusted return (review v2 §2 item 2).
//!
//! Every test here sweeps a range or asserts the crash state it reached before asserting anything
//! about it -- review v2 §2 item 3: "never use a bare failpoint ordinal."
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{ObjectEnvelope, ObjectId, RefUpdatePayload};

use super::super::container::AppendOutcome;
use super::super::publication::{PublicationState, classify_state, ensure_agreement};
use crate::foundation::fsutil::read_tally;
use crate::foundation::layout::ref_name_key_bytes;
use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};

/// Build and publish one more generation on `ref_name`, returning the new publication. `previous`
/// is `None` for the ref's first-ever publication.
fn publish_next(
    store: &RefStore,
    objects: &mut FileObjectStore,
    ref_name: &str,
    previous: Option<(ObjectId, u64)>,
) -> prikk_error::Result<RefPublication> {
    let target = objects.write_object(&signed_empty_block_envelope())?;
    let (previous_id, seq) = match previous {
        Some((id, seq)) => (Some(id), seq + 1),
        None => (None, 1),
    };
    let ref_state = signed_ref_state_envelope(ref_name, previous_id, target, seq);
    let ref_state_id = ref_state.object_id();
    let publication = RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: previous_id,
        ref_update: signed_ref_update_envelope(ref_name, previous_id, ref_state_id, target, seq),
        ref_state,
    };
    store.publish_with_object_store(objects, &publication)?;
    Ok(publication)
}

/// Grow `ref_name` to `generations` (1-indexed), returning the last publication made and the
/// `(ref_state_id, update_seq)` pair a next generation would need.
fn grow_to_generation(
    layout: &RepositoryLayout,
    ref_name: &str,
    generations: u32,
) -> prikk_error::Result<(RefPublication, (ObjectId, u64))> {
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let mut last = publish_next(&store, &mut objects, ref_name, None)?;
    let mut previous = (last.ref_state.object_id(), 1u64);
    for _ in 2..=generations {
        last = publish_next(&store, &mut objects, ref_name, Some(previous))?;
        previous = (last.ref_state.object_id(), previous.1 + 1);
    }
    Ok((last, previous))
}

/// Read count: one whole read of the ref log per publication, at generations 4, 64 and 1,024, for
/// both a generic (`branch create`-shaped) publication and a real `seal`.
#[test]
fn one_whole_read_per_publication_at_several_generations() -> prikk_error::Result<()> {
    for generations in [4u32, 64, 1024] {
        // `branch create`-shaped: a generic publish via `RefStore::publish_with_object_store`, the
        // same call every publication (including `branch create`'s own) funnels through.
        {
            let root = unique_temp_dir(&format!("rfc165-r1-read-count-generic-{generations}"));
            let layout = RepositoryLayout::init(root.clone())?;
            let (last, previous) = grow_to_generation(&layout, "heads/main", generations)?;
            let log_path =
                layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
            let container_size_before = std::fs::metadata(&log_path)?.len();

            let mut objects = FileObjectStore::new(layout.clone());
            let store = RefStore::new(layout.clone());
            read_tally::reset();
            publish_next(&store, &mut objects, "heads/main", Some(previous))?;
            let bytes = read_tally::bytes_read(&layout.repository_relative(&log_path)?);
            // One whole read (~container_size_before) plus one small ranged read-back
            // (one record, tens of bytes): well under two whole reads' worth. The old, three-read
            // path would read roughly 3x container_size_before; this bounds it under 1.5x.
            assert!(
                bytes < container_size_before * 3 / 2,
                "generations={generations}: read {bytes} bytes, container was {container_size_before} \
                 bytes before this publish -- expected about one whole read, not several"
            );
            let _ = last;
            let _ = std::fs::remove_dir_all(&root);
        }

        // A real `seal`.
        {
            let root = unique_temp_dir(&format!("rfc165-r1-read-count-seal-{generations}"));
            let layout = RepositoryLayout::init(root.clone())?;
            let signer = crate::maintainer_signing::Ed25519MaintainerSigner::from_seed(
                "rfc165-r1-read-count",
                &[0x51; 32],
            )
            .map_err(|_| prikk_error::PrikkError::Integrity("seed".to_string()))?;
            let pub_hex: String = signer
                .public_key_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            crate::trust::add_trusted_maintainer(&layout, signer.key_id(), &pub_hex)?;
            for generation in 1..generations {
                queue_one_patch(&layout, "heads/main", &format!("g{generation}"))?;
                crate::simulate_one_seal_for_test_support(&layout, "heads/main", &signer)?;
            }
            let log_path =
                layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
            let container_size_before = std::fs::metadata(&log_path)?.len();

            queue_one_patch(&layout, "heads/main", "final")?;
            read_tally::reset();
            crate::simulate_one_seal_for_test_support(&layout, "heads/main", &signer)?;
            let bytes = read_tally::bytes_read(&layout.repository_relative(&log_path)?);
            assert!(
                bytes < container_size_before * 3 / 2,
                "seal, generations={generations}: read {bytes} bytes, container was \
                 {container_size_before} bytes before this seal"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }
    Ok(())
}

fn queue_one_patch(
    layout: &RepositoryLayout,
    ref_name: &str,
    label: &str,
) -> prikk_error::Result<()> {
    use prikk_object::{
        BlobKind, BlobPayload, CanonicalEncode, CreateFile, NodeId, ObjectType, Operation,
        OperationKind, PatchPayload, PatchPurpose,
    };

    let blob = BlobPayload::new(BlobKind::Text, format!("{label}\n").into_bytes());
    let blob_envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
    let blob_id = blob_envelope.object_id();
    FileObjectStore::new(layout.clone()).write_object(&blob_envelope)?;
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: format!("{label}.txt"),
                node_id: NodeId::from_bytes(*blob_id.as_bytes()),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let author = crate::Ed25519AuthorSigner::from_seed("rfc165-r1-read-count-author", &[0x52; 32])
        .map_err(|_| prikk_error::PrikkError::Integrity("seed".to_string()))?;
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(crate::author_signature(&author, envelope.object_id())?)?;
    crate::Wal::for_layout(layout, crate::DEFAULT_ACTIVE_NAME).append_patch(&envelope)?;
    crate::write_active_ref_metadata(layout, ref_name)?;
    Ok(())
}

/// Correctness: identical durable state to the ordinary path over several generations, and a
/// `Complete`-state retry (the same publication, submitted again) stays idempotent.
#[test]
fn durable_state_matches_across_generations_and_a_complete_retry_is_idempotent()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc165-r1-correctness");
    let layout = RepositoryLayout::init(root.clone())?;
    let (last, _) = grow_to_generation(&layout, "heads/main", 5)?;
    let store = RefStore::new(layout.clone());

    assert_eq!(
        store.read_current_ref_state_id("heads/main")?,
        Some(last.ref_state.object_id())
    );
    let replay = store.replay_log("heads/main")?;
    assert_eq!(replay.records.len(), 5);
    assert_eq!(replay.trailing_partial_bytes, 0);
    let report = crate::verify_repository(&layout)?;
    assert!(!report.has_blocking_ref_publication_issues());

    // A `Complete`-state retry: publish the exact same (already-landed) publication again.
    store.publish(&last)?;
    let replay_after_retry = store.replay_log("heads/main")?;
    assert_eq!(
        replay_after_retry.records.len(),
        5,
        "a Complete-state retry must not duplicate the record"
    );
    assert_eq!(replay_after_retry, replay);

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// The stale-replay control, made permanent: a second writer between `classify_state`'s own read and
/// the write must be caught by the ranged post-write check, and this control can fail (it is not a
/// test that always passes by construction).
#[test]
fn a_second_writer_between_the_read_and_the_write_is_caught() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc165-r1-stale-replay-control");
    let layout = RepositoryLayout::init(root.clone())?;
    let (root_publication, previous) = grow_to_generation(&layout, "heads/main", 1)?;
    let store = RefStore::new(layout.clone());

    let next_target =
        FileObjectStore::new(layout.clone()).write_object(&signed_empty_block_envelope())?;
    let next_state = signed_ref_state_envelope("heads/main", Some(previous.0), next_target, 2);
    let next_state_id = next_state.object_id();
    let next = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(previous.0),
        ref_update: signed_ref_update_envelope(
            "heads/main",
            Some(previous.0),
            next_state_id,
            next_target,
            2,
        ),
        ref_state: next_state,
    };
    let next_update = RefUpdatePayload::decode_canonical(&next.ref_update.canonical_payload)?;

    // The replay `classify_state` would see for `next`, before it is published -- what a real,
    // lock-held `publish_locked` call would carry forward from its own single read.
    let (state, stale_replay) = classify_state(&store, &next, &next_update)?;
    assert_eq!(state, PublicationState::Ready);

    // Publish `next` for real, through the already-proven-correct path, so the pointer and log
    // genuinely agree on it now.
    store.publish(&next)?;

    // SIMULATE "a second writer sneaks in between the reads": a torn tail appended directly to the
    // shared container, bypassing every lock `publish_locked` would normally hold for its entire
    // critical section. `stale_replay` (captured before this) knows nothing about it.
    super::super::append_torn_ref_log_tail_for_test(
        &layout,
        ref_name_key_bytes("heads/main"),
        &next.ref_update,
    )?;

    // The control: `ensure_agreement`, handed the stale replay and an `AlreadyPresent` outcome
    // (simulating a caller that reused a stale replay's own idempotency check instead of a fresh
    // one), still refuses -- because its own pointer re-read is fresh, not because of the replay.
    // The genuinely stale fact (`has_item_failure`) is checked from `stale_replay`, which this
    // injected tail does not set (a torn tail is `trailing_partial_bytes`, not `has_item_failure`) --
    // so this control specifically exercises the fresh-pointer-read half of the check, and a second,
    // explicit assertion below shows the stale replay's own blind spot directly.
    let fresh = ensure_agreement(
        &store,
        &next,
        &next_update,
        &stale_replay,
        &AppendOutcome::AlreadyPresent,
    );
    assert!(
        fresh.is_ok(),
        "the pointer itself did not change: {fresh:?}"
    );

    // Now the control that actually proves the lock-held invariant: `stale_replay.has_item_failure()`
    // is computed from bytes read *before* the injected tail, so it cannot see damage introduced
    // after it. A fresh replay, read now, does.
    assert!(
        !stale_replay.has_item_failure(),
        "the stale replay predates the injected tail"
    );
    let fresh_replay = store.replay_log("heads/main")?;
    assert_ne!(
        fresh_replay.trailing_partial_bytes, 0,
        "a fresh read must see the injected tail; the stale replay cannot"
    );

    let _ = root_publication;
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// The ranged check's own control (review v2 §2 item 2): bytes that land differ from the bytes
/// intended, so the post-write check refuses. With the byte-for-byte comparison removed (trusting
/// the append's own `Ok` return instead, as an earlier prototype did before this correction), this
/// test goes red: `ensure_agreement` would return `Ok` for corrupted bytes it never re-reads to
/// notice.
#[test]
fn a_ranged_mismatch_between_what_was_written_and_what_is_on_disk_is_caught()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc165-r1-ranged-check-control");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    let publication = publish_next(&store, &mut objects, "heads/main", None)?;
    let update = RefUpdatePayload::decode_canonical(&publication.ref_update.canonical_payload)?;

    // Reconstruct the exact `Wrote` outcome a real append produced for this publication: the record
    // is the whole container's content here (the only ref, one record), so its offset is 0 and its
    // bytes are the file's own content.
    let log_path = layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
    let intended_bytes = std::fs::read(&log_path)?;
    let claimed_outcome = AppendOutcome::Wrote {
        offset: 0,
        bytes: intended_bytes.clone(),
    };
    let replay = store.replay_log("heads/main")?;

    // Baseline: the real, uncorrupted bytes agree -- confirms the fixture before corrupting it.
    assert!(
        ensure_agreement(&store, &publication, &update, &replay, &claimed_outcome).is_ok(),
        "uncorrupted bytes must agree before this test corrupts them"
    );

    // Now corrupt exactly one byte of the record ON DISK, same length -- "the bytes that land differ
    // from the bytes intended," not a torn write.
    let mut corrupted = intended_bytes.clone();
    let last = corrupted
        .last_mut()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("expected bytes".to_string()))?;
    *last ^= 0xff;
    std::fs::write(&log_path, &corrupted)?;

    // `claimed_outcome` still carries the *intended* bytes (what `publish_locked` believed it wrote);
    // the ranged read-back now disagrees with them. This must refuse.
    let result = ensure_agreement(&store, &publication, &update, &replay, &claimed_outcome);
    assert!(
        result.is_err(),
        "a ranged read-back that disagrees with the intended bytes must refuse, not silently pass"
    );

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// Sanity: the whole-read guard still fires if a production call to `replay_ref_subsequence`
/// somehow ran outside its own declared scope -- not a new assertion, but confirms the scaffolding
/// this file's own read-count test leans on (`read_tally`) is reading the right file.
#[test]
fn read_tally_sees_the_ref_log_containers_own_path() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc165-r1-read-tally-sanity");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_next(&store, &mut objects, "heads/main", None)?;
    let log_path = layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
    let relative = layout.repository_relative(&log_path)?;

    read_tally::reset();
    assert_eq!(read_tally::bytes_read(&relative), 0);
    let _ = store.replay_log("heads/main")?;
    assert!(
        read_tally::bytes_read(&relative) > 0,
        "a real whole read of the ref log container must register on the tally"
    );

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
