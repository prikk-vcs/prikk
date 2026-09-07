//! Publication-trust verification regressions.

use prikk_object::{ObjectEnvelope, ObjectType};

use super::super::trust::PublicationTrustVerifier;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout, add_trusted_maintainer,
    maintainer_signature,
};

#[test]
fn missing_policy_is_reported_once_while_count_advances() -> prikk_error::Result<()> {
    assert_invalid_policy_sequence("verify-trust-missing-policy", None)
}

#[test]
fn malformed_policy_is_reported_once_while_count_advances() -> prikk_error::Result<()> {
    assert_invalid_policy_sequence(
        "verify-trust-malformed-policy",
        Some(b"not a valid trust policy container at all"),
    )
}

#[test]
fn trusted_and_untrusted_envelopes_preserve_counts_and_order() -> prikk_error::Result<()> {
    let root = unique_temp_dir("verify-trust-envelope-order");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = Ed25519MaintainerSigner::from_seed("trusted-maintainer", &[0x46; 32])?;
    add_trusted_maintainer(&layout, signer.key_id(), &public_key_hex(&signer))?;

    let trusted = signed_envelope(b"trusted", &signer)?;
    let untrusted_first =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"untrusted-first".to_vec());
    let untrusted_second =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"untrusted-second".to_vec());
    let first_id = untrusted_first.object_id().to_string();
    let second_id = untrusted_second.object_id().to_string();

    let mut verifier = PublicationTrustVerifier::new(&layout);
    verifier.verify(&trusted)?;
    assert_eq!(verifier.checked_records, 1);
    assert!(verifier.issues.is_empty());

    verifier.verify(&untrusted_first)?;
    verifier.verify(&trusted)?;
    verifier.verify(&untrusted_second)?;

    assert_eq!(verifier.checked_records, 4);
    assert_eq!(verifier.issues.len(), 2);
    assert!(
        verifier
            .issues
            .iter()
            .all(|issue| { issue.code == "PRIKK-TRUST-PUBLICATION-UNTRUSTED" })
    );
    let mut issues = verifier.issues.iter();
    assert!(
        issues
            .next()
            .is_some_and(|issue| issue.message.contains(&first_id))
    );
    assert!(
        issues
            .next()
            .is_some_and(|issue| issue.message.contains(&second_id))
    );
    assert!(issues.next().is_none());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn verify_returns_the_matched_key_id_on_success() -> prikk_error::Result<()> {
    let root = unique_temp_dir("verify-trust-matched-key-id");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = Ed25519MaintainerSigner::from_seed("matched-maintainer", &[0x21; 32])?;
    add_trusted_maintainer(&layout, signer.key_id(), &public_key_hex(&signer))?;

    let envelope = signed_envelope(b"matched", &signer)?;
    let mut verifier = PublicationTrustVerifier::new(&layout);
    let matched = verifier.verify(&envelope)?;
    assert_eq!(matched, Some(signer.key_id().to_string()));
    assert!(verifier.issues.is_empty());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

fn assert_invalid_policy_sequence(
    fixture_name: &str,
    malformed_policy: Option<&[u8]>,
) -> prikk_error::Result<()> {
    let root = unique_temp_dir(fixture_name);
    let layout = RepositoryLayout::init(root.clone())?;
    if let Some(policy) = malformed_policy {
        std::fs::write(
            layout.trust_policy_container_slot_path(crate::foundation::layout::ContainerSlot::A),
            policy,
        )?;
    }

    let first = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"first".to_vec());
    let second = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"second".to_vec());
    let mut verifier = PublicationTrustVerifier::new(&layout);

    verifier.verify(&first)?;
    assert_eq!(verifier.checked_records, 1);
    assert_eq!(verifier.issues.len(), 1);
    assert_eq!(
        verifier.issues.first().map(|issue| issue.code),
        Some("PRIKK-TRUST-POLICY-INVALID")
    );

    verifier.verify(&second)?;
    assert_eq!(verifier.checked_records, 2);
    assert_eq!(verifier.issues.len(), 1);
    assert_eq!(
        verifier.issues.first().map(|issue| issue.code),
        Some("PRIKK-TRUST-POLICY-INVALID")
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

fn signed_envelope(
    payload: &[u8],
    signer: &impl MaintainerSigner,
) -> prikk_error::Result<ObjectEnvelope> {
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_vec());
    envelope.add_signature(maintainer_signature(
        signer,
        envelope.object_type,
        envelope.object_id(),
    )?)?;
    Ok(envelope)
}

fn public_key_hex(signer: &impl MaintainerSigner) -> String {
    signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
