use prikk_object::{CanonicalEncode, ObjectType, RefUpdatePayload};

use super::{malformed_envelope, signature};
use crate::refs::append_log_record_for_signature_test;
use crate::test_gates::test_support::{
    sample_object_id, signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    DEFAULT_ACTIVE_NAME, FileObjectStore, MemoryObjectStore, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, Wal,
};

pub(super) fn strict_rejection_variants(
    object_type: ObjectType,
    payload: &[u8],
) -> Vec<prikk_object::ObjectEnvelope> {
    let malformed = malformed_envelope(object_type, payload, 63);
    let duplicate = signature("a", 2);
    let mut repeated = duplicate.clone();
    repeated.created_at = 99;
    let duplicate = prikk_object::ObjectEnvelope {
        object_type,
        schema_version: 1,
        canonical_payload: payload.to_vec(),
        signatures: vec![duplicate, signature("b", 3), repeated],
    };
    let inverted = prikk_object::ObjectEnvelope {
        object_type,
        schema_version: 1,
        canonical_payload: payload.to_vec(),
        signatures: vec![signature("z", 3), signature("a", 1)],
    };
    vec![malformed, duplicate, inverted]
}

#[test]
fn object_and_memory_writers_reject_before_mutation() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc39-object-writers");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut memory = MemoryObjectStore::new();
    let mut file = FileObjectStore::new(layout.clone());
    for envelope in strict_rejection_variants(ObjectType::Blob, b"malformed") {
        let object_id = envelope.object_id();
        assert!(memory.write_object(&envelope).is_err());
        assert!(memory.is_empty());
        assert!(file.write_object(&envelope).is_err());
        assert!(!layout.object_path(ObjectType::Blob, object_id).exists());
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn wal_and_ref_log_reject_before_mutation() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc39-log-writers");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    for patch in strict_rejection_variants(ObjectType::Patch, b"patch") {
        assert!(wal.append_patch(&patch).is_err());
    }
    // RFC 102 Stage 1: the WAL file now exists from `init` onward (created empty), so "no mutation
    // happened" is proven by emptiness, not by absence.
    assert!(wal.replay().is_ok_and(|replay| replay.records.is_empty()));

    let target = sample_object_id("target");
    let state = sample_object_id("state");
    let update = signed_ref_update_envelope("heads/main", None, state, target, 1);
    for mut invalid in strict_rejection_variants(ObjectType::RefUpdate, &update.canonical_payload) {
        invalid.schema_version = update.schema_version;
        assert!(append_log_record_for_signature_test(&layout, "heads/main", &invalid).is_err());
    }
    assert!(
        RefStore::new(layout.clone())
            .replay_log("heads/main")?
            .records
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn publication_rejects_all_envelopes_before_first_mutation() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc39-publication");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = sample_object_id("target");
    let state = signed_ref_state_envelope("heads/main", None, target, 1);
    let state_id = state.object_id();
    let update = signed_ref_update_envelope("heads/main", None, state_id, target, 1);
    for invalid in strict_rejection_variants(ObjectType::RefUpdate, &update.canonical_payload) {
        let publication = RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: state.clone(),
            ref_update: invalid,
        };
        let store = RefStore::new(layout.clone());
        assert!(store.publish(&publication).is_err());
        assert!(!layout.object_path(ObjectType::RefState, state_id).exists());
        assert_eq!(store.read_current_ref_state_id("heads/main")?, None);
        assert!(store.replay_log("heads/main")?.records.is_empty());
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn ref_log_rejects_nonzero_schema1_clock_before_filesystem_mutation() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc39-ref-log-zero-clock");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = sample_object_id("target");
    let state = sample_object_id("state");
    let mut envelope = signed_ref_update_envelope("heads/main", None, state, target, 1);
    let mut update = RefUpdatePayload::decode_canonical(&envelope.canonical_payload)?;
    update.created_at = 7;
    envelope.canonical_payload = update.to_canonical_bytes()?;

    let log_path = layout.ref_log_path("heads/main");
    let parent = log_path
        .parent()
        .ok_or_else(|| prikk_error::PrikkError::Io {
            kind: None,
            context: "test ref-log path has no parent".to_string(),
        })?;
    let parent_existed = parent.exists();
    assert!(append_log_record_for_signature_test(&layout, "heads/main", &envelope).is_err());
    assert!(!log_path.exists());
    assert_eq!(parent.exists(), parent_existed);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn refupdate_retry_preserves_zero_sentinel_and_exact_log_bytes() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc39-refupdate-retry");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = sample_object_id("target");
    let state = signed_ref_state_envelope("heads/main", None, target, 1);
    let state_id = state.object_id();
    let update = signed_ref_update_envelope("heads/main", None, state_id, target, 1);
    let publication = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: state,
        ref_update: update,
    };
    let store = RefStore::new(layout.clone());
    store.publish(&publication)?;
    let container_path =
        layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
    let before = std::fs::read(&container_path)?;
    store.publish(&publication)?;
    let after = std::fs::read(&container_path)?;
    assert_eq!(before, after);
    let replay = store.replay_log("heads/main")?;
    let record = replay.records.first().ok_or_else(|| {
        prikk_error::PrikkError::Integrity("ref log replay returned no records".to_string())
    })?;
    let payload =
        prikk_object::RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
    assert_eq!(payload.created_at, 0);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
