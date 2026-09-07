use prikk_object::{ObjectEnvelope, ObjectType};

use super::admission::strict_rejection_variants;
use crate::foundation::layout::{ContainerSlot, DEFAULT_ACTIVE_NAME};
use crate::refs::encode_log_record_for_test;
use crate::test_gates::test_support::{
    signed_patch_blob_envelope, signed_patch_envelope, signed_ref_update_envelope, unique_temp_dir,
};
use crate::wal::encode_record_for_test;
use crate::{
    FileObjectStore, ObjectReader, RefStore, RepositoryLayout, Wal, WalRecord,
    derive_next_state_root, verify_repository,
};

/// Plants a structurally invalid envelope directly at its container-and-index location,
/// bypassing `FileObjectStore::write_object`'s own validation (which would refuse it) -- the
/// container-era equivalent of writing raw bytes at a loose object path. `read_object_envelope_at`
/// does not consult `container_checksum` (it recomputes the checksum from the frame itself), so a
/// placeholder value is fine.
fn write_structural_object(
    layout: &RepositoryLayout,
    envelope: &ObjectEnvelope,
) -> prikk_error::Result<prikk_object::ObjectId> {
    let object_id = envelope.object_id();
    let object_type = envelope.object_type;
    let record_bytes =
        crate::foundation::container::encode_container_record_for_test(object_type, envelope)?;
    std::fs::write(
        layout.container_slot_path(object_type, ContainerSlot::A),
        &record_bytes,
    )?;
    let entry = crate::foundation::index::IndexEntry {
        object_id,
        object_type,
        slot: ContainerSlot::A,
        offset: 0,
        length: record_bytes.len() as u64,
        container_checksum: [0_u8; 32],
    };
    std::fs::write(
        layout.container_index_path(),
        crate::foundation::index::encode_index_record(&entry)?,
    )?;
    Ok(object_id)
}

#[test]
fn format2_object_reads_reject_every_strict_envelope_failure() -> prikk_error::Result<()> {
    let valid = signed_patch_blob_envelope();
    for (index, invalid) in strict_rejection_variants(ObjectType::Blob, &valid.canonical_payload)
        .into_iter()
        .enumerate()
    {
        let root = unique_temp_dir(&format!("dc40-strict-object-read-{index}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let object_id = write_structural_object(&layout, &invalid)?;
        let objects = FileObjectStore::new(layout.clone());

        assert!(objects.read_object(object_id).is_err());
        // DC-95 Stage 2 Level 2: a single malformed object is now a Phase A item-level failure, not
        // a whole-`Objects`-stage failure.
        assert!(verify_repository(&layout)?.has_item_failure());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn format2_wal_reads_reject_every_strict_envelope_failure() -> prikk_error::Result<()> {
    let valid = signed_patch_envelope();
    for (index, invalid) in strict_rejection_variants(ObjectType::Patch, &valid.canonical_payload)
        .into_iter()
        .enumerate()
    {
        let root = unique_temp_dir(&format!("dc40-strict-wal-read-{index}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let record = WalRecord {
            seq: 1,
            envelope: invalid,
        };
        std::fs::write(
            layout.default_queue_wal_path(),
            encode_record_for_test(&record)?,
        )?;

        assert!(
            Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
                .replay()
                .is_err()
        );
        assert!(verify_repository(&layout)?.has_stage_failure());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn format2_ref_log_reads_reject_every_strict_envelope_failure() -> prikk_error::Result<()> {
    let valid = signed_ref_update_envelope(
        "heads/main",
        None,
        crate::test_gates::test_support::sample_object_id("state"),
        crate::test_gates::test_support::sample_object_id("target"),
        1,
    );
    for (index, invalid) in
        strict_rejection_variants(ObjectType::RefUpdate, &valid.canonical_payload)
            .into_iter()
            .enumerate()
    {
        let root = unique_temp_dir(&format!("dc40-strict-ref-log-read-{index}"));
        let layout = RepositoryLayout::init(root.clone())?;
        std::fs::write(
            layout.ref_log_container_slot_path(ContainerSlot::A),
            encode_log_record_for_test(&invalid)?,
        )?;

        // RFC 102 Stage 4 checkpoint review, design-v1.md §13.15: isolate-and-continue means a
        // malformed record is an item-level `Failed` outcome within an `Ok` replay, not a hard
        // `Err` from `replay_log` itself -- the same item-containment discipline the next assertion
        // already documents for `verify_repository`, now also true one layer down.
        assert!(
            RefStore::new(layout.clone())
                .replay_log("heads/main")?
                .has_item_failure()
        );
        // DC-95 Stage 2 Level 2: a single malformed ref-log record is now an item-level failure
        // (this ref's own log read), not a whole-`Refs`-stage failure.
        assert!(verify_repository(&layout)?.has_item_failure());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn format2_authoritative_replay_rejects_every_strict_patch_failure() -> prikk_error::Result<()> {
    let valid = signed_patch_envelope();
    for (index, invalid) in strict_rejection_variants(ObjectType::Patch, &valid.canonical_payload)
        .into_iter()
        .enumerate()
    {
        let root = unique_temp_dir(&format!("dc40-strict-replay-read-{index}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let patch_id = write_structural_object(&layout, &invalid)?;
        let objects = FileObjectStore::new(layout.clone());

        assert!(derive_next_state_root(&objects, None, &[patch_id]).is_err());
        // DC-95 Stage 2 Level 2: a single malformed Patch object is now a Phase A item-level
        // failure, not a whole-`Objects`-stage failure.
        assert!(verify_repository(&layout)?.has_item_failure());
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}
