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
    write_claim_with_parents(objects, signer, block_id, patch_ids, Vec::new())
}

fn write_claim_with_parents(
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
    block_id: ObjectId,
    patch_ids: Vec<ObjectId>,
    parent_block_ids: Vec<ObjectId>,
) -> Result<ObjectId> {
    let payload = RecognitionClaimPayload {
        block_id,
        patch_ids,
        parent_block_ids,
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
    Ok(
        std::fs::read(layout.container_slot_path(ObjectType::Block, ContainerSlot::A))
            .unwrap_or_default(),
    )
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

/// **RFC 159 site control (`sync seal`)**: a second claim, sealed on a tip the first one made (a checkpoint, recorded and
/// signed by the adopted maintainer), continues from that anchor **at the seal-from-accepted site and no other**, and
/// seals the **identical block** as the same two claims sealed with no record (the full walk). Two repositories are built
/// from the same fixed seeds; only the second has its record removed between the seals.
/// **Perturb:** `StateAnchoring::Never` at the call in `seal_from_accepted.rs`: `uses["seal-from-accepted"]` is 0 and only
/// this control (and the source scan) goes red.
#[test]
fn a_second_claim_continues_from_the_anchor_and_seals_the_identical_block() -> Result<()> {
    use crate::verified_blocks::record_path;
    use crate::{anchor_uses_for_test_support, reset_anchor_uses_for_test_support};

    let seal_two = |name: &str, drop_record: bool| -> Result<(ObjectId, usize)> {
        let layout = fresh_repo(name)?;
        let signer = maintainer_signer(0x74)?;
        adopt(&layout, &signer)?;
        let author = author_signer(0x75)?;
        let mut objects = FileObjectStore::new(layout.clone());
        let blob_a = write_blob(&mut objects, b"rfc159 first\n")?;
        let patch_a = write_create_file_patch(&mut objects, &author, "r159-a.txt", 0x76, blob_a)?;
        let claim_a = write_claim(
            &mut objects,
            &signer,
            ObjectId::from_bytes([0xF3; 32]),
            vec![patch_a],
        )?;
        seal_from_accepted_claim(&layout, TARGET_REF, claim_a, &signer)?;
        if drop_record {
            std::fs::remove_file(record_path(&layout))?;
        }
        let blob_b = write_blob(&mut objects, b"rfc159 second\n")?;
        let patch_b = write_create_file_patch(&mut objects, &author, "r159-b.txt", 0x77, blob_b)?;
        let claim_b = write_claim(
            &mut objects,
            &signer,
            ObjectId::from_bytes([0xF4; 32]),
            vec![patch_b],
        )?;
        reset_anchor_uses_for_test_support();
        let outcome = seal_from_accepted_claim(&layout, TARGET_REF, claim_b, &signer)?;
        let SealFromAcceptedOutcome::Sealed { block_id, .. } = outcome else {
            panic!("expected Sealed, got {outcome:?}");
        };
        let used = anchor_uses_for_test_support("seal-from-accepted");
        for other in ["seal", "merge", "worktree-write", "snapshot-materialize"] {
            assert_eq!(
                anchor_uses_for_test_support(other),
                0,
                "not the {other} site"
            );
        }
        cleanup(&layout);
        Ok((block_id, used))
    };
    let (anchored_block, anchored_uses) = seal_two("seal-from-accepted-159-anchored", false)?;
    let (full_block, full_uses) = seal_two("seal-from-accepted-159-full", true)?;
    assert_eq!(
        anchored_uses, 1,
        "the recorded, adopted checkpoint was the anchor"
    );
    assert_eq!(full_uses, 0, "with no record there is no anchor");
    assert_eq!(anchored_block, full_block, "the same block, byte for byte");
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
    // RFC 166 round 2 §0: valid ownership, set up before the raw append, so this fixture isolates
    // the "non-empty active WAL" precondition on its own -- without it, the commit-witness
    // classification's own row 9 (no durable owner) fires first, which is a different, already
    // separately tested condition.
    crate::commit_boundary::active::prepare_empty_active_ref_for_append(
        &fixture.layout,
        TARGET_REF,
    )?;
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

/// RFC 136 increment 2b §3: `sync seal --claim` seals through `seal_block_classified`, which records the
/// Block whose root it just computed.
#[test]
fn sealing_from_an_accepted_claim_records_the_sealed_block() -> Result<()> {
    let fixture = base_fixture("seal-from-accepted-records")?;
    assert!(
        crate::verified_blocks::load_verified_blocks(&fixture.layout).is_empty(),
        "fixture sanity: nothing recorded before the seal"
    );
    let outcome = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )?;
    let SealFromAcceptedOutcome::Sealed { block_id, .. } = outcome else {
        panic!("expected Sealed, got {outcome:?}");
    };
    assert!(
        crate::verified_blocks::load_verified_blocks(&fixture.layout).contains(&block_id),
        "sync seal recorded its Block"
    );
    cleanup(&fixture.layout);
    Ok(())
}

/// RFC 156 §5b: a claim held as several records — a superseding record adding a second MAINTAINER
/// signature — is enumerated **once**, so it cannot enter a `sync seal` batch twice.
#[test]
fn a_claim_held_as_several_records_is_enumerated_once() -> Result<()> {
    let layout = fresh_repo("seal-from-accepted-claim-superseded")?;
    let first = maintainer_signer(0x61)?;
    let second = maintainer_signer(0x62)?;
    let mut objects = FileObjectStore::new(layout.clone());
    let claim_id = write_claim(
        &mut objects,
        &first,
        ObjectId::from_bytes([0x5c; 32]),
        vec![ObjectId::from_bytes([0x5d; 32])],
    )?;
    let mut superseding = crate::ObjectReader::read_object(&objects, claim_id)?
        .ok_or_else(|| PrikkError::Integrity("claim written".to_string()))?;
    superseding.add_signature(maintainer_signature(
        &second,
        ObjectType::RecognitionClaim,
        claim_id,
    )?)?;
    crate::foundation::index::append_object_to_container(
        &layout,
        ObjectType::RecognitionClaim,
        &superseding,
    )?;

    let claims = super::enumerate_stored_claims(&layout)?;
    assert_eq!(
        claims.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![claim_id],
        "one claim, two records"
    );
    let _ = std::fs::remove_dir_all(layout.root());
    Ok(())
}

/// The fixture the patch-level reach tests share: three patches `p1`, `p2`, `p3` (files `a`, `b`,
/// `c`), the sender's grouping as claims -- `X1 = {p1, p2}` (root) and `X2 = {p3}` built on `X1` --
/// and a receiver that sealed `p1` and `p2` itself as two separate blocks, one claim each, so the
/// receiver's history holds both patches but no block named `X1`.
struct GroupingFixture {
    layout: RepositoryLayout,
    signer: Ed25519MaintainerSigner,
    x1: ObjectId,
    claim_x1: ObjectId,
    claim_x2: ObjectId,
}

fn divergent_grouping_fixture(name: &str, seal_p2: bool) -> Result<GroupingFixture> {
    let layout = fresh_repo(name)?;
    let signer = maintainer_signer(0x80)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0x81)?;
    let mut objects = FileObjectStore::new(layout.clone());
    let mut patch = |path: &str, seed: u8| -> Result<ObjectId> {
        let blob = write_blob(&mut objects, path.as_bytes())?;
        write_create_file_patch(&mut objects, &author, path, seed, blob)
    };
    let p1 = patch("grouping-a.txt", 0x82)?;
    let p2 = patch("grouping-b.txt", 0x83)?;
    let p3 = patch("grouping-c.txt", 0x84)?;
    let x1 = ObjectId::from_bytes([0xE1; 32]);
    let x2 = ObjectId::from_bytes([0xE2; 32]);
    // The sender's grouping.
    let claim_x1 = write_claim(&mut objects, &signer, x1, vec![p1, p2])?;
    let claim_x2 = write_claim_with_parents(&mut objects, &signer, x2, vec![p3], vec![x1])?;
    // The receiver's own grouping: one block per patch (ids that are not X1, by construction).
    let own_1 = write_claim(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xE3; 32]),
        vec![p1],
    )?;
    seal_from_accepted_claim(&layout, TARGET_REF, own_1, &signer)?;
    if seal_p2 {
        let own_2 = write_claim(
            &mut objects,
            &signer,
            ObjectId::from_bytes([0xE4; 32]),
            vec![p2],
        )?;
        seal_from_accepted_claim(&layout, TARGET_REF, own_2, &signer)?;
    }
    Ok(GroupingFixture {
        layout,
        signer,
        x1,
        claim_x1,
        claim_x2,
    })
}

fn tree_paths(layout: &RepositoryLayout) -> Result<Vec<String>> {
    let point = crate::resolve_point(layout, TARGET_REF, crate::ReceivedRefs::Read)?;
    let (listing, _) = crate::list_tree_at_point_reporting_anchor(layout, &point, None)?;
    let mut paths: Vec<String> = listing.entries.into_iter().map(|e| e.path).collect();
    paths.sort();
    Ok(paths)
}

/// **RFC 115 D7 / 0.57.0 step 0a, ruling 2.** The receiver groups the same patches differently
/// from the sender (`{p1}`, `{p2}` against `X1 = {p1, p2}`), so the sender's parent block `X1` is
/// not a block of the receiver's history. A literal block-id test would refuse; the patch-level
/// rule does not, because every patch of `X1` is in the history. The claim for `{p3}` seals, and
/// the tree is the sender's (`a`, `b`, `c`).
/// **Perturb:** make `parent_reached` answer only `history.blocks.contains(&parent)` -- this test
/// (and the CLI rows that need a held-claim parent) goes red.
#[test]
fn divergent_grouping_a_parent_whose_patches_are_all_in_the_history_is_reached() -> Result<()> {
    let f = divergent_grouping_fixture("seal-from-accepted-grouping-reached", true)?;
    let tip_before = current_tip(&f.layout)?.expect("the receiver has a tip");
    let outcome = seal_from_accepted_claim(&f.layout, TARGET_REF, f.claim_x2, &f.signer)?;
    let SealFromAcceptedOutcome::Sealed {
        block_id,
        patch_count,
        ..
    } = outcome
    else {
        panic!("expected Sealed, got {outcome:?}");
    };
    assert_eq!(patch_count, 1, "only p3 is new work");
    assert_ne!(Some(block_id), Some(tip_before));
    assert_eq!(current_tip(&f.layout)?, Some(block_id));
    assert_eq!(
        tree_paths(&f.layout)?,
        ["grouping-a.txt", "grouping-b.txt", "grouping-c.txt"],
        "the tree is the sender's"
    );
    cleanup(&f.layout);
    Ok(())
}

/// **The rule does not widen.** The same shape, but `p2` is not in the history: `X1`'s ancestry is
/// only partly there, so `X1` is not reached and the claim for `{p3}` refuses, writing nothing.
/// **Perturb:** make `parent_reached` answer `true` when *any* patch of the parent is in the
/// history (`any` for `all`) -- this test goes red.
#[test]
fn a_parent_with_only_some_of_its_patches_in_the_history_is_not_reached() -> Result<()> {
    let f = divergent_grouping_fixture("seal-from-accepted-grouping-partial", false)?;
    let tip_before = current_tip(&f.layout)?;
    let blocks_before = block_container_bytes(&f.layout)?;
    let err = seal_from_accepted_claim(&f.layout, TARGET_REF, f.claim_x2, &f.signer)
        .expect_err("a half-reached parent must refuse");
    let PrikkError::Precondition(message) = err else {
        panic!("expected a precondition refusal, got {err:?}");
    };
    assert!(
        message.contains(&format!("builds on block {}", f.x1)),
        "{message}"
    );
    assert!(message.contains("nothing was written"), "{message}");
    assert_eq!(current_tip(&f.layout)?, tip_before);
    assert_eq!(block_container_bytes(&f.layout)?, blocks_before);
    cleanup(&f.layout);
    Ok(())
}

/// **Ruling 3, at store level.** `X1`'s claim is held but its patches are not all in the history,
/// and no local branch holds `X1`: the refusal names the held claim, in the form `sync seal`
/// takes. With nothing held for the parent it names the carry instead.
/// **Perturb:** drop the `held_ancestry` arm from `parent_not_reached_refusal` -- the first half
/// goes red; make `held_unsealed_ancestry` answer `Some` for an unheld parent -- the second does.
#[test]
fn the_refusal_names_a_held_claim_for_the_parent_and_the_carry_when_none_is_held() -> Result<()> {
    let f = divergent_grouping_fixture("seal-from-accepted-grouping-names", false)?;
    let err = seal_from_accepted_claim(&f.layout, TARGET_REF, f.claim_x2, &f.signer)
        .expect_err("refuses");
    let PrikkError::Precondition(message) = err else {
        panic!("{err:?}")
    };
    assert!(
        message.contains(&format!(
            "prikk sync seal {TARGET_REF} --claim {}",
            f.claim_x1
        )),
        "the held claim for X1 is named: {message}"
    );
    assert!(
        !message.contains("sync have"),
        "no carry when the claim is held: {message}"
    );
    cleanup(&f.layout);

    // Nothing held for the parent: a claim whose parent block no claim or block here describes.
    let layout = fresh_repo("seal-from-accepted-grouping-unheld")?;
    let signer = maintainer_signer(0x85)?;
    adopt(&layout, &signer)?;
    let author = author_signer(0x86)?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob = write_blob(&mut objects, b"unheld")?;
    let patch = write_create_file_patch(&mut objects, &author, "unheld.txt", 0x87, blob)?;
    let unknown = ObjectId::from_bytes([0xE9; 32]);
    let claim = write_claim_with_parents(
        &mut objects,
        &signer,
        ObjectId::from_bytes([0xEA; 32]),
        vec![patch],
        vec![unknown],
    )?;
    let err = seal_from_accepted_claim(&layout, TARGET_REF, claim, &signer).expect_err("refuses");
    let PrikkError::Precondition(message) = err else {
        panic!("{err:?}")
    };
    assert!(
        message.contains("is not held on any local branch here"),
        "{message}"
    );
    assert!(
        message.contains("prikk sync have <that-branch> --output have.bin"),
        "{message}"
    );
    assert!(
        !message.contains("--claim "),
        "no held claim to name: {message}"
    );
    cleanup(&layout);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------------------
// 0.58.0 S2a: the per-claim refusals that had no test of their own (S1 §3). Each fires at the claim, with nothing published (the ref has
// no tip afterwards). The rest of the checklist is held where `s2a_sync_seal_corpus.rs`'s header lists it.
// ---------------------------------------------------------------------------------------------------------------------------------

/// `seal_from_accepted.rs:168`: a ref name that is not `heads/<name>`.
#[test]
fn s2a_an_invalid_ref_name_refuses_before_anything() -> Result<()> {
    let fixture = base_fixture("s2a-ref-name")?;
    let err = seal_from_accepted_claim(&fixture.layout, "topic", fixture.claim_id, &fixture.signer)
        .err()
        .ok_or_else(|| PrikkError::Integrity("an invalid ref name must refuse".to_string()))?;
    assert!(err.to_string().contains("expected heads/<name>"), "{err}");
    assert!(matches!(err, PrikkError::InvalidName(_)), "{err:?}");
    assert!(current_tip(&fixture.layout)?.is_none());
    cleanup(&fixture.layout);
    Ok(())
}

/// `seal_from_accepted.rs:186`: the named claim does not exist.
#[test]
fn s2a_a_claim_that_does_not_exist_refuses() -> Result<()> {
    let fixture = base_fixture("s2a-claim-missing")?;
    let missing = ObjectId::from_bytes([0xEE; 32]);
    let err = seal_from_accepted_claim(&fixture.layout, TARGET_REF, missing, &fixture.signer)
        .err()
        .ok_or_else(|| PrikkError::Integrity("a missing claim must refuse".to_string()))?;
    assert!(
        err.to_string()
            .contains(&format!("recognition claim {missing} does not exist")),
        "{err}"
    );
    assert!(current_tip(&fixture.layout)?.is_none());
    cleanup(&fixture.layout);
    Ok(())
}

/// `seal_from_accepted.rs:291` / `:298` (and `:287`, the commit-witness classification that speaks first): an active WAL with a torn
/// tail or a damaged record refuses with nothing sealed. The queue is made non-trivially owned first, as row 10 does.
#[test]
fn s2a_an_active_wal_with_a_torn_tail_refuses() -> Result<()> {
    let fixture = base_fixture("s2a-wal-tail")?;
    crate::commit_boundary::active::prepare_empty_active_ref_for_append(
        &fixture.layout,
        TARGET_REF,
    )?;
    let wal = Wal::for_layout(&fixture.layout, DEFAULT_ACTIVE_NAME);
    let author = author_signer(0x9C)?;
    let mut objects = FileObjectStore::new(fixture.layout.clone());
    let blob_id = write_blob(&mut objects, b"queued\n")?;
    wal.append_patch(&create_file_patch(&author, "queued.txt", 0x9D, blob_id)?)?;
    // A torn tail: a few bytes of the next record.
    let mut file = std::fs::OpenOptions::new().append(true).open(wal.path())?;
    std::io::Write::write_all(&mut file, &[0x50, 0x57, 0x41])?;
    drop(file);

    let err = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )
    .err()
    .ok_or_else(|| PrikkError::Integrity("a torn WAL tail must refuse".to_string()))?;
    assert!(
        err.to_string().contains("active WAL has 3 trailing partial bytes; run verify/doctor before sealing from an accepted claim"),
        "{err}"
    );
    assert!(current_tip(&fixture.layout)?.is_none());
    cleanup(&fixture.layout);
    Ok(())
}

/// `seal_from_accepted.rs:298`: a complete but damaged WAL record (two records; one byte flipped inside the first, so the second is a
/// sound record behind a damaged one). A flip inside a lone last record reads as a torn tail (`:291`), not as this.
#[test]
fn s2a_an_active_wal_with_a_damaged_record_refuses() -> Result<()> {
    let fixture = base_fixture("s2a-wal-damaged")?;
    crate::commit_boundary::active::prepare_empty_active_ref_for_append(
        &fixture.layout,
        TARGET_REF,
    )?;
    let wal = Wal::for_layout(&fixture.layout, DEFAULT_ACTIVE_NAME);
    let author = author_signer(0x9E)?;
    let mut objects = FileObjectStore::new(fixture.layout.clone());
    let first_blob = write_blob(&mut objects, b"queued one\n")?;
    wal.append_patch(&create_file_patch(&author, "one.txt", 0x9F, first_blob)?)?;
    let first_len = std::fs::metadata(wal.path())?.len() as usize;
    let second_blob = write_blob(&mut objects, b"queued two\n")?;
    wal.append_patch(&create_file_patch(&author, "two.txt", 0xA0, second_blob)?)?;
    let mut bytes = std::fs::read(wal.path())?;
    bytes[first_len / 2] ^= 0xff;
    std::fs::write(wal.path(), bytes)?;

    let err = seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )
    .err()
    .ok_or_else(|| PrikkError::Integrity("a damaged WAL record must refuse".to_string()))?;
    // The commit-witness classification (`:287`, RFC 166) speaks before the `:298` check, so this is its sentence.
    assert!(
        err.to_string().contains("active WAL has a damaged record"),
        "{err}"
    );
    assert!(current_tip(&fixture.layout)?.is_none());
    cleanup(&fixture.layout);
    Ok(())
}

/// `seal_from_accepted.rs:169` / `:247`: a ref whose RefState the store no longer holds is refused as an incomplete publication, before
/// `read_current_tip` (`:689-714`) can name it: those three messages are defensive, behind this check.
#[test]
fn s2a_a_ref_pointing_at_a_missing_ref_state_refuses_as_an_incomplete_publication() -> Result<()> {
    let fixture = base_fixture("s2a-tip-missing")?;
    // Seal once, so the ref has a tip; then lose the RefState objects and the index that names them.
    seal_from_accepted_claim(
        &fixture.layout,
        TARGET_REF,
        fixture.claim_id,
        &fixture.signer,
    )?;
    let ref_state_container = fixture
        .layout
        .container_slot_path(prikk_object::ObjectType::RefState, crate::ContainerSlot::A);
    std::fs::write(&ref_state_container, b"")?;
    std::fs::write(fixture.layout.container_index_path(), b"")?;

    let author = author_signer(0xA5)?;
    let mut objects = FileObjectStore::new(fixture.layout.clone());
    let blob_id = write_blob(&mut objects, b"second\n")?;
    let second_patch = write_create_file_patch(&mut objects, &author, "b.txt", 0xA6, blob_id)?;
    let second_claim = write_claim(
        &mut objects,
        &fixture.signer,
        ObjectId::from_bytes([0xAB; 32]),
        vec![second_patch],
    )?;
    let err = seal_from_accepted_claim(&fixture.layout, TARGET_REF, second_claim, &fixture.signer)
        .err()
        .ok_or_else(|| PrikkError::Integrity("a missing RefState must refuse".to_string()))?;
    assert!(
        err.to_string()
            .contains("repository mutation is blocked by incomplete ref publication"),
        "{err}"
    );
    cleanup(&fixture.layout);
    Ok(())
}

/// 0.58.0 S2b, "one fold": a batch of chained claims is derived **once** (the pre-batch state), however many claims it seals, and it
/// seals the same chain. The count is of `derive_candidate` calls on this thread. Before the fold each claim derived from its parent
/// (and the in-the-way simulation derived the grown list again for every claim): N claims made N or more calls.
#[test]
fn s2b_a_batch_of_chained_claims_is_derived_once() -> Result<()> {
    for claim_count in [5_usize, 66] {
        let fixture = base_fixture(&format!("s2b-one-fold-{claim_count}"))?;
        let author = author_signer(0x70)?;
        let mut objects = FileObjectStore::new(fixture.layout.clone());
        let mut claims = Vec::new();
        let mut previous_block = None;
        for index in 0..claim_count {
            let blob = write_blob(&mut objects, format!("file {index}\n").as_bytes())?;
            let patch = write_create_file_patch(
                &mut objects,
                &author,
                &format!("f{index}.txt"),
                0x71_u8.wrapping_add(u8::try_from(index % 100).unwrap_or(0)),
                blob,
            )?;
            let mut block = [0xB0_u8; 32];
            block[..8].copy_from_slice(&u64::try_from(index).unwrap_or(0).to_be_bytes());
            let block_id = ObjectId::from_bytes(block);
            claims.push(write_claim_with_parents(
                &mut objects,
                &fixture.signer,
                block_id,
                vec![patch],
                previous_block.into_iter().collect(),
            )?);
            previous_block = Some(block_id);
        }

        let lock = crate::lock::ActiveLock::acquire(&fixture.layout, DEFAULT_ACTIVE_NAME)?;
        let before = crate::block_state::derive_candidate_calls_for_test();
        let mut plan = super::plan_seal_batch(&fixture.layout, TARGET_REF, &claims);
        assert_eq!(plan.planned_claims(), claim_count, "every claim folds");
        for claim in &claims {
            let outcome = super::seal_from_accepted_claim_in_batch(
                &fixture.layout,
                &lock,
                TARGET_REF,
                *claim,
                &fixture.signer,
                &mut plan,
            )?;
            assert!(
                matches!(outcome, SealFromAcceptedOutcome::Sealed { .. }),
                "{outcome:?}"
            );
        }
        assert_eq!(
            crate::block_state::derive_candidate_calls_for_test() - before,
            1,
            "{claim_count} claims: the pre-batch state is derived once, and no claim derives from disk (a checkpoint Block is written \
             from the plan's kept state, not from a second derivation)"
        );
        assert_eq!(plan.planned_claims(), 0, "every step was consumed");
        drop(lock);
        cleanup(&fixture.layout);
    }
    Ok(())
}
