//! Object store and layout tests.

// DC-98 Stage 1: `mod immutable;` and `mod races;` (G5's own conformance and race tests) removed
// along with `DurabilityContract::publish_immutable` -- zero production callers, and G5 retired as
// a guarantee. DC-97 had deferred splitting `races.rs`'s helper rather than deciding it under time
// pressure; the answer turned out to be that the file goes away entirely, resolving that question
// by deletion.

use prikk_object::{ObjectEnvelope, ObjectType};

use crate::{
    FileObjectStore, MemoryObjectStore, ObjectReadSnapshot, ObjectReader, ObjectWriteSession,
    ObjectWriter, RepositoryLayout,
};

use crate::test_gates::test_support::{dummy_signature, unique_temp_dir};

#[test]
fn memory_store_roundtrips_object() {
    let mut store = MemoryObjectStore::new();
    let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"payload".to_vec());
    let id = store.write_object(&envelope);
    assert!(id.is_ok());
    if let Ok(id) = id {
        let read = store.read_object(id);
        assert_eq!(read, Ok(Some(envelope)));
    }
}

#[test]
fn repository_init_creates_required_directories() {
    let root = unique_temp_dir("layout");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        for dir in layout.required_directories() {
            assert!(dir.is_dir(), "missing directory: {}", dir.display());
        }
        assert!(layout.default_queue_wal_path().parent().is_some());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn file_store_roundtrips_signed_object() {
    let root = unique_temp_dir("filestore");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"payload".to_vec());
        assert!(envelope.add_signature(dummy_signature()).is_ok());
        let mut store = FileObjectStore::new(layout);
        let id = store.write_object(&envelope);
        assert!(id.is_ok());
        if let Ok(id) = id {
            let read = store.read_object(id);
            assert_eq!(read, Ok(Some(envelope)));
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn object_reads_and_writes_remain_on_retained_repository_root() -> prikk_error::Result<()> {
    let root = unique_temp_dir("object-root-replacement");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut first = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"first".to_vec());
    first.add_signature(dummy_signature())?;
    let mut store = FileObjectStore::new(layout.clone());
    let first_id = store.write_object(&first)?;
    let displaced = root.join(".prikk-displaced");
    std::fs::rename(layout.prikk_dir(), &displaced)?;
    std::fs::create_dir(root.join(".prikk"))?;
    std::fs::write(root.join(".prikk/FORMAT"), b"replacement")?;

    assert!(layout.validate_format().is_ok());
    assert_eq!(store.read_object(first_id)?, Some(first));
    let mut second = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"second".to_vec());
    second.add_signature(dummy_signature())?;
    let second_id = store.write_object(&second)?;
    assert!(store.read_object(second_id)?.is_some());
    assert_eq!(std::fs::read(root.join(".prikk/FORMAT"))?, b"replacement");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// --- RFC 111 §6.1: ObjectReadSnapshot / ObjectWriteSession -------------------------------------

#[test]
fn read_snapshot_roundtrips_an_object_written_before_it_was_opened() -> prikk_error::Result<()> {
    let root = unique_temp_dir("read-snapshot-roundtrip");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"before".to_vec());
    envelope.add_signature(dummy_signature())?;
    let mut store = FileObjectStore::new(layout.clone());
    let id = store.write_object(&envelope)?;

    let snapshot = ObjectReadSnapshot::open(&layout)?;
    assert_eq!(snapshot.read_object(id)?, Some(envelope.clone()));
    assert!(snapshot.contains_object(ObjectType::Blob, id));
    assert_eq!(snapshot.read_typed(id, ObjectType::Blob)?, Some(envelope));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn read_snapshot_does_not_see_an_object_written_after_it_was_opened() -> prikk_error::Result<()> {
    let root = unique_temp_dir("read-snapshot-staleness");
    let layout = RepositoryLayout::init(root.clone())?;
    let snapshot = ObjectReadSnapshot::open(&layout)?;

    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"after".to_vec());
    envelope.add_signature(dummy_signature())?;
    let mut store = FileObjectStore::new(layout.clone());
    let id = store.write_object(&envelope)?;

    // RFC 111 Q3/Q4: a reader's staleness under-reports, it never returns wrong content -- the
    // snapshot taken before this write simply does not know about it.
    assert_eq!(snapshot.read_object(id)?, None);
    assert!(!snapshot.contains_object(ObjectType::Blob, id));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn write_session_rewriting_the_same_object_is_a_no_op() -> prikk_error::Result<()> {
    let root = unique_temp_dir("write-session-idempotent");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"stable".to_vec());
    envelope.add_signature(dummy_signature())?;

    let mut session = ObjectWriteSession::open(&layout)?;
    let first_id = session.write_object(&envelope)?;
    let length_after_first = session.known_index_length_for_test();
    let count_after_first = session.entry_count_for_test();

    let second_id = session.write_object(&envelope)?;
    assert_eq!(first_id, second_id);
    assert_eq!(
        session.known_index_length_for_test(),
        length_after_first,
        "a same-id-same-bytes rewrite must not grow the index"
    );
    assert_eq!(session.entry_count_for_test(), count_after_first);

    let on_disk_length = std::fs::metadata(layout.container_index_path())?.len();
    assert_eq!(session.known_index_length_for_test(), on_disk_length);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn write_session_rejects_a_same_id_rewrite_with_different_bytes() -> prikk_error::Result<()> {
    let root = unique_temp_dir("write-session-conflict");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut first = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"conflict".to_vec());
    first.add_signature(dummy_signature())?;

    let mut session = ObjectWriteSession::open(&layout)?;
    session.write_object(&first)?;

    let mut second = first.clone();
    second.signatures.clear();
    let mut signature = dummy_signature();
    if let Some(byte) = signature.signature_bytes.get_mut(0) {
        *byte ^= 0x01;
    }
    second.add_signature(signature)?;
    assert_eq!(second.object_id(), first.object_id());

    let mut file_store = FileObjectStore::new(layout.clone());
    let Err(file_store_error) = file_store.write_object(&second) else {
        panic!("expected FileObjectStore to reject a same-id-different-bytes rewrite");
    };
    let Err(session_error) = session.write_object(&second) else {
        panic!("expected ObjectWriteSession to reject a same-id-different-bytes rewrite");
    };
    assert_eq!(
        session_error.to_string(),
        file_store_error.to_string(),
        "the session must report the same conflict FileObjectStore does"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn write_session_appending_a_new_object_updates_the_snapshot() -> prikk_error::Result<()> {
    let root = unique_temp_dir("write-session-new-object");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"new".to_vec());
    envelope.add_signature(dummy_signature())?;

    let mut session = ObjectWriteSession::open(&layout)?;
    assert_eq!(session.entry_count_for_test(), 0);
    let id = session.write_object(&envelope)?;
    assert_eq!(session.entry_count_for_test(), 1);
    assert_eq!(
        session.known_index_length_for_test(),
        std::fs::metadata(layout.container_index_path())?.len()
    );
    assert_eq!(session.read_object(id)?, Some(envelope));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 111 §6.1 addendum, C1's own scenario made executable: a writer *not* routed through the
/// session (exactly `refs/publication.rs`'s current shape) writes while a session is open. The
/// session's next write decision -- for a *different* object -- must see the nested write's entry via
/// `ensure_current`'s tail-decode, not duplicate it, and its own view of the index extent must land on
/// the true current file length, not a value it accumulated itself.
#[test]
fn write_session_catches_up_after_a_nested_unmediated_writer() -> prikk_error::Result<()> {
    let root = unique_temp_dir("write-session-nested-writer");
    let layout = RepositoryLayout::init(root.clone())?;

    let mut session = ObjectWriteSession::open(&layout)?;
    let mut own = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"session-own".to_vec());
    own.add_signature(dummy_signature())?;
    session.write_object(&own)?;

    // A separate, unmediated writer -- exactly `refs/publication.rs`'s current shape -- appends while
    // the session above is open and does not know about it.
    let mut nested = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"nested-writer".to_vec());
    nested.add_signature(dummy_signature())?;
    let mut nested_store = FileObjectStore::new(layout.clone());
    let nested_id = nested_store.write_object(&nested)?;

    // The session's next decision, for yet another new object, must first catch up.
    let mut third = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"session-third".to_vec());
    third.add_signature(dummy_signature())?;
    session.write_object(&third)?;

    assert_eq!(
        session.entry_count_for_test(),
        3,
        "the session must see the nested writer's entry, not just its own two"
    );
    assert!(
        session.read_object(nested_id)?.is_some(),
        "the session must be able to read the object it never wrote itself"
    );
    let on_disk_length = std::fs::metadata(layout.container_index_path())?.len();
    assert_eq!(
        session.known_index_length_for_test(),
        on_disk_length,
        "known_length must land on the true current extent, not the session's own running total"
    );

    // Ground truth: exactly one index entry per object id, never a duplicate from the session
    // re-appending something it thought was missing.
    let fresh = ObjectReadSnapshot::open(&layout)?;
    assert_eq!(fresh.read_object(nested_id)?, Some(nested));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 102, ruled 2026-09-12: `append_object_under_lock` must stay the **only** production route to
/// `append_object_to_container`.
///
/// The lock is correct only if nothing bypasses it, and that is a property of the *call graph*, not
/// of any one function — no assertion inside the append can observe it. So this reads the source: in
/// production code, the name may appear only where it is defined, where it is imported, and inside
/// the wrapper. A twelfth caller added anywhere else reopens the exact defect this round closed, and
/// would otherwise pass every other test in the suite.
#[test]
fn every_object_append_goes_through_the_locked_wrapper() {
    let crate_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut wrapper_calls = 0_usize;

    let mut files_scanned = 0_usize;
    let mut stack = vec![crate_src.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // Test code legitimately drives the raw append -- `foundation/index/tests.rs` exists to
            // exercise the write protocol itself, below the lock.
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name == "tests.rs" || path.components().any(|c| c.as_os_str() == "tests") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            files_scanned += 1;
            for (number, line) in text.lines().enumerate() {
                if !line.contains("append_object_to_container") {
                    continue;
                }
                let relative = path.strip_prefix(&crate_src).unwrap_or(&path);
                let is_definition = line.contains("pub(crate) fn append_object_to_container");
                let is_import = line.trim_start().starts_with("self,")
                    || line.contains("use crate::foundation::index");
                let is_comment =
                    line.trim_start().starts_with("//") || line.trim_start().starts_with("///");
                let in_wrapper = relative == std::path::Path::new("object_store.rs")
                    && line
                        .contains("    append_object_to_container(layout, object_type, envelope)");
                if in_wrapper {
                    wrapper_calls += 1;
                    continue;
                }
                if is_definition || is_import || is_comment {
                    continue;
                }
                offenders.push(format!(
                    "{}:{}: {}",
                    relative.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }

    // Two guards against a vacuous pass: the walk must have read a realistic number of production
    // files, and the wrapper's own call must have been seen. A scan that silently read nothing
    // would otherwise report "no offenders" and look exactly like success.
    assert!(
        files_scanned > 100,
        "expected to scan the whole production tree; scanned only {files_scanned} files"
    );
    assert_eq!(
        wrapper_calls, 1,
        "the wrapper's own call must still be there -- if this is 0 the scan is vacuous"
    );
    assert!(
        offenders.is_empty(),
        "these production sites reach `append_object_to_container` without the object-store lock: {offenders:#?}"
    );
}

/// RFC 102 control 1, ruled 2026-09-12: two writers appending **different** objects to the object
/// store at the same time.
///
/// The rendezvous is in the test, immediately before `write_object`, and deliberately **not** inside
/// the exclusive region: the pre-fix version of this race was demonstrated with a barrier at the
/// `AppendWrite` failpoint (both threads read the same container length, both proceed, both record
/// the same offset), but that barrier cannot survive the fix — the second thread can no longer enter
/// the region, so waiting inside it deadlocks.
///
/// **Both assertions can fail, and fail for different reasons.** Remove the lock from
/// `append_object_under_lock` and the conflict count drops to zero *deterministically*, whatever the
/// thread timing does; the offset assertion is the correctness property itself, and is the one that
/// caught the real defect — twelve concurrent `prikk tag create` left two index entries claiming one
/// offset and `prikk verify` failing, in two runs of four.
#[test]
fn two_racing_object_appends_serialise_and_never_share_an_offset() -> prikk_error::Result<()> {
    use std::sync::{Arc, Barrier};

    let root = unique_temp_dir("object-store-race");
    let layout = RepositoryLayout::init(root.clone())?;
    let start = Arc::new(Barrier::new(2));

    let mut handles = Vec::new();
    for tag in [b"racer-one".as_slice(), b"racer-two".as_slice()] {
        let layout = layout.clone();
        let start = Arc::clone(&start);
        handles.push(std::thread::spawn(move || {
            let mut object = ObjectEnvelope::unsigned(ObjectType::Blob, 1, tag.to_vec());
            let signed = object.add_signature(dummy_signature());
            let mut store = FileObjectStore::new(layout);
            // Everything expensive is done; from here the two threads are as close together as this
            // process can put them.
            start.wait();
            signed.and_then(|()| store.write_object(&object))
        }));
    }

    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| match handle.join() {
            Ok(result) => result,
            Err(_) => panic!("a racing writer thread panicked"),
        })
        .collect();

    let conflicts = results
        .iter()
        .filter(|result| matches!(result, Err(prikk_error::PrikkError::LockConflict(_))))
        .count();
    let succeeded = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(
        conflicts + succeeded,
        2,
        "every outcome must be either a success or a lock conflict: {results:?}"
    );
    assert_eq!(
        conflicts, 1,
        "exactly one racer must be refused -- zero means nothing serialised them: {results:?}"
    );

    // The correctness property: whatever the index holds, no two entries may claim one location.
    let index_bytes = std::fs::read(layout.container_index_path())?;
    let replay = crate::foundation::index::decode_index_records(&index_bytes, 0)?;
    assert!(
        !replay.entries.is_empty(),
        "the index must hold the winner's entry -- an empty index would pass the loop below vacuously"
    );
    let mut seen = std::collections::BTreeSet::new();
    for entry in replay.entries {
        assert!(
            seen.insert((entry.object_type, entry.offset)),
            "two index entries claim (type {:?}, offset {}) -- the defect this lock exists to \
             prevent",
            entry.object_type,
            entry.offset
        );
    }

    // And the winner's object is readable back through the ordinary path.
    let store = ObjectReadSnapshot::open(&layout)?;
    for result in results.into_iter().flatten() {
        assert!(
            store.read_object(result)?.is_some(),
            "winner must be readable"
        );
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
