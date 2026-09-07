use prikk_crypto::{Ed25519KeyPair, verify_ed25519};
use prikk_object::{
    CanonicalEncode, CreateFile, EditText, NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose, Signature, SignatureAlgorithm, SignerRole,
};

use crate::{
    AuthorSigner, DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, Ed25519MaintainerSigner,
    MaintainerSigner, author_signature, maintainer_signature,
};

const PUBLIC_KEY: [u8; 32] = [
    0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e, 0xab, 0x6c,
    0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06, 0x98, 0x81, 0xdb, 0x12,
];

const SIGNATURE: [u8; 64] = [
    0x10, 0x2c, 0x73, 0xaf, 0xdf, 0x34, 0xfc, 0xd4, 0x51, 0x7b, 0x9c, 0x47, 0x9a, 0x11, 0xc3, 0x92,
    0xe6, 0x29, 0xda, 0x37, 0xcd, 0xe5, 0x8b, 0x8e, 0x88, 0x2c, 0xc9, 0xb3, 0xae, 0x28, 0x26, 0x19,
    0x4c, 0x3a, 0xb6, 0xbe, 0x87, 0x44, 0x68, 0x65, 0xce, 0x5c, 0xda, 0xef, 0x12, 0xff, 0xc4, 0xed,
    0x8d, 0xd8, 0x7b, 0x1e, 0xc7, 0xf8, 0x7a, 0x8d, 0x8a, 0xe9, 0xe0, 0x2c, 0x5f, 0x1f, 0xb1, 0x0d,
];

fn golden_preimage() -> prikk_error::Result<Vec<u8>> {
    Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::RefUpdate,
        ObjectId::from_bytes(core::array::from_fn(|index| index as u8)),
        SignerRole::Maintainer,
        "maintainer_1",
    )
}

#[test]
fn dc39_literal_vector_signs_and_verifies_through_production_apis() -> prikk_error::Result<()> {
    assert_eq!(
        prikk_object::ED25519_SIGNATURE_LEN,
        prikk_crypto::ED25519_SIGNATURE_LEN
    );
    let preimage = golden_preimage()?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010004000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f0002000c6d61696e7461696e65725f31"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&[0x42; 32]).public_key_bytes(),
        PUBLIC_KEY
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&[0x42; 32]).sign(&preimage),
        SIGNATURE
    );
    verify_ed25519(&PUBLIC_KEY, &preimage, &SIGNATURE)
}

#[test]
fn every_signature_preimage_field_is_cryptographically_bound() -> prikk_error::Result<()> {
    let preimage = golden_preimage()?;
    for index in [0, 12, 14, 16, 48, 50, 52] {
        let mut changed = preimage.clone();
        let changed_byte = changed.get_mut(index).ok_or_else(|| {
            prikk_error::PrikkError::Integrity(
                "signature preimage mutation index was missing".to_string(),
            )
        })?;
        *changed_byte ^= 1;
        assert!(verify_ed25519(&PUBLIC_KEY, &changed, &SIGNATURE).is_err());
    }
    Ok(())
}

#[test]
fn production_author_and_maintainer_paths_use_verifiable_shared_preimages()
-> prikk_error::Result<()> {
    let object_id = ObjectId::from_bytes([0x33; 32]);
    let author = Ed25519AuthorSigner::from_seed("author", &[0x11; 32])?;
    let author_signature = author_signature(&author, object_id)?;
    let author_preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        object_id,
        SignerRole::Author,
        "author",
    )?;
    verify_ed25519(
        &author.public_key_bytes(),
        &author_preimage,
        &author_signature.signature_bytes,
    )?;

    let maintainer = Ed25519MaintainerSigner::from_seed("maintainer", &[0x22; 32])?;
    let maintainer_signature = maintainer_signature(&maintainer, ObjectType::RefState, object_id)?;
    let maintainer_preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::RefState,
        object_id,
        SignerRole::Maintainer,
        "maintainer",
    )?;
    verify_ed25519(
        &maintainer.public_key_bytes(),
        &maintainer_preimage,
        &maintainer_signature.signature_bytes,
    )
}

// DC-53 Stage 1, design-v1.md §4: committed literal vectors 1-5, the DC-40 precedent applied to
// AUTHOR signatures -- so the verification contract is reviewable independently of whichever
// public-key source Stage 1 ends up using, per `RFC-DC-53-stage1-report-v1.md`.

const DC53_PUBLIC_KEY: [u8; 32] = [
    0xf8, 0x0c, 0xcc, 0xdc, 0xe4, 0xae, 0x1c, 0x07, 0xae, 0x20, 0x8a, 0x2a, 0xdf, 0x99, 0xa3, 0x10,
    0xae, 0x42, 0x07, 0xe0, 0x30, 0x6f, 0xa0, 0x23, 0x61, 0x10, 0xb0, 0x68, 0x27, 0xbb, 0xb8, 0xd0,
];

const DC53_SIGNATURE: [u8; 64] = [
    0xf3, 0xbc, 0xe1, 0x56, 0x2d, 0x08, 0x93, 0x73, 0xc6, 0x9d, 0x62, 0xb1, 0xca, 0x05, 0x06, 0x44,
    0x54, 0xef, 0xe0, 0xaa, 0x81, 0xf6, 0x7d, 0x45, 0xe9, 0xf1, 0xc3, 0x12, 0x8d, 0xe9, 0x4f, 0xb2,
    0xb0, 0xae, 0x5f, 0x32, 0x09, 0x30, 0x0a, 0x9e, 0x56, 0xdc, 0x2b, 0x51, 0xcf, 0x42, 0x24, 0xce,
    0x53, 0x1c, 0x9e, 0x05, 0x2e, 0x7e, 0x0a, 0xc9, 0xcf, 0x0d, 0x31, 0x91, 0x23, 0x4a, 0x3c, 0x09,
];

/// Vector 2: a minimal single-operation Patch payload, literal enough to be reviewed on its own.
fn dc53_patch_payload() -> PatchPayload {
    PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "dc53.txt".to_string(),
                node_id: NodeId::from_bytes([0x53; 32]),
                blob_id: ObjectId::from_bytes([0x44; 32]),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    }
}

fn dc53_object_id() -> prikk_error::Result<ObjectId> {
    let bytes = dc53_patch_payload().to_canonical_bytes()?;
    Ok(ObjectId::from_canonical_payload(
        ObjectType::Patch,
        1,
        &bytes,
    ))
}

fn dc53_preimage() -> prikk_error::Result<Vec<u8>> {
    Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        dc53_object_id()?,
        SignerRole::Author,
        "dc53-author",
    )
}

/// Vectors 1-3: a known keypair, the known payload's canonical AUTHOR preimage, and a valid
/// signature over it that verifies.
#[test]
fn dc53_vectors_1_to_3_author_signature_signs_and_verifies() -> prikk_error::Result<()> {
    let preimage = dc53_preimage()?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010001aaec00c9e7985cf4592f8afb6bbcc411e90e450f225cb098af6398e902b62acd0001000b646335332d617574686f72"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&[0x53; 32]).public_key_bytes(),
        DC53_PUBLIC_KEY
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&[0x53; 32]).sign(&preimage),
        DC53_SIGNATURE
    );
    verify_ed25519(&DC53_PUBLIC_KEY, &preimage, &DC53_SIGNATURE)
}

/// Vector 4: one bit flipped in an otherwise-valid signature must fail.
#[test]
fn dc53_vector_4_a_mutated_signature_fails() -> prikk_error::Result<()> {
    let preimage = dc53_preimage()?;
    let mut mutated = DC53_SIGNATURE;
    mutated[0] ^= 1;
    assert!(verify_ed25519(&DC53_PUBLIC_KEY, &preimage, &mutated).is_err());
    Ok(())
}

/// Vector 5: a signature genuinely valid over a *different* preimage must still fail against this
/// one -- the vector that catches a preimage-construction error (design-v1.md §4, "the one most
/// likely to be omitted").
#[test]
fn dc53_vector_5_a_signature_valid_over_a_different_preimage_fails() -> prikk_error::Result<()> {
    let other_object_id = ObjectId::from_bytes([0x99; 32]);
    let other_preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        other_object_id,
        SignerRole::Author,
        "dc53-author",
    )?;
    let other_signature = Ed25519KeyPair::from_seed(&[0x53; 32]).sign(&other_preimage);
    // Sanity: the other signature is genuinely valid over its own preimage -- this is not a
    // malformed-signature test, it is a wrong-preimage test.
    verify_ed25519(&DC53_PUBLIC_KEY, &other_preimage, &other_signature)?;

    let preimage = dc53_preimage()?;
    assert!(verify_ed25519(&DC53_PUBLIC_KEY, &preimage, &other_signature).is_err());
    Ok(())
}

// DC-53 Stage 2, design-stage-2-v1.md §7: vector 6, a pin-conflict pair -- one `key_id`, two public
// keys. Different in kind from vectors 1-5: those exercise only `verify_ed25519`/
// `Signature::signed_bytes`, this exercises the container-level conflict rule (D8), so it needs a
// `RepositoryLayout` and `author_key_index`'s own functions. Same literal-value discipline: both
// keypairs' public key bytes are fixed-seed and asserted, not left as opaque generated values.

const VECTOR6_KEY_ID: &str = "dc53-vector6";

const VECTOR6_KEY_A_SEED: [u8; 32] = [0x62; 32];
const VECTOR6_KEY_A_PUBLIC: [u8; 32] = [
    0x2d, 0xf0, 0x41, 0x25, 0xf0, 0x01, 0x5a, 0xfb, 0x47, 0xce, 0x85, 0x3a, 0xef, 0x87, 0x72, 0x09,
    0x4f, 0xf9, 0x49, 0x8c, 0x14, 0xcb, 0x1b, 0x9e, 0x12, 0x97, 0x3c, 0x29, 0x27, 0xda, 0x0f, 0xa6,
];

const VECTOR6_KEY_B_SEED: [u8; 32] = [0x63; 32];
const VECTOR6_KEY_B_PUBLIC: [u8; 32] = [
    0xa7, 0xf6, 0xdf, 0xaf, 0x8f, 0x38, 0xb8, 0x9b, 0xa8, 0xce, 0x64, 0x9b, 0x59, 0x4f, 0x91, 0xe4,
    0xd0, 0x1f, 0xdc, 0x57, 0xf9, 0xc9, 0x49, 0x3d, 0xf4, 0x3b, 0x5e, 0x50, 0xa9, 0x98, 0x73, 0x67,
];

/// Vector 6, half 1: `record_author_key_material` rejects a second, distinct public key for a
/// `key_id` that already has one recorded -- D8's rule at the write path.
#[test]
fn dc53_vector_6_a_conflicting_key_is_rejected_at_record_time() -> prikk_error::Result<()> {
    assert_eq!(
        Ed25519KeyPair::from_seed(&VECTOR6_KEY_A_SEED).public_key_bytes(),
        VECTOR6_KEY_A_PUBLIC
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&VECTOR6_KEY_B_SEED).public_key_bytes(),
        VECTOR6_KEY_B_PUBLIC
    );

    let root = crate::test_gates::test_support::unique_temp_dir("dc53-vector6-record");
    let layout = crate::foundation::layout::RepositoryLayout::init(root.clone())?;
    let active_lock = crate::lock::ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    crate::author::author_key_index::record_author_key_material(
        &layout,
        VECTOR6_KEY_ID,
        VECTOR6_KEY_A_PUBLIC,
        &active_lock,
    )?;
    assert!(
        crate::author::author_key_index::record_author_key_material(
            &layout,
            VECTOR6_KEY_ID,
            VECTOR6_KEY_B_PUBLIC,
            &active_lock,
        )
        .is_err()
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Vector 6, half 2: once a `key_id` carries two distinct recorded keys (planted directly, since
/// half 1 already proved the production write path refuses to create this state itself),
/// `verify_author_signature` fails closed -- D8's rule at the verify path, D3's fourth row -- for a
/// Patch signed by key A even though that signature genuinely verifies against key A.
#[test]
fn dc53_vector_6_verification_fails_closed_against_a_conflicting_key_id() -> prikk_error::Result<()>
{
    let root = crate::test_gates::test_support::unique_temp_dir("dc53-vector6-verify");
    let layout = crate::foundation::layout::RepositoryLayout::init(root.clone())?;
    let active_lock = crate::lock::ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    crate::author::author_key_index::record_author_key_material(
        &layout,
        VECTOR6_KEY_ID,
        VECTOR6_KEY_A_PUBLIC,
        &active_lock,
    )?;
    crate::author::author_key_index::force_conflicting_author_key_entry_for_test(
        &layout,
        VECTOR6_KEY_ID,
        VECTOR6_KEY_B_PUBLIC,
    )?;

    let signer = Ed25519AuthorSigner::from_seed(VECTOR6_KEY_ID, &VECTOR6_KEY_A_SEED)?;
    assert_eq!(signer.public_key_bytes(), VECTOR6_KEY_A_PUBLIC);
    let payload = dc53_patch_payload();
    let canonical = payload.to_canonical_bytes()?;
    let mut envelope = prikk_object::ObjectEnvelope::unsigned(ObjectType::Patch, 1, canonical);
    let object_id = envelope.object_id();
    let signature = author_signature(&signer, object_id)?;
    envelope.add_signature(signature)?;

    // Sanity: the signature genuinely verifies against key A on its own -- this vector is about the
    // container's own conflicting state, not a malformed or wrong-preimage signature.
    let preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        object_id,
        SignerRole::Author,
        VECTOR6_KEY_ID,
    )?;
    let added_signature = envelope.signatures.first().ok_or_else(|| {
        prikk_error::PrikkError::Integrity("envelope carries the signature just added".to_string())
    })?;
    verify_ed25519(
        &VECTOR6_KEY_A_PUBLIC,
        &preimage,
        &added_signature.signature_bytes,
    )?;

    let result = crate::author::author_key_index::verify_author_signature(&layout, &envelope);
    assert!(
        matches!(&result, Err(err) if err.to_string().contains("has more than one distinct recorded public key")),
        "expected a conflicting-key_id failure, got: {result:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// RFC 114 §3 (Gate A): frozen identity vectors for the `(object_type, schema_version)` pairs that
// `validate_format2_schema` admits and that production code has actually written, beyond
// `(Patch, 1)` which vectors 1-3 above already cover (vector 14, below, covers Patch's second
// admitted pair, `(Patch, PATCH_PARENT_IDS_RETIRED_SCHEMA)`, per the patch-schema-2 handoff).
// `(Attestation, 1)` is admitted but never constructed in production (see
// `RFC114_ADMITTED_BUT_UNWRITTEN` below) and so is deliberately not frozen here -- freezing a pair
// nothing has ever written would pin an arbitrary literal, not a real compatibility obligation.
// Same literal-value discipline as vectors 1-6: every byte string below is a fixed-seed,
// hand-verified value, never generated at test time.

/// Pairs `validate_format2_schema` admits that no production code path has ever constructed, per
/// RFC 114 §2's "has ever been written" criterion (`prikk-rfc114-implementation-plan-v1.md` §1).
/// Gate A's completeness self-guard (below) checks every admitted pair is either frozen with a
/// vector or listed here -- so the day something starts writing `Attestation` (or any other
/// admitted pair not yet vectored), that test fails instead of silently shipping an unfrozen
/// identity-bearing type. Pair-granular, per `(ObjectType, u32)` -- gate-a-pair-granularity
/// handoff v1: a type-granular list here would let a *second* schema on an already-declared type
/// slip through unvectored just as silently as `frozen` below did before this fix.
const RFC114_ADMITTED_BUT_UNWRITTEN: &[(ObjectType, u32)] = &[(ObjectType::Attestation, 1)];

/// Every `ObjectType` this crate has. RFC 118 stage 6: this used to be a hand-copied list,
/// checked against the real enum by a separate exhaustive-match test (`all_object_types_is_exhaustive`,
/// removed) -- a hand-written list can miss a variant (`file_codec/tests.rs`'s own list did,
/// silently, for `RecognitionClaim`), so this is now `ObjectType::ALL` itself, generated by the
/// enum's own defining macro. There is no separate list left to drift, and so no test needed to
/// prove it hasn't: `ALL`'s completeness relative to the enum is structural, not asserted.
const ALL_OBJECT_TYPES: &[ObjectType] = ObjectType::ALL;

/// RFC 114 §3's completeness self-guard: call the *real* `validate_format2_schema` -- not a copy
/// of its logic -- for every `(object_type, schema_version)` pair across a schema-version sweep,
/// and assert every pair it admits is either frozen with an identity vector above or declared
/// deliberately unwritten in `RFC114_ADMITTED_BUT_UNWRITTEN`. This is the guard that fails the day
/// something starts writing `Attestation` (or any newly-admitted pair) without a vector following
/// it in the same change.
///
/// **Pair-granular by construction** (gate-a-pair-granularity handoff v1, `8c31a78`'s own review):
/// both `frozen` and `RFC114_ADMITTED_BUT_UNWRITTEN` are `(ObjectType, u32)` lists and the
/// assertion below checks membership of the whole pair, not just `object_type` -- so a second
/// admitted schema on an already-declared type (as `RefState` and `Patch` both now are) is
/// checked independently of its type's first schema, rather than riding along for free on it.
///
/// **Sweep bound derived, not hardcoded** (gate-a-sweep-bound handoff v1): the upper bound is
/// `SWEEP_MARGIN_ABOVE_HIGHEST_ADMITTED` schema versions above whatever `admitted_schemas` reports
/// as the highest schema any type actually admits today, so the guard's real coverage grows
/// automatically the day a third schema is admitted on any type, instead of silently stopping at a
/// number picked for today's schemas. This reads `admitted_schemas` only to size the *probe range*
/// -- the completeness predicate itself (`frozen`/`RFC114_ADMITTED_BUT_UNWRITTEN` membership) stays
/// hand-maintained and independent, so this is not the "guard derived from the thing it guards"
/// trap `f1528b8` already caught once.
const SWEEP_MARGIN_ABOVE_HIGHEST_ADMITTED: u32 = 4;

#[test]
fn rfc114_gate_a_every_admitted_pair_is_frozen_or_declared_unwritten() {
    let frozen: &[(ObjectType, u32)] = &[
        (ObjectType::Block, 2),
        (ObjectType::RefState, 1),
        (ObjectType::RefState, prikk_object::REF_STATE_CLOSED_SCHEMA),
        (ObjectType::Patch, 1),
        (
            ObjectType::Patch,
            prikk_object::PATCH_PARENT_IDS_RETIRED_SCHEMA,
        ),
        (ObjectType::Patch, prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA),
        (ObjectType::Patch, prikk_object::PATCH_MESSAGE_SCHEMA),
        (ObjectType::RefUpdate, 1),
        (ObjectType::Tag, 1),
        (ObjectType::Blob, 1),
        (ObjectType::RecognitionClaim, 1),
    ];
    let highest_admitted = ALL_OBJECT_TYPES
        .iter()
        .filter_map(|&object_type| crate::format::admitted_schemas(object_type))
        .flat_map(|schemas| schemas.iter().copied())
        .max()
        .unwrap_or(0);
    let sweep_upper_bound = highest_admitted + SWEEP_MARGIN_ABOVE_HIGHEST_ADMITTED;
    for &object_type in ALL_OBJECT_TYPES {
        for schema_version in 0..=sweep_upper_bound {
            let envelope = ObjectEnvelope::unsigned(object_type, schema_version, Vec::new());
            if crate::format::validate_format2_schema(&envelope).is_ok() {
                let pair = (object_type, schema_version);
                assert!(
                    frozen.contains(&pair) || RFC114_ADMITTED_BUT_UNWRITTEN.contains(&pair),
                    "({object_type:?}, {schema_version}) is admitted by \
                     validate_format2_schema but is neither frozen with an identity vector nor \
                     declared unwritten in RFC114_ADMITTED_BUT_UNWRITTEN -- if production now \
                     writes this pair, it needs a vector; if not, add it to the unwritten list \
                     with a reason"
                );
            }
        }
    }
}

const RFC114_MAINTAINER_KEY_ID: &str = "rfc114-vector-maintainer";
const RFC114_MAINTAINER_SEED: [u8; 32] = [0x99; 32];
const RFC114_MAINTAINER_PUBLIC_KEY: [u8; 32] = [
    0x33, 0x2e, 0xbe, 0x8d, 0x27, 0xcb, 0x73, 0x23, 0xb3, 0xa4, 0x01, 0xc1, 0xc1, 0x3b, 0x5d, 0xd6,
    0x4b, 0xcc, 0xc0, 0xe1, 0x0e, 0xcd, 0xa1, 0xc2, 0xb5, 0xd1, 0x1a, 0x03, 0x77, 0x9a, 0x85, 0xe5,
];

#[test]
fn rfc114_maintainer_seed_public_key_matches() {
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).public_key_bytes(),
        RFC114_MAINTAINER_PUBLIC_KEY
    );
}

fn rfc114_maintainer_preimage(
    object_type: ObjectType,
    id: ObjectId,
) -> prikk_error::Result<Vec<u8>> {
    Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        object_type,
        id,
        SignerRole::Maintainer,
        RFC114_MAINTAINER_KEY_ID,
    )
}

/// Vector 7: `(Block, 2)`, MAINTAINER-signed.
fn rfc114_block_payload() -> prikk_object::BlockPayload {
    prikk_object::BlockPayload {
        parent_block_ids: vec![ObjectId::from_bytes([0x71; 32])],
        kind: prikk_object::BlockKind::Normal,
        patch_ids: vec![ObjectId::from_bytes([0x72; 32])],
        state_merkle_root: prikk_object::MerkleRoot([0x73; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    }
}

const RFC114_BLOCK_SIGNATURE: [u8; 64] = [
    0x26, 0x1e, 0x09, 0xdb, 0x6f, 0x6d, 0xac, 0x38, 0xaf, 0x67, 0x41, 0x71, 0x7f, 0x9e, 0x15, 0x29,
    0x1c, 0x0c, 0x50, 0xde, 0x9e, 0x4c, 0x95, 0x35, 0x24, 0xba, 0x6f, 0xa8, 0xe5, 0xd0, 0x51, 0xe6,
    0xfb, 0xe9, 0xce, 0xc2, 0xf8, 0x4f, 0xce, 0xa0, 0xb6, 0x75, 0xf5, 0x5e, 0xb5, 0xfb, 0x4c, 0xf4,
    0x5a, 0x18, 0xbf, 0x41, 0x04, 0x22, 0xde, 0xc5, 0xec, 0x10, 0x31, 0x6c, 0xbf, 0x0d, 0xdd, 0x08,
];

#[test]
fn rfc114_vector_7_block_schema_2_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_block_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "00011200000000000000207171717171717171717171717171717171717171717171717171717171717171000205000000000000000200020003120000000000000020727272727272727272727272727272727272727272727272727272727272727200041100000000000000207373737373737373737373737373737373737373737373737373737373737373"
    );
    let id = ObjectId::from_canonical_payload(ObjectType::Block, 2, &canonical);
    assert_eq!(
        id.to_string(),
        "d65d3453105222235153474fee0cc2ddf4c70860a29e85873c8987c462bd44b3"
    );
    let preimage = rfc114_maintainer_preimage(ObjectType::Block, id)?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010002d65d3453105222235153474fee0cc2ddf4c70860a29e85873c8987c462bd44b3000200187266633131342d766563746f722d6d61696e7461696e6572"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage),
        RFC114_BLOCK_SIGNATURE
    );
    verify_ed25519(
        &RFC114_MAINTAINER_PUBLIC_KEY,
        &preimage,
        &RFC114_BLOCK_SIGNATURE,
    )
}

/// Vector 8: `(RefState, 1)`, the open (non-closed, DC-61 pre-existing) schema, MAINTAINER-signed.
fn rfc114_ref_state_open_payload() -> prikk_object::RefStatePayload {
    prikk_object::RefStatePayload {
        ref_name: "heads/rfc114-vector".to_string(),
        kind: prikk_object::RefKind::Branch,
        target_object_id: ObjectId::from_bytes([0x74; 32]),
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    }
}

const RFC114_REF_STATE_OPEN_SIGNATURE: [u8; 64] = [
    0xeb, 0x96, 0x5c, 0xb9, 0xb6, 0x6a, 0x08, 0x71, 0x00, 0xcd, 0xf6, 0xbf, 0xcc, 0x16, 0x86, 0x14,
    0x3d, 0xed, 0xc4, 0x27, 0xb6, 0x7a, 0x92, 0xa7, 0x81, 0xba, 0x07, 0x56, 0x15, 0xca, 0xca, 0x2d,
    0x79, 0x9c, 0xad, 0xd4, 0xf4, 0x13, 0xd2, 0x51, 0x6f, 0x01, 0x05, 0x95, 0xda, 0xac, 0x0f, 0x56,
    0xac, 0xa9, 0x0e, 0x35, 0x9d, 0xe9, 0x5f, 0x8b, 0xfa, 0xa6, 0x09, 0xc8, 0xa3, 0xc8, 0x6c, 0x01,
];

#[test]
fn rfc114_vector_8_ref_state_schema_1_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_ref_state_open_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "000110000000000000001368656164732f7266633131342d766563746f72000212000000000000002074747474747474747474747474747474747474747474747474747474747474740003040000000000000008000000000000000100060500000000000000020001"
    );
    let id = ObjectId::from_canonical_payload(ObjectType::RefState, 1, &canonical);
    assert_eq!(
        id.to_string(),
        "3d15f5ee8046b7ef3ef5ae0c876984b95b69407f6c6c3494764c5ad149f06033"
    );
    let preimage = rfc114_maintainer_preimage(ObjectType::RefState, id)?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e7631000100033d15f5ee8046b7ef3ef5ae0c876984b95b69407f6c6c3494764c5ad149f06033000200187266633131342d766563746f722d6d61696e7461696e6572"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage),
        RFC114_REF_STATE_OPEN_SIGNATURE
    );
    verify_ed25519(
        &RFC114_MAINTAINER_PUBLIC_KEY,
        &preimage,
        &RFC114_REF_STATE_OPEN_SIGNATURE,
    )
}

/// Vector 9: `(RefState, REF_STATE_CLOSED_SCHEMA)`, DC-61's `closed` field, MAINTAINER-signed.
fn rfc114_ref_state_closed_payload() -> prikk_object::RefStatePayload {
    prikk_object::RefStatePayload {
        ref_name: "heads/rfc114-vector".to_string(),
        kind: prikk_object::RefKind::Branch,
        target_object_id: ObjectId::from_bytes([0x74; 32]),
        update_seq: 2,
        previous_ref_state_id: Some(ObjectId::from_bytes([0x75; 32])),
        required_attestation_ids: Vec::new(),
        closed: true,
    }
}

const RFC114_REF_STATE_CLOSED_SIGNATURE: [u8; 64] = [
    0x3d, 0xfe, 0xc4, 0x4e, 0x2b, 0xcd, 0x6f, 0xd8, 0x71, 0x85, 0xe5, 0xd7, 0xcd, 0xc0, 0x6e, 0x5b,
    0x6e, 0xc9, 0x40, 0x94, 0x19, 0x8a, 0x94, 0xd7, 0x7c, 0x81, 0x72, 0x1a, 0xdb, 0x50, 0x3a, 0x4c,
    0x17, 0x36, 0x99, 0xed, 0x24, 0xd4, 0x9a, 0x3e, 0x8c, 0xda, 0x69, 0xf1, 0xe0, 0x3a, 0x3b, 0x71,
    0xd1, 0x2c, 0x00, 0x0a, 0x6f, 0x14, 0x29, 0x1d, 0xa1, 0x43, 0x7b, 0xcd, 0xcd, 0x89, 0x14, 0x0a,
];

#[test]
fn rfc114_vector_9_ref_state_closed_schema_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_ref_state_closed_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "000110000000000000001368656164732f7266633131342d766563746f7200021200000000000000207474747474747474747474747474747474747474747474747474747474747474000304000000000000000800000000000000020004120000000000000020757575757575757575757575757575757575757575757575757575757575757500060500000000000000020001000701000000000000000101"
    );
    let id = ObjectId::from_canonical_payload(
        ObjectType::RefState,
        prikk_object::REF_STATE_CLOSED_SCHEMA,
        &canonical,
    );
    assert_eq!(
        id.to_string(),
        "ae835d10ed0e3cbf1cbdf28ea1039656c2e66d6b6e3b645391226625ab912b5c"
    );
    let preimage = rfc114_maintainer_preimage(ObjectType::RefState, id)?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010003ae835d10ed0e3cbf1cbdf28ea1039656c2e66d6b6e3b645391226625ab912b5c000200187266633131342d766563746f722d6d61696e7461696e6572"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage),
        RFC114_REF_STATE_CLOSED_SIGNATURE
    );
    verify_ed25519(
        &RFC114_MAINTAINER_PUBLIC_KEY,
        &preimage,
        &RFC114_REF_STATE_CLOSED_SIGNATURE,
    )
}

/// Vector 10: `(RefUpdate, 1)` -- inline in the ref log, not container-stored, but routed through
/// the same `validate_read_schema` gate (`refs.rs`, `wal.rs`), so it is identity-bearing the same
/// way. MAINTAINER-signed.
fn rfc114_ref_update_payload() -> prikk_object::RefUpdatePayload {
    prikk_object::RefUpdatePayload {
        ref_name: "heads/rfc114-vector".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ObjectId::from_bytes([0x76; 32]),
        new_target_object_id: ObjectId::from_bytes([0x77; 32]),
        update_seq: 1,
        created_at: 0,
        author_key_id: "rfc114-vector-key".to_string(),
    }
}

const RFC114_REF_UPDATE_SIGNATURE: [u8; 64] = [
    0x5b, 0x2e, 0x73, 0x94, 0xb6, 0xb4, 0xd2, 0xcb, 0x39, 0x94, 0x4a, 0xb4, 0x26, 0xf6, 0x56, 0xbd,
    0xa1, 0x9b, 0xed, 0x4d, 0x49, 0x12, 0xa8, 0xf5, 0xc1, 0xde, 0xc2, 0xfb, 0x08, 0x0a, 0x00, 0xd0,
    0x92, 0x13, 0x64, 0xc3, 0x87, 0x7a, 0x19, 0x5b, 0xd5, 0x12, 0x5f, 0xb8, 0xae, 0x06, 0xfe, 0xc0,
    0x3e, 0x37, 0x1b, 0x8d, 0x0d, 0x47, 0xd0, 0xc0, 0x1d, 0xe0, 0xc0, 0xef, 0x7d, 0xba, 0x88, 0x04,
];

#[test]
fn rfc114_vector_10_ref_update_schema_1_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_ref_update_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "000110000000000000001368656164732f7266633131342d766563746f720003120000000000000020767676767676767676767676767676767676767676767676767676767676767600041200000000000000207777777777777777777777777777777777777777777777777777777777777777000504000000000000000800000000000000010006040000000000000008000000000000000000071000000000000000117266633131342d766563746f722d6b6579"
    );
    let id = ObjectId::from_canonical_payload(ObjectType::RefUpdate, 1, &canonical);
    assert_eq!(
        id.to_string(),
        "521f0f9383e7793211053551f6503595e55e8e53184f59b52d5328f2f9aab307"
    );
    let preimage = rfc114_maintainer_preimage(ObjectType::RefUpdate, id)?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010004521f0f9383e7793211053551f6503595e55e8e53184f59b52d5328f2f9aab307000200187266633131342d766563746f722d6d61696e7461696e6572"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage),
        RFC114_REF_UPDATE_SIGNATURE
    );
    verify_ed25519(
        &RFC114_MAINTAINER_PUBLIC_KEY,
        &preimage,
        &RFC114_REF_UPDATE_SIGNATURE,
    )
}

/// Vector 11: `(Tag, 1)`, MAINTAINER-signed.
///
/// **Moved twice, both authorized, both recorded here so the record shows when and under which
/// ruling each time (RFC 117 T7 §4's own instruction -- a vector that moved twice with only one
/// note reads like it moved once):**
///
/// - **Move 1, RFC 117 T1 (2026-08-22, owner ruling -- "No project has been created in production
///   in the world yet. Breaking change is accepted."):** `TagPayload` gained a required field 6,
///   `patch_set_digest`, amended in place at `schema_version` 1 (no schema 2).
/// - **Move 2, RFC 117 T7 (2026-08-22, owner ruling "Take it now", after stage 2 measured
///   resolution at O(N²)):** `TagPayload` gained a required field 7, `patch_count`, same terms.
///
/// Both moves are fixed, arbitrary values -- this vector's `target_block_id` names no real block
/// for a real digest or count to be computed over; only their presence and fixedness matter for
/// freezing identity, not their relationship to any block.
fn rfc114_tag_payload() -> prikk_object::TagPayload {
    prikk_object::TagPayload {
        name: "rfc114-vector".to_string(),
        target_block_id: ObjectId::from_bytes([0x78; 32]),
        message: None,
        created_at: 0,
        author_key_id: "rfc114-vector-key".to_string(),
        patch_set_digest: prikk_object::PatchSetDigest([0x7b; 32]),
        patch_count: 7,
    }
}

// RFC 117 T1 (2026-08-22, owner ruling), then RFC 117 T7 (2026-08-22, owner ruling "Take it now"):
// moved twice, deliberately, alongside the canonical bytes, object id, and preimage above -- see
// `rfc114_vector_11_tag_schema_1_identity_and_signature`'s own comment for both rulings.
const RFC114_TAG_SIGNATURE: [u8; 64] = [
    0x54, 0xd4, 0xf9, 0x43, 0x82, 0x57, 0x87, 0xfd, 0x63, 0x1b, 0x7f, 0xf6, 0x83, 0x0e, 0x16, 0x32,
    0xa3, 0x44, 0x68, 0xab, 0x81, 0xda, 0x00, 0x86, 0x7c, 0x92, 0xab, 0x00, 0xe7, 0x4a, 0x01, 0x0b,
    0x52, 0x41, 0x5f, 0xb2, 0xe9, 0x3c, 0x28, 0xe6, 0xa6, 0x95, 0x24, 0xee, 0x31, 0x87, 0x0a, 0x10,
    0xf4, 0xad, 0xeb, 0xeb, 0x9e, 0x6d, 0xb2, 0x84, 0xde, 0xab, 0x58, 0xd1, 0x26, 0x1d, 0xcb, 0x08,
];

#[test]
fn rfc114_vector_11_tag_schema_1_identity_and_signature() -> prikk_error::Result<()> {
    // Moved a second time here, RFC 117 T7 (2026-08-22, owner ruling "Take it now"): `TagPayload`
    // gained a required field 7, `patch_count`, at `schema_version` 1 -- see this test's own
    // struct-level doc comment (`rfc114_tag_payload`) for both rulings and both moves. Computed
    // once via a throwaway probe test and hardcoded here, the same discipline every other vector in
    // this file follows.
    let canonical = rfc114_tag_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "000110000000000000000d7266633131342d766563746f72000212000000000000002078787878787878787878787878787878787878787878787878787878787878780004040000000000000008000000000000000000051000000000000000117266633131342d766563746f722d6b657900061100000000000000207b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b00070400000000000000080000000000000007"
    );
    let id = ObjectId::from_canonical_payload(ObjectType::Tag, 1, &canonical);
    assert_eq!(
        id.to_string(),
        "e50d6117efac991dbd43a9f5e2fbae365ba3a3f68f4de58a0384ee2a51942878"
    );
    let preimage = rfc114_maintainer_preimage(ObjectType::Tag, id)?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010005e50d6117efac991dbd43a9f5e2fbae365ba3a3f68f4de58a0384ee2a51942878000200187266633131342d766563746f722d6d61696e7461696e6572"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage),
        RFC114_TAG_SIGNATURE
    );
    verify_ed25519(
        &RFC114_MAINTAINER_PUBLIC_KEY,
        &preimage,
        &RFC114_TAG_SIGNATURE,
    )
}

/// Vector 12: `(Blob, 1)`, **unsigned** -- `node_authoring.rs` constructs `Blob` via
/// `ObjectEnvelope::unsigned`, so this vector freezes identity only, no signature preimage.
fn rfc114_blob_payload() -> prikk_object::BlobPayload {
    prikk_object::BlobPayload {
        blob_kind: prikk_object::BlobKind::Text,
        content: b"rfc114 vector\n".to_vec(),
        declared_size: 14,
    }
}

#[test]
fn rfc114_vector_12_blob_schema_1_identity() -> prikk_error::Result<()> {
    let canonical = rfc114_blob_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "00010500000000000000020001000211000000000000000e72666331313420766563746f720a0003040000000000000008000000000000000e"
    );
    let id = ObjectId::from_canonical_payload(ObjectType::Blob, 1, &canonical);
    assert_eq!(
        id.to_string(),
        "95a222e07ad6730efb2430aa7a20cd70cd9113687ad5715694577e3473610e4a"
    );
    Ok(())
}

/// Vector 13: `(RecognitionClaim, 1)`, RFC 115 Stage 2 (design-v1.md D3), MAINTAINER-signed.
fn rfc114_recognition_claim_payload() -> prikk_object::RecognitionClaimPayload {
    prikk_object::RecognitionClaimPayload {
        block_id: ObjectId::from_bytes([0x79; 32]),
        patch_ids: vec![ObjectId::from_bytes([0x7a; 32])],
        parent_block_ids: Vec::new(),
    }
}

const RFC114_RECOGNITION_CLAIM_SIGNATURE: [u8; 64] = [
    0x6b, 0x55, 0xdd, 0xaa, 0x45, 0x51, 0x28, 0x33, 0x88, 0xe8, 0xa1, 0x92, 0xf5, 0xae, 0xf8, 0xe1,
    0xae, 0x7a, 0x4e, 0x7b, 0x94, 0x79, 0xb0, 0x97, 0xa8, 0xcf, 0x8c, 0xd9, 0xba, 0xad, 0x6c, 0x89,
    0x5c, 0x0c, 0x6c, 0x5e, 0xac, 0xb3, 0x30, 0xf6, 0x39, 0x45, 0xf6, 0xca, 0x47, 0x92, 0xf7, 0x41,
    0x58, 0x15, 0xa5, 0xb2, 0x07, 0x82, 0xc3, 0x87, 0x4c, 0x2b, 0x2e, 0xca, 0x82, 0x67, 0x0b, 0x06,
];

#[test]
fn rfc114_vector_13_recognition_claim_schema_1_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_recognition_claim_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "0001120000000000000020797979797979797979797979797979797979797979797979797979797979797900021200000000000000207a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a"
    );
    let id = ObjectId::from_canonical_payload(ObjectType::RecognitionClaim, 1, &canonical);
    assert_eq!(
        id.to_string(),
        "bd29bf5710e04a595c6309bf2ab7c3bca1e28f8e40da796e1a32ef5f3931528f"
    );
    let preimage = rfc114_maintainer_preimage(ObjectType::RecognitionClaim, id)?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e76310001000bbd29bf5710e04a595c6309bf2ab7c3bca1e28f8e40da796e1a32ef5f3931528f000200187266633131342d766563746f722d6d61696e7461696e6572"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage),
        RFC114_RECOGNITION_CLAIM_SIGNATURE
    );
    verify_ed25519(
        &RFC114_MAINTAINER_PUBLIC_KEY,
        &preimage,
        &RFC114_RECOGNITION_CLAIM_SIGNATURE,
    )
}

/// RFC 115 Stage 2 §7 row 1: a claim signed by key K verifies only against K's material -- a
/// signature genuinely valid against the real maintainer key must fail against a different one.
#[test]
fn rfc115_recognition_claim_signature_fails_against_a_different_maintainer_key()
-> prikk_error::Result<()> {
    let canonical = rfc114_recognition_claim_payload().to_canonical_bytes()?;
    let id = ObjectId::from_canonical_payload(ObjectType::RecognitionClaim, 1, &canonical);
    let preimage = rfc114_maintainer_preimage(ObjectType::RecognitionClaim, id)?;
    let signature = Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage);
    // Sanity: genuinely valid against the real key -- this is a wrong-key test, not a malformed one.
    verify_ed25519(&RFC114_MAINTAINER_PUBLIC_KEY, &preimage, &signature)?;

    let other_public_key = Ed25519KeyPair::from_seed(&[0x9a; 32]).public_key_bytes();
    assert_ne!(other_public_key, RFC114_MAINTAINER_PUBLIC_KEY);
    assert!(
        verify_ed25519(&other_public_key, &preimage, &signature).is_err(),
        "a signature genuinely valid against the real maintainer key must fail against a \
         different one"
    );
    Ok(())
}

/// Vector 14: `(Patch, PATCH_PARENT_IDS_RETIRED_SCHEMA)`, the patch-schema-2 handoff (tag 2,
/// `parent_patch_ids`, retired outright) -- AUTHOR-signed, mirroring vectors 1-3's `(Patch, 1)`
/// pattern above (`dc53_patch_payload`) rather than the maintainer-signed vectors 7-13, since
/// Patch objects are author-signed in production, same as schema 1.
fn rfc114_patch_schema2_payload() -> PatchPayload {
    PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "rfc114-patch-schema2.txt".to_string(),
                node_id: NodeId::from_bytes([0x7c; 32]),
                blob_id: ObjectId::from_bytes([0x7d; 32]),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    }
}

const RFC114_PATCH_SCHEMA2_KEY_ID: &str = "rfc114-patch-schema2-author";
const RFC114_PATCH_SCHEMA2_AUTHOR_SEED: [u8; 32] = [0x7e; 32];

const RFC114_PATCH_SCHEMA2_PUBLIC_KEY: [u8; 32] = [
    0x51, 0xbf, 0xae, 0x7f, 0xa9, 0x2c, 0x51, 0x55, 0x12, 0xcd, 0x3a, 0xd4, 0x42, 0x76, 0x0e, 0x41,
    0x61, 0x2e, 0x59, 0xdf, 0xec, 0x39, 0x13, 0xf6, 0x70, 0xb7, 0x21, 0xc0, 0x42, 0x55, 0xb0, 0x68,
];

const RFC114_PATCH_SCHEMA2_SIGNATURE: [u8; 64] = [
    0x1b, 0xf8, 0xf2, 0x5d, 0x59, 0xb3, 0xd4, 0x98, 0xcd, 0xd2, 0x58, 0xbd, 0xa1, 0x37, 0x1d, 0xcd,
    0x40, 0xd4, 0xd6, 0xb4, 0x70, 0xe0, 0xf8, 0xce, 0x53, 0xa0, 0xcd, 0xf9, 0xa5, 0x75, 0xbe, 0x17,
    0xc5, 0xc8, 0xb3, 0x52, 0x5b, 0x23, 0x09, 0xfa, 0xb8, 0x40, 0x90, 0x1f, 0x14, 0x79, 0xb2, 0x7b,
    0x5e, 0x76, 0xfe, 0x99, 0xcd, 0xd8, 0xb2, 0xfd, 0x80, 0xf8, 0x92, 0x26, 0x5c, 0x7f, 0xac, 0x0e,
];

#[test]
fn rfc114_vector_14_patch_schema_2_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_patch_schema2_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "00012100000000000000a2000103000000000000000400000001000a20000000000000008800011300000000000000187266633131342d70617463682d736368656d61322e74787400021100000000000000207c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c00031200000000000000207d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d0004030000000000000004000081a4"
    );
    let id = ObjectId::from_canonical_payload(
        ObjectType::Patch,
        prikk_object::PATCH_PARENT_IDS_RETIRED_SCHEMA,
        &canonical,
    );
    assert_eq!(
        id.to_string(),
        "a454c4eb4bdc3acd8b4518e44031130c4b86e8f05cb0484f39c21d31df251b44"
    );
    let preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        id,
        SignerRole::Author,
        RFC114_PATCH_SCHEMA2_KEY_ID,
    )?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010001a454c4eb4bdc3acd8b4518e44031130c4b86e8f05cb0484f39c21d31df251b440001001b7266633131342d70617463682d736368656d61322d617574686f72"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_PATCH_SCHEMA2_AUTHOR_SEED).public_key_bytes(),
        RFC114_PATCH_SCHEMA2_PUBLIC_KEY
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_PATCH_SCHEMA2_AUTHOR_SEED).sign(&preimage),
        RFC114_PATCH_SCHEMA2_SIGNATURE
    );
    verify_ed25519(
        &RFC114_PATCH_SCHEMA2_PUBLIC_KEY,
        &preimage,
        &RFC114_PATCH_SCHEMA2_SIGNATURE,
    )
}

/// Vector 15: `(Patch, PATCH_TEXT_SPAN_V2_SCHEMA)`, RFC 134 §8 (`EditText` tags 10/11,
/// content-unique anchor lengths) -- AUTHOR-signed, same pattern as vector 14.
fn rfc114_patch_schema3_payload() -> PatchPayload {
    PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::EditText(EditText {
                node_id: NodeId::from_bytes([0x8a; 32]),
                span_id: [0x8b; 32],
                old_span_hash: prikk_object::text_span_hash(b"old"),
                left_anchor_hash: [0x8c; 32],
                right_anchor_hash: [0x8d; 32],
                replacement_text: b"new".to_vec(),
                presentation_hint_line: None,
                presentation_hint_column: None,
                old_span_text: b"old".to_vec(),
                left_anchor_len: Some(64),
                right_anchor_len: Some(96),
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    }
}

const RFC114_PATCH_SCHEMA3_KEY_ID: &str = "rfc114-patch-schema3-author";
const RFC114_PATCH_SCHEMA3_AUTHOR_SEED: [u8; 32] = [0x8e; 32];

const RFC114_PATCH_SCHEMA3_PUBLIC_KEY: [u8; 32] = [
    0xab, 0x3f, 0xa2, 0xfd, 0xab, 0x77, 0xf9, 0x87, 0x06, 0xc7, 0x66, 0x97, 0xb4, 0x2b, 0xc9, 0x9d,
    0x7b, 0x5f, 0x96, 0x7c, 0xad, 0xcd, 0x59, 0xc1, 0x96, 0x33, 0xea, 0x50, 0x2f, 0xd1, 0xdc, 0xf3,
];

const RFC114_PATCH_SCHEMA3_SIGNATURE: [u8; 64] = [
    0x71, 0xfc, 0x19, 0xab, 0x95, 0x29, 0x3b, 0x63, 0xa7, 0x46, 0x40, 0x99, 0x56, 0x33, 0x50, 0xed,
    0xd5, 0x20, 0xe9, 0x4b, 0x88, 0x54, 0xc0, 0x73, 0x97, 0x02, 0xa6, 0xa7, 0x13, 0x92, 0x90, 0x46,
    0xda, 0xc3, 0xb8, 0xc7, 0x3f, 0xb2, 0x19, 0xd9, 0x2e, 0x26, 0xdb, 0xd1, 0xfa, 0x1b, 0xe2, 0x7a,
    0xc4, 0xc1, 0x27, 0x22, 0xc2, 0xc7, 0x11, 0xcf, 0xdb, 0xa5, 0x62, 0xe8, 0x0d, 0xb4, 0xcc, 0x08,
];

#[test]
fn rfc114_vector_15_patch_schema_3_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_patch_schema3_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "000121000000000000012b000103000000000000000400000001000c20000000000000011100011100000000000000208a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a00021100000000000000208b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b0003110000000000000020cba06b5736faf67e54b07b561eae94395e774c517a7d910a54369e1263ccfbd400041100000000000000208c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c00051100000000000000208d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d8d00061100000000000000036e657700091100000000000000036f6c64000a03000000000000000400000040000b03000000000000000400000060"
    );
    let id = ObjectId::from_canonical_payload(
        ObjectType::Patch,
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
        &canonical,
    );
    assert_eq!(
        id.to_string(),
        "51e8cff27ec4f590728244312bbe01897deb50017dbcc815312fd38cf8c09ce9"
    );
    let preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        id,
        SignerRole::Author,
        RFC114_PATCH_SCHEMA3_KEY_ID,
    )?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e76310001000151e8cff27ec4f590728244312bbe01897deb50017dbcc815312fd38cf8c09ce90001001b7266633131342d70617463682d736368656d61332d617574686f72"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_PATCH_SCHEMA3_AUTHOR_SEED).public_key_bytes(),
        RFC114_PATCH_SCHEMA3_PUBLIC_KEY
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_PATCH_SCHEMA3_AUTHOR_SEED).sign(&preimage),
        RFC114_PATCH_SCHEMA3_SIGNATURE
    );
    verify_ed25519(
        &RFC114_PATCH_SCHEMA3_PUBLIC_KEY,
        &preimage,
        &RFC114_PATCH_SCHEMA3_SIGNATURE,
    )
}

/// Vector 16: `(Patch, PATCH_MESSAGE_SCHEMA)`, RFC 123 §8 (`message`, tag 6) -- AUTHOR-signed, same
/// pattern as vectors 14-15.
fn rfc114_patch_schema4_payload() -> PatchPayload {
    PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "rfc114-patch-schema4.txt".to_string(),
                node_id: NodeId::from_bytes([0x9a; 32]),
                blob_id: ObjectId::from_bytes([0x9b; 32]),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: Some("rfc123 vector".to_string()),
    }
}

const RFC114_PATCH_SCHEMA4_KEY_ID: &str = "rfc114-patch-schema4-author";
const RFC114_PATCH_SCHEMA4_AUTHOR_SEED: [u8; 32] = [0x9c; 32];

const RFC114_PATCH_SCHEMA4_PUBLIC_KEY: [u8; 32] = [
    0x23, 0xb6, 0x31, 0x70, 0x9d, 0xf2, 0x28, 0x32, 0xa0, 0x4b, 0x29, 0xa1, 0x67, 0x7a, 0xb7, 0x61,
    0x05, 0x8e, 0xcb, 0x42, 0x32, 0x9e, 0x14, 0xfd, 0x48, 0x42, 0x69, 0xe6, 0x3f, 0xe5, 0x5c, 0x60,
];

const RFC114_PATCH_SCHEMA4_SIGNATURE: [u8; 64] = [
    0x5c, 0x75, 0xfb, 0x93, 0x47, 0xf6, 0x5b, 0xd1, 0xcd, 0xd8, 0xcb, 0x12, 0x0a, 0x2d, 0xea, 0x18,
    0x79, 0xb6, 0x41, 0x14, 0xd0, 0x51, 0x75, 0xfc, 0xfe, 0xab, 0x38, 0xbf, 0x2b, 0x94, 0x84, 0xfd,
    0x28, 0xf6, 0xad, 0x77, 0xd7, 0xaa, 0x9d, 0x2d, 0x21, 0xa8, 0xfc, 0x45, 0xd7, 0xcd, 0x69, 0xb9,
    0xd5, 0xbb, 0xf9, 0x9e, 0x3b, 0xb1, 0x15, 0x82, 0x4f, 0x5f, 0xfc, 0x5c, 0xb8, 0x7b, 0x95, 0x02,
];

#[test]
fn rfc114_vector_16_patch_schema_4_identity_and_signature() -> prikk_error::Result<()> {
    let canonical = rfc114_patch_schema4_payload().to_canonical_bytes()?;
    assert_eq!(
        prikk_hash::to_hex(&canonical),
        "00012100000000000000a2000103000000000000000400000001000a20000000000000008800011300000000000000187266633131342d70617463682d736368656d61342e74787400021100000000000000209a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a00031200000000000000209b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b9b0004030000000000000004000081a4000610000000000000000d72666331323320766563746f72"
    );
    let id = ObjectId::from_canonical_payload(
        ObjectType::Patch,
        prikk_object::PATCH_MESSAGE_SCHEMA,
        &canonical,
    );
    assert_eq!(
        id.to_string(),
        "788368756085523fbfcca01669d9ec7959e5e145bbe0ade4815074a8b50edb86"
    );
    let preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        id,
        SignerRole::Author,
        RFC114_PATCH_SCHEMA4_KEY_ID,
    )?;
    assert_eq!(
        prikk_hash::to_hex(&preimage),
        "7072696b6b2e7369672e763100010001788368756085523fbfcca01669d9ec7959e5e145bbe0ade4815074a8b50edb860001001b7266633131342d70617463682d736368656d61342d617574686f72"
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_PATCH_SCHEMA4_AUTHOR_SEED).public_key_bytes(),
        RFC114_PATCH_SCHEMA4_PUBLIC_KEY
    );
    assert_eq!(
        Ed25519KeyPair::from_seed(&RFC114_PATCH_SCHEMA4_AUTHOR_SEED).sign(&preimage),
        RFC114_PATCH_SCHEMA4_SIGNATURE
    );
    verify_ed25519(
        &RFC114_PATCH_SCHEMA4_PUBLIC_KEY,
        &preimage,
        &RFC114_PATCH_SCHEMA4_SIGNATURE,
    )
}

/// RFC 115 Stage 2 §7 row 2: a signature over a `RecognitionClaim` cannot be presented as a
/// signature over any other type. Generically covered already by
/// `every_signature_preimage_field_is_cryptographically_bound` (flips a byte at offset 14, inside
/// `object_type`); this exercises the same domain-separation property for this specific type
/// through the production signing path, so the relationship is on the record rather than the two
/// tests later being read as duplicates.
#[test]
fn rfc115_recognition_claim_signature_is_not_valid_for_another_object_type()
-> prikk_error::Result<()> {
    let canonical = rfc114_recognition_claim_payload().to_canonical_bytes()?;
    let id = ObjectId::from_canonical_payload(ObjectType::RecognitionClaim, 1, &canonical);
    let preimage = rfc114_maintainer_preimage(ObjectType::RecognitionClaim, id)?;
    let signature = Ed25519KeyPair::from_seed(&RFC114_MAINTAINER_SEED).sign(&preimage);
    verify_ed25519(&RFC114_MAINTAINER_PUBLIC_KEY, &preimage, &signature)?;

    let block_preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Block,
        id,
        SignerRole::Maintainer,
        RFC114_MAINTAINER_KEY_ID,
    )?;
    assert_ne!(preimage, block_preimage);
    assert!(
        verify_ed25519(&RFC114_MAINTAINER_PUBLIC_KEY, &block_preimage, &signature).is_err(),
        "a signature over a RecognitionClaim preimage must not verify against the same id/role/key \
         rebuilt with ObjectType::Block"
    );
    Ok(())
}
