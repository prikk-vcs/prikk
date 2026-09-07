//! DC-41 stage 4 property tests: envelope framing (target 1 of 5).
//!
//! Covers every live `ObjectType` variant (`ObjectType::ALL`, RFC 118 stage 6) -- until that
//! stage, this file held its own hand-copied nine-member list, missing `RecognitionClaim` (the
//! one type that actually travels on the sync wire) since the day it was added; the omission is
//! now closed by construction, not by being noticed a second time. Two properties, per the RFC:
//! round-trip (`decode(encode(x)) == x`) for structurally valid envelopes, and totality (never
//! panics) for arbitrary bytes fed to the decoder. A third check pins envelope-layer admission
//! against the DC-40 format-2 schema allowlist (`crate::format::validate_format2_schema`) so the
//! two variants excluded from format-2 identity positions (`BlockSummaryCache`, `RecoveryNote`)
//! are exercised on the rejection path rather than skipped.
//!
//! Case budget is proptest's own default (256/run), overridable with `PROPTEST_CASES` for a
//! campaign run; no explicit `ProptestConfig` override is set here.

#![allow(clippy::expect_used)]

use proptest::prelude::*;

use prikk_object::{ObjectEnvelope, ObjectType, Signature, SignatureAlgorithm, SignerRole};

use super::{decode_envelope_file, encode_envelope_file_structural};
use crate::format::validate_format2_schema;

fn object_type_strategy() -> impl Strategy<Value = ObjectType> {
    (0..ObjectType::ALL.len()).prop_map(|index| {
        ObjectType::ALL
            .get(index)
            .copied()
            .unwrap_or(ObjectType::Patch)
    })
}

fn key_id_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,8}"
}

fn signature_strategy() -> impl Strategy<Value = Signature> {
    (
        key_id_strategy(),
        proptest::collection::vec(any::<u8>(), 1..64),
        any::<u64>(),
    )
        .prop_map(|(key_id, signature_bytes, created_at)| Signature {
            algorithm: SignatureAlgorithm::Ed25519,
            key_id,
            signature_bytes,
            created_at,
            signer_role: SignerRole::Author,
        })
}

fn envelope_strategy() -> impl Strategy<Value = ObjectEnvelope> {
    (
        object_type_strategy(),
        1_u32..=4,
        proptest::collection::vec(any::<u8>(), 0..256),
        proptest::collection::vec(signature_strategy(), 0..3),
    )
        .prop_map(
            |(object_type, schema_version, canonical_payload, signatures)| ObjectEnvelope {
                object_type,
                schema_version,
                canonical_payload,
                signatures,
            },
        )
}

proptest! {
    #[test]
    fn envelope_round_trips_through_file_codec(envelope in envelope_strategy()) {
        let encoded = encode_envelope_file_structural(&envelope)
            .expect("generation invariants keep the envelope structurally valid");
        let decoded = decode_envelope_file(&encoded)
            .expect("bytes produced by the encoder must always decode");
        prop_assert_eq!(decoded, envelope);
    }

    #[test]
    fn decode_envelope_file_never_panics_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..512)
    ) {
        // Totality: arbitrary bytes must decode to Ok or Err, never panic. The call itself is
        // the assertion; a panic fails the test, an Ok/Err result passes it.
        let _ = decode_envelope_file(&bytes);
    }

    #[test]
    fn format2_schema_admission_matches_dc40_allowlist(
        object_type in object_type_strategy(),
        schema_version in 0_u32..=5
    ) {
        let envelope = ObjectEnvelope {
            object_type,
            schema_version,
            canonical_payload: Vec::new(),
            signatures: Vec::new(),
        };
        let expected_ok = match object_type {
            ObjectType::Block => schema_version == 2,
            // DC-61: RefState alone accepts two schemas — 1 (open) and REF_STATE_CLOSED_SCHEMA
            // (closed, carries the tag-7 `closed` field).
            ObjectType::RefState => {
                schema_version == 1 || schema_version == prikk_object::REF_STATE_CLOSED_SCHEMA
            }
            // Patch schema 2 handoff: `PATCH_PARENT_IDS_RETIRED_SCHEMA` retires tag 2
            // (`parent_patch_ids`) outright. RFC 134 §8: `PATCH_TEXT_SPAN_V2_SCHEMA` admits
            // `EditText` tags 10/11. RFC 123 §8: `PATCH_MESSAGE_SCHEMA` admits an optional
            // `message` (tag 6). Patch now accepts four schemas.
            ObjectType::Patch => {
                schema_version == 1
                    || schema_version == prikk_object::PATCH_PARENT_IDS_RETIRED_SCHEMA
                    || schema_version == prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA
                    || schema_version == prikk_object::PATCH_MESSAGE_SCHEMA
            }
            ObjectType::RefUpdate
            | ObjectType::Tag
            | ObjectType::Attestation
            | ObjectType::Blob
            | ObjectType::RecognitionClaim => schema_version == 1,
            ObjectType::BlockSummaryCache | ObjectType::RecoveryNote => false,
        };
        prop_assert_eq!(validate_format2_schema(&envelope).is_ok(), expected_ok);
    }
}
