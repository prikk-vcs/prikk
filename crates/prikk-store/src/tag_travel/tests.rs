//! RFC 117 stage 3 §5: security properties, as refusals, for [`adopt_tag`]/[`create_local_tag`]/
//! [`received_tag_ids`]/[`list_received_tags`]. A two-repository harness throughout -- a sender that
//! creates and exports a tag, a receiver that accepts the artifact and (in most tests) already holds
//! an independently-built local block with the identical patch set, mirroring
//! `patch_set_digest/tests.rs`'s own "same patches, different block structure" discipline: a fixture
//! that builds both sides identically would not prove resolution works across two genuinely separate
//! repositories.

#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use prikk_error::Result;
use prikk_object::{BlockKind, ObjectId};

use super::{ReceivedTagResolution, TagSignatureVerification, adopt_tag, received_tag_ids};
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::object_store::ObjectWriteSession;
use crate::patch_exchange::{AcceptOptions, accept_exchange_artifact, export_exchange_artifact};
use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
use crate::test_gates::test_support::{
    signed_block, signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};
use crate::trust::{add_trusted_maintainer, load_maintainer_trust_policy};
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};

fn signer(discriminant: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(
        format!("tag-travel-fixture-{discriminant}"),
        &[discriminant; 32],
    )
}

fn public_key_hex(signer: &Ed25519MaintainerSigner) -> String {
    signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_block(
    store: &mut FileObjectStore,
    kind: BlockKind,
    parent_block_ids: Vec<ObjectId>,
    patch_ids: Vec<ObjectId>,
) -> Result<ObjectId> {
    let block = signed_block(kind, parent_block_ids, patch_ids, None);
    store.write_object(&block)
}

fn publish_branch(
    ref_store: &RefStore,
    ref_name: &str,
    target_block_id: ObjectId,
    update_seq: u64,
) -> Result<()> {
    let ref_state = signed_ref_state_envelope(ref_name, None, target_block_id, update_seq);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope(ref_name, None, ref_state_id, target_block_id, update_seq);
    ref_store.publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// Build a sender repository holding one block on `heads/main` and a real, stored `Tag` naming it
/// (`p1`/`p2` as its patch set), signed by a fixed sender key distinct from any receiver key any test
/// uses. Returns the layout, the digest/count the tag carries (so a receiver fixture can build an
/// independently-structured but identical-patch-set local block), and a `PEXCH002` artifact carrying
/// zero patches/claims but exactly this one tag -- the shape stage 3's own §2 change makes possible
/// (tag-travel with an otherwise-empty delta).
#[allow(clippy::type_complexity)]
fn sender_with_one_tag(
    root_name: &str,
    tag_ref: &str,
) -> Result<(RepositoryLayout, prikk_object::PatchSetDigest, u64, Vec<u8>)> {
    let root = unique_temp_dir(root_name);
    let layout = RepositoryLayout::init(root)?;
    let mut store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());

    let p1 = ObjectId::from_bytes([0xa1; 32]);
    let p2 = ObjectId::from_bytes([0xa2; 32]);
    let block = write_block(&mut store, BlockKind::Root, Vec::new(), vec![p1, p2])?;
    publish_branch(&ref_store, "heads/main", block, 1)?;

    let (digest, count) = compute_patch_set_digest_and_count_from_block(&store, block)?;
    let sender_signer = signer(0x51)?;
    let mut write_session = ObjectWriteSession::open(&layout)?;
    let created = super::create_local_tag(
        &layout,
        &mut write_session,
        tag_ref,
        block,
        None,
        digest,
        count,
        &sender_signer,
    )?;

    let (_, bytes) = export_exchange_artifact(&layout, &[], &[], &[created.tag_object_id])?;
    Ok((layout, digest, count, bytes))
}

/// A fresh, empty receiver repository, with `signer`'s own maintainer key trusted (so
/// `adopt_tag`/`verify_signer_trusted` can succeed once the rest of a test's own preconditions hold).
fn fresh_receiver(
    root_name: &str,
    receiver_signer: &Ed25519MaintainerSigner,
) -> Result<RepositoryLayout> {
    let root = unique_temp_dir(root_name);
    let layout = RepositoryLayout::init(root)?;
    add_trusted_maintainer(
        &layout,
        receiver_signer.key_id(),
        &public_key_hex(receiver_signer),
    )?;
    Ok(layout)
}

/// Give `layout` a local block holding exactly `p1`/`p2` (the same fixed patch ids
/// `sender_with_one_tag` uses), structured differently from the sender's own single-block shape (two
/// blocks instead of one) -- proof this is resolution across genuinely different repositories, not
/// two copies of the same one.
fn give_receiver_the_same_patch_set(layout: &RepositoryLayout) -> Result<()> {
    let mut store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());
    let p1 = ObjectId::from_bytes([0xa1; 32]);
    let p2 = ObjectId::from_bytes([0xa2; 32]);
    let genesis = write_block(&mut store, BlockKind::Root, Vec::new(), vec![p1])?;
    let tip = write_block(&mut store, BlockKind::Normal, vec![genesis], vec![p2])?;
    publish_branch(&ref_store, "heads/main", tip, 1)?;
    Ok(())
}

/// §5 row 1, the one that proves T4: accepting an artifact carrying a tag creates no local `tags/*`
/// ref and adopts no key -- absence, not merely a report field, per the handoff's own instruction.
#[test]
fn row1_accepting_a_tag_creates_no_local_ref_and_adopts_no_key() -> Result<()> {
    let (_, _, _, bytes) = sender_with_one_tag("rfc117-t3-row1-sender", "tags/v1")?;
    let receiver_signer = signer(0x61)?;
    let receiver = fresh_receiver("rfc117-t3-row1-receiver", &receiver_signer)?;
    let policy_before = load_maintainer_trust_policy(&receiver)?;

    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(report.tag_count, 1);

    let ref_store = RefStore::new(receiver.clone());
    assert!(
        ref_store.read_current_ref_state_id("tags/v1")?.is_none(),
        "accept must not create a local tags/* ref by itself"
    );
    let policy_after = load_maintainer_trust_policy(&receiver)?;
    assert_eq!(
        policy_before, policy_after,
        "accept must not change the adopted-maintainer-key set"
    );

    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §5 row 2: adoption refuses when the patch set is not held locally -- `NotHeld`, not a pick.
#[test]
fn row2_adoption_refuses_when_patch_set_is_not_held() -> Result<()> {
    let (_, _, _, bytes) = sender_with_one_tag("rfc117-t3-row2-sender", "tags/v1")?;
    let receiver_signer = signer(0x62)?;
    let receiver = fresh_receiver("rfc117-t3-row2-receiver", &receiver_signer)?;
    accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    // Deliberately never call `give_receiver_the_same_patch_set` -- the receiver holds the tag
    // object but not a block with its patch set.

    let error = adopt_tag(&receiver, "tags/v1", &receiver_signer).unwrap_err();
    assert!(
        error.to_string().contains("not held locally"),
        "expected a NotHeld-shaped refusal, got: {error}"
    );
    let ref_store = RefStore::new(receiver.clone());
    assert!(ref_store.read_current_ref_state_id("tags/v1")?.is_none());

    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §5 row 3: adoption refuses on ambiguity, naming candidates -- T2's own refusal, propagated
/// unchanged through `adopt_tag`, never softened into a pick.
#[test]
fn row3_adoption_refuses_on_ambiguity_naming_candidates() -> Result<()> {
    let (_, _, _, bytes) = sender_with_one_tag("rfc117-t3-row3-sender", "tags/v1")?;
    let receiver_signer = signer(0x63)?;
    let receiver = fresh_receiver("rfc117-t3-row3-receiver", &receiver_signer)?;
    accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;

    // Two local blocks, same patch set in a different order -- the identical technique
    // `patch_set_digest/tests.rs`'s own row 3 uses: same digest and count, distinct block ids.
    let mut store = FileObjectStore::new(receiver.clone());
    let ref_store = RefStore::new(receiver.clone());
    let p1 = ObjectId::from_bytes([0xa1; 32]);
    let p2 = ObjectId::from_bytes([0xa2; 32]);
    let block_forward = write_block(&mut store, BlockKind::Root, Vec::new(), vec![p1, p2])?;
    let block_reversed = write_block(&mut store, BlockKind::Root, Vec::new(), vec![p2, p1])?;
    assert_ne!(
        block_forward, block_reversed,
        "fixture sanity: patch order must actually change the block id"
    );
    publish_branch(&ref_store, "heads/forward", block_forward, 1)?;
    publish_branch(&ref_store, "heads/reversed", block_reversed, 1)?;

    let error = adopt_tag(&receiver, "tags/v1", &receiver_signer).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains(&block_forward.to_string())
            && message.contains(&block_reversed.to_string()),
        "the refusal must name both candidate blocks: {message}"
    );

    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §5 row 4: a tag from an unadopted signer is reported `Unverifiable`, and accept still succeeds --
/// signature outcome never gates the exchange (design T3/T6).
#[test]
fn row4_unverifiable_signer_is_reported_not_refused() -> Result<()> {
    let (_, _, _, bytes) = sender_with_one_tag("rfc117-t3-row4-sender", "tags/v1")?;
    let receiver_signer = signer(0x64)?;
    // Note: `fresh_receiver` trusts `receiver_signer`'s own key, never the sender's -- so the
    // sender's tag signature is unverifiable here by construction, without any extra fixture work.
    let receiver = fresh_receiver("rfc117-t3-row4-receiver", &receiver_signer)?;

    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(report.tag_count, 1);
    assert_eq!(report.tag_signature_outcomes.len(), 1);
    assert!(
        matches!(
            report.tag_signature_outcomes[0].1,
            TagSignatureVerification::Unverifiable { .. }
        ),
        "expected Unverifiable, got {:?}",
        report.tag_signature_outcomes[0].1
    );

    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §5 row 5, the one that proves the signature means what it says: the adopted tag is signed by the
/// receiver's own key, not the sender's -- a different object with the same global identity (T4).
#[test]
fn row5_adopted_tag_is_signed_by_the_receivers_own_key() -> Result<()> {
    let (_, digest, count, bytes) = sender_with_one_tag("rfc117-t3-row5-sender", "tags/v1")?;
    let receiver_signer = signer(0x65)?;
    let receiver = fresh_receiver("rfc117-t3-row5-receiver", &receiver_signer)?;
    accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    give_receiver_the_same_patch_set(&receiver)?;

    let created = adopt_tag(&receiver, "tags/v1", &receiver_signer)?;

    let store = FileObjectStore::new(receiver.clone());
    let envelope = crate::object_store::ObjectReader::read_typed(
        &store,
        created.tag_object_id,
        prikk_object::ObjectType::Tag,
    )?
    .expect("adopted Tag object must exist");
    let payload = prikk_object::TagPayload::decode_canonical(&envelope.canonical_payload)?;
    assert_eq!(payload.author_key_id, receiver_signer.key_id());
    assert_ne!(
        payload.author_key_id,
        signer(0x51)?.key_id(),
        "the adopted tag must not carry the sender's key id"
    );
    assert_eq!(payload.patch_set_digest, digest);
    assert_eq!(payload.patch_count, count);

    let ref_store = RefStore::new(receiver.clone());
    assert_eq!(
        ref_store.read_current_ref_state_id("tags/v1")?,
        Some(created.ref_state_id)
    );

    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §5 row 6: a refused exchange records no tag -- forced by a bad claim referencing a block the
/// receiver already holds with contradicting patch ids (`check_recognition_claim_consistency`'s own
/// refusal), a tag carried in the very same artifact.
#[test]
fn row6_a_refused_exchange_records_no_tag() -> Result<()> {
    let (sender_layout, _, _, _) = sender_with_one_tag("rfc117-t3-row6-sender", "tags/v1")?;
    let receiver_signer = signer(0x66)?;
    let receiver = fresh_receiver("rfc117-t3-row6-receiver", &receiver_signer)?;

    // The receiver already holds a block whose patch_ids a contradicting claim will lie about.
    let mut receiver_store = FileObjectStore::new(receiver.clone());
    let ref_store = RefStore::new(receiver.clone());
    let held_block = write_block(
        &mut receiver_store,
        BlockKind::Root,
        Vec::new(),
        vec![ObjectId::from_bytes([0xb1; 32])],
    )?;
    publish_branch(&ref_store, "heads/held", held_block, 1)?;

    // Re-export the sender's own tag alongside a hand-built claim that contradicts the block the
    // receiver already holds -- `export_exchange_artifact` has no claim-authoring API of its own
    // (claims are `sender.rs`'s job), so this claim is signed and written directly here.
    let sender_store = FileObjectStore::new(sender_layout.clone());
    let ref_store_sender = RefStore::new(sender_layout.clone());
    let tag_ref_state_id = ref_store_sender
        .read_current_ref_state_id("tags/v1")?
        .expect("sender's own tag ref must exist");
    let tag_ref_envelope = crate::object_store::ObjectReader::read_typed(
        &sender_store,
        tag_ref_state_id,
        prikk_object::ObjectType::RefState,
    )?
    .expect("sender's own tag RefState must exist");
    let tag_ref_payload = prikk_object::RefStatePayload::decode_canonical(
        &tag_ref_envelope.canonical_payload,
        tag_ref_envelope.schema_version,
    )?;
    let tag_object_id = tag_ref_payload.target_object_id;

    let claim_signer = signer(0x67)?;
    let claim_payload = prikk_object::RecognitionClaimPayload {
        block_id: held_block,
        patch_ids: vec![ObjectId::from_bytes([0xb2; 32])], // contradicts the held block's own p1
        parent_block_ids: Vec::new(),
    };
    let mut claim_envelope = prikk_object::ObjectEnvelope::unsigned(
        prikk_object::ObjectType::RecognitionClaim,
        1,
        prikk_object::CanonicalEncode::to_canonical_bytes(&claim_payload)?,
    );
    let claim_id = claim_envelope.object_id();
    claim_envelope.add_signature(crate::maintainer_signing::maintainer_signature(
        &claim_signer,
        prikk_object::ObjectType::RecognitionClaim,
        claim_id,
    )?)?;
    let mut sender_write_session = ObjectWriteSession::open(&sender_layout)?;
    let written_claim_id = sender_write_session.write_object(&claim_envelope)?;

    let (_, bytes) =
        export_exchange_artifact(&sender_layout, &[], &[written_claim_id], &[tag_object_id])?;

    let error =
        accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits()).unwrap_err();
    assert!(
        error.to_string().contains("contradicts"),
        "expected the contradiction refusal, got: {error}"
    );
    assert!(
        received_tag_ids(&receiver)?.is_empty(),
        "a refused exchange must record no tag"
    );

    let _ = std::fs::remove_dir_all(receiver.root());
    let _ = std::fs::remove_dir_all(sender_layout.root());
    Ok(())
}

/// §5 row 7: `PEXCH001` is refused by the new (`PEXCH002`) reader.
#[test]
fn row7_pexch001_is_refused() {
    let mut bytes = vec![0_u8; 64];
    bytes[..8].copy_from_slice(b"PEXCH001");
    let error = accept_exchange_artifact(
        &fresh_layout_for_magic_test(),
        &bytes,
        &AcceptOptions::default_limits(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("magic"),
        "expected a magic-mismatch refusal naming the format, got: {error}"
    );
}

fn fresh_layout_for_magic_test() -> RepositoryLayout {
    RepositoryLayout::init(unique_temp_dir("rfc117-t3-row7-receiver")).expect("layout inits")
}

/// `list_received_tags` reports resolution alongside the signature outcome -- a smoke test that the
/// three-state `ReceivedTagResolution` reads correctly end to end (Resolved case; `NotHeld`/ambiguous
/// are already exercised as refusals above, but `adopt_tag` never returns a value for them, so this
/// is the only place they are observed as *values* rather than as `Err`s).
#[test]
fn list_received_tags_reports_resolved_state() -> Result<()> {
    let (_, digest, count, bytes) = sender_with_one_tag("rfc117-t3-list-sender", "tags/v1")?;
    let receiver_signer = signer(0x68)?;
    let receiver = fresh_receiver("rfc117-t3-list-receiver", &receiver_signer)?;
    accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    give_receiver_the_same_patch_set(&receiver)?;

    let summaries = super::list_received_tags(&receiver)?;
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "tags/v1");
    assert!(matches!(
        summaries[0].resolution,
        ReceivedTagResolution::Resolved(_)
    ));
    assert!(matches!(
        summaries[0].signature_outcome,
        TagSignatureVerification::Unverifiable { .. }
    ));
    let _ = digest;
    let _ = count;

    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}
