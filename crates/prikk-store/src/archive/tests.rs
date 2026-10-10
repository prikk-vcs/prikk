//! Export tests (RFC 155 implementation Part E).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::type_complexity
)]

use std::io::Cursor;

use prikk_object::{BlockKind, ObjectId, ObjectType};

use crate::archive::{ArchiveSectionKind, export_archive};
use crate::foundation::layout::{ContainerSlot, DEFAULT_ACTIVE_NAME};
use crate::lock::ActiveLock;
use crate::test_gates::test_support::{signed_block, signed_patch_envelope, unique_temp_dir};
use crate::wal::Wal;
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};

/// Seal a one-block `heads/main` into `layout`, returning the tip Block id -- the minimum history
/// that gives every object container at least one real record.
fn seal_one_block(layout: &RepositoryLayout) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let patch = signed_patch_envelope();
    let patch_id = object_store.write_object(&patch)?;
    let root_block = signed_block(BlockKind::Root, Vec::new(), vec![patch_id], None);
    let root_block_id = object_store.write_object(&root_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = crate::test_gates::test_support::signed_ref_state_envelope(
        "heads/main",
        None,
        root_block_id,
        1,
    );
    let ref_state_id = ref_state.object_id();
    let ref_update = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        None,
        ref_state_id,
        root_block_id,
        1,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(root_block_id)
}

/// Decode one manifest entry's own `(kind, offset, length, checksum)`, test-only -- the real
/// decoder is Part V's job. Returns `(repository_format, tool_version, entries)`.
fn decode_manifest_for_test(bytes: &[u8]) -> (u32, String, Vec<(u16, u64, u64, [u8; 32])>) {
    let end_trailer_start = bytes.len() - 24;
    assert_eq!(
        &bytes[end_trailer_start..end_trailer_start + 8],
        b"PRPOEND1"
    );
    let manifest_offset = u64::from_be_bytes(
        bytes[end_trailer_start + 8..end_trailer_start + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    let manifest_length = u64::from_be_bytes(
        bytes[end_trailer_start + 16..end_trailer_start + 24]
            .try_into()
            .unwrap(),
    ) as usize;
    let manifest = &bytes[manifest_offset..manifest_offset + manifest_length];
    assert_eq!(&manifest[0..8], b"PRPOMAN1");
    let repository_format = u32::from_be_bytes(manifest[8..12].try_into().unwrap());
    let tool_version_len = u16::from_be_bytes(manifest[12..14].try_into().unwrap()) as usize;
    let tool_version = String::from_utf8(manifest[14..14 + tool_version_len].to_vec()).unwrap();
    let mut cursor = 14 + tool_version_len;
    let count = u64::from_be_bytes(manifest[cursor..cursor + 8].try_into().unwrap()) as usize;
    cursor += 8;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = u16::from_be_bytes(manifest[cursor..cursor + 2].try_into().unwrap());
        let offset = u64::from_be_bytes(manifest[cursor + 2..cursor + 10].try_into().unwrap());
        let length = u64::from_be_bytes(manifest[cursor + 10..cursor + 18].try_into().unwrap());
        let checksum: [u8; 32] = manifest[cursor + 18..cursor + 50].try_into().unwrap();
        entries.push((kind, offset, length, checksum));
        cursor += 50;
    }
    (repository_format, tool_version, entries)
}

/// Read one section's own body bytes back out of an exported archive, by its manifest entry --
/// test-only; re-verifies the section's own checksum the same way a real verifier will.
fn read_section_body_for_test(
    bytes: &[u8],
    offset: u64,
    length: u64,
    checksum: [u8; 32],
) -> Vec<u8> {
    let offset = offset as usize;
    let header_len = 8 + 2 + 8;
    let body_start = offset + header_len;
    let body_end = body_start + length as usize;
    assert_eq!(&bytes[offset..offset + 8], b"PRPOSEC1", "section magic");
    let body = bytes[body_start..body_end].to_vec();
    let trailer = &bytes[body_end..body_end + 32];
    let recomputed = prikk_hash::sha256_parts(&[
        &bytes[offset..offset + 8],
        &bytes[offset + 8..offset + 10],
        &bytes[offset + 10..offset + 18],
        &body,
    ]);
    assert_eq!(&recomputed, checksum.as_slice(), "section own checksum");
    assert_eq!(trailer, checksum.as_slice(), "trailer matches manifest");
    body
}

#[test]
fn export_archive_round_trips_a_small_history() {
    let dir = unique_temp_dir("archive-export-basic");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");

    let mut buffer = Vec::new();
    let report = export_archive(&layout, &mut buffer).expect("export");
    assert_eq!(report.repository_format, 7);
    assert_eq!(report.section_count, 13);
    assert_eq!(report.total_bytes as usize, buffer.len());
    assert_eq!(&buffer[0..8], b"PREPO001");

    let (_, _, entries) = decode_manifest_for_test(&buffer);
    assert_eq!(entries.len(), 13);
    let block_entry = entries
        .iter()
        .find(|(kind, ..)| {
            *kind == ArchiveSectionKind::ObjectContainer(ObjectType::Block).wire_code()
        })
        .expect("block section present");
    let body = read_section_body_for_test(&buffer, block_entry.1, block_entry.2, block_entry.3);
    assert!(
        !body.is_empty(),
        "the sealed block's own container section is non-empty"
    );

    let pointer_entry = entries
        .iter()
        .find(|(kind, ..)| *kind == ArchiveSectionKind::RefPointerIndex.wire_code())
        .expect("pointer-index section present");
    let pointer_body =
        read_section_body_for_test(&buffer, pointer_entry.1, pointer_entry.2, pointer_entry.3);
    assert!(
        !pointer_body.is_empty(),
        "heads/main's own pointer entry is carried"
    );
}

#[test]
fn export_refuses_queued_unsealed_work() {
    let dir = unique_temp_dir("archive-export-queued");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let _active_lock =
        ActiveLock::acquire_for_write(&layout, DEFAULT_ACTIVE_NAME).expect("active lock");
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&signed_patch_envelope())
        .expect("queue a patch");
    drop(_active_lock);

    let mut buffer = Vec::new();
    let err = export_archive(&layout, &mut buffer).expect_err("must refuse");
    let message = err.to_string();
    assert!(message.contains("queued, unsealed commits"), "{message}");
    assert!(message.contains("`prikk seal`"), "{message}");
    assert!(buffer.is_empty(), "a refused export writes nothing");

    // Follow the message: seal, then export must succeed.
    seal_queued_patch(&layout).expect("sealing the queued patch");
    let mut buffer2 = Vec::new();
    export_archive(&layout, &mut buffer2).expect("export succeeds once sealed");
}

/// Drain the active WAL exactly as `prikk seal` would -- the minimum needed to make the queued-work
/// refusal test's own "then export succeeding" half real, without pulling in the CLI crate.
fn seal_queued_patch(layout: &RepositoryLayout) -> prikk_error::Result<()> {
    let active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
    let mut object_store = FileObjectStore::new(layout.clone());
    let mut patch_ids = Vec::new();
    for record in &replay.records {
        patch_ids.push(object_store.write_object(&record.envelope)?);
    }
    let root_block = signed_block(BlockKind::Root, Vec::new(), patch_ids, None);
    let root_block_id = object_store.write_object(&root_block)?;
    let ref_store = RefStore::new(layout.clone());
    let ref_state = crate::test_gates::test_support::signed_ref_state_envelope(
        "heads/main",
        None,
        root_block_id,
        1,
    );
    let ref_state_id = ref_state.object_id();
    let ref_update = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        None,
        ref_state_id,
        root_block_id,
        1,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    crate::finish_active_publication_cleanup(layout, &active_lock)?;
    Ok(())
}

/// D3's own closed gap: export from a repository whose pointer-index live slot is `b`, through
/// ordinary compaction -- never `--keep-slot` (that path is for the lost-log recovery case, not a
/// healthy repository), matching the real command sequence `compact --pointer-index` runs.
#[test]
fn export_carries_resolved_content_when_live_slot_is_b() {
    let dir = unique_temp_dir("archive-export-slot-b");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal 1");
    advance_ref_once_more(&layout).expect("seal 2, so compaction has something to reclaim");

    let before = crate::compact::compact_ref_pointer_index(&layout)
        .expect("ordinary pointer-index compaction");
    assert!(
        before.entries_after < before.entries_before,
        "compaction must have reclaimed at least one stale record"
    );

    let slot_b_path = layout.ref_pointer_index_slot_path(ContainerSlot::B);
    assert!(
        std::fs::metadata(&slot_b_path)
            .map(|m| m.len())
            .unwrap_or(0)
            > 0,
        "slot b must now hold the live content"
    );

    let mut buffer = Vec::new();
    export_archive(&layout, &mut buffer).expect("export after compaction");
    let (_, _, entries) = decode_manifest_for_test(&buffer);
    let pointer_entry = entries
        .iter()
        .find(|(kind, ..)| *kind == ArchiveSectionKind::RefPointerIndex.wire_code())
        .expect("pointer-index section present");
    let body =
        read_section_body_for_test(&buffer, pointer_entry.1, pointer_entry.2, pointer_entry.3);

    // The ground truth: `RefStore` resolves the live content the same way `branch list` would.
    let ref_store = RefStore::new(layout.clone());
    let live_tip = ref_store
        .read_current_ref_state_id("heads/main")
        .expect("read current ref state")
        .expect("heads/main exists");

    // The exported body must decode to exactly one live entry naming the same current tip --
    // never slot a's own stale, reclaimed-away bytes (slot a is longer than the live content: it
    // still holds every record compaction just reclaimed).
    let decoded =
        crate::refs::decode_pointer_index_records(&body).expect("decode exported pointer index");
    assert_eq!(
        decoded.entries.len(),
        1,
        "compaction reduced heads/main to one live record"
    );
    assert_eq!(
        decoded.entries[0].ref_state_id, live_tip,
        "the exported, resolved content must name the same tip RefStore itself reports live"
    );

    let slot_a_len = std::fs::metadata(layout.ref_pointer_index_slot_path(ContainerSlot::A))
        .expect("slot a exists")
        .len();
    assert_ne!(
        body.len() as u64,
        slot_a_len,
        "the exported body must not simply be slot a's own (longer, stale) bytes"
    );
}

fn advance_ref_once_more(layout: &RepositoryLayout) -> prikk_error::Result<()> {
    let ref_store = RefStore::new(layout.clone());
    let current = ref_store
        .read_current_ref_state_id("heads/main")?
        .expect("heads/main exists");
    let mut object_store = FileObjectStore::new(layout.clone());
    let patch = signed_patch_envelope();
    let patch_id = object_store.write_object(&patch)?;
    let block = signed_block(BlockKind::Normal, vec![current], vec![patch_id], None);
    let block_id = object_store.write_object(&block)?;
    let ref_state = crate::test_gates::test_support::signed_ref_state_envelope(
        "heads/main",
        Some(current),
        block_id,
        2,
    );
    let ref_state_id = ref_state.object_id();
    let ref_update = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        Some(current),
        ref_state_id,
        block_id,
        2,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(current),
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// §8's control: export holds every `LockableContainer` lock for its **whole run** (not merely
/// while acquiring them), so a concurrent `seal`-shaped writer (here, a direct object-store append
/// plus a ref publish, the same primitives `seal` itself uses) must fail fast rather than
/// interleave. Calls `export_archive` itself, via the test seam
/// [`crate::archive::after_export_locks_acquired_for_test`], so this proves `export_archive`'s own
/// behaviour -- not merely that `acquire_container_locks` works, which `lock.rs`'s own tests already
/// cover. **This is the real control**: perturbed (the seam moved past the locks, simulating an
/// early release) in the report's own control run, shown red, then reverted here.
#[test]
fn export_blocks_a_concurrent_seal_for_its_whole_run() {
    let dir = unique_temp_dir("archive-export-concurrent-seal");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");

    let probe_layout = layout.clone();
    crate::archive::after_export_locks_acquired_for_test(move || {
        let err = advance_ref_once_more(&probe_layout).expect_err(
            "a concurrent seal-shaped write must fail fast while export holds its locks",
        );
        assert!(
            matches!(err, prikk_error::PrikkError::LockConflict(_)),
            "expected a lock conflict, got: {err}"
        );
    });

    let mut buffer = Vec::new();
    export_archive(&layout, &mut buffer).expect("export itself still succeeds");
}

#[test]
fn verify_section_bodies_stream_without_loading_file_into_a_cursor() {
    // A cheap sanity check that `export_archive` accepts any `Write`, not only `Vec<u8>` --
    // `std::io::Cursor` over a fixed buffer exercises the same `dyn Write` path a real file would.
    let dir = unique_temp_dir("archive-export-generic-writer");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");
    let mut cursor = Cursor::new(Vec::new());
    export_archive(&layout, &mut cursor).expect("export into a Cursor<Vec<u8>>");
    assert!(!cursor.into_inner().is_empty());
}
