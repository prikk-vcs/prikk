//! DC-78 verify-local-tag-publication-trust (v2 amendment): end-to-end coverage for the
//! `LocalTagTrust` stage itself -- `PublicationTrustVerifier` in isolation is already covered by
//! `verify/tests/trust.rs`; these fixtures prove the *stage* reaches it for a real local Tag.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use prikk_error::Result;
use prikk_object::{
    BlockKind, BlockPayload, MerkleRoot, ObjectId, RefKind, RefStatePayload, RefUpdatePayload,
    TagPayload,
};

use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    add_trusted_maintainer, maintainer_signature, verify_repository,
};
use prikk_object::{CanonicalEncode, ObjectEnvelope, ObjectType};

fn signer(discriminant: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(
        format!("local-tag-trust-fixture-{discriminant}"),
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

fn signed(
    object_type: ObjectType,
    schema_version: u32,
    payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> Result<ObjectEnvelope> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, payload);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, object_type, id)?)?;
    Ok(envelope)
}

/// A real root block, signed by `block_signer` -- must itself be a *trusted* signer whenever a test
/// wants `Objects`' own Block trust-check to stay clean, so the fixture isolates the Tag path.
fn write_root_block(
    store: &mut FileObjectStore,
    block_signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([0_u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let envelope = signed(
        ObjectType::Block,
        2,
        payload.to_canonical_bytes()?,
        block_signer,
    )?;
    store.write_object(&envelope)
}

/// Publish `tags/<name>` naming `tag_object_id`, with the RefState/RefUpdate signed by
/// `ref_signer` -- deliberately a *different* parameter from the Tag object's own signer, so a test
/// can hold the ref-publication side trusted while the Tag object itself is not (or vice-versa),
/// isolating exactly which envelope `LocalTagTrust` is reacting to.
fn publish_tag_ref(
    layout: &RepositoryLayout,
    name: &str,
    tag_object_id: ObjectId,
    ref_signer: &Ed25519MaintainerSigner,
) -> Result<()> {
    let ref_name = format!("tags/{name}");
    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.clone(),
        kind: RefKind::Tag,
        target_object_id: tag_object_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state = signed(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
        ref_signer,
    )?;
    let ref_state_id = ref_state.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: ref_name.clone(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: tag_object_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: ref_signer.key_id().to_string(),
    };
    let ref_update = signed(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
        ref_signer,
    )?;
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name,
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// The central case: the Tag object's own MAINTAINER signature is under a key nobody adopted, while
/// the Block and the RefState/RefUpdate publishing it are all signed by the one trusted key --
/// isolating the finding to the Tag envelope specifically, not a coincidental Block/RefState/RefUpdate
/// failure `Objects`/`RefUpdateSchemaTrust` would already catch on their own. Constructed by calling
/// the same low-level primitives `tag_travel::create_local_tag` uses internally, not that function
/// itself -- `create_local_tag` signs the Tag and its RefState/RefUpdate with the same one signer, so
/// it cannot produce this isolated a fixture; going lower still bypasses `053e442`'s CLI-layer gate
/// (`verify_signer_trusted`), which lives in `crates/prikk-cli/src/tag.rs`, not in any store-level
/// function -- there is no public store-level surface left that refuses this today.
#[test]
fn verify_repository_fails_a_local_tag_signed_by_an_untrusted_key() -> Result<()> {
    let root = unique_temp_dir("verify-local-tag-trust-untrusted");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x61)?;
    add_trusted_maintainer(&layout, trusted.key_id(), &public_key_hex(&trusted))?;
    let untrusted = signer(0x62)?;

    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &trusted)?;
    let (digest, count) = compute_patch_set_digest_and_count_from_block(&store, block)?;

    let tag_payload = TagPayload {
        name: "tags/v1".to_string(),
        target_block_id: block,
        message: None,
        created_at: 0,
        author_key_id: untrusted.key_id().to_string(),
        patch_set_digest: digest,
        patch_count: count,
    };
    let tag_envelope = signed(
        ObjectType::Tag,
        1,
        tag_payload.to_canonical_bytes()?,
        &untrusted,
    )?;
    let tag_object_id = store.write_object(&tag_envelope)?;
    publish_tag_ref(&layout, "v1", tag_object_id, &trusted)?;

    let report = verify_repository(&layout)?;
    assert!(
        report.has_publication_trust_issues(),
        "an untrusted local Tag signature must be a publication-trust issue"
    );
    assert!(
        report.publication_trust_issues.iter().any(|issue| {
            issue.code == "PRIKK-TRUST-PUBLICATION-UNTRUSTED"
                && issue.message.contains(&tag_object_id.to_string())
        }),
        "expected an untrusted-signature issue naming the Tag object {tag_object_id}, got: {:?}",
        report.publication_trust_issues
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The companion positive case: the identical shape, but the Tag is signed by the trusted key too --
/// `verify_repository` must report no publication-trust issue, and `checked_publication_trust_records`
/// must be exactly **4**, not merely `Some`: 2 from `Objects` (the Block, plus the Tag's own RefState --
/// `Objects` trust-checks every persisted `RefState` regardless of `RefKind`), 1 from
/// `RefUpdateSchemaTrust` (the one RefUpdate log record), and 1 from `LocalTagTrust` (the Tag envelope
/// itself). A bare `is_some()` here does not distinguish `LocalTagTrust` running from it being silently
/// skipped -- `Objects`/`RefUpdateSchemaTrust` alone would already populate the field with `Some(3)` for
/// this exact fixture (review v1 §1 caught this by inverting the stage's own filter: test 2 failed as
/// it should, this test still passed at `Some(3)`, proving nothing). The exact total is only reachable
/// if `LocalTagTrust` genuinely examined the Tag.
#[test]
fn verify_repository_checks_a_trusted_local_tag_cleanly() -> Result<()> {
    let root = unique_temp_dir("verify-local-tag-trust-trusted");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x63)?;
    add_trusted_maintainer(&layout, trusted.key_id(), &public_key_hex(&trusted))?;

    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &trusted)?;
    let (digest, count) = compute_patch_set_digest_and_count_from_block(&store, block)?;

    let tag_payload = TagPayload {
        name: "tags/v1".to_string(),
        target_block_id: block,
        message: None,
        created_at: 0,
        author_key_id: trusted.key_id().to_string(),
        patch_set_digest: digest,
        patch_count: count,
    };
    let tag_envelope = signed(
        ObjectType::Tag,
        1,
        tag_payload.to_canonical_bytes()?,
        &trusted,
    )?;
    let tag_object_id = store.write_object(&tag_envelope)?;
    publish_tag_ref(&layout, "v1", tag_object_id, &trusted)?;

    let report = verify_repository(&layout)?;
    assert!(
        !report.has_publication_trust_issues(),
        "a trusted local Tag must not be reported as a publication-trust issue: {:?}",
        report.publication_trust_issues
    );
    assert_eq!(
        report.checked_publication_trust_records,
        Some(4),
        "2 (Objects: Block + the Tag's own RefState) + 1 (RefUpdateSchemaTrust) + 1 (LocalTagTrust) \
         -- a lower count means LocalTagTrust did not examine the Tag"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
