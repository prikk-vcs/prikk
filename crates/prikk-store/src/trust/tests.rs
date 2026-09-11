//! Trust-store tests.
//!
//! DC-78 changed `add_trusted_maintainer` from "add or replace the single key" to "add a new key,
//! confirm idempotently if it already matches, refuse if it conflicts" — every test below that
//! previously exercised "adding a second key id replaces the first" or "re-adding the same key id
//! with a different public key replaces it" has been rewritten, not merely patched: those behaviors
//! no longer exist, on purpose (§D5's TOFU enforcement).
//!
//! RFC 102 Stage 5, design-v1.md §14/§14.9: key material and policy moved onto `trust_index.rs`'s
//! containers, replacing `trust/keys/maintainer/*.pub` and `trust/policy.toml` outright. Tests that
//! directly manipulated those paths, or relied on `atomic_replace`'s own failpoints
//! (`MutableParentSync`/`MutableFileSync`/`MutableRename`), are redesigned onto the container paths
//! and `durable_append`'s own failpoints (`AppendWrite`/`RequiredFileSync`/`RequiredDirectorySync`) --
//! not ported, since the underlying primitive changed. Pure-behavior tests (through the public API
//! only) are unchanged.

#![allow(clippy::indexing_slicing)]

use prikk_crypto::Ed25519KeyPair;
use prikk_error::PrikkError;

use crate::{
    GatedOperation, RepositoryLayout, acquire_container_locks, add_trusted_maintainer,
    load_maintainer_trust_policy, remove_trusted_maintainer, verify_signer_trusted,
};

use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test, fail_once_for_test};
use crate::foundation::layout::LockableContainer;
use crate::maintainer_signing::Ed25519MaintainerSigner;
use crate::test_gates::test_support::unique_temp_dir;

fn public_key_hex(seed: &[u8; 32]) -> String {
    let key_pair = Ed25519KeyPair::from_seed(seed);
    key_pair
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn add_and_load_single_maintainer_policy() {
    let root = unique_temp_dir("trust-add-load");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let seed = [7_u8; 32];
        let public_key = public_key_hex(&seed);
        let added = add_trusted_maintainer(&layout, "maintainer_1", &public_key);
        assert!(added.is_ok());
        if let Ok((adopted, newly_added)) = added {
            assert!(newly_added);
            assert_eq!(adopted.key_id, "maintainer_1");
        }
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert_eq!(loaded.keys.len(), 1);
            assert_eq!(loaded.keys[0].key_id, "maintainer_1");
            assert_eq!(
                loaded.keys[0].public_key,
                Ed25519KeyPair::from_seed(&seed).public_key_bytes()
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 102 Stage 6 Step 2, design-v1.md §15.8: `add_trusted_maintainer`/`remove_trusted_maintainer`
/// actually acquire the `TrustPolicy` container lock, not just `ActiveLock` -- proven the same way as
/// `refs::tests::publish_refuses_while_either_container_lock_is_externally_held`: hold the lock
/// externally first, observe the refusal, release, observe success.
#[test]
fn trust_add_and_remove_refuse_while_trust_policy_lock_is_externally_held()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("trust-container-lock-conflict");
    let layout = RepositoryLayout::init(root.clone())?;
    let key = public_key_hex(&[7_u8; 32]);
    let other_key = public_key_hex(&[8_u8; 32]);

    let held = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy])?;
    assert!(add_trusted_maintainer(&layout, "blocked", &key).is_err());
    drop(held);
    assert!(add_trusted_maintainer(&layout, "blocked", &key).is_ok());
    assert!(add_trusted_maintainer(&layout, "kept", &other_key).is_ok());

    let held = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy])?;
    assert!(remove_trusted_maintainer(&layout, "blocked").is_err());
    drop(held);
    assert!(remove_trusted_maintainer(&layout, "blocked")?);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn adopting_a_second_key_id_keeps_the_first_trusted() {
    let root = unique_temp_dir("trust-add-second");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let first_key = public_key_hex(&[1_u8; 32]);
        let second_key = public_key_hex(&[2_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "alice", &first_key).is_ok());
        assert!(add_trusted_maintainer(&layout, "bob", &second_key).is_ok());
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert_eq!(loaded.keys.len(), 2);
            assert_eq!(loaded.keys[0].key_id, "alice");
            assert_eq!(loaded.keys[1].key_id, "bob");
            assert_eq!(
                loaded.keys[0].public_key,
                Ed25519KeyPair::from_seed(&[1_u8; 32]).public_key_bytes()
            );
            assert_eq!(
                loaded.keys[1].public_key,
                Ed25519KeyPair::from_seed(&[2_u8; 32]).public_key_bytes()
            );
        }
        // Both remain independently verifiable through the signer-trust path.
        if let Ok(alice) = Ed25519MaintainerSigner::from_seed("alice", &[1_u8; 32]) {
            assert!(verify_signer_trusted(&layout, &alice, GatedOperation::Seal).is_ok());
        }
        if let Ok(bob) = Ed25519MaintainerSigner::from_seed("bob", &[2_u8; 32]) {
            assert!(verify_signer_trusted(&layout, &bob, GatedOperation::Seal).is_ok());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn readopting_the_same_key_id_and_key_is_idempotent() {
    let root = unique_temp_dir("trust-readopt-same");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let key = public_key_hex(&[3_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "maintainer", &key).is_ok());
        let second_call = add_trusted_maintainer(&layout, "maintainer", &key);
        assert!(second_call.is_ok());
        if let Ok((_, newly_added)) = second_call {
            assert!(!newly_added);
        }
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert_eq!(
                loaded.keys.len(),
                1,
                "idempotent re-adoption must not duplicate"
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// DC-78 negative control 3: a changed public key for an already-adopted key id is refused, not
/// re-prompted or silently replaced.
#[test]
fn readopting_an_existing_key_id_with_a_different_key_is_refused() {
    let root = unique_temp_dir("trust-readopt-conflict");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let original = public_key_hex(&[4_u8; 32]);
        let conflicting = public_key_hex(&[5_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "maintainer", &original).is_ok());
        // RFC 147 §2d site 2: a trust-on-first-use collision is a caller precondition, detected
        // before anything is verified -- it reported `InvalidSignature` until this was pinned, and
        // `.is_err()` alone could not tell the difference.
        let refused = add_trusted_maintainer(&layout, "maintainer", &conflicting);
        assert!(
            matches!(refused, Err(PrikkError::Precondition(_))),
            "a key-id collision is a precondition, not a signature failure: {refused:?}"
        );
        // The refusal must not have changed anything.
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert_eq!(loaded.keys.len(), 1);
            assert_eq!(
                loaded.keys[0].public_key,
                Ed25519KeyPair::from_seed(&[4_u8; 32]).public_key_bytes()
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn policy_rejects_unsafe_key_id() {
    let root = unique_temp_dir("trust-unsafe-key");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let public_key = public_key_hex(&[7_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "../bad", &public_key).is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 102 Stage 5: replaces `policy_accepts_two_keys_and_rejects_malformed_two_key_syntax`, which
/// hand-wrote TOML text of various malformed shapes -- there is no TOML anymore to malform the same
/// way. The well-formed half is already covered by `adopting_a_second_key_id_keeps_the_first_trusted`;
/// the property worth protecting here is the container-era equivalent of "malformed input fails
/// closed" -- corrupt bytes in the policy container must not silently resolve to a stale or empty
/// policy. Decode-level corruption isolation (checksum mismatch, duplicate key id) has its own
/// dedicated coverage in `trust_index/tests.rs`; this proves the property end to end through the
/// public `load_maintainer_trust_policy` API.
#[test]
fn a_corrupt_policy_container_fails_closed_rather_than_resolving_stale_or_empty() {
    let root = unique_temp_dir("trust-corrupt-policy");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let key = public_key_hex(&[1_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "alice", &key).is_ok());
        assert!(
            std::fs::write(
                layout
                    .trust_policy_container_slot_path(crate::foundation::layout::ContainerSlot::A),
                b"not a valid trust policy container at all",
            )
            .is_ok()
        );
        assert!(
            load_maintainer_trust_policy(&layout).is_err(),
            "a corrupt policy container must fail closed, not resolve to the prior snapshot or empty"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn signer_trust_binding_rejects_seed_public_key_mismatch() {
    let root = unique_temp_dir("trust-seed-mismatch");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let trusted_seed = [7_u8; 32];
        let signing_seed = [8_u8; 32];
        let public_key = public_key_hex(&trusted_seed);
        assert!(add_trusted_maintainer(&layout, "maintainer", &public_key).is_ok());
        let signer = match Ed25519MaintainerSigner::from_seed("maintainer", &signing_seed) {
            Ok(signer) => signer,
            Err(error) => panic!("test maintainer signer should be constructible: {error}"),
        };
        // RFC 147 §2d: the *adjacent* site, deliberately unchanged. The id matched and the material
        // did not, which is a real key-binding failure -- pinned here so the round that reclassified
        // its neighbour (membership, below) cannot quietly take this one with it. RFC 132's own
        // follow-up lesson: guard what you preserved, not only what you changed.
        let refused = verify_signer_trusted(&layout, &signer, GatedOperation::Seal);
        assert!(
            matches!(refused, Err(PrikkError::InvalidSignature(_))),
            "a trusted id with the wrong material stays a signature failure: {refused:?}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 147 §2d site 1: a repository that has adopted nothing yet. Not a mocked snapshot -- a
/// genuinely fresh `init`, which is the state every second project starts in, reached end to end
/// through the CLI by `rfc132_trust_precondition_sites.rs` and `beginners_tutorial.rs`.
#[test]
fn loading_policy_before_any_adoption_is_a_precondition_not_damage() {
    let root = unique_temp_dir("trust-nothing-adopted");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(
            matches!(loaded, Err(PrikkError::Precondition(_))),
            "nothing adopted yet is a precondition, not an integrity failure: {loaded:?}"
        );
        // The honesty clause the wording ruling required: the message must not claim the narrow
        // replays-empty case impossible, so it names `doctor` alongside the common cause.
        let message = match loaded {
            Err(error) => error.to_string(),
            Ok(_) => unreachable!("asserted Err above"),
        };
        assert!(
            message.contains("trust maintainer add") && message.contains("doctor"),
            "both halves of the message must survive: {message}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 147 §2d site 3: policy *membership*, tested before any signature is verified. Distinct from
/// `signer_trust_binding_rejects_seed_public_key_mismatch` above, which keeps `InvalidSignature`.
#[test]
fn signing_with_an_unadopted_key_id_is_a_precondition() {
    let root = unique_temp_dir("trust-id-not-in-policy");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let seed = [9_u8; 32];
        let public_key = public_key_hex(&seed);
        assert!(add_trusted_maintainer(&layout, "adopted", &public_key).is_ok());
        let signer = match Ed25519MaintainerSigner::from_seed("not-adopted", &seed) {
            Ok(signer) => signer,
            Err(error) => panic!("test maintainer signer should be constructible: {error}"),
        };
        let refused = verify_signer_trusted(&layout, &signer, GatedOperation::Seal);
        assert!(
            matches!(refused, Err(PrikkError::Precondition(_))),
            "an unadopted key id is a precondition, not a signature failure: {refused:?}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// The key-material append (the first of the two `add_trusted_maintainer` performs for a genuinely
/// new key) fails: neither the key material nor the policy snapshot changes, since the policy append
/// is never reached.
#[test]
fn failed_key_append_keeps_previous_effective_policy() {
    let root = unique_temp_dir("trust-key-append-failure");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let old_key = public_key_hex(&[7_u8; 32]);
        let new_key = public_key_hex(&[8_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "old", &old_key).is_ok());
        // `AppendWrite` (not `RequiredDirectorySync`/`RequiredFileSync`) -- those two are shared with
        // `create_exclusive`, which `ActiveLock::acquire`'s own lock-file creation also calls, so a
        // one-shot failpoint on either would consume on the *lock*, not the key append, and leave the
        // lock file itself as orphaned debris from a "failed but landed" create (found empirically:
        // the retry below failed with the wrong failpoint choice). `AppendWrite` is exclusive to
        // `durable_append` and fires before any bytes are written, so nothing lands either way.
        fail_once_for_test(TestFailPoint::AppendWrite);
        assert!(add_trusted_maintainer(&layout, "new", &new_key).is_err());
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert_eq!(loaded.keys.len(), 1);
            assert_eq!(loaded.keys[0].key_id, "old");
        }
        assert!(add_trusted_maintainer(&layout, "new", &new_key).is_ok());
        let retried = load_maintainer_trust_policy(&layout);
        assert!(retried.is_ok());
        if let Ok(retried) = retried {
            assert_eq!(retried.keys.len(), 2);
            assert_eq!(retried.keys[0].key_id, "old");
            assert_eq!(retried.keys[1].key_id, "new");
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// The policy snapshot append (the third `RequiredDirectorySync` checkpoint this call reaches --
/// `ActiveLock::acquire`'s own lock-file creation is the first, since `create_exclusive` shares this
/// failpoint too, and the key material append is the second) fails at its own directory-sync
/// confirmation: the new snapshot's bytes are already physically appended by then (`durable_append`'s
/// write/file-sync steps both precede the directory sync), so the failed call already exposes the new
/// effective policy on a fresh read -- the failure is honestly reported (durability wasn't confirmed),
/// but the content already moved. Same "operation happened, confirmation failed" shape the retired
/// `atomic_replace`-based test proved for `MutableParentSync`. The skip count of 2 (not 1) is the
/// direct consequence of the shared-failpoint fact this module's other test found the hard way.
#[test]
fn failed_policy_append_exposes_retryable_new_effective_policy() {
    let root = unique_temp_dir("trust-policy-append-failure");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let old_key = public_key_hex(&[7_u8; 32]);
        let new_key = public_key_hex(&[8_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "old", &old_key).is_ok());
        // Adding a genuinely new key id ("new") fires `RequiredDirectorySync` four times: the
        // `ActiveLock`'s own creation, the `TrustPolicy` container lock's own creation (RFC 102
        // Stage 6 Step 2, design-v1.md §15.8 -- `acquire_container_locks`, hoisted to right after
        // `ActiveLock::acquire`), the new key-material entry's append, and the policy snapshot's
        // own append. Skip 3, not 2, to land on the policy snapshot's own directory sync -- the
        // point this test's own assertions are about.
        fail_after_for_test(TestFailPoint::RequiredDirectorySync, 3);
        assert!(add_trusted_maintainer(&layout, "new", &new_key).is_err());
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert_eq!(
                loaded.keys.len(),
                2,
                "the policy append already landed before the directory sync failed"
            );
            assert_eq!(loaded.keys[1].key_id, "new");
        }
        assert!(add_trusted_maintainer(&layout, "new", &new_key).is_ok());
        let retried = load_maintainer_trust_policy(&layout);
        assert!(retried.is_ok());
        if let Ok(retried) = retried {
            assert_eq!(retried.keys.len(), 2);
            assert_eq!(retried.keys[1].key_id, "new");
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn trust_reads_and_updates_remain_on_retained_repository_root() -> prikk_error::Result<()> {
    let root = unique_temp_dir("trust-root-replacement");
    let layout = RepositoryLayout::init(root.clone())?;
    let first = public_key_hex(&[7_u8; 32]);
    let second = public_key_hex(&[8_u8; 32]);
    assert!(add_trusted_maintainer(&layout, "maintainer", &first).is_ok());
    let displaced = root.join(".prikk-displaced");
    std::fs::rename(layout.prikk_dir(), &displaced)?;
    std::fs::create_dir_all(root.join(".prikk/trust"))?;
    std::fs::copy(
        displaced.join("trust/keys.container"),
        root.join(".prikk/trust/keys.container"),
    )?;
    // RFC 102 Stage 6 Step 1, design-v1.md §15.6: the policy container gained an A/B slot pair and a
    // generation log, replacing the single `policy.container` name -- all three copied here, even
    // though B and the generation log are empty in Step 1, so this test still exercises "every name
    // under `trust/` survives a root replacement," not just the one slot that happens to be live.
    std::fs::copy(
        displaced.join("trust/policy-a.container"),
        root.join(".prikk/trust/policy-a.container"),
    )?;
    std::fs::copy(
        displaced.join("trust/policy-b.container"),
        root.join(".prikk/trust/policy-b.container"),
    )?;
    std::fs::copy(
        displaced.join("trust/policy-generation.log"),
        root.join(".prikk/trust/policy-generation.log"),
    )?;

    assert_eq!(
        load_maintainer_trust_policy(&layout)?.keys[0].key_id,
        "maintainer"
    );
    assert!(add_trusted_maintainer(&layout, "second", &second).is_ok());
    let loaded = load_maintainer_trust_policy(&layout)?;
    assert_eq!(loaded.keys.len(), 2);
    assert_eq!(loaded.keys[1].key_id, "second");
    assert_eq!(
        loaded.keys[1].public_key,
        Ed25519KeyPair::from_seed(&[8_u8; 32]).public_key_bytes()
    );
    assert!(root.join(".prikk/trust/policy-a.container").is_file());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn trust_append_failures_pin_effective_state_and_retry() -> prikk_error::Result<()> {
    for (boundary, skip) in [("key", 0), ("policy", 1)] {
        let root = unique_temp_dir(&format!("trust-append-failure-{boundary}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let old_key = public_key_hex(&[7_u8; 32]);
        let new_key = public_key_hex(&[8_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "old", &old_key).is_ok());

        fail_after_for_test(TestFailPoint::AppendWrite, skip);
        assert!(add_trusted_maintainer(&layout, "new", &new_key).is_err());
        let retained = load_maintainer_trust_policy(&layout)?;
        assert_eq!(retained.keys.len(), 1, "boundary {boundary}");
        assert_eq!(retained.keys[0].key_id, "old");
        assert_eq!(
            retained.keys[0].public_key,
            Ed25519KeyPair::from_seed(&[7_u8; 32]).public_key_bytes()
        );
        assert!(add_trusted_maintainer(&layout, "new", &new_key).is_ok());
        let retried = load_maintainer_trust_policy(&layout)?;
        assert_eq!(retried.keys.len(), 2, "boundary {boundary}");
        assert_eq!(retried.keys[1].key_id, "new");

        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

/// design-v1.md §14.9: revocation is representable natively -- `remove_trusted_maintainer` appends a
/// shorter snapshot, and the removed key stops verifying while every other adopted key is unaffected.
#[test]
fn remove_revokes_a_key_without_disturbing_others() -> prikk_error::Result<()> {
    let root = unique_temp_dir("trust-remove-basic");
    let layout = RepositoryLayout::init(root.clone())?;
    let alice_key = public_key_hex(&[1_u8; 32]);
    let bob_key = public_key_hex(&[2_u8; 32]);
    add_trusted_maintainer(&layout, "alice", &alice_key)?;
    add_trusted_maintainer(&layout, "bob", &bob_key)?;

    assert!(remove_trusted_maintainer(&layout, "alice")?);
    let loaded = load_maintainer_trust_policy(&layout)?;
    assert_eq!(loaded.keys.len(), 1);
    assert_eq!(loaded.keys[0].key_id, "bob");

    let alice_signer = Ed25519MaintainerSigner::from_seed("alice", &[1_u8; 32])?;
    assert!(verify_signer_trusted(&layout, &alice_signer, GatedOperation::Seal).is_err());
    let bob_signer = Ed25519MaintainerSigner::from_seed("bob", &[2_u8; 32])?;
    assert!(verify_signer_trusted(&layout, &bob_signer, GatedOperation::Seal).is_ok());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Removing a key id that was never adopted is a no-op reported as such, matching `add`'s own
/// idempotent-no-op shape.
#[test]
fn remove_of_an_unadopted_key_id_is_a_reported_no_op() -> prikk_error::Result<()> {
    let root = unique_temp_dir("trust-remove-noop");
    let layout = RepositoryLayout::init(root.clone())?;
    add_trusted_maintainer(&layout, "alice", &public_key_hex(&[1_u8; 32]))?;

    assert!(!remove_trusted_maintainer(&layout, "never-adopted")?);
    let loaded = load_maintainer_trust_policy(&layout)?;
    assert_eq!(loaded.keys.len(), 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// design-v1.md §14.9's own note: an explicitly-empty policy is a state the old TOML parser could
/// never represent either (`keys = []` was rejected as malformed), so removing the last adopted key
/// is refused rather than introducing it now.
#[test]
fn remove_of_the_last_adopted_key_is_refused() -> prikk_error::Result<()> {
    let root = unique_temp_dir("trust-remove-last");
    let layout = RepositoryLayout::init(root.clone())?;
    add_trusted_maintainer(&layout, "alice", &public_key_hex(&[1_u8; 32]))?;

    assert!(remove_trusted_maintainer(&layout, "alice").is_err());
    let loaded = load_maintainer_trust_policy(&layout)?;
    assert_eq!(
        loaded.keys.len(),
        1,
        "the refused removal must not have changed anything"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// design-v1.md §14.9 §4's deliberate behavior change, stated and proven rather than left to surface
/// later: after removal, a case-insensitive variant of the removed id is addable -- the DC-72 collision
/// check is judged against the *active* policy now, not the key-material container's full history, so
/// a removed key's identity no longer goes on reserving its case-folded name.
#[test]
fn a_removed_keys_case_variant_becomes_addable() {
    let root = unique_temp_dir("trust-remove-case-variant");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let original_key = public_key_hex(&[1_u8; 32]);
        let other_key = public_key_hex(&[2_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "Dev-Maintainer", &original_key).is_ok());
        // A second, unrelated key must exist so removing "Dev-Maintainer" isn't refused as "the
        // last adopted key."
        assert!(add_trusted_maintainer(&layout, "someone-else", &other_key).is_ok());
        assert!(remove_trusted_maintainer(&layout, "Dev-Maintainer").is_ok());

        // The case-folded variant is now addable -- the removed identity no longer reserves its name.
        assert!(add_trusted_maintainer(&layout, "dev-maintainer", &other_key).is_ok());
    }
    let _ = std::fs::remove_dir_all(root);
}

/// The other half of the same ruling: TOFU history persists across removal, independent of the
/// collision check above -- re-adopting the *exact same* id (no case-fold collision to trip, since it
/// is absent from the active set after removal) with a *different* key is still refused, because the
/// key-material container is never pruned.
#[test]
fn a_changed_key_under_a_removed_and_readded_id_is_still_refused() {
    let root = unique_temp_dir("trust-remove-tofu-persists");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let original_key = public_key_hex(&[1_u8; 32]);
        let other_key = public_key_hex(&[2_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "maintainer", &original_key).is_ok());
        assert!(add_trusted_maintainer(&layout, "someone-else", &other_key).is_ok());
        assert!(remove_trusted_maintainer(&layout, "maintainer").is_ok());

        let changed_key = public_key_hex(&[3_u8; 32]);
        assert!(add_trusted_maintainer(&layout, "maintainer", &changed_key).is_err());
        // Re-adopting with the *original* key, by contrast, is the ordinary idempotent-material case.
        assert!(add_trusted_maintainer(&layout, "maintainer", &original_key).is_ok());
        let loaded = load_maintainer_trust_policy(&layout);
        assert!(loaded.is_ok());
        if let Ok(loaded) = loaded {
            assert!(loaded.keys.iter().any(|key| key.key_id == "maintainer"));
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
