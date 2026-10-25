//! RFC 111 Stage 2 gate review, blocking condition C1: a drift guard for
//! `rfc111_seal_decode_cost_gate`'s `simulate_one_seal`.
//!
//! That gate's own decode counter proves nothing about `simulate_one_seal`'s fidelity to the real
//! `seal.rs` -- the two could silently diverge in what they read or write and the count would still
//! look right. This test closes that gap the only way the review accepted: run the real `prikk seal`
//! binary (the only place `CARGO_BIN_EXE_prikk` is reachable) against a fixture, run
//! `simulate_one_seal` (exposed cross-crate via `prikk-store`'s non-default `test-support` feature,
//! `prikk_store::simulate_one_seal_for_test_support`) against an *identical* fixture, and assert the
//! two resulting repositories agree -- object ids and ref state. Per the review: "If the two cannot
//! be made to agree, that is the finding -- report it rather than loosening the comparison until it
//! passes."

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{ObjectType, RefStatePayload};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, ObjectReader, RefStore, RepositoryLayout,
    add_trusted_maintainer, simulate_one_seal_for_test_support,
};

#[path = "../support/mod.rs"]
mod support;

const REF_NAME: &str = "heads/main";

fn current_ref_state(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> (prikk_object::ObjectId, RefStatePayload) {
    let ref_state_id = RefStore::new(layout.clone())
        .read_current_ref_state_id(ref_name)
        .expect("read current ref state id")
        .expect("ref has a published RefState");
    let object_store = FileObjectStore::new(layout.clone());
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .expect("read RefState envelope")
        .expect("RefState object exists");
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
            .expect("decode RefState payload");
    (ref_state_id, payload)
}

#[test]
fn simulate_one_seal_agrees_with_the_real_seal_binary() {
    // Build one fixture (init, one authored commit) via the real CLI, then fork it in two:
    // `real_root` gets sealed by the actual `prikk seal` subprocess, `sim_root` by
    // `simulate_one_seal_for_test_support` in-process. Both start from byte-identical `.prikk` state.
    let real_root = support::unique_repo("rfc111-drift-real");
    support::init(&real_root);
    std::fs::write(real_root.join("f0.txt"), b"f0\n").unwrap();
    support::ok(
        &support::commit(&real_root, REF_NAME, "rfc111 drift guard fixture"),
        "commit fixture",
    );

    let sim_root = support::unique_repo("rfc111-drift-sim");
    std::fs::create_dir_all(sim_root.join(".prikk")).unwrap();
    support::copy_dir_recursive(&real_root.join(".prikk"), &sim_root.join(".prikk"));

    support::ok(&support::seal(&real_root, REF_NAME), "real seal");

    let sim_layout = RepositoryLayout::open(sim_root.clone()).expect("open sim repo");
    add_trusted_maintainer(
        &sim_layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .expect("trust maintainer in sim repo");
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .expect("fixed maintainer seed derives a valid signer");
    let simulated_ref_state_id =
        simulate_one_seal_for_test_support(&sim_layout, REF_NAME, &maintainer)
            .expect("simulated seal");

    let real_layout = RepositoryLayout::open(real_root.clone()).expect("open real repo");
    let (real_ref_state_id, real_payload) = current_ref_state(&real_layout, REF_NAME);
    let (_, simulated_payload) = current_ref_state(&sim_layout, REF_NAME);

    assert_eq!(
        real_ref_state_id, simulated_ref_state_id,
        "the real `prikk seal` binary and `simulate_one_seal` must publish an identical RefState id \
         for an identical fixture (RFC 111 Stage 2 gate review C1) -- real={real_ref_state_id} \
         simulated={simulated_ref_state_id}",
    );
    assert_eq!(
        real_payload.target_object_id, simulated_payload.target_object_id,
        "the real `prikk seal` binary and `simulate_one_seal` must publish RefStates pointing at the \
         same block id for an identical fixture",
    );
    assert_eq!(
        real_payload, simulated_payload,
        "the real `prikk seal` binary and `simulate_one_seal` must publish byte-identical RefState \
         payloads for an identical fixture",
    );

    support::ok(&support::verify(&real_root), "verify real repo");
    let sim_verify = support::prikk(&sim_root).arg("verify").output().unwrap();
    support::ok(&sim_verify, "verify sim repo");
}
