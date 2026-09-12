//! AUD-08: `merge` refuses rather than wraps when advancing `into_ref` would overflow `update_seq`.
//!
//! `execute_merge` decodes `into_ref`'s current `RefStatePayload` straight off the wire (no
//! upper-bound check on `update_seq` at decode time) and, unlike `seal`/`seal_from_accepted_claim`,
//! never calls `refs::ensure_no_incomplete_publication` anywhere in its own call chain -- confirmed
//! by inspection, not assumed -- so a `heads/main` pointed at a RefState that decodes with
//! `update_seq == u64::MAX` reaches this arm directly, with no unrelated consistency gate refusing
//! first the way it does for the other two sites (see the AUD-08 report for why those two are not
//! given the same kind of test). The corrupted RefState below is otherwise identical to the real one
//! `seal` itself wrote -- not a contrived malformed object -- so this is a decode path the system can
//! genuinely be in, not a state manufactured only to reach this line.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "support/mod.rs"]
mod support;

use support::*;

use prikk_object::{CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefStatePayload};
use prikk_store::{
    FileObjectStore, ObjectReader, ObjectWriter, RefStore, RepositoryLayout, maintainer_signature,
};

fn maintainer_signer() -> prikk_store::Ed25519MaintainerSigner {
    prikk_store::Ed25519MaintainerSigner::from_seed(MAINTAINER_KEY_ID, &MAINTAINER_SEED)
        .expect("fixed maintainer seed derives a valid signer")
}

fn current_ref_state(object_store: &FileObjectStore, ref_state_id: ObjectId) -> RefStatePayload {
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version).unwrap()
}

fn merge(
    repo: &std::path::Path,
    baseline: ObjectId,
    into_ref: &str,
    from_ref: &str,
) -> std::process::Output {
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            into_ref,
            "--from",
            from_ref,
        ])
        .output()
        .unwrap()
}

#[test]
fn merge_refuses_rather_than_wraps_when_into_ref_update_seq_would_overflow() {
    let repo = unique_repo("aud08-merge-overflow");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");

    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    let ref_store = RefStore::new(layout.clone());
    let mut object_store = FileObjectStore::new(layout.clone());
    let genesis_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let genesis_payload = current_ref_state(&object_store, genesis_ref_state_id);
    let baseline = genesis_payload.target_object_id;

    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );
    std::fs::write(repo.join("topic.txt"), b"from topic").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change"),
        "commit on topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    // Otherwise identical to the real genesis RefState `seal` wrote -- only `update_seq` and
    // `previous_ref_state_id` differ.
    let overflowed_payload = RefStatePayload {
        update_seq: u64::MAX,
        previous_ref_state_id: Some(genesis_ref_state_id),
        ..genesis_payload
    };
    let signer = maintainer_signer();
    let mut overflowed_envelope = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        overflowed_payload.to_canonical_bytes().unwrap(),
    );
    let overflowed_object_id = overflowed_envelope.object_id();
    overflowed_envelope
        .add_signature(
            maintainer_signature(&signer, ObjectType::RefState, overflowed_object_id).unwrap(),
        )
        .unwrap();
    let overflowed_object_id = object_store.write_object(&overflowed_envelope).unwrap();
    prikk_store::force_ref_pointer_to_arbitrary_state_for_test_support(
        &layout,
        "heads/main",
        overflowed_object_id,
    )
    .unwrap();

    let output = merge(&repo, baseline, "heads/main", "heads/topic");
    assert!(
        !output.status.success(),
        "merge must refuse an overflowing update_seq, got: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ref-state update_seq overflow"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("heads/main"), "stderr: {stderr}");
}
