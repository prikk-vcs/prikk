//! Merge execution tests.

use prikk_error::PrikkError;
use prikk_object::ObjectId;

use super::ensure_into_ref_unmoved;

/// DC-75 two-edits handoff §6: a ref another writer advanced between evidence and seal is a lock
/// conflict to retry, not damage and not a precondition.
#[test]
fn an_into_ref_advanced_during_evidence_is_a_lock_conflict() {
    let read = ObjectId::from_bytes([1; 32]);
    let advanced = ObjectId::from_bytes([2; 32]);
    assert!(ensure_into_ref_unmoved("heads/main", read, read).is_ok());
    match ensure_into_ref_unmoved("heads/main", advanced, read) {
        Err(PrikkError::LockConflict(message)) => assert_eq!(
            message,
            "ref heads/main advanced during merge evidence gathering; retry"
        ),
        other => panic!("expected a lock conflict, got {other:?}"),
    }
}

/// Part B review carry 2: `merge`'s own refusal names `prikk ref complete <ref>` when *another*
/// ref's refusal is a genuine N3 lead -- checked before any evidence gathering, so `baseline_block_id`
/// and `from_ref` never need to resolve to anything real for this one.
#[test]
fn execute_merge_names_ref_complete_for_another_refs_genuine_lead() -> prikk_error::Result<()> {
    use prikk_object::{
        CanonicalEncode, ObjectEnvelope, ObjectType, RefKind, RefStatePayload, RefUpdatePayload,
    };

    use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
    use crate::maintainer_signing::{
        Ed25519MaintainerSigner, MaintainerSigner, maintainer_signature,
    };
    use crate::object_store::ObjectWriter;
    use crate::test_gates::test_support::unique_temp_dir;
    use crate::{FileObjectStore, RefPublication, RefStore, add_trusted_maintainer};

    let root = unique_temp_dir("merge-019-5-2-ref-complete");
    let layout = RepositoryLayout::init(root)?;
    let signer = Ed25519MaintainerSigner::from_seed("merge-019-5-2-maintainer", &[0x91; 32])?;
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &prikk_hash::to_hex(&signer.public_key_bytes()),
    )?;

    let mut objects = FileObjectStore::new(layout.clone());
    let block = crate::test_gates::test_support::signed_empty_block_envelope();
    let target = objects.write_object(&block)?;
    let state = RefStatePayload {
        ref_name: "heads/other".to_string(),
        kind: RefKind::Branch,
        target_object_id: target,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes()?);
    let lead_id = state_env.object_id();
    state_env.add_signature(maintainer_signature(
        &signer,
        ObjectType::RefState,
        lead_id,
    )?)?;
    let update = RefUpdatePayload {
        ref_name: "heads/other".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: lead_id,
        new_target_object_id: target,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut update_env =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, update.to_canonical_bytes()?);
    let update_id = update_env.object_id();
    update_env.add_signature(maintainer_signature(
        &signer,
        ObjectType::RefUpdate,
        update_id,
    )?)?;
    let log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    let before_len = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: "heads/other".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: state_env,
        ref_update: update_env,
    })?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&log_path)?
        .set_len(before_len)?;

    let error = match super::execute_merge(
        &layout,
        ObjectId::from_bytes([0; 32]),
        "heads/main",
        "heads/x",
        &signer,
    ) {
        Err(error) => error.to_string(),
        Ok(report) => {
            panic!("expected execute_merge to refuse behind heads/other's own lead, got {report:?}")
        }
    };
    assert!(
        error.contains("run `prikk ref complete heads/other`"),
        "{error}"
    );

    let plan = match crate::ref_completion::plan_ref_completion(&layout, "heads/other")? {
        Ok(plan) => plan,
        Err(refusal) => panic!("heads/other must be a completable lead, got {refusal:?}"),
    };
    let active_lock = crate::lock::ActiveLock::acquire(&layout, crate::DEFAULT_ACTIVE_NAME)?;
    let mut object_store = crate::object_store::ObjectWriteSession::open(&layout)?;
    let completed = crate::ref_completion::complete_ref_publication(
        &layout,
        &mut object_store,
        &active_lock,
        &plan,
        &signer,
    )?;
    assert_eq!(completed, lead_id);
    Ok(())
}
