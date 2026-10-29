//! RFC 115 Stage 3 §5: `accepted_but_unsealed_patch_ids` -- the derived query, tested directly.
//! 0.56.0 step 1 part 1a: `pending_patches_with_claims`'s own claim join, tested directly too.

#![allow(clippy::indexing_slicing)]

use prikk_error::Result;
use prikk_object::{BlockKind, ObjectId, SignerRole};

use super::{
    PendingClaimSignatureOutcome, PendingPatchEntry, accepted_but_unsealed_patch_ids,
    patch_ids_pending_excluding_received, pending_patches_with_claims,
};
use crate::add_trusted_maintainer;
use crate::test_gates::test_support::{
    signed_block, signed_patch_blob_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    FileObjectStore, MaintainerSigner, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    write_received_pointer_for_test,
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

#[test]
fn pending_claims_a_patch_no_claim_names_has_an_empty_claims_list() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-claims-no-claim");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let author = super::exchange_test_support::author_signer(0x7A)?;
    let blob_id = objects.write_object(&super::exchange_test_support::signed_blob_envelope(
        b"no claim\n",
    )?)?;
    let patch_id =
        objects.write_object(&super::exchange_test_support::signed_author_patch_envelope(
            &author, "e.txt", 0x7B, blob_id,
        )?)?;

    assert_eq!(
        pending_patches_with_claims(&layout)?,
        vec![PendingPatchEntry {
            patch_id,
            claims: Vec::new()
        }]
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
        assert_eq!(claim_entry.parent_block_id, block_id);
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

/// 0.56.0 step 1a2, R2 (owner ruling 2026-10-29, "Exclude received history"): a patch reachable
/// only from a **received** pointer (never adopted into a local ref) is not pending, even though
/// `accepted_but_unsealed_patch_ids` -- the raw query `seal_from_accepted_claim` still uses
/// unchanged -- still reports it. Both queries are asserted here to show they now genuinely
/// diverge, not just that the new one happens to return the right thing.
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
        "the raw query, which `seal_from_accepted_claim` still uses, must stay unchanged by R2"
    );
    assert_eq!(
        patch_ids_pending_excluding_received(&layout)?,
        Vec::new(),
        "the listing's own view must exclude a patch the received history already sealed"
    );
    assert_eq!(
        pending_patches_with_claims(&layout)?,
        Vec::new(),
        "the claim join must see the same exclusion"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// 0.56.0 step 1a2, R2's own regression guard: R2 must not over-exclude. A patch with no received
/// block sealing it at all stays pending, exactly as before.
#[test]
fn a_patch_with_no_received_seal_is_still_pending() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-received-not-excluded");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    // No block, no ref, no received pointer at all -- nothing excludes this patch.
    assert_eq!(
        patch_ids_pending_excluding_received(&layout)?,
        vec![patch_id]
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A received pointer naming a block that does **not** seal the patch in question must not
/// exclude it -- R2's own subtraction is per reachable patch id, not "any received pointer
/// exists at all".
#[test]
fn a_received_pointer_not_sealing_the_patch_does_not_exclude_it() -> Result<()> {
    let root = unique_temp_dir("pexch-pending-received-unrelated");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    // A received history that seals nothing at all (an empty root block only).
    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let received_ref_state =
        signed_ref_state_envelope("remotes/origin/heads/main", None, root_block_id, 1);
    let received_ref_state_id = objects.write_object(&received_ref_state)?;
    write_received_pointer_for_test(&layout, "remotes/origin/heads/main", received_ref_state_id)?;

    assert_eq!(
        patch_ids_pending_excluding_received(&layout)?,
        vec![patch_id],
        "a received pointer that does not seal this patch must not exclude it"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
