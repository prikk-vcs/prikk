//! 0.52.0 release prep P3b (review `release-0.52.0-prep-P3-review-v1`, finding P3-1): `verify`
//! already counts a failed received ref toward its own exit status
//! (`RepositoryVerification::has_item_failure`, `received_ref_item_outcomes`), but its prose never
//! named which one or why -- `"ref items: 0 scanned, 0 failed"` stayed clean while the real failure
//! sat silently in a separate field the prose never read, and the generic `"see item outcomes
//! above"` pointed at nothing.
//!
//! This test builds a destination holding **two** received refs, one adopted and one not (the
//! review's own "trap to avoid": a received ref that fails only on publication trust, not yet
//! adopted, is reported through the trust path, never as an item failure, and must not be printed
//! twice or conflated with a genuine structural failure):
//! - `remotes/heads/main`, written **directly into the destination's own object store and received
//!   index**, in-process, via `prikk_store::write_received_pointer_for_test` (`test-support`-gated):
//!   structurally damaged (a branch `RefState` whose target is a carried `Blob`, not a `Block` --
//!   the identical shape step 0's `control_c_a_branch_targeting_a_carried_non_block_is_damage`
//!   builds), signed by a key this destination never adopts. **0.52.0 release prep P3c (review
//!   `release-0.52.0-prep-P3b-review-v1`, finding P3b-1): this no longer goes through 0.51.0's own
//!   published binary** -- that route depended on a path outside the test's own temporary
//!   directory, and passed silently, checking nothing, wherever that binary is absent (every
//!   machine but one, including CI). Recording the received pointer directly is also the more
//!   honest construction: the historical gap this release's Security fix closes is specifically
//!   that `archive import` used to *write* this shape; this test's own job is only to confirm the
//!   *reader* (`verify`) names it correctly once a repository already holds it, by whatever route;
//! - `remotes/heads/second`, landed via an ordinary, healthy `bundle export`/`bundle import` from a
//!   second, genuinely separate source repository, signed by a key this destination **does** adopt
//!   before importing it.
//!
//! The candidate's own `prikk verify` must name the first by its received-ref name with a message
//! naming the real cause (an object type mismatch -- the Blob is genuinely present under the id the
//! ref claims is a Block), and must never print any such line for the second.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

/// Write a signed Blob and a signed branch `RefState` targeting it (the identical shape
/// `control_c_a_branch_targeting_a_carried_non_block_is_damage`,
/// `crates/prikk-store/src/archive/verify/tests.rs`, builds -- a `Blob` is never a `Block`, so this
/// is damage the moment anything tries to resolve the ref's own target) directly into `layout`'s own
/// object store, then records it as a received pointer under `ref_name` via
/// `write_received_pointer_for_test` -- no `RefUpdate`, no `RefStore::publish`, no archive
/// export/import: those are for building an *exportable source* whose own local ref a real
/// `archive export` would carry; here the shape is written straight into the *destination*, the
/// state a received ref is actually in once something (by any route) has landed it.
fn write_damaged_received_ref(layout: &prikk_store::RepositoryLayout, ref_name: &str) {
    use prikk_object::{CanonicalEncode, ObjectEnvelope, ObjectId};
    use prikk_store::{FileObjectStore, ObjectWriter};

    let mut object_store = FileObjectStore::new(layout.clone());

    let blob_envelope = {
        let mut envelope = ObjectEnvelope::unsigned(
            prikk_object::ObjectType::Blob,
            1,
            prikk_object::BlobPayload {
                blob_kind: prikk_object::BlobKind::Text,
                declared_size: 12,
                content: b"not a block\n".to_vec(),
            }
            .to_canonical_bytes()
            .unwrap(),
        );
        envelope
            .add_signature(prikk_object::Signature {
                algorithm: prikk_object::SignatureAlgorithm::Ed25519,
                key_id: "p3c-crafted-author".to_string(),
                signature_bytes: vec![7; 64],
                created_at: 1,
                signer_role: prikk_object::SignerRole::Author,
            })
            .unwrap();
        envelope
    };
    let blob_id = object_store
        .write_object(&blob_envelope)
        .expect("write blob");

    let ref_state_payload = prikk_object::RefStatePayload {
        ref_name: ref_name.trim_start_matches("remotes/").to_string(),
        kind: prikk_object::RefKind::Branch,
        target_object_id: blob_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state = ObjectEnvelope::unsigned(
        prikk_object::ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes().unwrap(),
    );
    ref_state
        .add_signature(prikk_object::Signature {
            algorithm: prikk_object::SignatureAlgorithm::Ed25519,
            key_id: "p3c-crafted-maintainer".to_string(),
            signature_bytes: vec![5; 64],
            created_at: 1,
            signer_role: prikk_object::SignerRole::Maintainer,
        })
        .unwrap();
    let ref_state_id: ObjectId = ref_state.object_id();
    object_store
        .write_object(&ref_state)
        .expect("write ref state");

    prikk_store::write_received_pointer_for_test(layout, ref_name, ref_state_id)
        .expect("write received pointer");
}

#[test]
fn verify_names_a_failed_received_ref_and_never_the_healthy_adopted_one() {
    // A second, genuinely separate, healthy source -- a different local branch name
    // ("heads/second") so its own received pointer cannot collide with the crafted archive's own
    // "heads/main".
    let healthy_source = support::unique_repo("p3b-healthy-source");
    support::init(&healthy_source);
    support::generation(
        &healthy_source,
        "heads/second",
        "a.txt",
        b"healthy content\n",
        "genesis",
    );
    let healthy_bundle = support::unique_repo("p3b-healthy-bundle").join("healthy.pbndl2");
    let export = support::prikk(&healthy_source)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/second",
            "--output",
            healthy_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export, "bundle export (healthy source)");

    // The destination: adopt the healthy source's own (fixed, shared `support::` test) key first,
    // then land both received refs.
    let destination = support::unique_repo("p3b-destination");
    support::init(&destination);
    support::trust_maintainer(&destination);

    let import_healthy = support::prikk(&destination)
        .args([
            "bundle",
            "import",
            "--input",
            healthy_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&import_healthy, "bundle import (healthy, adopted key)");

    let destination_layout =
        prikk_store::RepositoryLayout::open(destination.clone()).expect("open destination layout");
    write_damaged_received_ref(&destination_layout, "remotes/heads/main");

    // Precondition: exactly two received refs are present, under two distinct names.
    let verify_output = support::verify(&destination);
    let stdout = String::from_utf8_lossy(&verify_output.stdout).to_string();
    assert!(
        !verify_output.status.success(),
        "verify must fail: the destination holds a structurally damaged received ref: {stdout}"
    );
    assert!(
        stdout.contains("received refs: 2"),
        "expected exactly two received refs: {stdout}"
    );
    assert!(
        stdout.contains("received-ref remotes/heads/main:"),
        "the damaged ref's own pointer line must still be printed: {stdout}"
    );
    assert!(
        stdout.contains("received-ref remotes/heads/second:"),
        "the healthy ref's own pointer line must still be printed: {stdout}"
    );

    // The fix's own assertion: the damaged ref is named, by name, with the real failure.
    let failed_lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.starts_with("received ref "))
        .collect();
    assert_eq!(
        failed_lines.len(),
        1,
        "expected exactly one failed-received-ref line, got {failed_lines:?} in: {stdout}"
    );
    // The real, observed message for this exact construction: the blob is genuinely present in the
    // destination's own object store (written directly, above), so resolving the ref's own target
    // finds an object whose *type* disagrees, not one that is missing outright -- both are real
    // "targets the wrong kind of object" shapes RFC 155/step 0's own typed-target check catches;
    // this one happens to surface as a type mismatch rather than a bare absence.
    assert!(
        failed_lines[0].contains("remotes/heads/main")
            && (failed_lines[0].contains("object type mismatch")
                || failed_lines[0].contains("targets missing block")),
        "the failed line must name the damaged ref and its real cause: {failed_lines:?}"
    );
    assert!(
        !failed_lines[0].contains("remotes/heads/second"),
        "the healthy, adopted ref must never be named as a failed item: {failed_lines:?}"
    );

    // The trap: the healthy ref's own adopted key must mean nothing about it needed the trust path
    // either -- its own objects' signatures are checked and trusted, not merely silent.
    assert!(
        stdout.contains("publication trust issues:")
            && !stdout.contains("publication trust issues: 0"),
        "the damaged ref's own untrusted key must still raise a nonzero publication trust count \
         (a separate, pre-existing mechanism this fix does not touch): {stdout}"
    );
}
