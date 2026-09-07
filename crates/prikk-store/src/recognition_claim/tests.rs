//! RFC 115 Stage 2 tests for `check_recognition_claim_consistency`. §7 rows 3 and 4. RFC 116 N3's
//! own §7 rows 3 and 4 (the field-discriminator pair) live at the bottom of this file.

use prikk_object::{
    BlockKind, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RecognitionClaimPayload,
};

use super::{
    ContradictedField, RecognitionClaimConsistency, check_recognition_claim_consistency,
    order_claims_for_sealing,
};
use crate::test_gates::test_support::{maintainer_signature, signed_block, unique_temp_dir};
use crate::trust::load_maintainer_trust_policy;
use crate::{FileObjectStore, ObjectWriter, RepositoryLayout};

/// A signed (dummy-signature) `RecognitionClaim` envelope, written directly -- `order_claims_for_
/// sealing` never checks a claim's signature, only its `block_id`/`parent_block_ids`, so the fixed
/// dummy signature `patch_set_digest/tests.rs`'s own fixtures use is enough here.
fn write_claim(
    objects: &mut FileObjectStore,
    block_id: ObjectId,
    patch_ids: Vec<ObjectId>,
    parent_block_ids: Vec<ObjectId>,
) -> prikk_error::Result<ObjectId> {
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
    envelope.add_signature(maintainer_signature())?;
    objects.write_object(&envelope)
}

/// §7 row 4: a claim about a block the receiver does not hold is *accepted*, reading `BlockAbsent`
/// -- not an error, and not something that reads as a degraded case. This is the expected shape of
/// real exchange (design §3), not an edge case.
#[test]
fn claim_about_an_absent_block_reads_block_absent() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc115-recognition-claim-absent");
    let layout = RepositoryLayout::init(root.clone())?;
    let store = FileObjectStore::new(layout);
    let claim = RecognitionClaimPayload {
        block_id: ObjectId::from_bytes([0x71; 32]),
        patch_ids: vec![ObjectId::from_bytes([0x72; 32])],
        parent_block_ids: Vec::new(),
    };
    let outcome = check_recognition_claim_consistency(&store, &claim)?;
    assert_eq!(outcome, RecognitionClaimConsistency::BlockAbsent);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// §7 row 3, and design §3's own required refinement: a claim contradicting a block the receiver
/// *does* hold is a detected lie, reported with both sides named.
#[test]
fn claim_contradicting_a_held_block_is_contradicted() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc115-recognition-claim-contradicted");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let real_patch = ObjectId::from_bytes([0x73; 32]);
    let block = signed_block(BlockKind::Normal, Vec::new(), vec![real_patch], None);
    let block_id = store.write_object(&block)?;

    let lying_patch = ObjectId::from_bytes([0x74; 32]);
    let claim = RecognitionClaimPayload {
        block_id,
        patch_ids: vec![lying_patch],
        parent_block_ids: Vec::new(),
    };
    let outcome = check_recognition_claim_consistency(&store, &claim)?;
    assert_eq!(
        outcome,
        RecognitionClaimConsistency::Contradicted {
            field: ContradictedField::PatchIds,
            claimed: vec![lying_patch],
            actual: vec![real_patch],
        }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// §4.1's replacement for the withdrawn unsorted-claim refusal (D6, `RFC-115-stage-4-ordering-
/// investigation-v1.md`): a claim carrying a block's own `patch_ids` verbatim -- unsorted, since
/// that is now the normal case -- round-trips through encode/decode with order preserved, and then
/// reads `Consistent` against the block it truthfully describes. Two patches, sealed in
/// *descending* id order (authoring order, never sorted by `ObjectId`), claimed in that same
/// descending order.
#[test]
fn claim_carrying_the_blocks_own_verbatim_order_round_trips_and_reads_consistent()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc115-recognition-claim-verbatim-round-trip");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let low = ObjectId::from_bytes([0x75; 32]);
    let high = ObjectId::from_bytes([0x76; 32]);
    let block = signed_block(BlockKind::Normal, Vec::new(), vec![high, low], None);
    let block_id = store.write_object(&block)?;

    let claim = RecognitionClaimPayload {
        block_id,
        patch_ids: vec![high, low],
        parent_block_ids: Vec::new(),
    };
    let bytes = claim.to_canonical_bytes()?;
    let decoded = RecognitionClaimPayload::decode_canonical(&bytes)?;
    assert_eq!(
        decoded.patch_ids,
        vec![high, low],
        "order must survive the round trip unchanged, not be re-sorted"
    );

    let outcome = check_recognition_claim_consistency(&store, &decoded)?;
    assert_eq!(outcome, RecognitionClaimConsistency::Consistent);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// §4.2's inversion (D6): a claim listing a held block's own patches, but in a *different* order
/// than the block actually sealed them, is now `Contradicted` -- exactly the case a prior version
/// of this test asserted stayed `Consistent`, under the withdrawn set-equality contract.
///
/// A block is content-addressed, so the same `block_id` names the same canonical payload, therefore
/// the same `patch_ids` sequence. An honest claim about a block the receiver genuinely holds
/// therefore matches it *in order*, always -- there is no honest way to name the right block and
/// the right patches in the wrong order. A differently-ordered claim about a held block cannot
/// arise from honesty; only from a lie or a lossy claim format. So sequence equality cannot produce
/// a false accusation; it can only detect one -- an order-lie the sorted-set contract could not.
#[test]
fn claim_permuting_a_held_blocks_own_order_is_contradicted() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc115-recognition-claim-permuted-contradicted");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let low = ObjectId::from_bytes([0x75; 32]);
    let high = ObjectId::from_bytes([0x76; 32]);
    // Block's own patch_ids deliberately descending -- authoring order, not sorted.
    let block = signed_block(BlockKind::Normal, Vec::new(), vec![high, low], None);
    let block_id = store.write_object(&block)?;

    // The claim lists the block's own two real patches, but ascending -- the wrong order.
    let claim = RecognitionClaimPayload {
        block_id,
        patch_ids: vec![low, high],
        parent_block_ids: Vec::new(),
    };
    let outcome = check_recognition_claim_consistency(&store, &claim)?;
    assert_eq!(
        outcome,
        RecognitionClaimConsistency::Contradicted {
            field: ContradictedField::PatchIds,
            claimed: vec![low, high],
            actual: vec![high, low],
        }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// §7 row 7: trust never expands across a claim check -- neither an absent-block claim, nor a
/// contradicted one, nor a consistent one may change the repository's adopted-key set. Checked
/// directly against `load_maintainer_trust_policy`, before and after each of the three outcomes.
#[test]
fn checking_a_recognition_claim_never_changes_the_adopted_key_set() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc115-recognition-claim-trust-inert");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    // A receiver with an already-adopted maintainer -- the realistic case, and the one where an
    // unwanted expansion (a second key silently appearing) would actually be observable.
    crate::trust::add_trusted_maintainer(
        &layout,
        "rfc115-trust-inert-maintainer",
        "1111111111111111111111111111111111111111111111111111111111111111",
    )?;

    let patch = ObjectId::from_bytes([0x77; 32]);
    let block = signed_block(BlockKind::Normal, Vec::new(), vec![patch], None);
    let block_id = store.write_object(&block)?;

    let before = load_maintainer_trust_policy(&layout)?;

    let absent_claim = RecognitionClaimPayload {
        block_id: ObjectId::from_bytes([0x78; 32]),
        patch_ids: vec![patch],
        parent_block_ids: Vec::new(),
    };
    check_recognition_claim_consistency(&store, &absent_claim)?;
    assert_eq!(load_maintainer_trust_policy(&layout)?, before);

    let contradicted_claim = RecognitionClaimPayload {
        block_id,
        patch_ids: vec![ObjectId::from_bytes([0x79; 32])],
        parent_block_ids: Vec::new(),
    };
    check_recognition_claim_consistency(&store, &contradicted_claim)?;
    assert_eq!(load_maintainer_trust_policy(&layout)?, before);

    let consistent_claim = RecognitionClaimPayload {
        block_id,
        patch_ids: vec![patch],
        parent_block_ids: Vec::new(),
    };
    check_recognition_claim_consistency(&store, &consistent_claim)?;
    assert_eq!(load_maintainer_trust_policy(&layout)?, before);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 116 N3 §7 row 3: a claim whose `parent_block_ids` disagree with a held block's own is
/// `Contradicted`, naming the parent field specifically -- not reported as a patch disagreement,
/// even though `patch_ids` themselves match exactly.
#[test]
fn claim_with_wrong_parents_is_contradicted_naming_the_parent_field() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc116-recognition-claim-parent-contradicted");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let real_parent = ObjectId::from_bytes([0x81; 32]);
    let patch = ObjectId::from_bytes([0x82; 32]);
    let block = signed_block(BlockKind::Normal, vec![real_parent], vec![patch], None);
    let block_id = store.write_object(&block)?;

    let lying_parent = ObjectId::from_bytes([0x83; 32]);
    let claim = RecognitionClaimPayload {
        block_id,
        patch_ids: vec![patch],
        parent_block_ids: vec![lying_parent],
    };
    let outcome = check_recognition_claim_consistency(&store, &claim)?;
    assert_eq!(
        outcome,
        RecognitionClaimConsistency::Contradicted {
            field: ContradictedField::ParentBlockIds,
            claimed: vec![lying_parent],
            actual: vec![real_parent],
        }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 116 N3 §7 row 4: a claim whose `patch_ids` disagree with a held block still reports as a
/// *patch* mismatch, even when the claim carries a real, matching `parent_block_ids` -- the field
/// discriminator must not garble or default to the wrong field just because both are present.
#[test]
fn claim_with_wrong_patches_still_reports_as_a_patch_mismatch() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc116-recognition-claim-patch-mismatch-with-parents");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let real_parent = ObjectId::from_bytes([0x84; 32]);
    let real_patch = ObjectId::from_bytes([0x85; 32]);
    let block = signed_block(BlockKind::Normal, vec![real_parent], vec![real_patch], None);
    let block_id = store.write_object(&block)?;

    let lying_patch = ObjectId::from_bytes([0x86; 32]);
    let claim = RecognitionClaimPayload {
        block_id,
        patch_ids: vec![lying_patch],
        parent_block_ids: vec![real_parent],
    };
    let outcome = check_recognition_claim_consistency(&store, &claim)?;
    assert_eq!(
        outcome,
        RecognitionClaimConsistency::Contradicted {
            field: ContradictedField::PatchIds,
            claimed: vec![lying_patch],
            actual: vec![real_patch],
        }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 116 stage 5 §4 row 1: a two-block batch seals parent-first regardless of the input order --
/// the child's claim is listed *before* the parent's own in the call, and the sort must still put
/// the parent first.
#[test]
fn order_claims_row1_two_block_batch_seals_parent_first_regardless_of_input_order()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc116-order-row1");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let patch = ObjectId::from_bytes([0x01; 32]);
    let parent_block = ObjectId::from_bytes([0x10; 32]);
    let child_block = ObjectId::from_bytes([0x20; 32]);

    let parent_claim = write_claim(&mut store, parent_block, vec![patch], Vec::new())?;
    let child_claim = write_claim(&mut store, child_block, vec![patch], vec![parent_block])?;

    // Deliberately child-first in the call -- the sort must still put the parent first.
    let order = order_claims_for_sealing(&store, &[child_claim, parent_claim])?;
    assert_eq!(order, vec![parent_claim, child_claim]);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 116 stage 5 §4 row 2 / §6 item 2: ordering is by `parent_block_ids`, not by artifact or id
/// order. The parent block's own id is constructed larger than the child's, and -- checked, not
/// assumed -- the resulting *claim* ids are confirmed to disagree with topological order too, or
/// this test could not tell a real topological sort from an incidental id-sort (the exact trap
/// that made the `parent_block_ids` control a no-op two increments ago).
#[test]
fn order_claims_row2_ordering_is_by_parent_block_ids_not_artifact_or_id_order()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc116-order-row2");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let patch = ObjectId::from_bytes([0x01; 32]);

    let parent_block = ObjectId::from_bytes([0xF0; 32]);
    let child_block = ObjectId::from_bytes([0x00; 32]);
    assert!(
        parent_block > child_block,
        "fixture must put the parent's own block id after the child's"
    );

    let parent_claim = write_claim(&mut store, parent_block, vec![patch], Vec::new())?;
    let child_claim = write_claim(&mut store, child_block, vec![patch], vec![parent_block])?;
    assert!(
        parent_claim > child_claim,
        "fixture must also produce a parent claim id that sorts after the child claim id -- \
         confirmed here, not assumed, or this test cannot distinguish a real topological sort \
         from sorting by claim id: parent={parent_claim} child={child_claim}"
    );

    let order = order_claims_for_sealing(&store, &[parent_claim, child_claim])?;
    assert_eq!(
        order,
        vec![parent_claim, child_claim],
        "the parent's claim must still come first, even though both the block ids and the claim \
         ids themselves sort the other way"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 116 stage 5 §4 row 3: a parent named outside the batch (an already-sealed ancestor, or
/// simply absent) is ignored for ordering, not refused -- the ordinary incremental case.
#[test]
fn order_claims_row3_parents_outside_the_batch_are_ignored_not_refused() -> prikk_error::Result<()>
{
    let root = unique_temp_dir("rfc116-order-row3");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let patch = ObjectId::from_bytes([0x01; 32]);
    let already_sealed_elsewhere = ObjectId::from_bytes([0xAB; 32]); // not any claim's block_id
    let block = ObjectId::from_bytes([0x30; 32]);

    let claim = write_claim(
        &mut store,
        block,
        vec![patch],
        vec![already_sealed_elsewhere],
    )?;

    let order = order_claims_for_sealing(&store, &[claim])?;
    assert_eq!(order, vec![claim]);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 116 stage 5 §4 row 4: a hostile cycle -- claim P asserts block B's parent is C, claim Q
/// asserts C's parent is B -- is refused, naming both claims. Asserted directly, not via a
/// timeout: the sort is bounded by the batch size and cannot hang.
#[test]
fn order_claims_row4_a_hostile_cycle_is_refused_naming_both_claims() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc116-order-row4");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let patch = ObjectId::from_bytes([0x01; 32]);
    let block_b = ObjectId::from_bytes([0x40; 32]);
    let block_c = ObjectId::from_bytes([0x50; 32]);

    let claim_p = write_claim(&mut store, block_b, vec![patch], vec![block_c])?;
    let claim_q = write_claim(&mut store, block_c, vec![patch], vec![block_b])?;

    let result = order_claims_for_sealing(&store, &[claim_p, claim_q]);
    let message = match result {
        Ok(order) => panic!("a hostile cycle must be refused, got an order: {order:?}"),
        Err(error) => error.to_string(),
    };
    assert!(
        message.contains(&claim_p.to_string()) && message.contains(&claim_q.to_string()),
        "the refusal must name both claims in the cycle: {message}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 116 stage 5 §4 row 5: the order is deterministic across repeated runs over the identical
/// batch -- two independent chains with no edge between them, so more than one topologically
/// valid interleaving exists, and the tie-break must pick the same one every time.
#[test]
fn order_claims_row5_the_order_is_deterministic_across_runs() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc116-order-row5");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout);
    let patch = ObjectId::from_bytes([0x01; 32]);

    let chain_a_root = write_claim(
        &mut store,
        ObjectId::from_bytes([0x61; 32]),
        vec![patch],
        Vec::new(),
    )?;
    let chain_a_leaf = write_claim(
        &mut store,
        ObjectId::from_bytes([0x62; 32]),
        vec![patch],
        vec![ObjectId::from_bytes([0x61; 32])],
    )?;
    let chain_b_root = write_claim(
        &mut store,
        ObjectId::from_bytes([0x71; 32]),
        vec![patch],
        Vec::new(),
    )?;
    let chain_b_leaf = write_claim(
        &mut store,
        ObjectId::from_bytes([0x72; 32]),
        vec![patch],
        vec![ObjectId::from_bytes([0x71; 32])],
    )?;
    let batch = [chain_b_leaf, chain_a_leaf, chain_b_root, chain_a_root];

    let first = order_claims_for_sealing(&store, &batch)?;
    for _ in 0..4 {
        let repeat = order_claims_for_sealing(&store, &batch)?;
        assert_eq!(
            repeat, first,
            "repeated runs over the identical batch must agree"
        );
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
