//! 0.50.0 step 1, A5 (019 §5's matrix finding, graded "AGREE on `containers/index.container`, the
//! last record cut"): `verify` and `doctor` must agree on a torn object-index tail, because the
//! index is a pure cache (RFC 162 rule 1).
//!
//! **The real defect, found by reproduction (not by reading the bug report alone):** the
//! most-recently-written object's index entry is exactly the one a crash leaves torn -- the record
//! is durable in its own container before the index is appended. `object_store.rs`'s unlocked
//! readers (`resolve_object_location`, `IndexSnapshot::open`) used to fall back to a fresh
//! container scan only when the index had *interior* damage (`has_item_failure()`), never on a
//! plain trailing-partial tail with no `Failed` entry -- so a lookup for that most-recent object
//! read `None` ("not found"), even though its own container record was fully sound. When that
//! object is a ref's current `RefState`, `refs/verify/scan.rs`'s ref-publication scan (built on
//! `ObjectReadSnapshot`, which shares `IndexSnapshot::open`) reported "missing RefState" and the
//! `local-tag-trust` stage failed outright -- a false failure over nothing actually damaged. Fixed
//! by making both unlocked lookups rebuild from the containers on a trailing-partial tail too, the
//! same condition the *locked* writer-side lookup (`resolve_object_location_locked`) already used.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::{BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectType, RefKind};

use crate::doctor::doctor_repository;
use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, add_trusted_maintainer, derive_next_state_root, maintainer_signature,
    verify_repository,
};

fn trusted_signer(seed_label: &str, byte: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(seed_label, &[byte; 32])
}

fn adopt(layout: &RepositoryLayout, signer: &Ed25519MaintainerSigner) -> Result<()> {
    let public_key_hex: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    add_trusted_maintainer(layout, signer.key_id(), &public_key_hex)?;
    Ok(())
}

/// A real, trusted, published `heads/main` at a fresh Root `Block`: Block, RefState, and RefUpdate
/// are all signed by `signer` (adopted) and reach the object store and the ref pointer/log through
/// the ordinary `RefStore::publish` path -- the same shape a real `prikk seal` leaves on disk.
fn publish_trusted_main(layout: &RepositoryLayout, signer: &Ed25519MaintainerSigner) -> Result<()> {
    let mut objects = FileObjectStore::new(layout.clone());
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: derive_next_state_root(&objects, None, &[])?,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block_envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, block_payload.to_canonical_bytes()?);
    let block_id = block_envelope.object_id();
    block_envelope.add_signature(maintainer_signature(signer, ObjectType::Block, block_id)?)?;
    objects.write_object(&block_envelope)?;

    let ref_state_payload = prikk_object::RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
    );
    let ref_state_id = ref_state.object_id();
    ref_state.add_signature(maintainer_signature(
        signer,
        ObjectType::RefState,
        ref_state_id,
    )?)?;

    let ref_update_payload = prikk_object::RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut ref_update = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
    );
    let ref_update_id = ref_update.object_id();
    ref_update.add_signature(maintainer_signature(
        signer,
        ObjectType::RefUpdate,
        ref_update_id,
    )?)?;

    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// Shortens `.prikk/containers/index.container` by `cut_bytes`, leaving every earlier record
/// intact and tearing only the last one -- the exact shape a crash between a record's own durable
/// container write and its (separate) index append leaves behind. `cut_bytes` must be smaller than
/// one full index record (header + body), confirmed by the caller's own total-length arithmetic, or
/// this would tear more than the intended single record.
fn truncate_index_tail(layout: &RepositoryLayout, cut_bytes: u64) -> Result<u64> {
    let path = layout.container_index_path();
    let original = std::fs::metadata(&path)?.len();
    let bytes = std::fs::read(&path)?;
    let new_len = usize::try_from(original - cut_bytes).expect("cut_bytes fits the file");
    std::fs::write(&path, &bytes[..new_len])?;
    Ok(original)
}

/// The A5 reproduction: a torn object-index tail over the current ref's own `RefState` must read as
/// a warning, not a failure -- `verify` and `doctor` must agree, both clean (`doctor`'s existing
/// behavior) rather than `verify` alone refusing over nothing actually damaged.
#[test]
fn torn_object_index_tail_over_current_ref_state_is_a_warning_not_a_failure() -> Result<()> {
    let root = unique_temp_dir("a5-object-index-tail-agree");
    let layout = RepositoryLayout::init(root)?;
    let signer = trusted_signer("a5-tail", 7)?;
    adopt(&layout, &signer)?;
    publish_trusted_main(&layout, &signer)?;

    // Tear only the last index record (133 = 50-byte header + 83-byte body), well clear of every
    // earlier one -- the most recently appended entry is always one of this publish's own three
    // objects (Block, RefState, RefUpdate), so this reliably lands on one of them, matching A5's own
    // shape regardless of which.
    truncate_index_tail(&layout, 10)?;

    let verification = verify_repository(&layout)?;
    assert!(
        !verification.has_stage_failure(),
        "a torn index tail alone must not fail any stage: {:?}",
        verification.stage_outcomes
    );
    assert!(
        !verification.has_item_failure(),
        "a torn index tail alone must not fail any object, block, or ref item: \
         object_outcomes={:?} ref_item_outcomes={:?} pointer_outcomes={:?} log_outcomes={:?}",
        verification.object_outcomes,
        verification.ref_item_outcomes,
        verification.pointer_outcomes,
        verification.log_outcomes
    );
    assert_eq!(verification.trailing_partial_object_index_bytes, Some(123));
    assert_eq!(verification.object_index_interior_damage, Some(false));

    let doctor = doctor_repository(&layout);
    assert!(
        doctor.is_healthy(),
        "doctor must report no error-severity issue over a torn index tail: {:?}",
        doctor.issues
    );
    assert!(
        doctor
            .issues
            .iter()
            .any(|issue| issue.code == "PRIKK-DOCTOR-OBJECT-INDEX-TRAILING-PARTIAL"),
        "doctor must still name the tail: {:?}",
        doctor.issues
    );
    Ok(())
}

/// Control: a damaged but **complete** index record (an interior checksum mismatch, not a tail)
/// must still be reported as interior damage -- unchanged by A5's fix, which only widens the
/// fallback's trigger to *also* cover a trailing-partial tail, never narrows what already counted
/// as damage.
#[test]
fn complete_damaged_index_record_still_reports_interior_damage() -> Result<()> {
    let root = unique_temp_dir("a5-object-index-interior-damage-control");
    let layout = RepositoryLayout::init(root)?;
    let signer = trusted_signer("a5-interior", 11)?;
    adopt(&layout, &signer)?;
    publish_trusted_main(&layout, &signer)?;

    let path = layout.container_index_path();
    let mut bytes = std::fs::read(&path)?;
    // Flip one byte inside the first record's body (header is 50 bytes), leaving the file's total
    // length -- and every record's own claimed boundaries -- unchanged: a complete record whose
    // checksum no longer matches, not a tail.
    bytes[60] ^= 0xFF;
    std::fs::write(&path, &bytes)?;

    let verification = verify_repository(&layout)?;
    assert_eq!(
        verification.object_index_interior_damage,
        Some(true),
        "a complete record's checksum mismatch must still be reported as interior damage"
    );
    assert_eq!(verification.trailing_partial_object_index_bytes, Some(0));
    Ok(())
}
