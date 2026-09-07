//! DC-86 property tests: the bundle wire format (magic, ref-name framing, object count, and each
//! length-prefixed encoded object). `decode_bundle` is the newest untrusted-input parser in the
//! product and the only one that consumes bytes from a party the operator does not control —
//! `EXECUTION-ORDER.md` §6 rule 3's randomized-decoder-input treatment, applied here the way DC-41
//! stage 4 applied it to the object decoders.
//!
//! Two properties: round-trip for an arbitrary, structurally-valid (not necessarily
//! semantically-valid — `decode_bundle` never inspects a payload's meaning, only `decode_envelope_file`
//! does that per object) set of objects, and totality for arbitrary bytes. Case budget is proptest's
//! own default (256/run), overridable with `PROPTEST_CASES` for a campaign run.

#![allow(clippy::expect_used)]

use proptest::prelude::*;

use prikk_object::{ObjectEnvelope, ObjectType, Signature, SignatureAlgorithm, SignerRole};

use crate::author::author_key_index::AuthorKeyEntry;

use super::super::{
    BundleManifest, BundleScope, DEFAULT_BUNDLE_MAX_OBJECT_COUNT, decode_bundle, encode_bundle,
};

fn key_id_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,8}"
}

/// DC-53 Stage 2: `key_id_strategy`'s own charset already matches
/// `Signature::validate_key_id`'s rule exactly, so it doubles as a strategy for author-key entries
/// -- `decode_bundle` now rejects a key_id outside that charset, and a round-trip property must only
/// ever generate what a real `encode_bundle` caller could produce.
fn author_key_strategy() -> impl Strategy<Value = AuthorKeyEntry> {
    (
        key_id_strategy(),
        proptest::collection::vec(any::<u8>(), 32),
    )
        .prop_map(|(key_id, public_key_bytes)| AuthorKeyEntry {
            key_id,
            public_key: public_key_bytes
                .try_into()
                .expect("the strategy above always generates exactly 32 bytes"),
        })
}

fn signature_strategy() -> impl Strategy<Value = Signature> {
    (
        key_id_strategy(),
        // encode_envelope_file validates Ed25519 signature shape strictly: exactly 64 bytes, not a
        // range — a length outside that is a real, distinct malformed-input case, not this
        // round-trip property's concern (`decode_bundle`'s own totality property below covers
        // arbitrary/malformed bytes, including malformed signature shapes, at the byte level).
        proptest::collection::vec(any::<u8>(), 64),
        any::<u64>(),
    )
        .prop_map(|(key_id, signature_bytes, created_at)| Signature {
            algorithm: SignatureAlgorithm::Ed25519,
            key_id,
            signature_bytes,
            created_at,
            signer_role: SignerRole::Maintainer,
        })
}

fn envelope_strategy() -> impl Strategy<Value = ObjectEnvelope> {
    (
        proptest::collection::vec(any::<u8>(), 0..128),
        proptest::collection::vec(signature_strategy(), 0..2),
    )
        .prop_map(|(canonical_payload, signatures)| ObjectEnvelope {
            object_type: ObjectType::RefState,
            schema_version: 1,
            canonical_payload,
            signatures,
        })
}

fn ref_name_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_/-]{0,32}"
}

/// DC-44 increment 3: `tool_version`'s own charset is unconstrained by any parser rule (unlike
/// `key_id_strategy`, which must match `Signature::validate_key_id`) -- any valid UTF-8 a real
/// `CARGO_PKG_VERSION` could ever contain, plus a wider net than that build ever actually produces.
fn tool_version_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9.+_-]{0,16}"
}

/// `BundleScope` has exactly one variant today ([`BundleScope::SingleRef`]) -- `Just` rather than a
/// generator with nothing to vary, so this strategy still reads as "the manifest's scope," ready to
/// widen the day a second variant exists.
fn manifest_strategy() -> impl Strategy<Value = BundleManifest> {
    (any::<u32>(), tool_version_strategy()).prop_map(|(repository_format, tool_version)| {
        BundleManifest {
            repository_format,
            tool_version,
            scope: BundleScope::SingleRef,
        }
    })
}

proptest! {
    #[test]
    fn bundle_round_trips_an_arbitrary_object_set(
        ref_name in ref_name_strategy(),
        objects in proptest::collection::vec(envelope_strategy(), 0..8),
        author_keys in proptest::collection::vec(author_key_strategy(), 0..8),
        manifest in manifest_strategy()
    ) {
        let bytes = encode_bundle(&ref_name, &objects, &author_keys, &manifest)
            .expect("generation invariants keep the ref name, objects, author keys, and manifest \
                     encodable");
        let (decoded_ref_name, decoded_objects, decoded_author_keys, decoded_manifest) =
            decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)
                .expect("a bundle this small must decode under the default object-count limit");
        prop_assert_eq!(decoded_ref_name, ref_name.clone());
        prop_assert_eq!(decoded_objects, objects.clone());
        prop_assert_eq!(decoded_author_keys, author_keys);
        let decoded_manifest =
            decoded_manifest.expect("encode_bundle always writes the current, manifest-bearing magic");
        prop_assert_eq!(decoded_manifest.repository_format, manifest.repository_format);
        prop_assert_eq!(decoded_manifest.tool_version, manifest.tool_version);
        prop_assert_eq!(decoded_manifest.scope, manifest.scope);
        prop_assert_eq!(decoded_manifest.declared_ref_name, ref_name);
        prop_assert_eq!(decoded_manifest.declared_object_count, objects.len() as u64);
    }

    #[test]
    fn decode_bundle_never_panics_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..512)
    ) {
        let _ = decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT);
    }
}
