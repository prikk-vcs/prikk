//! RFC 115 Stage 4 §6: the seal-from-accepted path's behaviour, and a negative control for every
//! row ("do not compress them" -- the handoff's own instruction). Each control mutates the single
//! narrowest line that should break the property under test.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, ChangePerm, CreateFile, DeleteNode, DeleteNodePreimage,
    NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose, RecognitionClaimPayload, RenamePath,
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

pub(in crate::seal_from_accepted) fn maintainer_signer(
    seed: u8,
) -> Result<Ed25519MaintainerSigner> {
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

pub(in crate::seal_from_accepted) fn write_claim_with_parents(
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
pub(in crate::seal_from_accepted) struct BaseFixture {
    pub(in crate::seal_from_accepted) layout: RepositoryLayout,
    pub(in crate::seal_from_accepted) signer: Ed25519MaintainerSigner,
    patch_id: ObjectId,
    claim_id: ObjectId,
}

pub(in crate::seal_from_accepted) fn base_fixture(name: &str) -> Result<BaseFixture> {
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

pub(in crate::seal_from_accepted) fn cleanup(layout: &RepositoryLayout) {
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

/// A `claim_count`-long chain of single-patch claims (each naming the previous as its parent) over `base_fixture`'s repository.
fn chained_batch_fixture(name: &str, claim_count: usize) -> Result<(BaseFixture, Vec<ObjectId>)> {
    let fixture = base_fixture(name)?;
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
    Ok((fixture, claims))
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
        let mut plan = super::plan_seal_batch(&fixture.layout, TARGET_REF, &claims, true);
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

/// 0.58.0 S3: a chained batch reads the stored claims **once** and walks the ref's history **once**, however many claims it seals.
/// Before S3 each claim enumerated the claim container two or three times (the ambiguity check, the parent check, the plan's mirror) and
/// walked the history twice. The counts are of whole-container enumerations and history walks on this thread.
#[test]
fn s3_a_batch_reads_the_claims_and_the_history_once() -> Result<()> {
    let (fixture, claims) = chained_batch_fixture("s3-reads-once", 6)?;
    let lock = crate::lock::ActiveLock::acquire(&fixture.layout, DEFAULT_ACTIVE_NAME)?;
    let before = super::batch_reads_for_test();
    let mut plan = super::SealBatchPlan::open(&fixture.layout, TARGET_REF, &claims)?;
    plan.require_parents_reached(&fixture.layout, &claims)?;
    plan.fold(&fixture.layout, &claims, true);
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
    let after = super::batch_reads_for_test();
    assert_eq!(
        (after.0 - before.0, after.1 - before.1),
        (1, 1),
        "one enumeration of the stored claims and one walk of the history, for {} claims",
        claims.len()
    );
    drop(lock);
    cleanup(&fixture.layout);
    Ok(())
}

/// 0.58.0 S6, part two: a batch builds its claims index (the by-block map and the by-patch lists the parent check and the order-ambiguity
/// check read) **once per claim set**, not once per claim: six claims, one build. A claim stored mid-batch re-reads the set and builds it
/// again, once (`s3_a_claim_stored_mid_batch_is_seen_by_the_next_claim` keeps passing).
#[test]
fn s6_a_batch_builds_its_claims_index_once() -> Result<()> {
    let (fixture, claims) = chained_batch_fixture("s6-index-once", 6)?;
    let lock = crate::lock::ActiveLock::acquire(&fixture.layout, DEFAULT_ACTIVE_NAME)?;
    let before = super::batch::claims_index_builds_for_test();
    let mut plan = super::SealBatchPlan::open(&fixture.layout, TARGET_REF, &claims)?;
    plan.require_parents_reached(&fixture.layout, &claims)?;
    plan.fold(&fixture.layout, &claims, true);
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
        super::batch::claims_index_builds_for_test() - before,
        1,
        "one index for {} claims",
        claims.len()
    );
    drop(lock);
    cleanup(&fixture.layout);
    Ok(())
}

/// 0.58.0 S6: the indexed order-ambiguity check answers as the whole-set check does -- the same refusal, naming the same (first stored) claim --
/// over claims that overlap in several ways: disjoint, agreeing, and two that disagree with the selected one (the first stored is named).
#[test]
fn s6_the_indexed_ambiguity_check_equals_the_whole_set_check() -> Result<()> {
    let id = |byte: u8| ObjectId::from_bytes([byte; 32]);
    let claim = |block: u8, patches: &[u8]| RecognitionClaimPayload {
        block_id: id(block),
        patch_ids: patches.iter().map(|byte| id(*byte)).collect(),
        parent_block_ids: Vec::new(),
    };
    let stored = vec![
        (id(101), claim(1, &[10, 11])),
        (id(102), claim(2, &[20, 21])),
        (id(103), claim(3, &[11, 10])), // disagrees with [10, 11]
        (id(104), claim(4, &[10, 12])),
        (id(105), claim(5, &[12, 11, 10])), // disagrees too, after 103
    ];
    let index = super::batch::ClaimsIndex::build(&stored);
    for selected in [
        claim(9, &[10, 11, 12]),
        claim(9, &[20, 21]),
        claim(9, &[21, 20]),
        claim(9, &[30]),
        claim(9, &[11, 12]),
    ] {
        for selected_id in [id(200), id(103)] {
            let whole = super::refuse_if_order_ambiguous_among(&stored, selected_id, &selected);
            let indexed = super::refuse_if_order_ambiguous_indexed(
                &stored,
                &index.by_patch,
                selected_id,
                &selected,
            );
            assert_eq!(
                whole.map_err(|error| error.to_string()),
                indexed.map_err(|error| error.to_string()),
                "selected {:?} as {selected_id}",
                selected.patch_ids
            );
        }
    }
    Ok(())
}

/// 0.58.0 S6, rule 2: a batch writes the replay-verified record **once**, when it ends, and a later claim sees an earlier claim's entry in
/// the meantime. After each sealed claim its Block is in `load_verified_blocks` (what a disk derivation anchors on) while the record file is
/// still not there; `finish_records` writes it; and what is written is exactly the Blocks the batch sealed (plus none the batch did not
/// confirm: every id is a Block this repository holds).
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn s6_a_batch_writes_the_record_once_and_a_later_claim_sees_an_earlier_entry() -> Result<()> {
    let (fixture, claims) = chained_batch_fixture("s6-record-once", 5)?;
    let lock = crate::lock::ActiveLock::acquire(&fixture.layout, DEFAULT_ACTIVE_NAME)?;
    let mut plan = super::SealBatchPlan::open(&fixture.layout, TARGET_REF, &claims)?;
    plan.require_parents_reached(&fixture.layout, &claims)?;
    plan.fold(&fixture.layout, &claims, true);
    let record = crate::verified_blocks::record_path(&fixture.layout);
    let mut sealed = Vec::new();
    for claim in &claims {
        let outcome = super::seal_from_accepted_claim_in_batch(
            &fixture.layout,
            &lock,
            TARGET_REF,
            *claim,
            &fixture.signer,
            &mut plan,
        )?;
        let SealFromAcceptedOutcome::Sealed { block_id, .. } = outcome else {
            panic!("{outcome:?}");
        };
        sealed.push(block_id);
        assert!(
            crate::verified_blocks::load_verified_blocks(&fixture.layout).contains(&block_id),
            "the entry is visible to the next claim"
        );
        assert!(!record.exists(), "and nothing is written yet");
    }
    plan.finish_records();
    let recorded = crate::verified_blocks::load_verified_blocks(&fixture.layout);
    assert!(record.exists(), "written when the batch ends");
    for block_id in &sealed {
        assert!(recorded.contains(block_id));
    }
    let objects = FileObjectStore::new(fixture.layout.clone());
    for id in recorded.iter() {
        assert!(
            crate::ObjectReader::has_object(&objects, *id, ObjectType::Block)?,
            "every recorded id is a Block this repository holds"
        );
    }
    drop(lock);
    cleanup(&fixture.layout);
    Ok(())
}

/// 0.58.0 S3, rule 1: a claim stored **mid-batch** is seen by the next claim's checks, as a per-claim enumeration saw it. `sync accept`
/// cannot add one while a batch holds the active lock, but `sync build` stores claims for the repository's own blocks under the
/// object-store lock alone. Here a claim that names the second claim's patches in the other order is stored after the first claim is
/// sealed: the second claim must refuse as ambiguous (the claim container changed, so the batch re-read it), at that claim, with the
/// first claim's seal kept.
#[test]
fn s3_a_claim_stored_mid_batch_is_seen_by_the_next_claim() -> Result<()> {
    let fixture = base_fixture("s3-mid-batch-claim")?;
    let author = author_signer(0x80)?;
    let mut objects = FileObjectStore::new(fixture.layout.clone());
    let blob_a = write_blob(&mut objects, b"a\n")?;
    let blob_b = write_blob(&mut objects, b"b\n")?;
    let blob_c = write_blob(&mut objects, b"c\n")?;
    let first = write_create_file_patch(&mut objects, &author, "first.txt", 0x81, blob_a)?;
    let second_a = write_create_file_patch(&mut objects, &author, "second_a.txt", 0x82, blob_b)?;
    let second_b = write_create_file_patch(&mut objects, &author, "second_b.txt", 0x83, blob_c)?;
    let first_block = ObjectId::from_bytes([0xC1; 32]);
    let second_block = ObjectId::from_bytes([0xC2; 32]);
    let first_claim = write_claim(&mut objects, &fixture.signer, first_block, vec![first])?;
    let second_claim = write_claim_with_parents(
        &mut objects,
        &fixture.signer,
        second_block,
        vec![second_a, second_b],
        vec![first_block],
    )?;
    let claims = vec![first_claim, second_claim];

    let lock = crate::lock::ActiveLock::acquire(&fixture.layout, DEFAULT_ACTIVE_NAME)?;
    let mut plan = super::SealBatchPlan::open(&fixture.layout, TARGET_REF, &claims)?;
    plan.require_parents_reached(&fixture.layout, &claims)?;
    plan.fold(&fixture.layout, &claims, true);
    let sealed = super::seal_from_accepted_claim_in_batch(
        &fixture.layout,
        &lock,
        TARGET_REF,
        first_claim,
        &fixture.signer,
        &mut plan,
    )?;
    assert!(matches!(sealed, SealFromAcceptedOutcome::Sealed { .. }));

    // What a concurrent `sync build` would store: a claim about a local block that holds the second claim's patches in the other order.
    let _ambiguous = write_claim(
        &mut objects,
        &fixture.signer,
        ObjectId::from_bytes([0xC3; 32]),
        vec![second_b, second_a],
    )?;

    let refused = super::seal_from_accepted_claim_in_batch(
        &fixture.layout,
        &lock,
        TARGET_REF,
        second_claim,
        &fixture.signer,
        &mut plan,
    )
    .err()
    .ok_or_else(|| {
        PrikkError::Integrity("the mid-batch claim must refuse the second claim".to_string())
    })?;
    assert!(
        refused
            .to_string()
            .contains("disagree on their relative order"),
        "{refused}"
    );
    assert!(
        current_tip(&fixture.layout)?.is_some(),
        "the first claim's seal is kept"
    );
    drop(lock);
    cleanup(&fixture.layout);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------------------------
// 0.59.0 P5a1: the fold holds one state at a time, and the claim drops its read snapshot before it writes.
// ---------------------------------------------------------------------------------------------------------------------------------------

/// One claim of a shaped batch: a file created at a new path (the tree gains a path), a mode change of the first file (the tree stays the
/// same size), or a mode change of a node that does not exist (a claim whose patches cannot apply), or a path that differs only in case from
/// the first file's (a claim whose state collides).
#[derive(Clone, Copy)]
enum BatchStep {
    Create,
    TogglePerm,
    UnknownNodePerm,
    CollidingCreate,
}

fn perm_patch(
    signer: &Ed25519AuthorSigner,
    node_seed: u8,
    old_mode: u32,
    new_mode: u32,
    message: &str,
) -> Result<ObjectEnvelope> {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id: NodeId::from_bytes([node_seed; 32]),
                old_mode,
                new_mode,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: Some(message.to_string()),
    };
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        prikk_object::PATCH_MESSAGE_SCHEMA,
        payload.to_canonical_bytes()?,
    );
    let id = envelope.object_id();
    envelope.add_signature(author_signature(signer, id)?)?;
    Ok(envelope)
}

/// A chain of single-patch claims (each naming the previous as its parent) of the given steps, over `base_fixture`'s repository. The first
/// `Create` is the node every `TogglePerm` changes.
fn shaped_batch_fixture(name: &str, steps: &[BatchStep]) -> Result<(BaseFixture, Vec<ObjectId>)> {
    let fixture = base_fixture(name)?;
    let author = author_signer(0x70)?;
    let mut objects = FileObjectStore::new(fixture.layout.clone());
    let mut claims = Vec::new();
    let mut previous_block = None;
    let mut creates = 0_u8;
    let mut exec = false;
    for (index, step) in steps.iter().enumerate() {
        let blob = write_blob(&mut objects, format!("file {index}\n").as_bytes())?;
        let patch = match step {
            BatchStep::Create => {
                let seed = 0x71_u8.wrapping_add(creates);
                let path = format!("f{creates}.txt");
                creates += 1;
                write_create_file_patch(&mut objects, &author, &path, seed, blob)?
            }
            BatchStep::CollidingCreate => {
                write_create_file_patch(&mut objects, &author, "F0.TXT", 0xE0, blob)?
            }
            BatchStep::TogglePerm => {
                let (old, new) = if exec {
                    (0o100_755, 0o100_644)
                } else {
                    (0o100_644, 0o100_755)
                };
                exec = !exec;
                objects.write_object(&perm_patch(
                    &author,
                    0x71,
                    old,
                    new,
                    &format!("toggle {index}"),
                )?)?
            }
            BatchStep::UnknownNodePerm => objects.write_object(&perm_patch(
                &author,
                0xEE,
                0o100_644,
                0o100_755,
                &format!("unknown {index}"),
            )?)?,
        };
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
    Ok((fixture, claims))
}

/// What sealing a batch left: the outcomes up to the first refusal, the tip, the Block container's bytes, and what the plan counted.
struct BatchRun {
    sealed: usize,
    refusal: Option<String>,
    tip: Option<ObjectId>,
    blocks: Vec<u8>,
    counts: (usize, usize),
    retained_peak: usize,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    index_copies: usize,
    planned_after_fold: usize,
    judged_entries: Option<usize>,
    steps_capacity: usize,
}

/// Seal `claims` in order onto `ref_name`, stopping at the first refusal, as `sync seal` does. `plan`: `None` seals every claim from disk
/// (no plan: the semantics every batch had before 0.58.0), `Some(current_branch)` plans the batch as the CLI does.
fn run_batch(
    fixture: &BaseFixture,
    claims: &[ObjectId],
    ref_name: &str,
    plan: Option<bool>,
) -> Result<BatchRun> {
    // A batch holds the active lock for its whole length; the disk path takes it claim by claim itself.
    let lock = plan
        .map(|_| crate::lock::ActiveLock::acquire(&fixture.layout, DEFAULT_ACTIVE_NAME))
        .transpose()?;
    let mut planned = match plan {
        Some(current_branch) => {
            let mut planned = super::SealBatchPlan::open(&fixture.layout, ref_name, claims)?;
            planned.require_parents_reached(&fixture.layout, claims)?;
            planned.fold(&fixture.layout, claims, current_branch);
            Some(planned)
        }
        None => None,
    };
    let planned_after_fold = planned
        .as_ref()
        .map_or(0, super::SealBatchPlan::planned_claims);
    let judged_entries = planned
        .as_ref()
        .and_then(super::SealBatchPlan::final_state_entries);
    let _ = crate::block_state::batch_fold::retained_states_peak_for_test();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let copies_before = crate::object_store::index_data_copies_for_test();
    let mut sealed = 0;
    let mut refusal = None;
    for claim in claims {
        let outcome = match (planned.as_mut(), lock.as_ref()) {
            (Some(plan), Some(lock)) => super::seal_from_accepted_claim_in_batch(
                &fixture.layout,
                lock,
                ref_name,
                *claim,
                &fixture.signer,
                plan,
            ),
            _ => {
                super::seal_from_accepted_claim(&fixture.layout, ref_name, *claim, &fixture.signer)
            }
        };
        match outcome {
            Ok(SealFromAcceptedOutcome::Sealed { .. }) => sealed += 1,
            Ok(other) => panic!("{other:?}"),
            Err(error) => {
                refusal = Some(error.to_string());
                break;
            }
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let index_copies = crate::object_store::index_data_copies_for_test() - copies_before;
    let retained_peak = crate::block_state::batch_fold::retained_states_peak_for_test();
    let counts = planned
        .as_ref()
        .map_or((0, 0), super::SealBatchPlan::sealed_counts);
    let steps_capacity = planned
        .as_ref()
        .map_or(0, super::SealBatchPlan::steps_capacity);
    drop(planned);
    drop(lock);
    let ref_store = RefStore::new(fixture.layout.clone());
    let tip = match ref_store.read_current_ref_state_id(ref_name)? {
        Some(ref_state_id) => {
            let object_store = FileObjectStore::new(fixture.layout.clone());
            let envelope = crate::object_store::ObjectReader::read_typed(
                &object_store,
                ref_state_id,
                ObjectType::RefState,
            )?
            .expect("ref state must exist");
            Some(
                prikk_object::RefStatePayload::decode_canonical(
                    &envelope.canonical_payload,
                    envelope.schema_version,
                )?
                .target_object_id,
            )
        }
        None => None,
    };
    Ok(BatchRun {
        sealed,
        refusal,
        tip,
        blocks: block_container_bytes(&fixture.layout)?,
        counts,
        retained_peak,
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        index_copies,
        planned_after_fold,
        judged_entries,
        steps_capacity,
    })
}

fn growing_steps(claims: usize) -> Vec<BatchStep> {
    vec![BatchStep::Create; claims]
}

fn fixed_steps(claims: usize) -> Vec<BatchStep> {
    let mut steps = vec![BatchStep::Create];
    steps.extend(std::iter::repeat_n(BatchStep::TogglePerm, claims - 1));
    steps
}

/// The batch's bytes with the running fold equal the batch's bytes from disk, in both shapes (a tree that gains a path per claim, a tree that
/// does not), on the current branch and on another, over a batch long enough to cross a checkpoint (the 65th Block); no claim of a normal batch
/// falls back to disk; and **a control that can fail**: with the last entry of each checkpoint's snapshot left out the bytes differ.
#[test]
fn p5a1_the_running_fold_seals_the_bytes_the_disk_path_seals() -> Result<()> {
    for (shape, steps) in [("growing", growing_steps(66)), ("fixed", fixed_steps(66))] {
        for (ref_name, current_branch) in [
            (TARGET_REF, true),
            (TARGET_REF, false),
            ("heads/other", false),
        ] {
            let case = format!("{shape}-{}-{current_branch}", ref_name.replace('/', "-"));
            let (disk_fixture, disk_claims) =
                shaped_batch_fixture(&format!("p5a1-disk-{case}"), &steps)?;
            let disk = run_batch(&disk_fixture, &disk_claims, ref_name, None)?;
            let (fixture, claims) = shaped_batch_fixture(&format!("p5a1-plan-{case}"), &steps)?;
            let run = run_batch(&fixture, &claims, ref_name, Some(current_branch))?;
            assert_eq!(
                disk.sealed, 66,
                "{case}: the disk path seals every claim ({:?})",
                disk.refusal
            );
            assert_eq!(run.sealed, 66, "{case}");
            assert_eq!(run.refusal, None, "{case}");
            assert_eq!(run.tip, disk.tip, "{case}: the same tip");
            assert_eq!(
                run.blocks, disk.blocks,
                "{case}: the same Blocks, byte for byte"
            );
            assert_eq!(
                run.counts,
                (66, 0),
                "{case}: every claim sealed from the plan's running fold, none from disk"
            );
            assert_eq!(
                run.steps_capacity, 0,
                "{case}: a finished plan keeps no capacity of its steps buffer"
            );
            assert!(
                run.retained_peak >= 1,
                "{case}: the batch crossed a checkpoint, so a state was kept for a snapshot"
            );
            cleanup(&fixture.layout);
            cleanup(&disk_fixture.layout);
        }
        // The control: the same batch with each checkpoint's snapshot short of its last entry must not equal the disk path.
        let (disk_fixture, disk_claims) =
            shaped_batch_fixture(&format!("p5a1-ctl-disk-{shape}"), &steps)?;
        let disk = run_batch(&disk_fixture, &disk_claims, TARGET_REF, None)?;
        let (fixture, claims) = shaped_batch_fixture(&format!("p5a1-ctl-plan-{shape}"), &steps)?;
        let perturbed =
            crate::block_state::batch_fold::with_snapshot_entry_dropped_for_test(|| {
                run_batch(&fixture, &claims, TARGET_REF, Some(true))
            })?;
        assert_ne!(
            perturbed.blocks, disk.blocks,
            "{shape}: the comparison above can fail"
        );
        cleanup(&fixture.layout);
        cleanup(&disk_fixture.layout);
    }
    Ok(())
}

/// The fold holds **at most one checkpoint state at a time** (not a timing: a count of live states). Three checkpoints (the 1st, 65th and
/// 129th Blocks), and the plan itself holds no state per step.
#[test]
fn p5a1_the_batch_holds_one_checkpoint_state_at_a_time() -> Result<()> {
    for current_branch in [true, false] {
        let (fixture, claims) = shaped_batch_fixture(
            &format!("p5a1-one-state-{current_branch}"),
            &growing_steps(130),
        )?;
        let run = run_batch(&fixture, &claims, TARGET_REF, Some(current_branch))?;
        assert_eq!(run.sealed, 130);
        assert_eq!(
            run.retained_peak, 1,
            "130 claims cross three checkpoints; one state is alive at a time (current branch: {current_branch})"
        );
        assert_eq!(run.counts, (130, 0), "none sealed from disk");
        cleanup(&fixture.layout);
    }
    Ok(())
}

/// The up-front apply-only pass runs on the current branch only: it is what the in-the-way check judges, and nothing else uses it.
#[test]
fn p5a1_the_state_the_check_judges_is_built_for_the_current_branch_only() -> Result<()> {
    let steps = growing_steps(5);
    let (fixture, claims) = shaped_batch_fixture("p5a1-judged-current", &steps)?;
    let current = run_batch(&fixture, &claims, TARGET_REF, Some(true))?;
    assert_eq!(
        current.judged_entries,
        Some(5),
        "the state after all five claims"
    );
    cleanup(&fixture.layout);
    let (fixture, claims) = shaped_batch_fixture("p5a1-judged-other", &steps)?;
    let other = run_batch(&fixture, &claims, TARGET_REF, Some(false))?;
    assert_eq!(
        other.judged_entries, None,
        "no state is built for a branch that is not the current one"
    );
    assert_eq!(other.planned_after_fold, 5, "every claim is still planned");
    cleanup(&fixture.layout);
    Ok(())
}

/// A claim whose patches cannot apply, or whose state collides, ends the plan there, as the 0.58.0 fold did: the state the check judges is the
/// one after the last claim that folded, the claims before it seal from the plan, and the claim itself is refused from disk with the
/// refusal the disk path gives (the same text with and without a plan, on the current branch and not).
#[test]
fn p5a1_a_claim_that_will_not_fold_ends_the_plan_there() -> Result<()> {
    for (label, bad) in [
        ("unapplicable", BatchStep::UnknownNodePerm),
        ("colliding", BatchStep::CollidingCreate),
    ] {
        let steps = [BatchStep::Create, BatchStep::Create, bad, BatchStep::Create];
        let (disk_fixture, disk_claims) =
            shaped_batch_fixture(&format!("p5a1-bad-disk-{label}"), &steps)?;
        let disk = run_batch(&disk_fixture, &disk_claims, TARGET_REF, None)?;
        assert_eq!(
            disk.sealed, 2,
            "{label}: the disk path seals two and refuses the third"
        );
        let refusal = disk.refusal.clone().expect("the third claim is refused");
        for current_branch in [true, false] {
            let (fixture, claims) =
                shaped_batch_fixture(&format!("p5a1-bad-{label}-{current_branch}"), &steps)?;
            let run = run_batch(&fixture, &claims, TARGET_REF, Some(current_branch))?;
            assert_eq!(run.sealed, 2, "{label} {current_branch}");
            assert_eq!(
                run.refusal.as_deref(),
                Some(refusal.as_str()),
                "{label} {current_branch}: the same refusal"
            );
            assert_eq!(
                run.blocks, disk.blocks,
                "{label} {current_branch}: the same Blocks"
            );
            assert_eq!(
                run.counts,
                (2, 0),
                "{label} {current_branch}: the claim that does not fold is refused before a Block is sealed"
            );
            if current_branch {
                assert_eq!(
                    run.planned_after_fold, 2,
                    "{label}: the plan ends at the claim that does not fold"
                );
                assert_eq!(
                    run.judged_entries,
                    Some(2),
                    "{label}: the state the check judges is the one after the second claim"
                );
            } else {
                assert_eq!(run.judged_entries, None);
            }
            cleanup(&fixture.layout);
        }
        cleanup(&disk_fixture.layout);
    }
    Ok(())
}

/// A claim's write extends the one kept object index in place: the claim's read snapshot is gone before the write session opens, so
/// `Arc::make_mut` finds no other holder and copies nothing. Before 0.59.0 every claim of a batch copied the whole index (two live copies
/// at the peak).
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn p5a1_a_claim_does_not_copy_the_object_index() -> Result<()> {
    let (fixture, claims) = shaped_batch_fixture("p5a1-index-copies", &growing_steps(8))?;
    let run = run_batch(&fixture, &claims, TARGET_REF, Some(true))?;
    assert_eq!(run.sealed, 8);
    assert_eq!(
        run.index_copies, 0,
        "no copy-on-write copy of the index during the batch"
    );
    cleanup(&fixture.layout);
    Ok(())
}

/// The shape of the real-kill sweep `sync_seal_checkpoint`: a base of 62 sealed Blocks and a batch of three claims, which crosses a checkpoint
/// (the 65th Block carries a snapshot), so the sweep kills a batch whose running fold hands a state to a snapshot write. The pin that the shape
/// does what the sweep's name says.
#[test]
fn p5a1_a_short_batch_onto_62_blocks_crosses_a_checkpoint() -> Result<()> {
    let (fixture, claims) = shaped_batch_fixture("p5a1-short-batch", &growing_steps(65))?;
    let base = run_batch(&fixture, &claims[..62], TARGET_REF, Some(true))?;
    assert_eq!(base.sealed, 62);
    assert_eq!(base.retained_peak, 1, "the first Block is a checkpoint");
    let batch = run_batch(&fixture, &claims[62..], TARGET_REF, Some(true))?;
    assert_eq!(batch.sealed, 3);
    assert_eq!(
        batch.counts,
        (3, 0),
        "all three sealed from the running fold"
    );
    assert_eq!(
        batch.retained_peak, 1,
        "the third claim's Block is the 65th: a checkpoint inside a batch of three"
    );
    cleanup(&fixture.layout);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------------------------
// 0.59.0 P5a1b: the state the in-the-way check judges is exactly the 0.58.0 fold's, collision by collision.
// ---------------------------------------------------------------------------------------------------------------------------------------

/// One claim of a scripted batch: create a file (`seed` names its node), delete that node, or rename it.
#[derive(Clone, Copy)]
enum ScriptOp {
    Create(&'static str, u8),
    Delete(u8),
    Rename(u8, &'static str),
}

fn op_patch(
    signer: &Ed25519AuthorSigner,
    kind: OperationKind,
    message: &str,
) -> Result<ObjectEnvelope> {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind,
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: Some(message.to_string()),
    };
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        prikk_object::PATCH_MESSAGE_SCHEMA,
        payload.to_canonical_bytes()?,
    );
    let id = envelope.object_id();
    envelope.add_signature(author_signature(signer, id)?)?;
    Ok(envelope)
}

/// A chain of single-patch claims, one per `ScriptOp`, over `base_fixture`'s repository.
fn scripted_batch_fixture(name: &str, script: &[ScriptOp]) -> Result<(BaseFixture, Vec<ObjectId>)> {
    let fixture = base_fixture(name)?;
    let author = author_signer(0x70)?;
    let mut objects = FileObjectStore::new(fixture.layout.clone());
    let mut claims = Vec::new();
    let mut previous_block = None;
    let mut files: std::collections::BTreeMap<u8, (String, ObjectId)> =
        std::collections::BTreeMap::new();
    for (index, op) in script.iter().enumerate() {
        let message = format!("script {index}");
        let kind = match *op {
            ScriptOp::Create(path, seed) => {
                let blob = write_blob(&mut objects, format!("file {index}\n").as_bytes())?;
                files.insert(seed, (path.to_string(), blob));
                OperationKind::CreateFile(CreateFile {
                    path: path.to_string(),
                    node_id: NodeId::from_bytes([seed; 32]),
                    blob_id: blob,
                    mode: 0o100_644,
                })
            }
            ScriptOp::Delete(seed) => {
                let (path, blob) = files.get(&seed).cloned().expect("a created node");
                OperationKind::DeleteNode(DeleteNode {
                    path,
                    node_id: NodeId::from_bytes([seed; 32]),
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: blob,
                        old_mode: 0o100_644,
                    },
                })
            }
            ScriptOp::Rename(seed, to) => {
                let entry = files.get_mut(&seed).expect("a created node");
                let old_path = std::mem::replace(&mut entry.0, to.to_string());
                OperationKind::RenamePath(RenamePath {
                    node_id: NodeId::from_bytes([seed; 32]),
                    old_path,
                    new_path: to.to_string(),
                })
            }
        };
        let patch = objects.write_object(&op_patch(&author, kind, &message)?)?;
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
    Ok((fixture, claims))
}

/// (claims kept, entries judged) by the quick way and by the 0.58.0 way, how many claims a real batch sealed, and its refusal.
type JudgedBothWays = (
    (usize, Option<usize>),
    (usize, Option<usize>),
    usize,
    Option<String>,
);

/// The plan's judged state for a script, by the quick way and by the 0.58.0 way (every claim's entries and the policy over all of them),
/// and what a real batch seals: (claims kept, entries judged) each, and how many claims the loop sealed before it refused.
fn judged_both_ways(name: &str, script: &[ScriptOp]) -> Result<JudgedBothWays> {
    let mut results = Vec::new();
    for exact in [false, true] {
        let (fixture, claims) = scripted_batch_fixture(&format!("{name}-{exact}"), script)?;
        super::batch::FORCE_EXACT.with(|flag| flag.set(exact));
        let run = run_batch(&fixture, &claims, TARGET_REF, Some(true));
        super::batch::FORCE_EXACT.with(|flag| flag.set(false));
        let run = run?;
        results.push((
            (run.planned_after_fold, run.judged_entries),
            run.sealed,
            run.refusal,
        ));
        cleanup(&fixture.layout);
    }
    let exact = results.pop().expect("two runs");
    let quick = results.pop().expect("two runs");
    assert_eq!(quick.1, exact.1, "{name}: the same claims seal");
    assert_eq!(quick.2, exact.2, "{name}: the same refusal");
    Ok((quick.0, exact.0, quick.1, quick.2))
}

/// The state judged by the in-the-way check is the 0.58.0 fold's, in each way a claim's state can collide: collide then heal (the case P5a1's
/// quick way got wrong), collide and stay, a rename into a collision, the removal of one of a pair, and the pairs the policy folds beyond ASCII.
/// Each is compared with the 0.58.0 way run on the same batch, and pinned to what it must be.
#[test]
fn p5a1b_the_judged_state_is_the_0_58_0_fold_collision_by_collision() -> Result<()> {
    use ScriptOp::{Create, Delete, Rename};
    // (name, script, claims kept, entries judged)
    let cases: Vec<(&str, Vec<ScriptOp>, usize, Option<usize>)> = vec![
        (
            "heal",
            vec![
                Create("a.txt", 1),
                Create("b.txt", 2),
                Create("A.TXT", 3),
                Delete(3),
                Create("c.txt", 4),
            ],
            2,
            Some(2),
        ),
        (
            "stay",
            vec![Create("a.txt", 1), Create("A.TXT", 2), Create("c.txt", 3)],
            1,
            Some(1),
        ),
        (
            "rename-into",
            vec![
                Create("a.txt", 1),
                Create("b.txt", 2),
                Rename(2, "A.TXT"),
                Create("c.txt", 4),
            ],
            2,
            Some(2),
        ),
        (
            "remove-one-of-a-pair",
            vec![
                Create("a.txt", 1),
                Create("A.TXT", 2),
                Delete(1),
                Create("c.txt", 4),
            ],
            1,
            Some(1),
        ),
        (
            "sharp-s",
            vec![
                Create("x", 1),
                Create("stra\u{df}e", 2),
                Create("strasse", 3),
                Delete(3),
                Create("y", 4),
            ],
            2,
            Some(2),
        ),
        (
            "normalization",
            vec![Create("\u{e9}", 1), Create("e\u{301}", 2), Create("z", 3)],
            1,
            Some(1),
        ),
        (
            "a-rename-that-heals-nothing",
            vec![
                Create("a.txt", 1),
                Create("b.txt", 2),
                Rename(2, "c.txt"),
                Rename(1, "d.txt"),
            ],
            4,
            Some(2),
        ),
        ("no-collision", vec![Create("a.txt", 1)], 1, Some(1)),
    ];
    for (name, script, kept, entries) in cases {
        let (quick, exact, sealed, refusal) = judged_both_ways(&format!("p5a1b-{name}"), &script)?;
        assert_eq!(
            quick, exact,
            "{name}: the quick way judges what the 0.58.0 fold judged"
        );
        assert_eq!(quick, (kept, entries), "{name}");
        assert_eq!(sealed, kept, "{name}: the loop seals the same claims");
        assert_eq!(
            refusal.is_some(),
            kept < script.len(),
            "{name}: a claim that collides is refused, from disk: {refusal:?}"
        );
    }
    Ok(())
}
