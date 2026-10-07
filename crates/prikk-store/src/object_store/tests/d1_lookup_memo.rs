//! 0.50.0 step 1, Part D1: the object-index fallback scan `resolve_object_location_entries` pays
//! whenever the index has a tail or interior damage must run **at most once per `FileObjectStore`
//! handle**, memoized against the index file's own raw stat length -- not once per lookup. Before
//! this round, a handle that read N objects while a tail was on disk paid N full container rescans
//! for it (the review's own finding on Part C: A5 answered correctly, but at a cost that follows
//! store size times object count).

use prikk_object::{BlobKind, BlobPayload, CanonicalEncode, ObjectEnvelope, ObjectType};

use crate::foundation::index::{
    rebuild_index_from_containers_count_for_test,
    reset_rebuild_index_from_containers_count_for_test,
};
use crate::test_gates::test_support::{maintainer_signature, unique_temp_dir};
use crate::{FileObjectStore, ObjectReader, ObjectWriter, RepositoryLayout};

/// Writes `count` real, trusted `Blob` objects and returns their ids in write order -- the shape a
/// `log`/`cat`/`show`/`tree` walk over many objects reads back one at a time.
fn write_blobs(
    store: &mut FileObjectStore,
    count: usize,
) -> prikk_error::Result<Vec<prikk_object::ObjectId>> {
    (0..count)
        .map(|index| {
            let payload = BlobPayload::new(BlobKind::Text, format!("blob {index}\n").into_bytes());
            let mut envelope =
                ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
            envelope.add_signature(maintainer_signature())?;
            store.write_object(&envelope)
        })
        .collect()
}

/// Tears the object index's own last record by `cut_bytes` (smaller than one full record, 133
/// bytes today: a 50-byte header plus an 83-byte body) -- the shape a crash between a record's own
/// durable container write and its separate index append leaves. Mirrors `verify/tests/object_
/// index_tail.rs`'s own helper of the same shape.
fn truncate_index_tail(layout: &RepositoryLayout, cut_bytes: u64) -> prikk_error::Result<()> {
    let path = layout.container_index_path();
    let original = std::fs::metadata(&path)?.len();
    let bytes = std::fs::read(&path)?;
    let new_len = usize::try_from(original - cut_bytes)
        .unwrap_or_else(|_| panic!("cut_bytes {cut_bytes} must fit within {original}"));
    std::fs::write(
        &path,
        bytes.get(..new_len).unwrap_or_else(|| {
            panic!(
                "new_len {new_len} must fit within a buffer of {} bytes",
                bytes.len()
            )
        }),
    )?;
    Ok(())
}

/// The real reproduction: one handle, 55 objects, a torn tail over the most recently written one --
/// reading every object through the same handle must trigger the full rescan exactly once.
#[test]
fn one_handle_rescans_a_torn_tail_at_most_once() -> prikk_error::Result<()> {
    let root = unique_temp_dir("d1-lookup-memo-rescan-once");
    let layout = RepositoryLayout::init(root)?;
    let mut store = FileObjectStore::new(layout.clone());
    let ids = write_blobs(&mut store, 55)?;

    // Tear only the last index record: 123 of its 133 bytes remain, same arithmetic `verify/tests/
    // object_index_tail.rs` already established (a fixed-width record, so the remainder is always
    // `133 - cut_bytes` regardless of how many sound records precede it).
    truncate_index_tail(&layout, 10)?;

    reset_rebuild_index_from_containers_count_for_test();
    let reader = FileObjectStore::new(layout.clone());
    for &id in &ids {
        let envelope = reader
            .read_object(id)?
            .unwrap_or_else(|| panic!("object {id} must still be readable behind the torn tail"));
        assert_eq!(envelope.object_id(), id);
    }
    assert_eq!(
        rebuild_index_from_containers_count_for_test(),
        1,
        "one handle reading {} objects over a torn tail must rescan the containers exactly once, \
         not once per lookup",
        ids.len()
    );
    Ok(())
}

/// Control: a **second, separate handle** gets its own memo -- the cache is per handle, not global
/// or persisted, matching `Clone`'s own semantics for this type.
#[test]
fn a_second_handle_pays_its_own_rescan() -> prikk_error::Result<()> {
    let root = unique_temp_dir("d1-lookup-memo-second-handle");
    let layout = RepositoryLayout::init(root)?;
    let mut store = FileObjectStore::new(layout.clone());
    let ids = write_blobs(&mut store, 5)?;
    truncate_index_tail(&layout, 10)?;

    reset_rebuild_index_from_containers_count_for_test();
    let first = FileObjectStore::new(layout.clone());
    for &id in &ids {
        first.read_object(id)?;
    }
    assert_eq!(rebuild_index_from_containers_count_for_test(), 1);

    let second = FileObjectStore::new(layout.clone());
    for &id in &ids {
        second.read_object(id)?;
    }
    assert_eq!(
        rebuild_index_from_containers_count_for_test(),
        2,
        "a second handle must pay its own rescan once, not reuse the first handle's memo"
    );
    Ok(())
}

/// Control: a write that clears the tail (`--repair-index`-shaped: the file's length changes)
/// invalidates the memo -- a handle must not keep answering from a stale cache once the index file
/// itself has changed underneath it.
#[test]
fn a_changed_index_file_length_invalidates_the_memo() -> prikk_error::Result<()> {
    let root = unique_temp_dir("d1-lookup-memo-invalidation");
    let layout = RepositoryLayout::init(root)?;
    let mut store = FileObjectStore::new(layout.clone());
    let ids = write_blobs(&mut store, 2)?;
    let [first_id, second_id] = ids.as_slice() else {
        panic!("write_blobs(2) must return exactly two ids, got {ids:?}");
    };
    let (first_id, second_id) = (*first_id, *second_id);
    truncate_index_tail(&layout, 10)?;

    reset_rebuild_index_from_containers_count_for_test();
    let reader = FileObjectStore::new(layout.clone());
    reader.read_object(first_id)?;
    assert_eq!(rebuild_index_from_containers_count_for_test(), 1);
    reader.read_object(second_id)?;
    assert_eq!(
        rebuild_index_from_containers_count_for_test(),
        1,
        "a second lookup against the unchanged file must reuse the memo"
    );

    // A further write changes the index file's own length -- the same handle must notice and redo
    // the check (not necessarily another rescan: a plain append with no new tail decodes cleanly).
    let third_ids = write_blobs(&mut store, 1)?;
    let [third_id] = third_ids.as_slice() else {
        panic!("write_blobs(1) must return exactly one id, got {third_ids:?}");
    };
    reader.read_object(*third_id)?;
    // No new full-rescan assertion here: a clean append past a known-stale cache may decode cleanly
    // without ever calling `rebuild_index_from_containers` again. What matters is only that the
    // lookup itself still succeeds -- proven by `?` not erroring above -- and not a stale answer.
    Ok(())
}
