//! 0.58.0 S4: the object index a hold keeps (`object_store/index_snapshot.rs`).
//!
//! What these hold: nothing is kept outside a hold, and the last release drops it; under a hold the whole file is read once and
//! later opens read only the canary and what was appended; the copy is never stale (another writer's append, a replaced file, a
//! truncate-and-regrow, a damaged tail); a reader's snapshot keeps its point in time; the writer's own append is in the copy
//! without a re-read. **Linux and macOS only**: the identity is `(device, inode)`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, Signature,
    SignatureAlgorithm, SignerRole,
};

use crate::foundation::fsutil::read_tally;
use crate::foundation::index::{INDEX_RECORD_LEN, replay_index};
use crate::lock::acquire_container_locks;
use crate::test_gates::test_support::{maintainer_signature, unique_temp_dir};
use crate::{
    LockableContainer, ObjectReadSnapshot, ObjectWriteSession, ObjectWriter, RepositoryLayout,
    append_superseding_record_for_test_support,
};

const RECORD: u64 = INDEX_RECORD_LEN as u64;

fn blob(text: &str) -> Result<ObjectEnvelope> {
    let payload = BlobPayload::new(BlobKind::Text, text.as_bytes().to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    Ok(envelope)
}

fn write(layout: &RepositoryLayout, text: &str) -> Result<ObjectId> {
    ObjectWriteSession::open(layout)?.write_object(&blob(text)?)
}

fn index_path(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.container_index_path()
}

fn index_relative(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout
        .repository_relative(&layout.container_index_path())
        .expect("relative")
}

fn index_len(layout: &RepositoryLayout) -> u64 {
    std::fs::metadata(index_path(layout)).expect("index").len()
}

/// Bytes of the index read through the anchored reader on this thread since `read_tally::reset`.
fn index_bytes_read(layout: &RepositoryLayout) -> u64 {
    read_tally::bytes_read(&index_relative(layout))
}

/// Another process: the same repository opened afresh has its own `MutationRoot`, and so no hold and no kept index.
fn another_process(layout: &RepositoryLayout) -> RepositoryLayout {
    RepositoryLayout::open(layout.root().to_path_buf()).expect("reopen")
}

/// A hold that takes no object-store lock, as a `sync seal`'s active lock does not: the writer's object-store lock is taken and released
/// inside it, per write.
fn hold(layout: &RepositoryLayout) -> crate::lock::ContainerLockGuard {
    acquire_container_locks(layout, &[LockableContainer::TrustPolicy]).expect("hold")
}

fn contains(layout: &RepositoryLayout, id: ObjectId) -> bool {
    ObjectReadSnapshot::open(layout)
        .expect("open")
        .contains_object(ObjectType::Blob, id)
}

/// Rule 1: nothing is kept outside a hold, and the last release drops it.
#[test]
fn the_kept_index_is_absent_outside_a_hold_and_dropped_with_the_last_release() -> Result<()> {
    let root = unique_temp_dir("s4-absent-outside-a-hold");
    let layout = RepositoryLayout::init(root.clone())?;
    write(&layout, "one")?;
    let mutation = layout.repository_mutation_root();

    let _outside = ObjectReadSnapshot::open(&layout)?;
    assert!(!mutation.kept_is_held_for_test(), "no hold, nothing kept");

    let first = hold(&layout);
    let _inside = ObjectReadSnapshot::open(&layout)?;
    assert!(mutation.kept_is_held_for_test(), "a hold keeps the index");
    let second = acquire_container_locks(&layout, &[LockableContainer::RefLog])?;
    drop(first);
    assert!(mutation.kept_is_held_for_test(), "one hold is still open");
    drop(second);
    assert!(
        !mutation.kept_is_held_for_test(),
        "the last release drops it"
    );
    let _after = ObjectReadSnapshot::open(&layout)?;
    assert!(
        !mutation.kept_is_held_for_test(),
        "and nothing is kept again"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The reads: the whole file once under a hold, then the canary (one record) per open, then the canary and the appended tail; outside a
/// hold every open reads the whole file, as before.
#[test]
fn a_hold_reads_the_whole_index_once_then_the_canary_and_what_was_appended() -> Result<()> {
    let root = unique_temp_dir("s4-reads-once-per-hold");
    let layout = RepositoryLayout::init(root.clone())?;
    for number in 0..20 {
        write(&layout, &format!("blob {number}"))?;
    }
    let whole = index_len(&layout);

    read_tally::reset();
    for _ in 0..3 {
        let _ = ObjectReadSnapshot::open(&layout)?;
    }
    assert_eq!(
        index_bytes_read(&layout),
        3 * whole,
        "outside a hold each open reads the whole index"
    );

    let _hold = hold(&layout);
    read_tally::reset();
    let _first = ObjectReadSnapshot::open(&layout)?;
    assert_eq!(
        index_bytes_read(&layout),
        whole,
        "the first open reads it whole"
    );
    read_tally::reset();
    for _ in 0..5 {
        let _ = ObjectReadSnapshot::open(&layout)?;
    }
    assert_eq!(
        index_bytes_read(&layout),
        5 * RECORD,
        "every later open reads the canary, one record"
    );

    let appended = write(&layout, "appended")?;
    read_tally::reset();
    let after = ObjectReadSnapshot::open(&layout)?;
    assert!(after.contains_object(ObjectType::Blob, appended));
    assert!(
        index_bytes_read(&layout) <= RECORD,
        "the writer's own append is already in the kept copy: {} bytes read",
        index_bytes_read(&layout)
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 2: another writer appends between two of this command's lookups, and the lookup sees the new entry (a reader's open, and a
/// writer's decision).
#[test]
fn another_writers_append_between_two_lookups_is_seen() -> Result<()> {
    let root = unique_temp_dir("s4-concurrent-append");
    let layout = RepositoryLayout::init(root.clone())?;
    write(&layout, "before")?;
    let other = another_process(&layout);

    let _hold = hold(&layout);
    let _first = ObjectReadSnapshot::open(&layout)?;
    let mut session = ObjectWriteSession::open(&layout)?;

    let appended = write(&other, "appended by another process")?;
    assert!(
        contains(&layout, appended),
        "the next open reads the appended tail"
    );
    assert!(
        session.contains_object(ObjectType::Blob, appended)?,
        "and an open writer session catches up before it decides"
    );
    // The appended entry is read from the tail, not found by a re-read of the whole file.
    read_tally::reset();
    let _ = ObjectReadSnapshot::open(&layout)?;
    assert!(index_bytes_read(&layout) <= RECORD);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 3, interior damage: damage appears in the tail between two lookups. A reader finds the object by the container scan, as a fresh
/// replay does, and keeps nothing from it. A writer session does what a fresh one does on a damaged index (reads by the scan, appends).
#[test]
fn damage_in_the_tail_between_two_lookups_takes_the_fresh_replay_path() -> Result<()> {
    let root = unique_temp_dir("s4-damaged-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    write(&layout, "before")?;
    let other = another_process(&layout);
    let mutation = layout.repository_mutation_root();

    let _hold = hold(&layout);
    let _first = ObjectReadSnapshot::open(&layout)?;
    assert!(mutation.kept_is_held_for_test());
    let mut session = ObjectWriteSession::open(&layout)?;

    let appended = write(&other, "appended, then damaged")?;
    let mut bytes = std::fs::read(index_path(&layout))?;
    let flip = bytes.len() - 10;
    bytes[flip] ^= 0xff;
    std::fs::write(index_path(&layout), &bytes)?;
    assert!(replay_index(&layout)?.has_item_failure(), "fixture sanity");

    assert!(
        contains(&layout, appended),
        "a reader still finds the object: the containers are sound"
    );
    assert!(
        !mutation.kept_is_held_for_test(),
        "and nothing damaged is kept"
    );
    assert!(
        replay_index(&layout)?.has_item_failure(),
        "a reader repairs nothing on disk"
    );
    assert!(
        session.contains_object(ObjectType::Blob, appended)?,
        "a writer session finds it too"
    );
    let next = session.write_object(&blob("written after the damage")?)?;
    assert!(contains(&layout, appended) && contains(&layout, next));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 3, a torn tail (the last record cut short): a reader finds the object by the scan and keeps nothing; a writer repairs the index on
/// disk under its lock before it appends, so the append does not bury the torn bytes.
#[test]
fn a_torn_tail_between_two_lookups_is_repaired_by_a_writer_and_read_by_a_reader() -> Result<()> {
    let root = unique_temp_dir("s4-torn-tail");
    let layout = RepositoryLayout::init(root.clone())?;
    write(&layout, "before")?;
    let other = another_process(&layout);
    let mutation = layout.repository_mutation_root();

    let _hold = hold(&layout);
    let _first = ObjectReadSnapshot::open(&layout)?;
    let mut session = ObjectWriteSession::open(&layout)?;

    let appended = write(&other, "appended, then torn")?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(index_path(&layout))?;
    file.set_len(index_len(&layout) - 20)?;
    drop(file);
    assert_ne!(
        replay_index(&layout)?.trailing_partial_bytes,
        0,
        "fixture sanity"
    );

    assert!(contains(&layout, appended), "a reader finds it by the scan");
    assert!(!mutation.kept_is_held_for_test(), "and keeps nothing");

    let next = session.write_object(&blob("written after the tear")?)?;
    let replay = replay_index(&layout)?;
    assert_eq!(
        replay.trailing_partial_bytes, 0,
        "the writer repaired the tail first"
    );
    assert!(!replay.has_item_failure());
    assert!(contains(&layout, appended) && contains(&layout, next));
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The index swapped for another file of the **same length** (a permutation, as `repair_index_from_containers` installs by rename): a hold
/// reads it whole and answers from the new file. Index: `[A1, C, A2]` becomes `[A2, C, A1]`; the last record for `A` was `A2`'s and is
/// now `A1`'s.
fn index_with_a_superseded_id(
    name: &str,
) -> Result<(RepositoryLayout, std::path::PathBuf, ObjectId, u64, u64)> {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone())?;
    let first = blob("superseded")?;
    let id = ObjectWriteSession::open(&layout)?.write_object(&first)?;
    write(&layout, "bystander")?;
    let mut second = first.clone();
    second.add_signature(Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "maintainer-key-2".to_string(),
        signature_bytes: vec![6; 64],
        created_at: 9,
        signer_role: SignerRole::Maintainer,
    })?;
    append_superseding_record_for_test_support(&layout, &second)?;
    let entries = replay_index(&layout)?.entries;
    assert_eq!(entries.len(), 3, "fixture sanity: [A1, C, A2]");
    let (first_offset, second_offset) = (entries[0].offset, entries[2].offset);
    assert_ne!(first_offset, second_offset);
    Ok((layout, root, id, first_offset, second_offset))
}

/// Replace the index by a file with its first and last records swapped: the same length, another inode.
fn swap_first_and_last_records(layout: &RepositoryLayout) -> Result<()> {
    let path = index_path(layout);
    let bytes = std::fs::read(&path)?;
    let record = INDEX_RECORD_LEN;
    let mut swapped = bytes.clone();
    swapped[..record].copy_from_slice(&bytes[bytes.len() - record..]);
    let tail_start = bytes.len() - record;
    swapped[tail_start..].copy_from_slice(&bytes[..record]);
    let temporary = path.with_extension("swap");
    std::fs::write(&temporary, &swapped)?;
    std::fs::rename(&temporary, &path)?;
    assert_eq!(std::fs::metadata(&path)?.len() as usize, bytes.len());
    Ok(())
}

#[test]
fn a_replaced_index_of_the_same_length_is_read_whole() -> Result<()> {
    let (layout, root, id, first_offset, second_offset) =
        index_with_a_superseded_id("s4-replaced-same-length")?;
    let _hold = hold(&layout);
    let before = ObjectReadSnapshot::open(&layout)?;
    assert_eq!(
        before.snapshot.lookup(id).map(|entry| entry.offset),
        Some(second_offset)
    );

    swap_first_and_last_records(&layout)?;
    read_tally::reset();
    let after = ObjectReadSnapshot::open(&layout)?;
    assert_eq!(
        after.snapshot.lookup(id).map(|entry| entry.offset),
        Some(first_offset),
        "the copy of the old file is not used for the new one"
    );
    assert_eq!(
        index_bytes_read(&layout),
        index_len(&layout),
        "a different inode means a full re-read"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Item 5 of the S4a rulings: today's `ensure_current` compared the **length only**, so an open writer session did not see an index
/// rebuilt to the same length (an equal-length permutation). Its answer for a repeated id was the old file's.
#[test]
fn an_open_session_sees_an_index_replaced_by_another_file_of_the_same_length() -> Result<()> {
    let (layout, root, id, first_offset, second_offset) =
        index_with_a_superseded_id("s4-session-replaced-same-length")?;
    let mut session = ObjectWriteSession::open(&layout)?;
    assert_eq!(
        session.snapshot.lookup(id).map(|entry| entry.offset),
        Some(second_offset)
    );
    swap_first_and_last_records(&layout)?;
    session.snapshot.ensure_current(&layout)?;
    assert_eq!(
        session.snapshot.lookup(id).map(|entry| entry.offset),
        Some(first_offset)
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The canary: the index is truncated in place below the copy's extent and grows past it again with other records. Same inode, longer than
/// before -- only the record at the copy's end tells.
#[test]
fn a_truncate_and_regrow_past_the_copy_is_read_whole() -> Result<()> {
    let root = unique_temp_dir("s4-truncate-regrow");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut cut = Vec::new();
    for number in 0..6 {
        cut.push(write(&layout, &format!("kept then cut {number}"))?);
    }
    let other = another_process(&layout);
    let _hold = hold(&layout);
    let _first = ObjectReadSnapshot::open(&layout)?;
    let known = index_len(&layout);

    // Cut the last two records, then append three others: the file is one record longer than the copy decoded.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(index_path(&layout))?;
    file.set_len(known - 2 * RECORD)?;
    drop(file);
    let mut fresh = Vec::new();
    for number in 0..3 {
        fresh.push(write(&other, &format!("after the cut {number}"))?);
    }
    assert_eq!(index_len(&layout), known + RECORD, "fixture sanity");

    read_tally::reset();
    let after = ObjectReadSnapshot::open(&layout)?;
    assert_eq!(
        index_bytes_read(&layout),
        index_len(&layout) + 2 * RECORD,
        "the canary does not match (the attempt read the canary and the tail): the file is then read whole"
    );
    for id in &fresh {
        assert!(after.contains_object(ObjectType::Blob, *id));
    }
    for id in &cut[..4] {
        assert!(after.contains_object(ObjectType::Blob, *id));
    }
    for id in &cut[4..] {
        assert!(
            !after.contains_object(ObjectType::Blob, *id),
            "an entry the file no longer holds is not answered from the old copy"
        );
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Sharing: a reader's snapshot keeps its point in time while the kept copy moves on (the extension copies the data instead of changing
/// what the reader holds).
#[test]
fn a_reader_snapshot_keeps_its_point_in_time_while_the_kept_copy_moves_on() -> Result<()> {
    let root = unique_temp_dir("s4-point-in-time");
    let layout = RepositoryLayout::init(root.clone())?;
    write(&layout, "before")?;
    let _hold = hold(&layout);
    let reader = ObjectReadSnapshot::open(&layout)?;
    let appended = write(&layout, "after the reader opened")?;
    assert!(
        !reader.contains_object(ObjectType::Blob, appended),
        "the reader's snapshot is as of its open"
    );
    assert!(contains(&layout, appended), "a new open sees it");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A missing index file reads as an empty index with no identity: nothing is kept for it.
#[test]
fn a_missing_index_is_not_kept() -> Result<()> {
    let root = unique_temp_dir("s4-missing-index");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::remove_file(index_path(&layout))?;
    let _hold = hold(&layout);
    let snapshot = ObjectReadSnapshot::open(&layout)?;
    assert!(!snapshot.contains_object(ObjectType::Blob, ObjectId::from_bytes([1; 32])));
    assert!(!layout.repository_mutation_root().kept_is_held_for_test());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 4: memory. The kept copy holds the entries and the id map; one replay holds those **and** the file's bytes and an outcome per record
/// while it decodes. So the kept copy is less per object than one replay's peak, and a re-read drops the copy before it decodes (the
/// copy is taken out of the slot, then replaced) -- it never adds to a replay's peak. Counted from the types' sizes and the containers'
/// capacities (the workspace forbids the `unsafe` a counting allocator needs); the numbers print with `--nocapture`.
#[test]
fn the_kept_copy_costs_less_per_object_than_one_replay() -> Result<()> {
    use crate::foundation::index::{IndexEntry, IndexRecordOutcome};
    let root = unique_temp_dir("s4-memory");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut session = ObjectWriteSession::open(&layout)?;
    for number in 0..507 {
        session.write_object(&blob(&format!("object {number}"))?)?;
    }
    let objects = replay_index(&layout)?.entries.len();
    let snapshot = ObjectReadSnapshot::open(&layout)?;
    let kept = snapshot.snapshot.data_heap_bytes_for_test();
    let replay_extra = objects * (INDEX_RECORD_LEN + std::mem::size_of::<IndexRecordOutcome>());
    let replay_peak = kept + replay_extra;
    println!(
        "objects {objects}: kept copy {kept} B ({} B/object; IndexEntry {} B), one replay's peak {replay_peak} B ({} B/object)",
        kept / objects,
        std::mem::size_of::<IndexEntry>(),
        replay_peak / objects
    );
    assert!(kept < replay_peak);
    assert!(
        kept * 2 > replay_peak / 2,
        "fixture sanity: same order of size"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
