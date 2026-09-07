//! `created_at == 0` malformed-data detection (RFC 103: unconditional since format-1 retirement,
//! no longer "a format-2 repository contaminated by format-1 records").

use prikk_object::{CanonicalEncode, ObjectEnvelope, ObjectType, RefUpdatePayload};

use super::root_publication;
use crate::foundation::layout::{ContainerSlot, ref_name_key_bytes};
use crate::test_gates::test_support::{
    maintainer_signature, signed_patch_envelope, unique_temp_dir,
};
use crate::{
    ActiveSession, DEFAULT_ACTIVE_PATCH_LIMIT, RefStore, RepositoryLayout, verify_repository,
};

#[test]
fn format2_legacy_timestamp_is_not_normalized_and_blocks_mutation() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-legacy-timestamp");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let store = RefStore::new(layout.clone());
    store.publish(&publication)?;
    // `heads/main` is the only ref this fixture ever publishes, so emptying the whole shared log
    // container and re-appending one replacement record is equivalent to replacing just its own
    // subsequence.
    std::fs::write(layout.ref_log_container_slot_path(ContainerSlot::A), b"")?;
    let mut update = RefUpdatePayload::decode_canonical(&publication.ref_update.canonical_payload)?;
    update.created_at = 7;
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, update.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    // RFC 102 Stage 4 checkpoint review, design-v1.md §13.15: `append_ref_container_record` now
    // itself refuses a nonzero-`created_at` write (the check this test used to rely on `verify_
    // repository` alone to catch is restored at the write choke point too) -- this test's whole
    // point is proving `verify_repository` still catches the defect independently of write-time
    // enforcement, so it must bypass the append path's own validation the same way a real crash or
    // an older writer could have, not go through it and be refused before the scenario exists.
    let record = crate::refs::encode_ref_container_record_for_test(
        ref_name_key_bytes("heads/main"),
        &envelope,
    )?;
    std::fs::write(layout.ref_log_container_slot_path(ContainerSlot::A), record)?;

    assert!(verify_repository(&layout)?.has_stage_failure());
    assert!(
        ActiveSession::new(layout.clone())
            .append_patch(&signed_patch_envelope(), DEFAULT_ACTIVE_PATCH_LIMIT)
            .is_err()
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
