//! RFC 115 Stage 4 §6: the seal-from-accepted path's behaviour, and a negative control for every
//! row ("do not compress them" -- the handoff's own instruction). Each control mutates the single
//! narrowest line that should break the property under test.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId,
    ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, RecognitionClaimPayload,
};

use super::{SealFromAcceptedOutcome, seal_from_accepted_claim};
use crate::author::author_signing::author_signature;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{ContainerSlot, DEFAULT_ACTIVE_NAME};
use crate::maintainer_signing::{
    Ed25519MaintainerSigner, MaintainerSigner as _, maintainer_signature,
};
use crate::recognition_claim::ClaimSignatureVerification;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::{
    Ed25519AuthorSigner, FileObjectStore, ObjectWriter, RefStore, RepositoryLayout,
    add_trusted_maintainer,
};

const TARGET_REF: &str = "heads/main";

fn fresh_repo(name: &str) -> Result<RepositoryLayout> {
    RepositoryLayout::init(unique_temp_dir(name))
}

fn maintainer_signer(seed: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(format!("seal-from-accepted-maintainer-{seed}"), &[seed; 32])
}

fn author_signer(seed: u8) -> Result<Ed25519AuthorSigner> {
    Ed25519AuthorSigner::from_seed(format!("seal-from-accepted-author-{seed}"), &[seed; 32])
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

fn write_blob(objects: &mut FileObjectStore, content: &[u8]) -> Result<ObjectId> {
    let signer = maintainer_signer(0xFF)?;
    let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(&signer, ObjectType::Blob, id)?)?;
    objects.write_object(&envelope)
}

/// A standalone, AUTHOR-signed `CreateFile` patch. `blob_id` need not exist -- row 5's own control
/// deliberately passes one that does not.
fn create_file_patch(
    signer: &Ed25519AuthorSigner,
    path: &str,
    node_seed: u8,
    blob_id: ObjectId,
) -> Result<ObjectEnvelope> {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_seed; 32]),
                blob_id,
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
    let id = envelope.object_id();
    envelope.add_signature(author_signature(signer, id)?)?;
    Ok(envelope)
}

fn write_create_file_patch(
    objects: &mut FileObjectStore,
    signer: &Ed25519AuthorSigner,
    path: &str,
    node_seed: u8,
    blob_id: ObjectId,
) -> Result<ObjectId> {
    let envelope = create_file_patch(signer, path, node_seed, blob_id)?;
    objects.write_object(&envelope)
}

fn write_claim(
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
    block_id: ObjectId,
    patch_ids: Vec<ObjectId>,
) -> Result<ObjectId> {
    let payload = RecognitionClaimPayload {
        block_id,
        patch_ids,
        parent_block_ids: Vec::new(),
    };
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes()?,
    );
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(
        signer,
        ObjectType::RecognitionClaim,
        id,
    )?)?;
    objects.write_object(&envelope)
}

fn current_tip(layout: &RepositoryLayout) -> Result<Option<ObjectId>> {
    let ref_store = RefStore::new(layout.clone());
    let Some(ref_state_id) = ref_store.read_current_ref_state_id(TARGET_REF)? else {
        return Ok(None);
    };
    let object_store = FileObjectStore::new(layout.clone());
    let envelope = crate::object_store::ObjectReader::read_typed(
        &object_store,
        ref_state_id,
        ObjectType::RefState,
    )?
    .expect("ref state must exist");
    let payload = prikk_object::RefStatePayload::decode_canonical(
        &envelope.canonical_payload,
        envelope.schema_version,
    )?;
    Ok(Some(payload.target_object_id))
}

/// The raw bytes of the Block container's primary slot -- a byte-for-byte proof that no new Block
/// was written, not merely that the ref's own tip didn't move.
fn block_container_bytes(layout: &RepositoryLayout) -> Result<Vec<u8>> {
    let relative = layout
        .repository_relative(&layout.container_slot_path(ObjectType::Block, ContainerSlot::A))?;
    Ok(read_file_if_exists(layout.repository_mutation_root(), &relative)?.unwrap_or_default())
}

/// The base fixture every other test builds on: one repository, one adopted+trusted maintainer
/// signer, one AUTHOR signer, one blob, one unsealed patch, one claim naming it (block_id is
/// decorative for this path -- see the module doc -- so an arbitrary id is fine).
struct BaseFixture {
    layout: RepositoryLayout,
    signer: Ed25519MaintainerSigner,
    patch_id: ObjectId,
    claim_id: ObjectId,
}

fn base_fixture(name: &str) -> Result<BaseFixture> {
    let layout = fresh_repo(name)?;
    let signer = maintainer_signer(0x10)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0x11)?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob_id = write_blob(&mut objects, b"seal-from-accepted fixture\n")?;
    let patch_id = write_create_file_patch(&mut objects, &author, "a.txt", 0x12, blob_id)?;
    let claim_id = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xAA; 32]),
        vec![patch_id],
    )?;
    Ok(BaseFixture {
        layout,
        signer,
        patch_id,
        claim_id,
    })
}

fn cleanup(layout: &RepositoryLayout) {
    let _ = std::fs::remove_dir_all(layout.root());
}

#[test]
fn a_healthy_claim_seals_a_new_block_carrying_its_patches() -> Result<()> {
    let fixture = base_fixture("seal-from-accepted-healthy")?;
    let outcome = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )?;
    let block_id = match outcome {
        SealFromAcceptedOutcome::Sealed {
            ref_name,
            patch_count,
            claim_signature_outcome,
            block_id,
            ..
        } => {
            assert_eq!(ref_name, TARGET_REF);
            assert_eq!(patch_count, 1);
            assert_eq!(
                claim_signature_outcome,
                ClaimSignatureVerification::Sound {
                    key_id: fixture.signer.key_id().to_string()
                }
            );
            block_id
        }
        other => panic!("expected Sealed, got {other:?}"),
    };
    assert_eq!(current_tip(&fixture.layout)?, Some(block_id));
    let object_store = FileObjectStore::new(fixture.layout.clone());
    let block_envelope =
        crate::object_store::ObjectReader::read_typed(&object_store, block_id, ObjectType::Block)?
            .expect("sealed block must exist");
    let block_payload =
        prikk_object::BlockPayload::decode_canonical(&block_envelope.canonical_payload)?;
    assert_eq!(block_payload.patch_ids, vec![fixture.patch_id]);
    cleanup(&fixture.layout);
    Ok(())
}

/// §6 row 1: sealing requires a locally trusted signer. A second maintainer is adopted so the
/// refusal is specifically "this key is not trusted", not merely "no policy exists at all".
#[test]
fn row1_sealing_with_an_unadopted_signer_refuses() -> Result<()> {
    let fixture = base_fixture("seal-from-accepted-row1")?;
    let unadopted = maintainer_signer(0x20)?;
    let result =
        seal_from_accepted_claim(&fixture.layout, TARGET_REF, fixture.claim_id, &unadopted);
    assert!(result.is_err(), "an unadopted signer must refuse to seal");
    assert!(
        current_tip(&fixture.layout)?.is_none(),
        "nothing must be written"
    );
    cleanup(&fixture.layout);
    Ok(())
}

/// §6 row 2: trust does not expand across a successful seal.
#[test]
fn row2_a_successful_seal_leaves_the_adopted_set_unchanged() -> Result<()> {
    let fixture = base_fixture("seal-from-accepted-row2")?;
    let before = crate::trust::load_maintainer_trust_policy(&fixture.layout)?;
    let outcome = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )?;
    assert!(matches!(outcome, SealFromAcceptedOutcome::Sealed { .. }));
    let after = crate::trust::load_maintainer_trust_policy(&fixture.layout)?;
    assert_eq!(before, after);
    cleanup(&fixture.layout);
    Ok(())
}

/// §6 row 3: an `Unverifiable` claim -- signed by a maintainer this repository has never adopted --
/// can still supply the order sealed under. The *sealing* signer is a second, genuinely adopted key.
#[test]
fn row3_an_unverifiable_claim_still_supplies_the_order() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row3")?;
    let sealing_signer = maintainer_signer(0x30)?;
    adopt(&layout, &sealing_signer)?;
    let claim_signer = maintainer_signer(0x31)?; // never adopted
    let author = author_signer(0x32)?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob_id = write_blob(&mut objects, b"row3 fixture\n")?;
    let patch_id = write_create_file_patch(&mut objects, &author, "row3.txt", 0x33, blob_id)?;
    let claim_id = write_claim(
        &mut objects,
        &claim_signer,
        ObjectId::from_bytes([0xBB; 32]),
        vec![patch_id],
    )?;

    let outcome = seal_from_accepted_claim(&layout, TARGET_REF, claim_id, &sealing_signer)?;
    match outcome {
        SealFromAcceptedOutcome::Sealed {
            claim_signature_outcome,
            patch_count,
            ..
        } => {
            assert_eq!(patch_count, 1);
            assert_eq!(
                claim_signature_outcome,
                ClaimSignatureVerification::Unverifiable {
                    key_id: claim_signer.key_id().to_string()
                }
            );
        }
        other => panic!("expected Sealed, got {other:?}"),
    }
    // §6 row 2, in the one context where an expansion would actually be observable: the claim
    // signer's own key_id must not have been adopted as a side effect of a successful seal.
    let policy = crate::trust::load_maintainer_trust_policy(&layout)?;
    assert!(
        !policy
            .keys
            .iter()
            .any(|adopted| adopted.key_id == claim_signer.key_id()),
        "the claim signer's key must not have been adopted by a successful seal"
    );
    cleanup(&layout);
    Ok(())
}

/// §6 rows 4 and 5: the pair that proves classification is real, not a label -- both exercised
/// through the identical code path (`classify_patch_application_failure`), differing only in which
/// `LifecycleReplayError` variant they trigger.
#[test]
fn row4_a_patch_that_cannot_apply_to_the_tip_refuses_as_divergence() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row4")?;
    let signer = maintainer_signer(0x40)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0x41)?;
    let mut objects = FileObjectStore::new(layout.clone());

    // First, a genuine successful seal, establishing a real tip with "shared.txt" already present.
    let blob_a = write_blob(&mut objects, b"row4 first\n")?;
    let patch_a = write_create_file_patch(&mut objects, &author, "shared.txt", 0x42, blob_a)?;
    let claim_a = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xC1; 32]),
        vec![patch_a],
    )?;
    let first = seal_from_accepted_claim(&layout, TARGET_REF, claim_a, &signer)?;
    assert!(matches!(first, SealFromAcceptedOutcome::Sealed { .. }));
    let tip_after_first = current_tip(&layout)?;

    // A second, unsealed patch that creates the *same path* under a different node -- an ordinary
    // divergence: two histories that would disagree about "shared.txt", not a broken repository.
    let blob_b = write_blob(&mut objects, b"row4 second\n")?;
    let patch_b = write_create_file_patch(&mut objects, &author, "shared.txt", 0x43, blob_b)?;
    let claim_b = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xC2; 32]),
        vec![patch_b],
    )?;

    let result = seal_from_accepted_claim(&layout, TARGET_REF, claim_b, &signer);
    let error = result.expect_err("a path conflict must refuse the seal");
    let message = error.to_string();
    assert!(
        message.contains("divergence"),
        "expected a divergence-classified refusal, got: {message}"
    );
    assert!(!message.contains("seal refused: integrity"));
    assert_eq!(
        current_tip(&layout)?,
        tip_after_first,
        "a divergence refusal must write nothing"
    );
    cleanup(&layout);
    Ok(())
}

#[test]
fn row5_a_patch_referencing_a_missing_blob_refuses_as_integrity() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row5")?;
    let signer = maintainer_signer(0x50)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0x51)?;
    let mut objects = FileObjectStore::new(layout.clone());

    // The patch's own CreateFile names a blob that was never written -- accept's own closure check
    // should have caught this before the patch ever reached storage; this repository's own state is
    // broken, not merely diverged from the sender's.
    let missing_blob_id = ObjectId::from_bytes([0xD1; 32]);
    let patch_id =
        write_create_file_patch(&mut objects, &author, "row5.txt", 0x52, missing_blob_id)?;
    let claim_id = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xD2; 32]),
        vec![patch_id],
    )?;

    let result = seal_from_accepted_claim(&layout, TARGET_REF, claim_id, &signer);
    let error = result.expect_err("a missing blob must refuse the seal");
    let message = error.to_string();
    assert!(
        message.contains("integrity"),
        "expected an integrity-classified refusal, got: {message}"
    );
    assert!(!message.contains("divergence"));
    assert!(current_tip(&layout)?.is_none(), "nothing must be written");
    cleanup(&layout);
    Ok(())
}

/// §6 row 6: a missing named patch refuses.
#[test]
fn row6_a_claim_naming_a_missing_patch_refuses() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row6")?;
    let signer = maintainer_signer(0x60)?;
    adopt(&layout, &signer)?;
    let mut objects = FileObjectStore::new(layout.clone());
    let missing_patch_id = ObjectId::from_bytes([0xE1; 32]);
    let claim_id = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xE2; 32]),
        vec![missing_patch_id],
    )?;

    let result = seal_from_accepted_claim(&layout, TARGET_REF, claim_id, &signer);
    assert!(
        result.is_err(),
        "a missing named patch must refuse the seal"
    );
    assert!(current_tip(&layout)?.is_none());
    cleanup(&layout);
    Ok(())
}

/// §6 row 7 (D7, inverted from the withdrawn refusal -- `RFC-115-stage-4-d7-refusal-shape-handoff-
/// v1.md` §3): a claim naming some already-sealed and some not-yet-sealed patches seals the
/// unsealed remainder rather than refusing. The already-sealed patch's effect is already in this
/// repository's state; sealing only the new one is precisely right.
#[test]
fn row7_a_mixed_claim_seals_the_unsealed_remainder() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row7")?;
    let signer = maintainer_signer(0x70)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0x71)?;
    let mut objects = FileObjectStore::new(layout.clone());

    let blob_a = write_blob(&mut objects, b"row7 first\n")?;
    let patch_a = write_create_file_patch(&mut objects, &author, "row7-a.txt", 0x72, blob_a)?;
    let claim_a = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xF1; 32]),
        vec![patch_a],
    )?;
    let first = seal_from_accepted_claim(&layout, TARGET_REF, claim_a, &signer)?;
    assert!(matches!(first, SealFromAcceptedOutcome::Sealed { .. }));

    let blob_b = write_blob(&mut objects, b"row7 second\n")?;
    let patch_b = write_create_file_patch(&mut objects, &author, "row7-b.txt", 0x73, blob_b)?;
    // Names the already-sealed patch_a and the still-unsealed patch_b together.
    let claim_mixed = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xF2; 32]),
        vec![patch_a, patch_b],
    )?;

    let outcome = seal_from_accepted_claim(&layout, TARGET_REF, claim_mixed, &signer)?;
    let block_id = match outcome {
        SealFromAcceptedOutcome::Sealed {
            patch_count,
            block_id,
            ..
        } => {
            assert_eq!(patch_count, 1, "only the unsealed patch is new work");
            block_id
        }
        other => panic!("expected Sealed, got {other:?}"),
    };
    let object_store = FileObjectStore::new(layout.clone());
    let block_envelope =
        crate::object_store::ObjectReader::read_typed(&object_store, block_id, ObjectType::Block)?
            .expect("sealed block must exist");
    let block_payload =
        prikk_object::BlockPayload::decode_canonical(&block_envelope.canonical_payload)?;
    assert_eq!(
        block_payload.patch_ids,
        vec![patch_b],
        "the block carries only the unsealed subset, not the already-sealed patch"
    );
    assert_eq!(current_tip(&layout)?, Some(block_id));
    cleanup(&layout);
    Ok(())
}

/// §6 row 8: two stored claims naming overlapping patches but disagreeing on their relative order
/// refuse, naming both.
#[test]
fn row8_ambiguous_overlapping_claims_refuse() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row8")?;
    let signer = maintainer_signer(0x80)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0x81)?;
    let mut objects = FileObjectStore::new(layout.clone());

    let blob_a = write_blob(&mut objects, b"row8 first\n")?;
    let patch_a = write_create_file_patch(&mut objects, &author, "row8-a.txt", 0x82, blob_a)?;
    let blob_b = write_blob(&mut objects, b"row8 second\n")?;
    let patch_b = write_create_file_patch(&mut objects, &author, "row8-b.txt", 0x83, blob_b)?;

    let claim_forward = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0x91; 32]),
        vec![patch_a, patch_b],
    )?;
    let claim_reversed = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0x92; 32]),
        vec![patch_b, patch_a],
    )?;

    let result = seal_from_accepted_claim(&layout, TARGET_REF, claim_forward, &signer);
    let error = result.expect_err("overlapping, order-disagreeing claims must refuse");
    let message = error.to_string();
    assert!(message.contains(&claim_forward.to_string()));
    assert!(message.contains(&claim_reversed.to_string()));
    assert!(current_tip(&layout)?.is_none());
    cleanup(&layout);
    Ok(())
}

/// §6 row 9: replay is inert -- sealing the same claim twice writes no second block.
#[test]
fn row9_sealing_the_same_claim_twice_is_a_no_op_the_second_time() -> Result<()> {
    let fixture = base_fixture("seal-from-accepted-row9")?;
    let first = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )?;
    assert!(matches!(first, SealFromAcceptedOutcome::Sealed { .. }));
    let tip_after_first = current_tip(&fixture.layout)?;

    let second = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )?;
    match second {
        SealFromAcceptedOutcome::AlreadySealed { ref_name, claim_id } => {
            assert_eq!(ref_name, TARGET_REF);
            assert_eq!(claim_id, fixture.claim_id);
        }
        other => panic!("expected AlreadySealed, got {other:?}"),
    }
    assert_eq!(
        current_tip(&fixture.layout)?,
        tip_after_first,
        "no second block"
    );
    cleanup(&fixture.layout);
    Ok(())
}

/// Review condition (`RFC-115-stage-4-seal-from-accepted-review-v1.md` §4): pins the invariant the
/// "signer trust is not checked on the no-op path" inference (§3 of that review) rests on -- that
/// the no-op path performs no trust-gated act, so it is safe to skip `verify_signer_trusted` there.
/// That is true today and enforced by nothing; a later edit that records anything on that path would
/// silently convert an un-gated read into an un-gated write, and no other test here would notice.
/// An **unadopted** signer, against a claim whose patches are **all already sealed**, must return
/// `AlreadySealed` and leave the repository byte-identical: no new Block, the ref's own `RefState`
/// unmoved, and the adopted-maintainer set unchanged.
#[test]
fn the_no_op_path_is_byte_identical_even_for_an_unadopted_signer() -> Result<()> {
    let fixture = base_fixture("seal-from-accepted-noop-byte-identical")?;
    let first = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )?;
    assert!(matches!(first, SealFromAcceptedOutcome::Sealed { .. }));

    let ref_store = RefStore::new(fixture.layout.clone());
    let ref_state_before = ref_store.read_current_ref_state_id(TARGET_REF)?;
    let block_bytes_before = block_container_bytes(&fixture.layout)?;
    let trust_before = crate::trust::load_maintainer_trust_policy(&fixture.layout)?;

    let unadopted = maintainer_signer(0xB0)?; // never adopted in this repository
    let outcome =
        seal_from_accepted_claim(&fixture.layout, TARGET_REF, fixture.claim_id, &unadopted)?;
    match outcome {
        SealFromAcceptedOutcome::AlreadySealed { ref_name, claim_id } => {
            assert_eq!(ref_name, TARGET_REF);
            assert_eq!(claim_id, fixture.claim_id);
        }
        other => panic!("expected AlreadySealed even for an unadopted signer, got {other:?}"),
    }

    let ref_state_after = ref_store.read_current_ref_state_id(TARGET_REF)?;
    let block_bytes_after = block_container_bytes(&fixture.layout)?;
    let trust_after = crate::trust::load_maintainer_trust_policy(&fixture.layout)?;
    assert_eq!(
        ref_state_before, ref_state_after,
        "the ref's RefState must not move"
    );
    assert_eq!(
        block_bytes_before, block_bytes_after,
        "byte-for-byte: no new block written"
    );
    assert_eq!(
        trust_before, trust_after,
        "the adopted-maintainer set must not change"
    );

    cleanup(&fixture.layout);
    Ok(())
}

/// §6 row 10: a non-empty active WAL refuses.
#[test]
fn row10_a_non_empty_active_wal_refuses() -> Result<()> {
    let fixture = base_fixture("seal-from-accepted-row10")?;
    let author = author_signer(0x9A)?;
    let mut objects = FileObjectStore::new(fixture.layout.clone());
    let blob_id = write_blob(&mut objects, b"queued\n")?;
    let queued = create_file_patch(&author, "queued.txt", 0x9B, blob_id)?;
    Wal::for_layout(&fixture.layout, DEFAULT_ACTIVE_NAME).append_patch(&queued)?;

    let result = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    );
    let err = result.err().ok_or_else(|| {
        PrikkError::Integrity("a non-empty active WAL must refuse the seal".to_string())
    })?;
    // RFC 132 part 2: a non-empty active WAL is a caller precondition here, not a lock -- nothing
    // is held and no other writer is racing this one.
    assert!(
        matches!(err, PrikkError::Precondition(_)),
        "unexpected error variant: {err:?}"
    );
    assert!(
        err.to_string().starts_with("precondition not met:"),
        "unexpected error: {err}"
    );
    assert!(current_tip(&fixture.layout)?.is_none());
    cleanup(&fixture.layout);
    Ok(())
}

/// §6 row 11: the sealed block carries the claimed order verbatim, even when that order is not
/// sorted by `ObjectId`.
#[test]
fn row11_the_sealed_block_carries_the_claims_order_verbatim() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row11")?;
    let signer = maintainer_signer(0xA0)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0xA1)?;
    let mut objects = FileObjectStore::new(layout.clone());

    let blob_a = write_blob(&mut objects, b"row11 first\n")?;
    let patch_a = write_create_file_patch(&mut objects, &author, "row11-a.txt", 0xA2, blob_a)?;
    let blob_b = write_blob(&mut objects, b"row11 second\n")?;
    let patch_b = write_create_file_patch(&mut objects, &author, "row11-b.txt", 0xA3, blob_b)?;
    let (first, second) = if patch_a < patch_b {
        (patch_b, patch_a) // deliberately descending
    } else {
        (patch_a, patch_b)
    };
    let claim_id = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xA4; 32]),
        vec![first, second],
    )?;

    let outcome = seal_from_accepted_claim(&layout, TARGET_REF, claim_id, &signer)?;
    let SealFromAcceptedOutcome::Sealed { block_id, .. } = outcome else {
        panic!("expected Sealed");
    };
    let object_store = FileObjectStore::new(layout.clone());
    let block_envelope =
        crate::object_store::ObjectReader::read_typed(&object_store, block_id, ObjectType::Block)?
            .expect("sealed block must exist");
    let block_payload =
        prikk_object::BlockPayload::decode_canonical(&block_envelope.canonical_payload)?;
    assert_eq!(block_payload.patch_ids, vec![first, second]);
    cleanup(&layout);
    Ok(())
}

/// §6 row 11, the mixed case (D7): when one of three claimed patches is already sealed, the block
/// carries the remaining two in the claim's own verbatim order, restricted to that subset -- not
/// the claim's order with a gap silently closed some other way, and not re-sorted.
#[test]
fn row11_mixed_the_sealed_block_carries_the_unsealed_subset_in_claim_order() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-row11-mixed")?;
    let signer = maintainer_signer(0xA5)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0xA6)?;
    let mut objects = FileObjectStore::new(layout.clone());

    let blob_a = write_blob(&mut objects, b"row11-mixed first\n")?;
    let patch_a =
        write_create_file_patch(&mut objects, &author, "row11-mixed-a.txt", 0xA7, blob_a)?;
    let claim_a = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xA8; 32]),
        vec![patch_a],
    )?;
    let first = seal_from_accepted_claim(&layout, TARGET_REF, claim_a, &signer)?;
    assert!(matches!(first, SealFromAcceptedOutcome::Sealed { .. }));

    let blob_b = write_blob(&mut objects, b"row11-mixed second\n")?;
    let patch_b =
        write_create_file_patch(&mut objects, &author, "row11-mixed-b.txt", 0xA9, blob_b)?;
    let blob_c = write_blob(&mut objects, b"row11-mixed third\n")?;
    let patch_c =
        write_create_file_patch(&mut objects, &author, "row11-mixed-c.txt", 0xAB, blob_c)?;
    // The claim's own verbatim order is patch_c, patch_a, patch_b -- not sorted, not the order the
    // two patches were written in -- so the filtered subset's order can only have come from the
    // claim, not from insertion order or from `ObjectId` order.
    let claim_mixed = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xAC; 32]),
        vec![patch_c, patch_a, patch_b],
    )?;

    let outcome = seal_from_accepted_claim(&layout, TARGET_REF, claim_mixed, &signer)?;
    let SealFromAcceptedOutcome::Sealed { block_id, .. } = outcome else {
        panic!("expected Sealed");
    };
    let object_store = FileObjectStore::new(layout.clone());
    let block_envelope =
        crate::object_store::ObjectReader::read_typed(&object_store, block_id, ObjectType::Block)?
            .expect("sealed block must exist");
    let block_payload =
        prikk_object::BlockPayload::decode_canonical(&block_envelope.canonical_payload)?;
    assert_eq!(block_payload.patch_ids, vec![patch_c, patch_b]);
    cleanup(&layout);
    Ok(())
}

/// §5's motivating deadlock, end to end (`RFC-115-stage-4-d7-refusal-shape-handoff-v1.md` §5): a
/// receiver seals one claim, then meets a second claim naming that already-sealed patch plus a new
/// one. Under the withdrawn rule this refused permanently; under D7 the second seal succeeds, and
/// -- the assertion that actually proves the deadlock is gone -- the new patch is afterwards
/// reachable from the ref tip.
#[test]
fn d7_a_claim_overlapping_an_already_sealed_patch_no_longer_deadlocks() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-d7-deadlock")?;
    let signer = maintainer_signer(0xB1)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0xB2)?;
    let mut objects = FileObjectStore::new(layout.clone());

    let blob_shared = write_blob(&mut objects, b"d7 shared\n")?;
    let patch_shared =
        write_create_file_patch(&mut objects, &author, "d7-shared.txt", 0xB3, blob_shared)?;
    let claim_first = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xB4; 32]),
        vec![patch_shared],
    )?;
    let first = seal_from_accepted_claim(&layout, TARGET_REF, claim_first, &signer)?;
    assert!(matches!(first, SealFromAcceptedOutcome::Sealed { .. }));

    let blob_new = write_blob(&mut objects, b"d7 new\n")?;
    let patch_new = write_create_file_patch(&mut objects, &author, "d7-new.txt", 0xB5, blob_new)?;
    // As merge_execute.rs's own adopted_patch_ids does: a second claim re-names the already-sealed
    // patch (from a different block) alongside a genuinely new one.
    let claim_second = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xB6; 32]),
        vec![patch_shared, patch_new],
    )?;

    let outcome = seal_from_accepted_claim(&layout, TARGET_REF, claim_second, &signer)?;
    let block_id = match outcome {
        SealFromAcceptedOutcome::Sealed { block_id, .. } => block_id,
        other => panic!("expected the second seal to succeed, got {other:?}"),
    };

    // The assertion that matters: the new patch's effect is reachable from the ref tip, not merely
    // that the refusal is gone.
    let object_store = FileObjectStore::new(layout.clone());
    let block_envelope =
        crate::object_store::ObjectReader::read_typed(&object_store, block_id, ObjectType::Block)?
            .expect("sealed block must exist");
    let block_payload =
        prikk_object::BlockPayload::decode_canonical(&block_envelope.canonical_payload)?;
    assert_eq!(block_payload.patch_ids, vec![patch_new]);
    assert_eq!(current_tip(&layout)?, Some(block_id));
    cleanup(&layout);
    Ok(())
}
