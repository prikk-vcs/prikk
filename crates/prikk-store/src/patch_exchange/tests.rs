//! RFC 115 Stage 3 §5: `accepted_but_unsealed_patch_ids` -- the derived query, tested directly.
//! 0.56.0 step 1 part 1a: `pending_patches_with_claims`'s own claim join, tested directly too.
//! 0.56.0 step 3g: the A1 ruling's redefinition of `pending` -- `patch_ids_pending` and
//! `pending_patches_with_claims` against the design review's own five-object table.

#![allow(clippy::indexing_slicing)]

use prikk_error::Result;
use prikk_object::{BlockKind, ObjectId, SignerRole};

use super::{
    PendingClaimSignatureOutcome, accepted_but_unsealed_patch_ids, patch_ids_pending,
    pending_patches_with_claims,
};
use crate::add_trusted_maintainer;
use crate::patch_set_digest::patch_ids_pending_disabling_held_block_subtraction_for_test;
use crate::test_gates::test_support::{
    signed_block, signed_patch_blob_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    FileObjectStore, MaintainerSigner, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    remove_received_pointer, write_received_pointer_for_test,
};

#[test]
fn an_empty_repository_has_nothing_accepted_but_unsealed() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-empty");
    let layout = RepositoryLayout::init(root.clone())?;
    assert_eq!(accepted_but_unsealed_patch_ids(&layout)?, Vec::new());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_patch_sealed_into_a_ref_reachable_block_is_not_unsealed() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-sealed");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = objects.write_object(&child_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", None, ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;

    assert_eq!(accepted_but_unsealed_patch_ids(&layout)?, Vec::new());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_patch_written_but_reachable_from_no_ref_is_accepted_but_unsealed() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-orphan");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    // No block, no ref -- exactly what an accept path (§1's scope cut) leaves behind: a Patch
    // object present in the store, reachable from nothing.
    assert_eq!(accepted_but_unsealed_patch_ids(&layout)?, vec![patch_id]);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn one_sealed_and_one_unsealed_patch_are_told_apart() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-mixed");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let sealed_patch = signed_patch_envelope();
    let sealed_patch_id = objects.write_object(&sealed_patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let child_block = signed_block(
        BlockKind::Normal,
        vec![root_block_id],
        vec![sealed_patch_id],
        None,
    );
    let child_block_id = objects.write_object(&child_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", None, ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;

    // A second, distinct Patch object -- a different path and a different signer, so its id
    // genuinely differs from `sealed_patch_id` -- written directly, sealed into nothing.
    let signer = super::exchange_test_support::author_signer(0x61)?;
    let unsealed_blob = super::exchange_test_support::signed_blob_envelope(b"unsealed fixture\n")?;
    let unsealed_blob_id = objects.write_object(&unsealed_blob)?;
    let unsealed_patch = super::exchange_test_support::signed_author_patch_envelope(
        &signer,
        "unsealed.txt",
        0x62,
        unsealed_blob_id,
    )?;
    let unsealed_patch_id = objects.write_object(&unsealed_patch)?;
    assert_ne!(
        unsealed_patch_id, sealed_patch_id,
        "fixture sanity: distinct content"
    );

    assert_eq!(
        accepted_but_unsealed_patch_ids(&layout)?,
        vec![unsealed_patch_id]
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn pending_claims_an_empty_queue_has_no_entries() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-empty");
    let layout = RepositoryLayout::init(root.clone())?;
    assert_eq!(pending_patches_with_claims(&layout)?, Vec::new());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Design review v2's A1 ruling, object 2 ("accepted, no claim (hand-made only)") -- and, for the
/// listing column, classification-identical to object 5 ("a refused or crashed import's
/// leftover"): both are a held Patch with no held claim and no sealing Block, so both read
/// `unclaimed`, not `pending` -- 0.56.0 step 3g. Before this round, this exact fixture appeared
/// with `claims: []`; now it does not appear in this listing at all.
#[test]
fn pending_claims_a_patch_no_claim_names_is_not_listed() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-no-claim");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let author = super::exchange_test_support::author_signer(0x7A)?;
    let blob_id = objects.write_object(&super::exchange_test_support::signed_blob_envelope(
        b"no claim\n",
    )?)?;
    let _patch_id =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "e.txt", 0x7B, blob_id,
        )?)?;

    assert_eq!(
        pending_patches_with_claims(&layout)?,
        Vec::new(),
        "an unclaimed patch must not appear in the listing at all, never claims: []"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn pending_claims_one_claim_naming_two_pending_patches_appears_on_both() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-two-patches");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());

    let signer = super::exchange_test_support::maintainer_signer(0x70)?;
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &super::exchange_test_support::public_key_hex(&signer),
    )?;

    let author = super::exchange_test_support::author_signer(0x71)?;
    let blob_id = objects.write_object(&super::exchange_test_support::signed_blob_envelope(
        b"one claim two patches\n",
    )?)?;
    let patch_1 =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "a.txt", 0x72, blob_id,
        )?)?;
    let patch_2 =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "b.txt", 0x73, blob_id,
        )?)?;

    let block_id = ObjectId::from_bytes([0xBB; 32]);
    let claim_id = objects.write_object(&super::exchange_test_support::signed_claim_envelope(
        &signer,
        block_id,
        vec![patch_1, patch_2],
    )?)?;

    let entries = pending_patches_with_claims(&layout)?;
    assert_eq!(
        entries.len(),
        2,
        "both patches must be pending: {entries:?}"
    );
    for entry in &entries {
        assert_eq!(
            entry.claims.len(),
            1,
            "patch {} must show exactly one claim",
            entry.patch_id
        );
        let claim_entry = &entry.claims[0];
        assert_eq!(claim_entry.claim_id, claim_id);
        assert_eq!(claim_entry.target_block_id, block_id);
        assert_eq!(
            claim_entry.parent_block_ids,
            Vec::<ObjectId>::new(),
            "this fixture's claim names no parent -- `signed_claim_envelope`'s own default"
        );
        assert_eq!(
            claim_entry.signature,
            PendingClaimSignatureOutcome::Sound {
                key_id: signer.key_id().to_string()
            }
        );
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn pending_claims_two_claims_sharing_one_pending_patch_appear_together_once() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-shared-patch");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());

    let signer = super::exchange_test_support::maintainer_signer(0x74)?;
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &super::exchange_test_support::public_key_hex(&signer),
    )?;

    let author = super::exchange_test_support::author_signer(0x75)?;
    let blob_id = objects.write_object(&super::exchange_test_support::signed_blob_envelope(
        b"shared patch\n",
    )?)?;
    let patch_id =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "c.txt", 0x76, blob_id,
        )?)?;

    let block_a = ObjectId::from_bytes([0xC1; 32]);
    let block_b = ObjectId::from_bytes([0xC2; 32]);
    let claim_a_id = objects.write_object(&super::exchange_test_support::signed_claim_envelope(
        &signer,
        block_a,
        vec![patch_id],
    )?)?;
    let claim_b_id = objects.write_object(&super::exchange_test_support::signed_claim_envelope(
        &signer,
        block_b,
        vec![patch_id],
    )?)?;

    let entries = pending_patches_with_claims(&layout)?;
    assert_eq!(
        entries.len(),
        1,
        "the shared patch must appear exactly once: {entries:?}"
    );
    let claims = &entries[0].claims;
    assert_eq!(claims.len(), 2, "both claims must be listed: {claims:?}");
    let mut expected = vec![claim_a_id, claim_b_id];
    expected.sort_unstable();
    let actual: Vec<ObjectId> = claims.iter().map(|claim| claim.claim_id).collect();
    assert_eq!(actual, expected, "claims must be sorted by claim id");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn pending_claims_an_unverifiable_signer_is_reported_without_refusing() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-unverifiable");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());

    // Never adopted -- this repository has not trusted this key.
    let signer = super::exchange_test_support::maintainer_signer(0x77)?;

    let author = super::exchange_test_support::author_signer(0x78)?;
    let blob_id = objects.write_object(&super::exchange_test_support::signed_blob_envelope(
        b"unverifiable\n",
    )?)?;
    let patch_id =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "d.txt", 0x79, blob_id,
        )?)?;

    let block_id = ObjectId::from_bytes([0xD0; 32]);
    objects.write_object(&super::exchange_test_support::signed_claim_envelope(
        &signer,
        block_id,
        vec![patch_id],
    )?)?;

    let entries = pending_patches_with_claims(&layout)?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].claims.len(), 1);
    assert_eq!(
        entries[0].claims[0].signature,
        PendingClaimSignatureOutcome::Unverifiable {
            key_id: signer.key_id().to_string()
        }
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The forged-claim-under-a-now-trusted-identity case `verify_claim_signature`'s own doc calls "an
/// integrity failure, not a trust question" -- every *gate* (`accept_exchange_artifact`,
/// `seal_from_accepted_claim`) still refuses on it via that function's own `Err`. This reader
/// does not: 0.56.0 step 1a2's own ruling is that a listing must show the operator which claim
/// failed, not hide the whole listing behind one forged claim's refusal, so
/// `pending_patches_with_claims` maps this `Err` to `PendingClaimSignatureOutcome::Invalid` for
/// that one claim instead. Built by relabelling a genuine signature's own `key_id` to an
/// *adopted* one it was never produced for, rather than generating a signature with mismatched
/// bytes directly -- `maintainer_signature` has no seam for the latter, and this produces the
/// identical, real failure: a public key that does not verify the bytes presented under its name.
#[test]
fn pending_claims_a_forged_signature_under_an_adopted_key_is_reported_invalid() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-forged");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());

    let real_signer = super::exchange_test_support::maintainer_signer(0x7C)?;
    let other_signer = super::exchange_test_support::maintainer_signer(0x7D)?;
    add_trusted_maintainer(
        &layout,
        other_signer.key_id(),
        &super::exchange_test_support::public_key_hex(&other_signer),
    )?;

    let author = super::exchange_test_support::author_signer(0x7E)?;
    let blob_id = objects.write_object(&super::exchange_test_support::signed_blob_envelope(
        b"forged\n",
    )?)?;
    let patch_id =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "f.txt", 0x7F, blob_id,
        )?)?;

    let block_id = ObjectId::from_bytes([0xE0; 32]);
    let mut claim = super::exchange_test_support::signed_claim_envelope(
        &real_signer,
        block_id,
        vec![patch_id],
    )?;
    for signature in &mut claim.signatures {
        if signature.signer_role == SignerRole::Maintainer {
            signature.key_id = other_signer.key_id().to_string();
        }
    }
    objects.write_object(&claim)?;

    let entries = pending_patches_with_claims(&layout)?;
    assert_eq!(entries.len(), 1, "the listing must not refuse: {entries:?}");
    assert_eq!(entries[0].claims.len(), 1);
    match &entries[0].claims[0].signature {
        PendingClaimSignatureOutcome::Invalid { key_id, reason } => {
            assert_eq!(key_id.as_deref(), Some(other_signer.key_id()));
            assert!(
                reason.contains("does not verify against adopted key"),
                "reason must name the real refusal text: {reason:?}"
            );
        }
        other => panic!("a forged signature under an adopted key must report Invalid: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// 0.56.0 step 1b2, R1: a claim naming more than one parent block (a merge-shaped claim) reports
/// every one of them, in the payload's own order -- not refused, and not truncated to one. Unlike
/// `extract --claim` (which refuses such a claim outright, having no `mainline_parent_id` to pick
/// a mainline from), a listing has no such need: it just reports what the claim states.
#[test]
fn pending_claims_a_merge_shaped_claim_reports_every_parent_block_id() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-merge");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());

    let signer = super::exchange_test_support::maintainer_signer(0x80)?;
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &super::exchange_test_support::public_key_hex(&signer),
    )?;

    let author = super::exchange_test_support::author_signer(0x81)?;
    let blob_id = objects.write_object(&super::exchange_test_support::signed_blob_envelope(
        b"merge claim\n",
    )?)?;
    let patch_id =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "m.txt", 0x82, blob_id,
        )?)?;

    let block_id = ObjectId::from_bytes([0xF0; 32]);
    let first_parent = ObjectId::from_bytes([0xF1; 32]);
    let second_parent = ObjectId::from_bytes([0xF2; 32]);
    let claim_id = objects.write_object(
        &super::exchange_test_support::signed_claim_envelope_with_parents(
            &signer,
            block_id,
            vec![patch_id],
            vec![first_parent, second_parent],
        )?,
    )?;

    let entries = pending_patches_with_claims(&layout)?;
    assert_eq!(entries.len(), 1, "the listing must not refuse: {entries:?}");
    assert_eq!(entries[0].claims.len(), 1);
    let claim_entry = &entries[0].claims[0];
    assert_eq!(claim_entry.claim_id, claim_id);
    assert_eq!(claim_entry.target_block_id, block_id);
    assert_eq!(
        claim_entry.parent_block_ids,
        vec![first_parent, second_parent],
        "both parents must be reported, in the payload's own order, not refused or truncated"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// 0.56.0 step 1a2, R2 (owner ruling 2026-10-29, "Exclude received history"); still true under
/// 0.56.0 step 3g's redefinition, now for a different reason: a patch reachable only from a
/// **received** pointer (never adopted into a local ref) is not pending, even though
/// `accepted_but_unsealed_patch_ids` -- the raw query `seal_from_accepted_claim` still uses
/// unchanged -- still reports it. The received history's own Block is held (`archive import`
/// writes it into the local object store even for a `remotes/*` landing), so
/// `patch_ids_named_by_any_held_block`'s subtraction excludes it -- no received-pointer walk
/// needed at all any more. Both queries are asserted here to show they still genuinely diverge.
#[test]
fn a_patch_reachable_only_from_a_received_pointer_is_not_pending() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-received-excluded");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    // The sender's own history already sealed this patch -- held here only as received, never
    // adopted into a local ref, exactly `archive import`'s default (non-`--adopt`) landing.
    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = objects.write_object(&child_block)?;
    let received_ref_state =
        signed_ref_state_envelope("remotes/origin/heads/main", None, child_block_id, 1);
    let received_ref_state_id = objects.write_object(&received_ref_state)?;
    write_received_pointer_for_test(&layout, "remotes/origin/heads/main", received_ref_state_id)?;

    assert_eq!(
        accepted_but_unsealed_patch_ids(&layout)?,
        vec![patch_id],
        "the raw query, which `seal_from_accepted_claim` still uses, must stay unchanged"
    );
    assert_eq!(
        patch_ids_pending(&layout)?,
        Vec::new(),
        "the listing's own view must exclude a patch a held Block already seals"
    );
    assert_eq!(
        pending_patches_with_claims(&layout)?,
        Vec::new(),
        "the claim join must see the same exclusion"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// 0.56.0 step 3g regression guard (formerly 1a2's R2): the held-Block subtraction must not
/// over-exclude. A patch named by a held claim, with no block sealing it at all, stays pending.
#[test]
fn a_patch_with_no_sealing_block_and_a_held_claim_is_still_pending() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-received-not-excluded");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    // A held claim names it (0.56.0 step 3g: pending now requires one) -- no block, no ref, no
    // received pointer at all, so nothing excludes this patch.
    let signer = super::exchange_test_support::maintainer_signer(0x90)?;
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &super::exchange_test_support::public_key_hex(&signer),
    )?;
    let block_id = ObjectId::from_bytes([0x90; 32]);
    objects.write_object(&super::exchange_test_support::signed_claim_envelope(
        &signer,
        block_id,
        vec![patch_id],
    )?)?;

    assert_eq!(patch_ids_pending(&layout)?, vec![patch_id]);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A received pointer naming a block that does **not** seal the patch in question must not
/// exclude it -- the held-Block subtraction is per patch id a block actually names, not "any
/// received pointer exists at all". A held claim names the patch too (0.56.0 step 3g: otherwise
/// it would read `unclaimed`, not demonstrating this subtraction's own precision at all).
#[test]
fn a_received_pointer_not_sealing_the_patch_does_not_exclude_it() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-received-unrelated");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    let signer = super::exchange_test_support::maintainer_signer(0x91)?;
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &super::exchange_test_support::public_key_hex(&signer),
    )?;
    let claimed_block_id = ObjectId::from_bytes([0x91; 32]);
    objects.write_object(&super::exchange_test_support::signed_claim_envelope(
        &signer,
        claimed_block_id,
        vec![patch_id],
    )?)?;

    // A received history that seals nothing at all (an empty root block only).
    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let received_ref_state =
        signed_ref_state_envelope("remotes/origin/heads/main", None, root_block_id, 1);
    let received_ref_state_id = objects.write_object(&received_ref_state)?;
    write_received_pointer_for_test(&layout, "remotes/origin/heads/main", received_ref_state_id)?;

    assert_eq!(
        patch_ids_pending(&layout)?,
        vec![patch_id],
        "a received pointer that does not seal this patch must not exclude it"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Design review v2's A1 ruling, object 3 ("rolled back (its Block held)"): a block a local ref
/// rolled away from is still held, and still names its own patch -- `sealed`, not `pending`, even
/// though no local ref reaches it any more. The patch is also named by a held claim, so without
/// the held-Block subtraction it would wrongly read `pending` -- see the red control below, which
/// disables exactly that subtraction and reproduces this old, wrong classification.
#[test]
fn a_rolled_back_block_still_seals_its_patch_even_with_a_claim_naming_it() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-rollback");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = objects.write_object(&child_block)?;

    // A held claim naming the same patch -- without the held-Block subtraction, this is exactly
    // what would make the rolled-back patch wrongly read `pending` again.
    let signer = super::exchange_test_support::maintainer_signer(0x92)?;
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &super::exchange_test_support::public_key_hex(&signer),
    )?;
    let _claim_id = objects.write_object(&super::exchange_test_support::signed_claim_envelope(
        &signer,
        child_block_id,
        vec![patch_id],
    )?)?;

    let ref_store = RefStore::new(layout.clone());
    let forward_ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let forward_ref_state_id = forward_ref_state.object_id();
    let forward_ref_update =
        signed_ref_update_envelope("heads/main", None, forward_ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: forward_ref_state,
        ref_update: forward_ref_update,
    })?;

    // Roll `heads/main` back to the root block -- `child_block_id` (and the patch it names) is
    // still held, just no longer reachable from any live tip.
    let rollback_ref_state =
        signed_ref_state_envelope("heads/main", Some(forward_ref_state_id), root_block_id, 2);
    let rollback_ref_state_id = rollback_ref_state.object_id();
    let rollback_ref_update = signed_ref_update_envelope(
        "heads/main",
        Some(forward_ref_state_id),
        rollback_ref_state_id,
        root_block_id,
        2,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(forward_ref_state_id),
        ref_state: rollback_ref_state,
        ref_update: rollback_ref_update,
    })?;

    assert_eq!(
        patch_ids_pending(&layout)?,
        Vec::new(),
        "a rolled-back but still-held block must keep its patch sealed, not pending"
    );
    assert_eq!(
        pending_patches_with_claims(&layout)?,
        Vec::new(),
        "the claim join must agree: not listed"
    );

    // Red control (A1's own instruction): disable exactly the held-Block subtraction, nothing
    // else, and confirm the rolled-back patch regresses to its old, wrong `pending` classification
    // -- reproducing precisely the bug the subtraction exists to prevent.
    assert_eq!(
        patch_ids_pending_disabling_held_block_subtraction_for_test(&layout)?,
        vec![patch_id],
        "disabling only the held-Block subtraction must reproduce the old, wrong `pending` \
         classification for a rolled-back-but-held patch"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Design review v2's A1 ruling, object 4 ("after `received remove` (its Block held)"): removing
/// a received pointer deletes only the pointer, never the objects it reached -- the block it
/// named is still held, still names its own patch, so `patch_ids_pending`/
/// `pending_patches_with_claims` must keep excluding it exactly as before the removal.
#[test]
fn received_remove_leaves_its_sealed_patch_excluded() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-received-remove");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = objects.write_object(&child_block)?;
    let received_ref_state =
        signed_ref_state_envelope("remotes/origin/heads/main", None, child_block_id, 1);
    let received_ref_state_id = objects.write_object(&received_ref_state)?;
    write_received_pointer_for_test(&layout, "remotes/origin/heads/main", received_ref_state_id)?;

    assert_eq!(patch_ids_pending(&layout)?, Vec::new());

    remove_received_pointer(&layout, "remotes/origin/heads/main")?;

    assert_eq!(
        patch_ids_pending(&layout)?,
        Vec::new(),
        "the patch's own Block is still held after `received remove`, so it must stay sealed"
    );
    assert_eq!(pending_patches_with_claims(&layout)?, Vec::new());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
