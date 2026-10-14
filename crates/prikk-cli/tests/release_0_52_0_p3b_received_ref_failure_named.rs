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
//! - `remotes/heads/main`, landed via **0.51.0's own published binary** (the only route that will
//!   still import this shape at all -- the candidate's own `archive import` now refuses it
//!   outright, which is the whole point of this release's own Security fix), structurally damaged
//!   (a branch `RefState` whose target is a carried `Blob`, not a `Block` -- the identical shape
//!   step 0's `control_c_a_branch_targeting_a_carried_non_block_is_damage` builds), signed by a key
//!   this destination never adopts;
//! - `remotes/heads/second`, landed via an ordinary, healthy `bundle export`/`bundle import` from a
//!   second, genuinely separate source repository, signed by a key this destination **does** adopt
//!   before importing it.
//!
//! The candidate's own `prikk verify` must name the first by its received-ref name with a message
//! naming the real cause (an object type mismatch, since 0.51.0's own import genuinely writes the
//! carried Blob into the destination under the id the ref claims is a Block), and must never print
//! any such line for the second.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

mod support;

/// Build the identical fixture `control_c_a_branch_targeting_a_carried_non_block_is_damage`
/// (`crates/prikk-store/src/archive/verify/tests.rs`) builds -- a branch `RefState` whose own
/// target names a carried `Blob`, not a `Block` -- and export it to `output`. Lives here, not as a
/// second copy inside `prikk-store`'s own test tree, so no throwaway test needs adding and removing
/// there for this one CLI-level reproduction; the construction itself is small enough to inline.
fn write_crafted_branch_targets_blob_archive(output: &Path) {
    use prikk_object::{CanonicalEncode, ObjectEnvelope, ObjectId};
    use prikk_store::{FileObjectStore, ObjectWriter, RefPublication, RefStore};

    let dir = support::unique_repo("p3b-crafted-source");
    let layout = prikk_store::RepositoryLayout::init(&dir).expect("init layout");
    let mut object_store = FileObjectStore::new(layout.clone());

    // A signed Blob, and a Branch RefState naming it as the target -- a Blob is never a Block, so
    // this is damage the moment anything tries to resolve the ref's own target.
    let blob_envelope = ObjectEnvelope::unsigned(
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
    let mut blob_envelope = blob_envelope;
    blob_envelope
        .add_signature(prikk_object::Signature {
            algorithm: prikk_object::SignatureAlgorithm::Ed25519,
            key_id: "p3b-crafted-author".to_string(),
            signature_bytes: vec![7; 64],
            created_at: 1,
            signer_role: prikk_object::SignerRole::Author,
        })
        .unwrap();
    let blob_id = object_store
        .write_object(&blob_envelope)
        .expect("write blob");

    let ref_state_payload = prikk_object::RefStatePayload {
        ref_name: "heads/main".to_string(),
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
            key_id: "p3b-crafted-maintainer".to_string(),
            signature_bytes: vec![5; 64],
            created_at: 1,
            signer_role: prikk_object::SignerRole::Maintainer,
        })
        .unwrap();
    let ref_state_id: ObjectId = ref_state.object_id();

    let ref_update_payload = prikk_object::RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: blob_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: "p3b-crafted-maintainer".to_string(),
    };
    let mut ref_update = ObjectEnvelope::unsigned(
        prikk_object::ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes().unwrap(),
    );
    ref_update
        .add_signature(prikk_object::Signature {
            algorithm: prikk_object::SignatureAlgorithm::Ed25519,
            key_id: "p3b-crafted-maintainer".to_string(),
            signature_bytes: vec![5; 64],
            created_at: 1,
            signer_role: prikk_object::SignerRole::Maintainer,
        })
        .unwrap();

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        })
        .expect("publish");

    let mut bytes = Vec::new();
    prikk_store::export_archive(&layout, &mut bytes).expect("export archive");
    std::fs::write(output, bytes).expect("write crafted archive");
}

/// Run `bin` with the identical key-environment isolation `support::prikk` gives the candidate's
/// own binary -- required here because 0.51.0's own published binary is invoked directly, not
/// through `support::prikk` (which always resolves `CARGO_BIN_EXE_prikk`, the candidate).
fn run_isolated(bin: &Path, repo: &Path, args: &[&str]) -> Output {
    let home = support::unique_repo("p3b-home");
    Command::new(bin)
        .current_dir(repo)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("APPDATA", &home)
        .env_remove("PRIKK_AUTHOR_SEED")
        .env_remove("PRIKK_AUTHOR_KEY_ID")
        .env_remove("PRIKK_AUTHOR_SEED_FILE")
        .env_remove("PRIKK_MAINTAINER_SEED")
        .env_remove("PRIKK_MAINTAINER_KEY_ID")
        .env_remove("PRIKK_MAINTAINER_SEED_FILE")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn verify_names_a_failed_received_ref_and_never_the_healthy_adopted_one() {
    let released_051 = Path::new("/home/nabbisen/.pgtmp/rel-0.51.0-bin/prikk");
    if !released_051.exists() {
        eprintln!(
            "skipping: 0.51.0's published binary is not present at {}",
            released_051.display()
        );
        return;
    }

    // The damaged archive, built once.
    let crafted = support::unique_repo("p3b-crafted-archive").join("crafted.prepo");
    write_crafted_branch_targets_blob_archive(&crafted);

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

    let import_damaged = run_isolated(
        released_051,
        &destination,
        &["archive", "import", crafted.to_str().unwrap()],
    );
    assert!(
        import_damaged.status.success(),
        "0.51.0's own archive import must still land this damaged shape (that is the historical \
         gap this release fixes): stdout={} stderr={}",
        String::from_utf8_lossy(&import_damaged.stdout),
        String::from_utf8_lossy(&import_damaged.stderr)
    );

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
    // The real, observed message for this exact construction: the blob genuinely lands in the
    // destination's own object store (0.51.0's own import writes it), so resolving the ref's own
    // target finds an object whose *type* disagrees, not one that is missing outright -- both are
    // real "targets the wrong kind of object" shapes RFC 155/step 0's own typed-target check
    // catches; this one happens to surface as a type mismatch rather than a bare absence.
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
