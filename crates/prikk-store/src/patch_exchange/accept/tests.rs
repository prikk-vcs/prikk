//! RFC 115 Stage 3 §4/§7: the accept path's behaviour, and a negative control for every row of §7
//! ("do not compress them" -- the handoff's own instruction). Each control mutates the single
//! narrowest line that should break the property under test, not the whole fixture, per this
//! project's own standing review discipline.

#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::{BlockKind, ObjectId, ObjectType};

use super::{AcceptOptions, ClaimSignatureVerification, accept_exchange_artifact};
use crate::author::author_key_index::{lookup_author_key_entries, record_author_key_material};
use crate::author::author_signing::AuthorSigner as _;
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::lock::ActiveLock;
use crate::maintainer_signing::MaintainerSigner as _;
use crate::patch_exchange::exchange_test_support::public_key_hex;
use crate::patch_exchange::exchange_test_support::{
    author_signer, maintainer_signer, reencode_artifact, signed_author_patch_envelope,
    signed_blob_envelope, signed_claim_envelope, signed_tag_envelope,
};
use crate::patch_exchange::export_exchange_artifact;
use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
use crate::tag_travel::TagSignatureVerification;
use crate::test_gates::test_support::{signed_block, unique_temp_dir};
use crate::verify::AuthorSignatureVerification;
use crate::{FileObjectStore, ObjectWriter, RepositoryLayout};
use crate::{ObjectReadSnapshot, add_trusted_maintainer, load_maintainer_trust_policy};

fn fresh_repo(name: &str) -> Result<RepositoryLayout> {
    RepositoryLayout::init(unique_temp_dir(name))
}

/// Build a sender repository holding one AUTHOR-signed patch (and its blob), record the signer's
/// key material locally if `record_locally`, and export a `PEXCH001` artifact carrying just that
/// patch. Returns the artifact bytes and the patch's object id.
fn build_single_patch_artifact(
    sender_name: &str,
    signer: &crate::Ed25519AuthorSigner,
    record_locally: bool,
) -> Result<(Vec<u8>, ObjectId)> {
    let sender = fresh_repo(sender_name)?;
    let mut objects = FileObjectStore::new(sender.clone());
    let blob = signed_blob_envelope(b"accept fixture\n")?;
    let blob_id = objects.write_object(&blob)?;
    let patch = signed_author_patch_envelope(signer, "accept.txt", 0x30, blob_id)?;
    let patch_id = objects.write_object(&patch)?;
    if record_locally {
        let active_lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
        record_author_key_material(
            &sender,
            signer.key_id(),
            signer.public_key_bytes(),
            &active_lock,
        )?;
    }
    let (_, bytes) = export_exchange_artifact(&sender, &[patch_id], &[], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());
    Ok((bytes, patch_id))
}

#[test]
fn a_healthy_artifact_is_accepted_and_writes_everything_it_carries() -> Result<()> {
    let signer = author_signer(0x10)?;
    let (bytes, patch_id) =
        build_single_patch_artifact("pexch-accept-healthy-sender", &signer, true)?;

    let receiver = fresh_repo("pexch-accept-healthy-receiver")?;
    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(report.patch_count, 1);
    assert_eq!(report.blob_count, 1);
    assert_eq!(report.written_object_count, 2);
    assert_eq!(report.recorded_author_key_count, 1);
    assert_eq!(
        report.author_signature_outcomes,
        vec![(
            patch_id,
            AuthorSignatureVerification::Sound {
                key_id: signer.key_id().to_string()
            }
        )]
    );

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(snapshot.contains_object(ObjectType::Patch, patch_id));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 1: a refused exchange records no key material and no claim. Mutates exactly the AUTHOR
/// signature's own bytes -- the narrowest line that should break Phase C's verification -- leaving
/// everything else (including the artifact's own transported author-key material) genuine, so the
/// refusal is attributable only to that one corrupted signature.
#[test]
fn row1_a_refused_exchange_records_no_key_material_and_no_claim() -> Result<()> {
    let signer = author_signer(0x11)?;
    let claim_signer = maintainer_signer(0x12)?;

    let sender = fresh_repo("pexch-accept-row1-sender")?;
    let mut objects = FileObjectStore::new(sender.clone());
    let blob = signed_blob_envelope(b"row1 fixture\n")?;
    let blob_id = objects.write_object(&blob)?;
    let mut patch = signed_author_patch_envelope(&signer, "row1.txt", 0x13, blob_id)?;
    // The single mutated line: flip one byte of the AUTHOR signature itself.
    patch.signatures[0].signature_bytes[0] ^= 0x01;
    let patch_id = objects.write_object(&patch)?;
    let active_lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(
        &sender,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    drop(active_lock);

    // A validly-signed, otherwise-acceptable claim rides along -- proving its own soundness would
    // not, by itself, save the exchange from the patch's corrupted signature.
    let claim = signed_claim_envelope(
        &claim_signer,
        ObjectId::from_bytes([0x99; 32]),
        vec![patch_id],
    );
    let claim = claim?;
    let claim_id = objects.write_object(&claim)?;

    let (_, bytes) = export_exchange_artifact(&sender, &[patch_id], &[claim_id], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    // The receiver is not empty: it already carries unrelated author-key material of its own
    // (from a wholly different signer). A trivially-empty-before/empty-after comparison would not
    // prove the container is untouched -- it would only prove nothing was ever written to it. This
    // pre-seeded entry is what "byte-for-byte" actually needs to be checked against.
    let receiver = fresh_repo("pexch-accept-row1-receiver")?;
    let unrelated_signer = author_signer(0x14)?;
    let unrelated_lock = ActiveLock::acquire(&receiver, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(
        &receiver,
        unrelated_signer.key_id(),
        unrelated_signer.public_key_bytes(),
        &unrelated_lock,
    )?;
    drop(unrelated_lock);
    let author_key_bytes_before = author_key_container_bytes(&receiver)?;
    assert!(!author_key_bytes_before.is_empty(), "fixture sanity");

    let result = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits());
    assert!(
        result.is_err(),
        "a corrupted AUTHOR signature must refuse the whole exchange"
    );

    let author_key_bytes_after = author_key_container_bytes(&receiver)?;
    assert_eq!(
        author_key_bytes_before, author_key_bytes_after,
        "byte-for-byte: the author-key container -- including its pre-existing unrelated entry --\
         must be untouched by a refused exchange"
    );
    assert!(lookup_author_key_entries(&receiver, signer.key_id())?.is_empty());

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(!snapshot.contains_object(ObjectType::Patch, patch_id));
    assert!(!snapshot.contains_object(ObjectType::RecognitionClaim, claim_id));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

fn author_key_container_bytes(layout: &RepositoryLayout) -> Result<Vec<u8>> {
    let relative = layout.repository_relative(&layout.author_key_container_path())?;
    Ok(crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )?
    .unwrap_or_default())
}

/// Review condition 1 (`RFC-115-stage-3-exchange-artifact-review-v1.md` §2): row 1's own test above
/// only reaches a **Phase C** failure, which precedes every write -- it cannot exercise the window
/// this guards. This test forces a failure inside **Phase D itself**, after Phase C has already
/// verified everything and after patches/blobs have already been written, by holding the receiver's
/// `ActiveLock` open before calling accept -- the nested `ActiveLock::acquire` inside Phase D then
/// fails deterministically on exclusive-create contention, the same failure mode design §8.1's
/// "a concurrent writer... or an I/O error during `record_author_key_material`" describes, without
/// needing a genuine second thread. Before the fix, the claim (written in the old Phase D item 10,
/// ahead of the lock) would have survived this exact failure; the reordered code writes claims only
/// after item 11 succeeds, so it must not.
#[test]
fn phase_d_lock_contention_after_verification_leaves_no_claim_behind() -> Result<()> {
    let signer = author_signer(0x15)?;
    let claim_signer = maintainer_signer(0x16)?;

    let sender = fresh_repo("pexch-accept-phased-sender")?;
    let mut objects = FileObjectStore::new(sender.clone());
    let blob = signed_blob_envelope(b"phase-d fixture\n")?;
    let blob_id = objects.write_object(&blob)?;
    let patch = signed_author_patch_envelope(&signer, "phase-d.txt", 0x17, blob_id)?;
    let patch_id = objects.write_object(&patch)?;
    let active_lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(
        &sender,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    drop(active_lock);
    // A validly-signed claim -- everything about it passes Phase C on its own; only the later
    // author-key lock step is made to fail.
    let claim = signed_claim_envelope(
        &claim_signer,
        ObjectId::from_bytes([0x9a; 32]),
        vec![patch_id],
    )?;
    let claim_id = objects.write_object(&claim)?;
    let (_, bytes) = export_exchange_artifact(&sender, &[patch_id], &[claim_id], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let receiver = fresh_repo("pexch-accept-phased-receiver")?;
    // Held for the whole accept call below -- the single line simulating the Phase D window: the
    // nested `ActiveLock::acquire` inside `accept_exchange_artifact` must fail while this is held.
    let held_lock = ActiveLock::acquire(&receiver, DEFAULT_ACTIVE_NAME)?;

    let result = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits());
    assert!(
        result.is_err(),
        "lock contention during Phase D must fail the whole accept"
    );
    drop(held_lock);

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(
        !snapshot.contains_object(ObjectType::RecognitionClaim, claim_id),
        "a claim must not survive a Phase D failure, even one that occurs after Phase C's own \
         verification already passed"
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 rows 2 and 3: trust never expands on receipt, and a claim naming an unadopted key still
/// accepts the exchange -- the key stays unadopted, and the claim reads `Unverifiable`, never
/// `Sound`.
#[test]
fn row2_row3_an_unadopted_claim_signer_accepts_but_confers_no_trust() -> Result<()> {
    let signer = author_signer(0x20)?;
    let claim_signer = maintainer_signer(0x21)?;
    let (bytes, patch_id) = {
        let sender = fresh_repo("pexch-accept-row23-sender")?;
        let mut objects = FileObjectStore::new(sender.clone());
        let blob = signed_blob_envelope(b"row23 fixture\n")?;
        let blob_id = objects.write_object(&blob)?;
        let patch = signed_author_patch_envelope(&signer, "row23.txt", 0x22, blob_id)?;
        let patch_id = objects.write_object(&patch)?;
        let active_lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
        record_author_key_material(
            &sender,
            signer.key_id(),
            signer.public_key_bytes(),
            &active_lock,
        )?;
        drop(active_lock);
        let claim = signed_claim_envelope(
            &claim_signer,
            ObjectId::from_bytes([0x88; 32]),
            vec![patch_id],
        )?;
        let claim_id = objects.write_object(&claim)?;
        let (_, bytes) = export_exchange_artifact(&sender, &[patch_id], &[claim_id], &[])?;
        let _ = std::fs::remove_dir_all(sender.root());
        (bytes, patch_id)
    };

    let receiver = fresh_repo("pexch-accept-row23-receiver")?;
    let policy_before = load_maintainer_trust_policy(&receiver);
    // A fresh repository has no trust policy snapshot at all yet -- confirm that, then confirm it
    // is still true after a successful accept: trust does not expand from nothing to something.
    assert!(policy_before.is_err());

    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(report.patch_count, 1);
    assert_eq!(
        report.claim_signature_outcomes,
        vec![(
            report.claim_signature_outcomes[0].0,
            ClaimSignatureVerification::Unverifiable {
                key_id: claim_signer.key_id().to_string()
            }
        )]
    );

    let policy_after = load_maintainer_trust_policy(&receiver);
    assert!(
        policy_after.is_err(),
        "an unadopted claim signer must not have been adopted by accepting the claim"
    );

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(snapshot.contains_object(ObjectType::Patch, patch_id));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 4: missing closure refuses the whole exchange, no partial apply. The single mutated
/// line: the blob section is re-encoded to omit the one blob the carried patch's `CreateFile`
/// operation references. `parent_patch_ids` and the declared digest are untouched, isolating the
/// failure to the closure check alone.
#[test]
fn row4_a_missing_referenced_blob_refuses_the_whole_exchange() -> Result<()> {
    let signer = author_signer(0x40)?;
    let (bytes, patch_id) = build_single_patch_artifact("pexch-accept-row4-sender", &signer, true)?;
    let mutated = reencode_artifact(&bytes, None, Some(Vec::new()), None, None)?;

    let receiver = fresh_repo("pexch-accept-row4-receiver")?;
    let result = accept_exchange_artifact(&receiver, &mutated, &AcceptOptions::default_limits());
    assert!(
        result.is_err(),
        "a missing referenced blob must refuse the whole exchange"
    );

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(
        !snapshot.contains_object(ObjectType::Patch, patch_id),
        "no partial apply: the patch itself must not have been written either"
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 5: bounds enforced before decoding -- a declared count over the configured limit is
/// rejected on the integer, at the public `accept_exchange_artifact` entry point (the low-level
/// decode function's own version of this is `artifact::tests`' own coverage).
///
/// Review condition 2 (`RFC-115-stage-3-exchange-artifact-review-v1.md` §3): the fixture must carry
/// **zero** recorded author-key entries. With one recorded (as an earlier version of this test
/// built), the artifact's own author-key section also declares a nonzero count, and *that* section's
/// own bound check independently refuses under `max_object_count: 0` -- so disabling only the
/// patches/blobs/claims section's shared count guard left this test passing for the wrong reason. A
/// fixture with no author-key material isolates the control to the one line this test means to
/// cover.
#[test]
fn row5_a_declared_count_over_the_configured_limit_is_rejected() -> Result<()> {
    let signer = author_signer(0x50)?;
    let (bytes, _patch_id) =
        build_single_patch_artifact("pexch-accept-row5-sender", &signer, false)?;

    let receiver = fresh_repo("pexch-accept-row5-receiver")?;
    let tight_options = AcceptOptions::default_limits().with_max_object_count(0);
    let result = accept_exchange_artifact(&receiver, &bytes, &tight_options);
    assert!(result.is_err());

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(
        !snapshot.contains_object(ObjectType::Patch, _patch_id),
        "nothing must be written when the declared count alone is refused"
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 6, first half: a patch failing against transported material refuses. Same single-byte
/// signature mutation as row 1, checked here for its own error content.
#[test]
fn row6_a_patch_signature_that_fails_against_transported_material_refuses() -> Result<()> {
    let signer = author_signer(0x60)?;
    let sender = fresh_repo("pexch-accept-row6-sender")?;
    let mut objects = FileObjectStore::new(sender.clone());
    let blob = signed_blob_envelope(b"row6 fixture\n")?;
    let blob_id = objects.write_object(&blob)?;
    let mut patch = signed_author_patch_envelope(&signer, "row6.txt", 0x61, blob_id)?;
    patch.signatures[0].signature_bytes[0] ^= 0x01;
    let patch_id = objects.write_object(&patch)?;
    let active_lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(
        &sender,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    drop(active_lock);
    let (_, bytes) = export_exchange_artifact(&sender, &[patch_id], &[], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let receiver = fresh_repo("pexch-accept-row6a-receiver")?;
    let error =
        accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits()).unwrap_err();
    assert!(error.to_string().contains("does not verify"));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 6, second half: a patch with no material available in either source reads
/// `Unverifiable`, never `Sound`, and does not by itself refuse.
#[test]
fn row6_a_patch_with_no_material_anywhere_reads_unverifiable_not_sound() -> Result<()> {
    let signer = author_signer(0x62)?;
    // `record_locally = false`: the sender never records material, so export carries an empty
    // author-key section for this signer's key_id (material is optional per-author).
    let (bytes, patch_id) =
        build_single_patch_artifact("pexch-accept-row6b-sender", &signer, false)?;

    let receiver = fresh_repo("pexch-accept-row6b-receiver")?;
    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(
        report.author_signature_outcomes,
        vec![(
            patch_id,
            AuthorSignatureVerification::Unverifiable {
                key_id: signer.key_id().to_string()
            }
        )]
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 7: replay is inert. Accepting an identical artifact twice writes no new object, records
/// no new key material, changes no state on the second pass. Asserted directly against the second
/// `AcceptReport`, never via `verify_repository`.
#[test]
fn row7_replaying_an_identical_artifact_is_inert() -> Result<()> {
    let signer = author_signer(0x70)?;
    let (bytes, _patch_id) =
        build_single_patch_artifact("pexch-accept-row7-sender", &signer, true)?;

    let receiver = fresh_repo("pexch-accept-row7-receiver")?;
    let first = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(first.written_object_count, 2);
    assert_eq!(first.recorded_author_key_count, 1);

    let author_key_bytes_after_first = author_key_container_bytes(&receiver)?;

    let second = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(second.written_object_count, 0, "no new object on replay");
    // `recorded_author_key_count` mirrors `BundleImportReport::recorded_author_key_count`'s own
    // established semantics (`bundle.rs`'s `import_bundle`, copied here per the handoff's own
    // instruction): a count of entries the artifact carried and `record_author_key_material` was
    // asked to record, not a count of entries that were genuinely new -- `record_author_key_material`
    // is itself idempotent, so this is expected to equal the first call's count again. The container
    // itself not growing (below) is the actual inertness property; `bundle/tests.rs`'s own
    // `reimporting_the_same_bundle_records_no_new_author_key_entries` asserts the same way, for the
    // same reason.
    assert_eq!(
        second.recorded_author_key_count,
        first.recorded_author_key_count
    );

    let author_key_bytes_after_second = author_key_container_bytes(&receiver)?;
    assert_eq!(
        author_key_bytes_after_first, author_key_bytes_after_second,
        "byte-for-byte: the author-key container must not grow on replay"
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 8: digest mismatch refuses before signature work. The single mutated line: the patch
/// list is re-encoded down to one of its two patches, while the declared digest (still covering
/// both) is left exactly as the original export produced it.
#[test]
fn row8_a_digest_mismatch_refuses_before_signature_work() -> Result<()> {
    let signer = author_signer(0x80)?;
    let sender = fresh_repo("pexch-accept-row8-sender")?;
    let mut objects = FileObjectStore::new(sender.clone());
    let blob_a = signed_blob_envelope(b"row8 fixture a\n")?;
    let blob_a_id = objects.write_object(&blob_a)?;
    let patch_a = signed_author_patch_envelope(&signer, "row8-a.txt", 0x81, blob_a_id)?;
    let patch_a_id = objects.write_object(&patch_a)?;
    let blob_b = signed_blob_envelope(b"row8 fixture b\n")?;
    let blob_b_id = objects.write_object(&blob_b)?;
    let patch_b = signed_author_patch_envelope(&signer, "row8-b.txt", 0x82, blob_b_id)?;
    let patch_b_id = objects.write_object(&patch_b)?;
    let active_lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(
        &sender,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    drop(active_lock);
    let (_, bytes) = export_exchange_artifact(&sender, &[patch_a_id, patch_b_id], &[], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let decoded = crate::patch_exchange::artifact::decode_exchange_artifact(&bytes, 10_000_000)?;
    let truncated_patches = vec![decoded.patches[0].clone()];
    let mutated = reencode_artifact(&bytes, Some(truncated_patches), None, None, None)?;

    let receiver = fresh_repo("pexch-accept-row8-receiver")?;
    let error = accept_exchange_artifact(&receiver, &mutated, &AcceptOptions::default_limits())
        .unwrap_err();
    assert!(error.to_string().contains("digest"));

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(!snapshot.contains_object(ObjectType::Patch, patch_a_id));
    assert!(!snapshot.contains_object(ObjectType::Patch, patch_b_id));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// §7 row 9: a claim contradicting a block this repository already holds refuses the whole
/// exchange, regardless of whether the claim's own signer is locally adopted -- a demonstrated
/// integrity failure is not a trust question (design D3, handoff §4.2 item 9's own note).
#[test]
fn row9_a_claim_contradicting_a_held_block_refuses_the_exchange() -> Result<()> {
    let claim_signer = maintainer_signer(0x90)?;
    let held_patch_id = ObjectId::from_bytes([0x91; 32]);
    let claimed_patch_id = ObjectId::from_bytes([0x92; 32]);
    assert_ne!(
        held_patch_id, claimed_patch_id,
        "fixture sanity: a genuine contradiction"
    );

    let receiver = fresh_repo("pexch-accept-row9-receiver")?;
    let mut receiver_objects = FileObjectStore::new(receiver.clone());
    let held_block = signed_block(BlockKind::Root, Vec::new(), vec![held_patch_id], None);
    let block_id = receiver_objects.write_object(&held_block)?;

    let sender = fresh_repo("pexch-accept-row9-sender")?;
    let mut sender_objects = FileObjectStore::new(sender.clone());
    let claim = signed_claim_envelope(&claim_signer, block_id, vec![claimed_patch_id])?;
    let claim_id = sender_objects.write_object(&claim)?;
    let (_, bytes) = export_exchange_artifact(&sender, &[], &[claim_id], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let error =
        accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits()).unwrap_err();
    assert!(error.to_string().contains("contradicts"));

    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(!snapshot.contains_object(ObjectType::RecognitionClaim, claim_id));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// A claim consistent with a block this repository already holds must not refuse -- the positive
/// case alongside row 9's negative one, and alongside row 2/3's `BlockAbsent` case, exercises all
/// three `RecognitionClaimConsistency` outcomes across this file.
#[test]
fn a_claim_consistent_with_a_held_block_accepts() -> Result<()> {
    let claim_signer = maintainer_signer(0xA0)?;
    let held_patch_id = ObjectId::from_bytes([0xA1; 32]);

    let receiver = fresh_repo("pexch-accept-consistent-receiver")?;
    let mut receiver_objects = FileObjectStore::new(receiver.clone());
    let held_block = signed_block(BlockKind::Root, Vec::new(), vec![held_patch_id], None);
    let block_id = receiver_objects.write_object(&held_block)?;

    let sender = fresh_repo("pexch-accept-consistent-sender")?;
    let mut sender_objects = FileObjectStore::new(sender.clone());
    let claim = signed_claim_envelope(&claim_signer, block_id, vec![held_patch_id])?;
    let claim_id = sender_objects.write_object(&claim)?;
    let (_, bytes) = export_exchange_artifact(&sender, &[], &[claim_id], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(report.claim_count, 1);
    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(snapshot.contains_object(ObjectType::RecognitionClaim, claim_id));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// A claim naming a `key_id` this repository *has* adopted reads `Sound`, completing the three
/// `ClaimSignatureVerification`/`RecognitionClaimConsistency` outcomes exercised in this file
/// alongside row 2/3's `Unverifiable` case and row 9's contradiction refusal.
#[test]
fn a_claim_naming_an_adopted_key_reads_sound() -> Result<()> {
    let claim_signer = maintainer_signer(0xC0)?;

    let receiver = fresh_repo("pexch-accept-adopted-claim-receiver")?;
    add_trusted_maintainer(
        &receiver,
        claim_signer.key_id(),
        &public_key_hex(&claim_signer),
    )?;

    let sender = fresh_repo("pexch-accept-adopted-claim-sender")?;
    let mut sender_objects = FileObjectStore::new(sender.clone());
    let claim = signed_claim_envelope(
        &claim_signer,
        ObjectId::from_bytes([0xC1; 32]),
        vec![ObjectId::from_bytes([0xC2; 32])],
    )?;
    let claim_id = sender_objects.write_object(&claim)?;
    let (_, bytes) = export_exchange_artifact(&sender, &[], &[claim_id], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(
        report.claim_signature_outcomes,
        vec![(
            claim_id,
            ClaimSignatureVerification::Sound {
                key_id: claim_signer.key_id().to_string()
            }
        )]
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// A carried patch with a non-empty `parent_patch_ids` refuses the whole exchange -- this field is
/// always empty today; nothing authors a non-empty one, so this test constructs one directly.
///
/// `PatchPayload` no longer has a `parent_patch_ids` field at all (Patch schema 2 handoff: tag 2 is
/// retired, never emitted by `encode_canonical`) -- schema 1 still legally carries it (backward
/// compatibility with every patch already written), so this fixture bypasses `PatchPayload` and
/// writes tag 2 directly with `CanonicalWriter`, exactly where `encode_canonical` used to emit it,
/// to construct the one shape no production code can author anymore.
#[test]
fn a_non_empty_parent_patch_ids_refuses() -> Result<()> {
    use prikk_object::{
        CanonicalWriter, CreateFile, NodeId, ObjectEnvelope, ObjectType, Operation, OperationKind,
    };

    let signer = author_signer(0xB0)?;
    let sender = fresh_repo("pexch-accept-parent-ids-sender")?;
    let mut objects = FileObjectStore::new(sender.clone());
    let blob = signed_blob_envelope(b"parent-ids fixture\n")?;
    let blob_id = objects.write_object(&blob)?;
    let operations = vec![Operation {
        op_seq: 1,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(CreateFile {
            path: "parent-ids.txt".to_string(),
            node_id: NodeId::from_bytes([0xB1; 32]),
            blob_id,
            mode: 0o100_644,
        }),
    }];
    let mut writer = CanonicalWriter::new();
    writer.repeated_record_list(1, &operations)?;
    writer.repeated_object_id(2, &[ObjectId::from_bytes([0xB2; 32])])?;
    let canonical_payload = writer.finish();
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, canonical_payload);
    let id = patch.object_id();
    patch.add_signature(crate::author::author_signing::author_signature(
        &signer, id,
    )?)?;
    let patch_id = objects.write_object(&patch)?;
    let active_lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(
        &sender,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )?;
    drop(active_lock);
    let (_, bytes) = export_exchange_artifact(&sender, &[patch_id], &[], &[])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let receiver = fresh_repo("pexch-accept-parent-ids-receiver")?;
    let error =
        accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits()).unwrap_err();
    assert!(error.to_string().contains("parent_patch_ids"));
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// RFC 117 stage 3 §3: a healthy artifact carrying a Tag object is accepted, the tag is written, and
/// its `Sound` signature outcome is reported -- the same treatment a recognition claim already gets.
#[test]
fn a_healthy_artifact_with_a_sound_tag_is_accepted_and_writes_it() -> Result<()> {
    let sender = fresh_repo("pexch-accept-tag-sound-sender")?;
    let mut sender_objects = FileObjectStore::new(sender.clone());
    let block = sender_objects.write_object(&signed_block(
        BlockKind::Root,
        Vec::new(),
        vec![ObjectId::from_bytes([0x41; 32])],
        None,
    ))?;
    let (digest, count) = compute_patch_set_digest_and_count_from_block(&sender_objects, block)?;
    let tag_signer = maintainer_signer(0x42)?;
    let tag = signed_tag_envelope(&tag_signer, "tags/accept-sound", block, digest, count)?;
    let tag_id = sender_objects.write_object(&tag)?;
    let (_, bytes) = export_exchange_artifact(&sender, &[], &[], &[tag_id])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let receiver = fresh_repo("pexch-accept-tag-sound-receiver")?;
    add_trusted_maintainer(&receiver, tag_signer.key_id(), &public_key_hex(&tag_signer))?;
    let report = accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(report.tag_count, 1);
    assert_eq!(
        report.tag_signature_outcomes,
        vec![(
            tag_id,
            TagSignatureVerification::Sound {
                key_id: tag_signer.key_id().to_string()
            }
        )]
    );
    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(snapshot.contains_object(ObjectType::Tag, tag_id));

    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// A declared tag count over the configured limit refuses before any tag is decoded -- the fifth
/// application of DC-86's rule this format now makes (module doc).
#[test]
fn decode_rejects_a_declared_tag_count_over_the_configured_limit() -> Result<()> {
    let sender = fresh_repo("pexch-accept-tag-limit-sender")?;
    let mut sender_objects = FileObjectStore::new(sender.clone());
    let block = sender_objects.write_object(&signed_block(
        BlockKind::Root,
        Vec::new(),
        vec![ObjectId::from_bytes([0x43; 32])],
        None,
    ))?;
    let (digest, count) = compute_patch_set_digest_and_count_from_block(&sender_objects, block)?;
    let tag_signer = maintainer_signer(0x44)?;
    let tag = signed_tag_envelope(&tag_signer, "tags/accept-limit", block, digest, count)?;
    let tag_id = sender_objects.write_object(&tag)?;
    let (_, bytes) = export_exchange_artifact(&sender, &[], &[], &[tag_id])?;
    let _ = std::fs::remove_dir_all(sender.root());

    let receiver = fresh_repo("pexch-accept-tag-limit-receiver")?;
    let error = accept_exchange_artifact(
        &receiver,
        &bytes,
        &AcceptOptions::default_limits().with_max_object_count(0),
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("tags") && message.contains('0'),
        "expected a declared-count-over-limit refusal naming the section and the limit, got: \
         {message}"
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}
