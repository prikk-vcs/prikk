#![allow(clippy::expect_used, clippy::indexing_slicing)]

use prikk_error::Result;
use prikk_object::{
    CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use super::{
    WriteDecision, append_object_to_container, decide_write_outcome, lookup_object_location,
    rebuild_index_from_containers,
};
use crate::foundation::container;
use crate::foundation::fsutil::append_file_required;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::test_gates::test_support::{
    sample_object_id, signed_patch_blob_envelope, signed_patch_envelope,
};

/// Composes the pieces `write_object_to_container` used to be, exactly reproducing its old
/// behavior (idempotency check, then unconditional append on `New`) -- for the tests below that are
/// about the container/index write pipeline itself (append-only, rebuild, damaged-index handling),
/// not about the idempotency decision specifically (RFC 111 §6.1 addendum, C2 moved those two tests
/// down to `decide_write_outcome` directly, below).
fn write_object_to_container_for_test(
    layout: &RepositoryLayout,
    object_type: ObjectType,
    envelope: &ObjectEnvelope,
) -> Result<ObjectId> {
    let existing = lookup_object_location(layout, envelope.object_id())?;
    match decide_write_outcome(layout, object_type, envelope, existing.as_ref())? {
        WriteDecision::AlreadyPresent(id) => Ok(id),
        WriteDecision::New => {
            append_object_to_container(layout, object_type, envelope).map(|entry| entry.object_id)
        }
        WriteDecision::Merge(union) => {
            append_object_to_container(layout, object_type, &union).map(|entry| entry.object_id)
        }
    }
}

fn normal_patch_envelope(label: &str) -> Result<ObjectEnvelope> {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: NodeId::from_bytes([0x61; 32]),
                blob_id: sample_object_id(label),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(crate::test_gates::test_support::rollback_author_signature())?;
    Ok(envelope)
}

#[test]
fn write_then_lookup_round_trips() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-write-lookup");
    let layout = RepositoryLayout::init(root.clone())?;
    let envelope = signed_patch_envelope();
    let id = write_object_to_container_for_test(&layout, ObjectType::Patch, &envelope)?;
    assert_eq!(id, envelope.object_id());

    let entry = lookup_object_location(&layout, id)?.expect("entry must be found after write");
    assert_eq!(entry.object_id, id);
    assert_eq!(entry.object_type, ObjectType::Patch);
    assert_eq!(entry.slot, ContainerSlot::A);
    assert_eq!(entry.offset, 0);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff §4 / §5 acceptance criterion 2, demonstrated rather than merely inspected (the same
/// technique `worktree_marker/tests.rs::marker_dirty_set_appends_rather_than_replaces` used for RFC
/// 102 Stage 1's own version of this criterion): `atomic_replace` would overwrite each file's content
/// outright on every write, so a second, distinct object landing in a container/index file that is
/// exactly the first object's bytes with the second's appended after is only possible if both files
/// are genuinely appended to -- confirming by observed behavior what `write_object_to_container`'s own
/// imports (`append_file_required`, never `write_file_atomically`) already show by inspection.
#[test]
fn container_and_index_writes_append_rather_than_replace() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-append-not-replace");
    let layout = RepositoryLayout::init(root.clone())?;
    let container_path = layout.container_slot_path(ObjectType::Patch, ContainerSlot::A);
    let index_path = layout.container_index_path();

    write_object_to_container_for_test(
        &layout,
        ObjectType::Patch,
        &normal_patch_envelope("first")?,
    )?;
    let container_after_first = std::fs::read(&container_path)?;
    let index_after_first = std::fs::read(&index_path)?;
    assert!(!container_after_first.is_empty());
    assert!(!index_after_first.is_empty());

    write_object_to_container_for_test(
        &layout,
        ObjectType::Patch,
        &normal_patch_envelope("second")?,
    )?;
    let container_after_second = std::fs::read(&container_path)?;
    let index_after_second = std::fs::read(&index_path)?;

    assert!(container_after_second.len() > container_after_first.len());
    assert!(container_after_second.starts_with(&container_after_first));
    assert!(index_after_second.len() > index_after_first.len());
    assert!(index_after_second.starts_with(&index_after_first));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 111 §6.1 addendum, C2: moved down onto `decide_write_outcome` itself, proving the rule
/// independent of either caller (`FileObjectStore` and `ObjectWriteSession` both reach this same
/// function; only where their `existing` lookup comes from differs).
#[test]
fn decide_write_outcome_is_idempotent_for_a_matching_existing_object() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-idempotent-rewrite");
    let layout = RepositoryLayout::init(root.clone())?;
    let envelope = signed_patch_envelope();
    write_object_to_container_for_test(&layout, ObjectType::Patch, &envelope)?;
    let existing = lookup_object_location(&layout, envelope.object_id())?;

    assert_eq!(
        decide_write_outcome(&layout, ObjectType::Patch, &envelope, existing.as_ref())?,
        WriteDecision::AlreadyPresent(envelope.object_id()),
        "the same envelope against its own existing entry must be a no-op decision"
    );
    assert_eq!(
        decide_write_outcome(&layout, ObjectType::Patch, &envelope, None)?,
        WriteDecision::New,
        "with no existing entry the same envelope must decide New, regardless of what is on disk"
    );

    let replay = super::replay_index(&layout)?;
    assert_eq!(
        replay.entries.len(),
        1,
        "the index itself must still hold exactly one entry: this test never appends a second one"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn rebuild_recovers_every_sound_object_across_all_container_types() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-rebuild-multi-type");
    let layout = RepositoryLayout::init(root.clone())?;
    let patch = normal_patch_envelope("rebuild-patch")?;
    let blob = signed_patch_blob_envelope();
    write_object_to_container_for_test(&layout, ObjectType::Patch, &patch)?;
    write_object_to_container_for_test(&layout, ObjectType::Blob, &blob)?;

    let rebuilt = rebuild_index_from_containers(&layout)?;
    let ids: Vec<_> = rebuilt.iter().map(|entry| entry.object_id).collect();
    assert!(ids.contains(&patch.object_id()));
    assert!(ids.contains(&blob.object_id()));
    assert_eq!(rebuilt.len(), 2);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The acceptance criterion (handoff §5 criterion 3): a crash between the container append and the
/// index append leaves the object present and unindexed, recoverable by rebuild -- never the reverse
/// (an index entry pointing at bytes that are not there). Simulated by calling exactly step 1 of
/// `write_object_to_container`'s own protocol (append the container record) without step 2.
#[test]
fn crash_between_container_and_index_append_leaves_the_object_unindexed_and_recoverable()
-> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-crash-ordering");
    let layout = RepositoryLayout::init(root.clone())?;
    let envelope = signed_patch_envelope();

    // Step 1 only, matching `write_object_to_container`'s own first step exactly -- no index append
    // follows, simulating a crash right after the container append became durable.
    let record_bytes = container::encode_container_record(ObjectType::Patch, &envelope)?;
    let container_relative = layout
        .repository_relative(&layout.container_slot_path(ObjectType::Patch, ContainerSlot::A))?;
    append_file_required(
        layout.repository_mutation_root(),
        &container_relative,
        &record_bytes,
    )?;

    // The object is present (a container scan finds it)...
    let container_bytes =
        std::fs::read(layout.container_slot_path(ObjectType::Patch, ContainerSlot::A))?;
    let container_replay =
        container::decode_container_records(ObjectType::Patch, &container_bytes)?;
    assert_eq!(container_replay.records.len(), 1);
    assert_eq!(container_replay.records[0].envelope, envelope);

    // ...but unindexed: an ordinary lookup finds nothing, never a dangling entry pointing at these
    // bytes (the property that must never be violated, design §5).
    assert!(lookup_object_location(&layout, envelope.object_id())?.is_none());

    // Recovered by rebuild, the safe direction.
    let rebuilt = rebuild_index_from_containers(&layout)?;
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt[0].object_id, envelope.object_id());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 111 §6.1 addendum, C2: moved down onto `decide_write_outcome` itself. Preserves
/// `publish_immutable_file`'s exact idempotency contract (the loose-file mechanism this replaces): a
/// same-`object_id` rewrite is a silent no-op only when its full envelope bytes match what is already
/// stored. `object_id` does not cover signatures, so two envelopes can share an id while differing in
/// signature content -- that must still be a reported conflict, not silently accepted, exactly as the
/// old `compare_existing`'s `bytes != candidate` check enforced.
#[test]
fn decide_write_outcome_rejects_a_same_id_rewrite_with_different_signatures() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-conflicting-rewrite");
    // Format 6's rule: one record per id (RFC 156 §5b). Format 7 merges instead.
    let layout = crate::test_gates::test_support::init_format_6_repository(root.clone())?;
    let first = signed_patch_envelope();
    write_object_to_container_for_test(&layout, ObjectType::Patch, &first)?;
    let existing = lookup_object_location(&layout, first.object_id())?;

    // Same canonical_payload/type/schema -- hence the same object_id, which does not cover
    // signatures -- but a different signature.
    let mut second = first.clone();
    second.signatures.clear();
    let mut signature = crate::test_gates::test_support::rollback_author_signature();
    signature.signature_bytes[0] ^= 0x01;
    second.add_signature(signature)?;
    assert_eq!(
        second.object_id(),
        first.object_id(),
        "the two envelopes must share an object_id for this test to prove anything"
    );

    assert!(decide_write_outcome(&layout, ObjectType::Patch, &second, existing.as_ref()).is_err());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 156 §5b, Stage 2a control 2: a **format-6** repository keeps refusing a second envelope for a
/// stored id — and the refusal names `prikk format upgrade`.
#[test]
fn a_format_6_repository_refuses_a_second_envelope_naming_the_upgrade() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-format-6-rewrite");
    RepositoryLayout::init(root.clone())?;
    std::fs::write(root.join(".prikk").join("FORMAT"), b"6\n")?;
    let layout = RepositoryLayout::open(root.clone())?;
    assert_eq!(
        layout.format(),
        crate::foundation::layout::RepositoryFormat::CurrentV6
    );
    let first = signed_patch_envelope();
    write_object_to_container_for_test(&layout, ObjectType::Patch, &first)?;
    let existing = lookup_object_location(&layout, first.object_id())?;
    let mut second = first.clone();
    second.signatures.clear();
    let mut signature = crate::test_gates::test_support::rollback_author_signature();
    signature.signature_bytes[0] ^= 0x01;
    second.add_signature(signature)?;

    let refused = decide_write_outcome(&layout, ObjectType::Patch, &second, existing.as_ref());
    assert!(
        matches!(refused, Err(prikk_error::PrikkError::Integrity(ref message))
            if message.contains("differs from candidate") && message.contains("prikk format upgrade")),
        "{refused:?}"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_damaged_index_entry_blocks_lookup_as_a_reported_defect() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-damaged-entry");
    let layout = RepositoryLayout::init(root.clone())?;
    let envelope = signed_patch_envelope();
    write_object_to_container_for_test(&layout, ObjectType::Patch, &envelope)?;

    let index_path = layout.container_index_path();
    let mut bytes = std::fs::read(&index_path)?;
    let last = bytes.last_mut().expect("index file must be non-empty");
    *last ^= 0x01;
    std::fs::write(&index_path, &bytes)?;

    assert!(lookup_object_location(&layout, envelope.object_id()).is_err());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 156 Stage 2b control 7: an envelope whose payload differs from the stored record it is decided
/// against is not another signer's copy — it refuses as Integrity in format 7 too, never merges.
#[test]
fn a_payload_mismatch_under_one_id_refuses_as_integrity_in_format_7() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-payload-mismatch");
    let layout = RepositoryLayout::init(root.clone())?;
    assert_eq!(
        layout.format(),
        crate::foundation::layout::RepositoryFormat::V7
    );
    let first = signed_patch_envelope();
    write_object_to_container_for_test(&layout, ObjectType::Patch, &first)?;
    let existing = lookup_object_location(&layout, first.object_id())?;

    // The store is asked about `first`'s record for a candidate carrying other payload bytes — the state
    // a corrupted index entry or a hash collision would present.
    let mut other = first.clone();
    other.canonical_payload.push(0x00);
    let refused = decide_write_outcome(&layout, ObjectType::Patch, &other, existing.as_ref());
    assert!(
        matches!(refused, Err(prikk_error::PrikkError::Integrity(ref message))
            if message.contains("in its payload")),
        "{refused:?}"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 156 Stage 2b: in format 7 the same payload under another signature merges into the union, and a
/// candidate adding no signature is already present.
#[test]
fn a_format_7_repository_merges_another_signers_copy() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-format-7-merge");
    let layout = RepositoryLayout::init(root.clone())?;
    let first = signed_patch_envelope();
    write_object_to_container_for_test(&layout, ObjectType::Patch, &first)?;
    let existing = lookup_object_location(&layout, first.object_id())?;
    let mut second = first.clone();
    second.signatures.clear();
    second.add_signature({
        let mut other = crate::test_gates::test_support::dummy_signature();
        other.key_id = "zzz-another-signer".to_string();
        other
    })?;

    let decision = decide_write_outcome(&layout, ObjectType::Patch, &second, existing.as_ref())?;
    let WriteDecision::Merge(union) = decision else {
        return Err(prikk_error::PrikkError::Integrity(format!(
            "expected a merge, got {decision:?}"
        )));
    };
    assert_eq!(union.signatures.len(), first.signatures.len() + 1);
    assert!(
        first
            .signatures
            .iter()
            .chain(&second.signatures)
            .all(|signature| union.signatures.contains(signature))
    );
    let subset = first.clone();
    assert!(matches!(
        decide_write_outcome(&layout, ObjectType::Patch, &subset, existing.as_ref())?,
        WriteDecision::AlreadyPresent(_)
    ));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// ---- RFC 102, the append-length round: an object append learns its container's length from a stat of the descriptor it appends
// ---- to, never by reading the container. Controls 1-4 of `rfcs/handoffs/102-container-based-durability/
// ---- append-length-without-reading-handoff-v1.md` (control 5, the race controls, stays where it was: `object_store/tests.rs`).

fn container_relative(layout: &RepositoryLayout, object_type: ObjectType) -> std::path::PathBuf {
    layout
        .repository_relative(&layout.container_slot_path(object_type, ContainerSlot::A))
        .expect("the container is under the repository")
}

fn blob_envelope(label: &str, size: usize) -> ObjectEnvelope {
    let mut payload = label.as_bytes().to_vec();
    payload.resize(size.max(payload.len()), b'.');
    ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload)
}

/// **Control 1 -- an append does not read its container.** Forty blobs are appended to a repository's blob container through the file
/// store's own append path; the anchored reader's tally (`read_tally`, `cfg(test)`) shows **no byte** of that container read. A
/// positive control reads one object back -- which does read the container -- and sees the tally move, so the counter is known to be
/// able to see a container read.
/// **Perturb:** put `read_file_if_exists(container)?.map_or(0, |bytes| bytes.len())` back in `append_object_to_container` (the
/// whole-container read this round removed): the first assertion goes red.
#[test]
fn an_object_append_does_not_read_its_container() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-append-reads-nothing");
    let layout = RepositoryLayout::init(root.clone())?;
    let relative = container_relative(&layout, ObjectType::Blob);
    crate::foundation::fsutil::read_tally::reset();
    let mut last = None;
    for index in 0..40 {
        let envelope = blob_envelope(&format!("blob-{index}"), 4096);
        last = Some(append_object_to_container(
            &layout,
            ObjectType::Blob,
            &envelope,
        )?);
    }
    assert_eq!(
        crate::foundation::fsutil::read_tally::bytes_read(&relative),
        0,
        "forty appends read nothing of the container"
    );
    // The counter can see a container read: reading one object back reads the container.
    let entry = last.expect("appended");
    crate::foundation::index::read_object_envelope_at(&layout, &entry)?;
    assert!(
        crate::foundation::fsutil::read_tally::bytes_read(&relative) > 0,
        "fixture sanity: an object read reads the container, and the tally sees it"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// **Control 2 -- offsets unchanged.** A real repository -- sealed history, so patches, blocks, blobs, ref states -- is checked entry
/// by entry: within each container, sorted by offset, every entry's offset equals the sum of the lengths of the records before it, the
/// first is 0, and the last ends exactly at the container's length; and `verify` finds nothing.
/// **Perturb:** `offset + 1` in `append_object_to_container`: red.
#[test]
fn every_index_entry_offset_is_the_sum_of_the_records_before_it_and_verify_is_clean() -> Result<()>
{
    let history = crate::test_gates::test_support::AnchoredHistory::standard("index-offsets", 30);
    let layout = &history.layout;
    let replay = crate::foundation::index::replay_index(layout)?;
    assert!(replay.entries.len() > 60, "fixture sanity: many objects");
    let mut by_container: std::collections::BTreeMap<String, Vec<(u64, u64)>> =
        std::collections::BTreeMap::new();
    for entry in &replay.entries {
        by_container
            .entry(format!("{:?}/{:?}", entry.object_type, entry.slot))
            .or_default()
            .push((entry.offset, entry.length));
    }
    assert!(
        by_container.len() >= 3,
        "fixture sanity: mixed object types"
    );
    for (container, mut records) in by_container {
        records.sort_unstable();
        let mut expected = 0_u64;
        for (offset, length) in &records {
            assert_eq!(
                *offset, expected,
                "{container}: an entry's offset is the sum of the records before it"
            );
            expected += length;
        }
        let object_type = replay
            .entries
            .iter()
            .find(|entry| format!("{:?}/{:?}", entry.object_type, entry.slot) == container)
            .map(|entry| entry.object_type)
            .expect("an entry");
        let file_len =
            std::fs::metadata(layout.container_slot_path(object_type, ContainerSlot::A))?.len();
        assert_eq!(
            expected, file_len,
            "{container}: the last record ends at the container's length"
        );
    }
    let verification = crate::verify_repository(layout)?;
    assert!(
        !verification.has_item_failure() && !verification.has_stage_failure(),
        "verify is clean: {verification:?}"
    );
    Ok(())
}

/// **Control 3 -- a torn tail counts in the length, exactly as the read counted it.** Garbage bytes appended to the end of a container
/// (what an interrupted append leaves) are part of the file; the next object's recorded offset is the container's **full** length,
/// torn bytes included. The same scenario ran on 0.47.0's code before this change and gave the same offset.
/// **Perturb:** take the length only up to the last complete frame (`offset` minus the garbage): red.
#[test]
fn a_torn_tail_counts_in_the_next_objects_offset() -> Result<()> {
    let root = crate::test_gates::test_support::unique_temp_dir("index-append-torn-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    for index in 0..3 {
        append_object_to_container(
            &layout,
            ObjectType::Blob,
            &blob_envelope(&format!("before-{index}"), 300),
        )?;
    }
    let container = layout.container_slot_path(ObjectType::Blob, ContainerSlot::A);
    let mut file = std::fs::OpenOptions::new().append(true).open(&container)?;
    std::io::Write::write_all(&mut file, &[0xAB; 7])?;
    drop(file);
    let full_length = std::fs::metadata(&container)?.len();
    let entry =
        append_object_to_container(&layout, ObjectType::Blob, &blob_envelope("after", 300))?;
    assert_eq!(
        entry.offset, full_length,
        "the offset is the whole file's length, the 7 torn bytes included"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Run `work` on a thread and wait at most `limit`: a regression that made an append **block** on a FIFO must fail this control, not
/// hang the suite.
fn within<T: Send + 'static>(
    limit: std::time::Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver.recv_timeout(limit).ok()
}

/// **Control 4 -- refusals unchanged.** A container replaced by a directory, by a symlink (to a real file, which must not be appended
/// to) and by a FIFO (which must never be blocked on) is refused, and a missing container is refused; each leaves the index empty.
/// **Perturb:** follow the final component (drop `OFlags::NOFOLLOW` from `open_existing_regular`): the symlink case goes red.
#[test]
fn an_append_to_a_container_that_is_not_a_regular_file_or_is_missing_is_refused() -> Result<()> {
    let attempt =
        |setup: &dyn Fn(&std::path::Path, &std::path::Path) -> Result<()>| -> Result<(bool, u64)> {
            let root = crate::test_gates::test_support::unique_temp_dir("index-append-refusals");
            let layout = RepositoryLayout::init(root.clone())?;
            let container = layout.container_slot_path(ObjectType::Blob, ContainerSlot::A);
            setup(&container, &root)?;
            let result = within(std::time::Duration::from_secs(10), {
                let layout = layout.clone();
                move || {
                    append_object_to_container(
                        &layout,
                        ObjectType::Blob,
                        &blob_envelope("refused", 300),
                    )
                    .is_err()
                }
            });
            let index_len = std::fs::metadata(layout.container_index_path())?.len();
            let _ = std::fs::remove_dir_all(&root);
            Ok((
                result.expect("the append returned (it did not block)"),
                index_len,
            ))
        };
    // A missing container.
    let (refused, index_len) = attempt(&|container, _| Ok(std::fs::remove_file(container)?))?;
    assert!(refused, "a missing container is refused");
    assert_eq!(index_len, 0, "and nothing is indexed");
    // A directory.
    let (refused, index_len) = attempt(&|container, _| {
        std::fs::remove_file(container)?;
        Ok(std::fs::create_dir(container)?)
    })?;
    assert!(refused, "a directory is refused");
    assert_eq!(index_len, 0);
    #[cfg(unix)]
    {
        // A symlink to a real file: refused, and the target is not appended to.
        let target = std::env::temp_dir().join(format!(
            "prikk-append-symlink-target-{}",
            std::process::id()
        ));
        std::fs::write(&target, b"target bytes")?;
        let (refused, index_len) = attempt(&|container, _| {
            std::fs::remove_file(container)?;
            Ok(std::os::unix::fs::symlink(&target, container)?)
        })?;
        assert!(refused, "a symlink at the final component is not followed");
        assert_eq!(index_len, 0);
        assert_eq!(
            std::fs::read(&target)?,
            b"target bytes",
            "the symlink's target was not appended to"
        );
        let _ = std::fs::remove_file(&target);
        // A FIFO: refused, never blocked on.
        let (refused, index_len) = attempt(&|container, _| {
            std::fs::remove_file(container)?;
            let status = std::process::Command::new("mkfifo")
                .arg(container)
                .status()?;
            assert!(status.success(), "mkfifo");
            Ok(())
        })?;
        assert!(refused, "a FIFO is refused");
        assert_eq!(index_len, 0);
    }
    Ok(())
}
