//! RFC 156 Stage 1: every signature of a role is checked, not the first.
//!
//! Each control builds an envelope whose **first** signature (canonical order: key id first) is valid
//! and whose **second** is made by a key this repository trusts or records, then corrupted. A check that
//! reads only the first signature passes these envelopes; every control here must refuse them.

use prikk_object::{ObjectEnvelope, ObjectType, Signature};

use super::super::trust::PublicationTrustVerifier;
use crate::author::author_key_index::{record_author_key_material, verify_author_signature};
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::lock::ActiveLock;
use crate::recognition_claim::{ClaimSignatureVerification, verify_claim_signature};
use crate::tag_travel::{TagSignatureVerification, verify_tag_signature};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    AuthorSigner, Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout,
    add_trusted_maintainer, author_signature, load_maintainer_trust_policy, maintainer_signature,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn corrupted(mut signature: Signature) -> Signature {
    if let Some(byte) = signature.signature_bytes.first_mut() {
        *byte ^= 0x01;
    }
    signature
}

/// Two adopted maintainers, `a-first` and `b-second`, so `b-second` sorts second.
fn two_adopted_maintainers(
    tag: &str,
) -> prikk_error::Result<(
    std::path::PathBuf,
    RepositoryLayout,
    Ed25519MaintainerSigner,
    Ed25519MaintainerSigner,
)> {
    let root = unique_temp_dir(tag);
    let layout = RepositoryLayout::init(root.clone())?;
    let first = Ed25519MaintainerSigner::from_seed("a-first-maintainer", &[0x31; 32])?;
    let second = Ed25519MaintainerSigner::from_seed("b-second-maintainer", &[0x32; 32])?;
    add_trusted_maintainer(&layout, first.key_id(), &hex(&first.public_key_bytes()))?;
    add_trusted_maintainer(&layout, second.key_id(), &hex(&second.public_key_bytes()))?;
    Ok((root, layout, first, second))
}

/// `envelope` signed validly by `first` and, invalidly (`corrupt_second`) or validly, by `second`.
fn maintainer_signed(
    object_type: ObjectType,
    first: &Ed25519MaintainerSigner,
    second: &Ed25519MaintainerSigner,
    corrupt_second: bool,
) -> prikk_error::Result<ObjectEnvelope> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, 1, b"rfc156 stage 1".to_vec());
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(first, object_type, id)?)?;
    let signature = maintainer_signature(second, object_type, id)?;
    envelope.add_signature(if corrupt_second {
        corrupted(signature)
    } else {
        signature
    })?;
    Ok(envelope)
}

#[test]
fn a_second_author_signature_that_does_not_verify_fails_the_patch() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc156-every-author-signature");
    let layout = RepositoryLayout::init(root.clone())?;
    let first = Ed25519AuthorSigner::from_seed("a-first-author", &[0x41; 32])?;
    let second = Ed25519AuthorSigner::from_seed("b-second-author", &[0x42; 32])?;
    let lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(&layout, first.key_id(), first.public_key_bytes(), &lock)?;
    record_author_key_material(&layout, second.key_id(), second.public_key_bytes(), &lock)?;
    drop(lock);

    let build = |corrupt_second: bool| -> prikk_error::Result<ObjectEnvelope> {
        let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, b"two authors".to_vec());
        let id = envelope.object_id();
        envelope.add_signature(author_signature(&first, id)?)?;
        let signature = author_signature(&second, id)?;
        envelope.add_signature(if corrupt_second {
            corrupted(signature)
        } else {
            signature
        })?;
        Ok(envelope)
    };

    // The twin: both valid, so the answer names the first in canonical order.
    assert_eq!(
        verify_author_signature(&layout, &build(false)?)?,
        Some(("a-first-author".to_string(), true))
    );
    let refused = verify_author_signature(&layout, &build(true)?);
    assert!(
        matches!(refused, Err(prikk_error::PrikkError::InvalidSignature(ref message)) if message.contains("b-second-author")),
        "the invalid second AUTHOR signature must fail the patch: {refused:?}"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_second_author_signature_with_no_material_reads_unverifiable() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc156-every-author-unverifiable");
    let layout = RepositoryLayout::init(root.clone())?;
    let first = Ed25519AuthorSigner::from_seed("a-first-author", &[0x43; 32])?;
    let unrecorded = Ed25519AuthorSigner::from_seed("b-unrecorded-author", &[0x44; 32])?;
    let lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(&layout, first.key_id(), first.public_key_bytes(), &lock)?;
    drop(lock);

    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, b"one unrecorded".to_vec());
    let id = envelope.object_id();
    envelope.add_signature(author_signature(&first, id)?)?;
    envelope.add_signature(author_signature(&unrecorded, id)?)?;
    // Not sound as a whole: one signer cannot be checked, and that signer is the one named.
    assert_eq!(
        verify_author_signature(&layout, &envelope)?,
        Some(("b-unrecorded-author".to_string(), false))
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_second_adopted_maintainer_signature_that_does_not_verify_is_a_trust_issue()
-> prikk_error::Result<()> {
    let (root, layout, first, second) = two_adopted_maintainers("rfc156-every-publication")?;

    let mut verifier = PublicationTrustVerifier::new(&layout);
    assert_eq!(
        verifier.verify(&maintainer_signed(
            ObjectType::Block,
            &first,
            &second,
            false
        )?)?,
        Some("a-first-maintainer".to_string())
    );
    assert!(verifier.issues.is_empty());

    let mut verifier = PublicationTrustVerifier::new(&layout);
    assert_eq!(
        verifier.verify(&maintainer_signed(
            ObjectType::Block,
            &first,
            &second,
            true
        )?)?,
        None
    );
    assert_eq!(
        verifier.issues.first().map(|issue| issue.code),
        Some("PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE"),
        "{:?}",
        verifier.issues
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_signature_by_a_key_no_longer_adopted_is_not_an_issue() -> prikk_error::Result<()> {
    let root = unique_temp_dir("rfc156-every-publication-unadopted");
    let layout = RepositoryLayout::init(root.clone())?;
    let first = Ed25519MaintainerSigner::from_seed("a-first-maintainer", &[0x33; 32])?;
    let stranger = Ed25519MaintainerSigner::from_seed("b-stranger-maintainer", &[0x34; 32])?;
    add_trusted_maintainer(&layout, first.key_id(), &hex(&first.public_key_bytes()))?;

    // Corrupt, but by a key this repository never adopted: nothing here can check it.
    let mut verifier = PublicationTrustVerifier::new(&layout);
    assert_eq!(
        verifier.verify(&maintainer_signed(
            ObjectType::Block,
            &first,
            &stranger,
            true
        )?)?,
        Some("a-first-maintainer".to_string())
    );
    assert!(verifier.issues.is_empty(), "{:?}", verifier.issues);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_second_adopted_maintainer_signature_that_does_not_verify_refuses_a_claim()
-> prikk_error::Result<()> {
    let (root, layout, first, second) = two_adopted_maintainers("rfc156-every-claim")?;
    let policy = load_maintainer_trust_policy(&layout)?;
    assert!(matches!(
        verify_claim_signature(
            &maintainer_signed(ObjectType::RecognitionClaim, &first, &second, false)?,
            &policy
        )?,
        ClaimSignatureVerification::Sound { ref key_id } if key_id == "a-first-maintainer"
    ));
    assert!(
        verify_claim_signature(
            &maintainer_signed(ObjectType::RecognitionClaim, &first, &second, true)?,
            &policy
        )
        .is_err()
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_second_adopted_maintainer_signature_that_does_not_verify_refuses_a_tag()
-> prikk_error::Result<()> {
    let (root, layout, first, second) = two_adopted_maintainers("rfc156-every-tag")?;
    let policy = load_maintainer_trust_policy(&layout)?;
    assert!(matches!(
        verify_tag_signature(
            &maintainer_signed(ObjectType::Tag, &first, &second, false)?,
            &policy
        )?,
        TagSignatureVerification::Sound { ref key_id } if key_id == "a-first-maintainer"
    ));
    assert!(
        verify_tag_signature(
            &maintainer_signed(ObjectType::Tag, &first, &second, true)?,
            &policy
        )
        .is_err()
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
