//! 0.56.0 step 3f, R2: CLI-level proof of `extract-report-v1`'s chain fields and the chain walk's
//! four new refusal ids. Before this file, `plan.chain`/`plan.chain_signature`/`other_candidates`,
//! `claim-signature-invalid`, `claim-does-not-fold`, `claim-chain-shared-block` and
//! `claim-chain-cycle`, and the new text lines (`link N: …`, `also recognized by, not used: …`,
//! `unverified`) were proven only at the `prikk-store` library level
//! (`crates/prikk-store/src/extract/tests.rs`); the CLI's own printers
//! (`crates/prikk-cli/src/extract.rs`, `print_chain_lines`/`push_chain_link_json`) were never
//! driven through the compiled binary. planeter reads this document, so this file closes that gap.
//!
//! **Fixture technique:** two kinds, both already established elsewhere in this test suite:
//! - An honest two-repository `sync build`/`sync accept` flow (`release_0_56_0_step1a_sync_pending_json.rs`'s
//!   own `accept_one_pending_patch`, extended to three generations) for the sound and unverifiable
//!   chains — real signatures, real patches, nothing hand-crafted.
//! - Hand-crafted `RecognitionClaim`/`Patch` objects written directly with `FileObjectStore`, into a
//!   repository `prikk init` already created and `prikk commit`/`seal` already gave a real sealed
//!   genesis — the identical technique `crates/prikk-store/src/extract/tests.rs` uses at the library
//!   level, and already established for CLI-level tests elsewhere in this crate (`FileObjectStore`/
//!   `RepositoryLayout::open` are used the same way in `branch_and_tag/dc61_branch_closure.rs`) —
//!   for the shapes no honest CLI flow can produce at all (a claim whose own patch does not fold, a
//!   shared block recognized by two independent claims, a cycle). The compiled binary still does
//!   every read: only the fixture's own setup bypasses the CLI.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::extract::assert_refusal_json;
use crate::support;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, ChangePerm, CreateFile, NodeId, ObjectEnvelope,
    ObjectId, ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
    RecognitionClaimPayload,
};
use prikk_store::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectWriter,
    RepositoryLayout, author_signature, maintainer_signature,
};

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// `extract --claim <claim_id> --format json`, writing into a fresh target under `repo`'s own
/// unique temp area (never reused across calls in the same test).
fn extract_claim_json(repo: &Path, claim_id: &str, target: &Path) -> Output {
    support::prikk(repo)
        .args([
            "extract",
            "--claim",
            claim_id,
            "--format",
            "json",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn extract_claim_text(repo: &Path, claim_id: &str, target: &Path) -> Output {
    support::prikk(repo)
        .args(["extract", "--claim", claim_id, target.to_str().unwrap()])
        .output()
        .unwrap()
}

fn target_dir(tag: &str) -> PathBuf {
    let dir = support::unique_repo(tag);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A fresh scratch *file* path (never created as a directory), for a sync artifact/have-list --
/// `release_0_56_0_step1a_sync_pending_json.rs`'s own `sync_file` helper, duplicated here rather
/// than shared (that file's helper is private to its own module).
fn sync_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "step3f-claim-chain-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

/// The tip block id of `ref_name`, read through `log` (the same text `rfc153_point_resolver.rs`'s
/// own `blocks()` helper parses), never through a library call -- this one value is read from the
/// compiled binary's own output even in the hand-crafted fixtures below.
fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    let output = support::prikk(repo)
        .args(["log", "--ref", ref_name, "--limit", "1"])
        .output()
        .unwrap();
    support::ok(&output, "log");
    let text = stdout(&output);
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("block "))
        .unwrap_or_else(|| panic!("log must print a block line: {text}"));
    line.trim().parse().expect("a well-formed block id")
}

/// Every `claim <id>` (or `  claim <id>`) line `sync accept` printed, in print order.
fn accepted_claim_ids(accept_stdout: &str) -> Vec<String> {
    accept_stdout
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("claim "))
        .filter_map(|rest| rest.split(':').next())
        .map(|id| id.trim().to_string())
        .collect()
}

/// Build repo `a` (three sealed generations of `a.txt`: "1", "2", "3") and repo `b` (empty, never
/// seals, takes a have-list while empty), then land `a`'s single `sync build` artifact in `b` in one
/// `sync accept` -- the review's own measured fixture (`.git-exclude/reviewed/0.56.0-step-3e-review-v1.md`,
/// "Measured on the build of `f80fc5f6`"). `trust_b_first` mirrors `release_0_56_0_step1a_sync_pending_json.rs`'s
/// own choice of trusting before accept to get `Sound` outcomes that persist to extract time;
/// skipping it leaves every claim `Unverifiable` at extract time too (the trust policy is read fresh
/// each time, never pinned at accept time -- D2's own design note).
fn three_generation_chain(tag: &str, trust_b_first: bool) -> (PathBuf, PathBuf, Vec<String>) {
    let repo_a = support::unique_repo(&format!("{tag}-a"));
    let repo_b = support::unique_repo(&format!("{tag}-b"));
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "a.txt", b"1\n", "gen1");
    support::generation(&repo_a, "heads/main", "a.txt", b"2\n", "gen2");
    support::generation(&repo_a, "heads/main", "a.txt", b"3\n", "gen3");
    support::init(&repo_b);

    let have_file = sync_file(&format!("{tag}-have"));
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );

    let artifact_file = sync_file(&format!("{tag}-artifact"));
    let build = support::prikk(&repo_a)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            have_file.to_str().unwrap(),
            "--output",
            artifact_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&build, "sync build");

    if trust_b_first {
        support::trust_maintainer(&repo_b);
    }

    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let claim_ids = accepted_claim_ids(&stdout(&accept));
    assert_eq!(
        claim_ids.len(),
        3,
        "three generations must land as three claims: {}",
        stdout(&accept)
    );
    (repo_a, repo_b, claim_ids)
}

/// Among `claim_ids` (any order), the one whose own `extract --claim --format json` chain has
/// exactly `len` links -- measured, never assumed, since `sync build`/`accept`'s own print order is
/// not part of this round's contract.
fn claim_with_chain_len(repo_b: &Path, claim_ids: &[String], len: usize) -> Option<String> {
    for claim_id in claim_ids {
        let target = target_dir(&format!("chain-probe-{claim_id}"));
        let output = extract_claim_json(repo_b, claim_id, &target);
        let _ = std::fs::remove_dir_all(&target);
        if !output.status.success() {
            continue;
        }
        let json = support::json::parse(&stdout(&output));
        if json.get("chain").as_array().len() == len {
            return Some(claim_id.clone());
        }
    }
    None
}

/// **Test 1** (R2): a real two-link chain, `sound` throughout, through `--format json` and the text
/// form.
#[test]
fn extract_claim_json_reports_a_two_link_sound_chain() {
    let (_repo_a, repo_b, claim_ids) =
        three_generation_chain("step3f-two-link-sound", /* trust_b_first */ true);
    let mid_claim = claim_with_chain_len(&repo_b, &claim_ids, 2)
        .unwrap_or_else(|| panic!("no claim has a two-link chain among {claim_ids:?}"));

    let target = target_dir("step3f-two-link-sound-out");
    let output = extract_claim_json(&repo_b, &mid_claim, &target);
    support::ok(&output, "extract --claim --format json");
    let report = stdout(&output);
    let json = support::json::parse(&report);

    let chain = json.get("chain").as_array();
    assert_eq!(chain.len(), 2, "{report}");
    assert_eq!(chain[1].get("claim_id").as_str(), mid_claim, "{report}");
    for link in chain {
        assert!(!link.get("claim_id").as_str().is_empty(), "{report}");
        assert!(!link.get("target_block_id").as_str().is_empty(), "{report}");
        assert_eq!(
            link.get("signature").get("outcome").as_str(),
            "sound",
            "{report}"
        );
        assert!(
            link.get("other_candidates").as_array().is_empty(),
            "no shared-block collision in this fixture: {report}"
        );
    }
    // The root link's own declared parent is the repository's very first (root) block, which has
    // no parent at all; the second link's own declared parent is the root link's own target block
    // -- root to foot order, proven structurally rather than assumed.
    assert_eq!(
        chain[1].get("parent_block_ids").as_array()[0].as_str(),
        chain[0].get("target_block_id").as_str(),
        "{report}"
    );
    assert_eq!(
        json.get("chain_signature").as_str(),
        "sound",
        "the weakest (and only) outcome in the chain: {report}"
    );

    let target_text = target_dir("step3f-two-link-sound-out-text");
    let text_output = extract_claim_text(&repo_b, &mid_claim, &target_text);
    support::ok(&text_output, "extract --claim (text)");
    let text = stdout(&text_output);
    assert!(text.contains("chain: 2 link(s), root to foot"), "{text}");
    assert!(
        text.contains(&format!("link 2: claim {mid_claim} (sound)")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line.trim_start().starts_with("link 1: claim ")
                && line.trim_end().ends_with("(sound)")),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&_repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
}

/// **Test 2** (R2): the identical fixture, but `b` never adopts `a`'s maintainer key -- the chain is
/// `unverifiable` throughout, reported as `chain_signature: "unverifiable"` and the text form's own
/// `unverified: …` line.
#[test]
fn extract_claim_json_reports_an_unverifiable_chain() {
    let (_repo_a, repo_b, claim_ids) = three_generation_chain(
        "step3f-two-link-unverifiable",
        /* trust_b_first */ false,
    );
    let mid_claim = claim_with_chain_len(&repo_b, &claim_ids, 2)
        .unwrap_or_else(|| panic!("no claim has a two-link chain among {claim_ids:?}"));

    let target = target_dir("step3f-unverifiable-out");
    let output = extract_claim_json(&repo_b, &mid_claim, &target);
    support::ok(&output, "extract --claim --format json");
    let report = stdout(&output);
    let json = support::json::parse(&report);
    assert_eq!(
        json.get("chain_signature").as_str(),
        "unverifiable",
        "{report}"
    );
    for link in json.get("chain").as_array() {
        assert_eq!(
            link.get("signature").get("outcome").as_str(),
            "unverifiable",
            "{report}"
        );
    }

    let target_text = target_dir("step3f-unverifiable-out-text");
    let text_output = extract_claim_text(&repo_b, &mid_claim, &target_text);
    support::ok(&text_output, "extract --claim (text)");
    let text = stdout(&text_output);
    assert!(
        text.contains(
            "unverified: at least one link's MAINTAINER signature names a key this repository \
             has not adopted"
        ),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&_repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
}

/// A repository with one real, admitted (replay-verified) genesis block, through `prikk init` and
/// one real `generation` -- the CLI-level equivalent of `crates/prikk-store/src/extract/tests.rs`'s
/// own `admitted_repo_for_claim_fixtures`.
fn admitted_repo(tag: &str) -> (PathBuf, RepositoryLayout, ObjectId) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "README.md", b"genesis\n", "genesis");
    let genesis_block = tip_block_id(&repo, "heads/main");
    let layout = RepositoryLayout::open(&repo).unwrap();
    (repo, layout, genesis_block)
}

/// A single `CreateFile` patch, author-signed for real (`author_signature`) -- patch signatures are
/// never read by the chain walk/fold, only the claim's own MAINTAINER signature is in scope, so any
/// genuine signature shape is enough.
fn write_simple_create_file_patch(
    store: &mut FileObjectStore,
    author: &Ed25519AuthorSigner,
    path: &str,
    node_byte: u8,
    content: &[u8],
) -> ObjectId {
    let blob_payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    let mut blob_envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        blob_payload.to_canonical_bytes().unwrap(),
    );
    let blob_id = blob_envelope.object_id();
    let blob_sig = author_signature(author, blob_id).unwrap();
    blob_envelope.add_signature(blob_sig).unwrap();
    let blob = store.write_object(&blob_envelope).unwrap();

    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_byte; 32]),
                blob_id: blob,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        patch_payload.to_canonical_bytes().unwrap(),
    );
    let patch_id = patch.object_id();
    let patch_sig = author_signature(author, patch_id).unwrap();
    patch.add_signature(patch_sig).unwrap();
    store.write_object(&patch).unwrap()
}

/// A single `ChangePerm` patch naming a node id that is never live in any state this fold would
/// reach -- a genuine fold mismatch, the identical fixture shape
/// `crates/prikk-store/src/extract/tests.rs::write_non_folding_change_perm_patch` uses, built here
/// through the same real author-signing path.
fn write_non_folding_change_perm_patch(
    store: &mut FileObjectStore,
    author: &Ed25519AuthorSigner,
) -> ObjectId {
    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id: NodeId::from_bytes([0xFE; 32]),
                old_mode: 0o100644,
                new_mode: 0o100755,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        patch_payload.to_canonical_bytes().unwrap(),
    );
    let patch_id = patch.object_id();
    let patch_sig = author_signature(author, patch_id).unwrap();
    patch.add_signature(patch_sig).unwrap();
    store.write_object(&patch).unwrap()
}

/// Store `payload` as a `RecognitionClaim` signed for real by `signer` -- `Sound` when `signer`'s
/// own key is adopted by the repository the claim is later read from, `Unverifiable` otherwise.
fn write_claim_signed_by(
    store: &mut FileObjectStore,
    payload: RecognitionClaimPayload,
    signer: &impl MaintainerSigner,
) -> ObjectId {
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes().unwrap(),
    );
    let claim_id = envelope.object_id();
    let signature = maintainer_signature(signer, ObjectType::RecognitionClaim, claim_id).unwrap();
    envelope.add_signature(signature).unwrap();
    store.write_object(&envelope).unwrap()
}

fn author_signer(label: &str, byte: u8) -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed(label, &[byte; 32]).unwrap()
}

fn maintainer_signer(label: &str, byte: u8) -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed(label, &[byte; 32]).unwrap()
}

/// **Test 3** (R2): `claim-signature-invalid`, built honestly through `sync build`/`sync accept`
/// (one generation is enough — no chain needed), then adopting a *bogus* public key under the same
/// key id `sync build` actually signed with, after accept (so accept itself still sees an
/// unadopted, merely `Unverifiable` signature and does not refuse) -- the CLI-level twin of
/// `crates/prikk-store/src/extract/tests.rs::adopt_bogus_maintainer_key_named_maintainer_key`, run
/// through `prikk trust maintainer add` instead of a direct `add_trusted_maintainer` call. The real
/// signature now fails to verify against the adopted-but-wrong key -- a forged signature under a
/// trusted identity.
#[test]
fn extract_claim_refuses_claim_signature_invalid_via_bogus_adopted_key() {
    let repo_a = support::unique_repo("step3f-claim-signature-invalid-a");
    let repo_b = support::unique_repo("step3f-claim-signature-invalid-b");
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "a.txt", b"one\n", "gen1");
    support::init(&repo_b);

    let have_file = sync_file("claim-signature-invalid-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact_file = sync_file("claim-signature-invalid-artifact");
    let build = support::prikk(&repo_a)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            have_file.to_str().unwrap(),
            "--output",
            artifact_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&build, "sync build");
    // `b` never adopts the real key before accept: `Unverifiable`, not a refusal.
    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let claim_ids = accepted_claim_ids(&stdout(&accept));
    assert_eq!(claim_ids.len(), 1, "{}", stdout(&accept));
    let claim_id = &claim_ids[0];

    // A bogus (shape-valid, wrong) 32-byte public key, adopted under the *real* key id, after
    // accept.
    let bogus_public_key_hex = prikk_hash::to_hex(&[0x42_u8; 32]);
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                support::MAINTAINER_KEY_ID,
                "--public-key",
                &bogus_public_key_hex,
            ])
            .output()
            .unwrap(),
        "trust maintainer add (bogus key)",
    );

    let target = target_dir("step3f-claim-signature-invalid-out");
    let output = extract_claim_json(&repo_b, claim_id, &target);
    assert_refusal_json(
        &output,
        "claim-signature-invalid",
        None,
        "does not carry a usable MAINTAINER signature",
    );

    let _ = std::fs::remove_dir_all(&repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
}

/// **Test 4** (R2): `claim-does-not-fold` -- a non-foot link's own patch does not fold onto its
/// declared parent's state. No honest `sync build` ever produces this (every patch it carries folds
/// by construction), so this uses the hand-crafted-object technique described at this file's top.
#[test]
fn extract_claim_refuses_claim_does_not_fold() {
    let (repo, layout, genesis_block) = admitted_repo("step3f-claim-does-not-fold");
    let mut store = FileObjectStore::new(layout.clone());
    let author = author_signer("step3f-does-not-fold-author", 0xC1);
    let intermediate_block = ObjectId::from_bytes([0xC2; 32]);

    let non_folding_patch = write_non_folding_change_perm_patch(&mut store, &author);
    let root_claim = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: intermediate_block,
            patch_ids: vec![non_folding_patch],
            parent_block_ids: vec![genesis_block],
        },
        &maintainer_signer("step3f-does-not-fold-maintainer", 0xC3),
    );
    let foot_patch =
        write_simple_create_file_patch(&mut store, &author, "foot.txt", 0xC4, b"foot\n");
    let foot_claim = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0xC5; 32]),
            patch_ids: vec![foot_patch],
            parent_block_ids: vec![intermediate_block],
        },
        &maintainer_signer("step3f-does-not-fold-maintainer", 0xC3),
    );

    let target = target_dir("step3f-claim-does-not-fold-out");
    let output = extract_claim_json(&repo, &foot_claim.to_string(), &target);
    assert_refusal_json(&output, "claim-does-not-fold", None, "do not apply cleanly");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&root_claim.to_string()),
        "the mid-chain link's own id must be named: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// **Test 5** (R2): `claim-chain-shared-block` -- two held claims recognize the same prospective
/// block, neither signed by an adopted key. Hand-crafted, for the identical reason test 4 is: no
/// honest `sync build` produces two independent claims naming the same `block_id` (a real author's
/// `NodeId` is minted with real entropy, so two honest generations never collide).
#[test]
fn extract_claim_refuses_claim_chain_shared_block_with_no_sound_candidate() {
    let (repo, layout, genesis_block) = admitted_repo("step3f-shared-block-no-sound");
    let mut store = FileObjectStore::new(layout.clone());
    let author = author_signer("step3f-shared-block-author", 0xD1);
    let shared_block = ObjectId::from_bytes([0xD2; 32]);
    let never_adopted = maintainer_signer("step3f-shared-block-never-adopted", 0xD3);

    let patch_a =
        write_simple_create_file_patch(&mut store, &author, "a-candidate.txt", 0xD4, b"a\n");
    let candidate_a = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![patch_a],
            parent_block_ids: vec![genesis_block],
        },
        &never_adopted,
    );
    let patch_b =
        write_simple_create_file_patch(&mut store, &author, "b-candidate.txt", 0xD5, b"b\n");
    let candidate_b = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![patch_b],
            parent_block_ids: vec![genesis_block],
        },
        &never_adopted,
    );
    let foot_patch =
        write_simple_create_file_patch(&mut store, &author, "foot.txt", 0xD6, b"foot\n");
    let foot_claim = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0xD7; 32]),
            patch_ids: vec![foot_patch],
            parent_block_ids: vec![shared_block],
        },
        &never_adopted,
    );

    let target = target_dir("step3f-shared-block-no-sound-out");
    let output = extract_claim_json(&repo, &foot_claim.to_string(), &target);
    assert_refusal_json(
        &output,
        "claim-chain-shared-block",
        None,
        "is recognized by more than one held recognition claim",
    );
    let stderr_text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr_text.contains(&candidate_a.to_string()),
        "{stderr_text}"
    );
    assert!(
        stderr_text.contains(&candidate_b.to_string()),
        "{stderr_text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// **Test 6** (R2): `other_candidates`, D1's own exception -- exactly one of two candidates
/// recognizing the same block is `Sound` (this repo's own trusted maintainer, adopted by
/// `generation`'s own seal); the chain resolves through it, and the losing candidate is still
/// printed, never silently dropped.
#[test]
fn extract_claim_json_reports_other_candidates_when_one_sound_candidate_wins() {
    let (repo, layout, genesis_block) = admitted_repo("step3f-shared-block-one-sound");
    let mut store = FileObjectStore::new(layout.clone());
    let author = author_signer("step3f-one-sound-author", 0xE1);
    let shared_block = ObjectId::from_bytes([0xE2; 32]);
    // The repo's own trusted maintainer key, adopted by `generation`'s own `seal`.
    let adopted =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();
    let never_adopted = maintainer_signer("step3f-one-sound-never-adopted", 0xE3);

    let sound_patch =
        write_simple_create_file_patch(&mut store, &author, "sound.txt", 0xE4, b"sound\n");
    let sound_candidate = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![sound_patch],
            parent_block_ids: vec![genesis_block],
        },
        &adopted,
    );
    let unverifiable_patch = write_simple_create_file_patch(
        &mut store,
        &author,
        "unverifiable.txt",
        0xE5,
        b"unverifiable\n",
    );
    let unverifiable_candidate = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: shared_block,
            patch_ids: vec![unverifiable_patch],
            parent_block_ids: vec![genesis_block],
        },
        &never_adopted,
    );
    let foot_patch =
        write_simple_create_file_patch(&mut store, &author, "foot.txt", 0xE6, b"foot\n");
    let foot_claim = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0xE7; 32]),
            patch_ids: vec![foot_patch],
            parent_block_ids: vec![shared_block],
        },
        &adopted,
    );

    let target = target_dir("step3f-shared-block-one-sound-out");
    let output = extract_claim_json(&repo, &foot_claim.to_string(), &target);
    support::ok(&output, "extract --claim --format json");
    let report = stdout(&output);
    let json = support::json::parse(&report);
    let chain = json.get("chain").as_array();
    assert_eq!(chain.len(), 2, "{report}");
    assert_eq!(
        chain[0].get("claim_id").as_str(),
        sound_candidate.to_string(),
        "{report}"
    );
    assert_eq!(
        chain[0].get("signature").get("outcome").as_str(),
        "sound",
        "{report}"
    );
    let others = chain[0].get("other_candidates").as_array();
    assert_eq!(others.len(), 1, "{report}");
    assert_eq!(
        others[0].get("claim_id").as_str(),
        unverifiable_candidate.to_string(),
        "the losing candidate is still named, not silently dropped: {report}"
    );
    assert_eq!(
        others[0].get("signature").get("outcome").as_str(),
        "unverifiable",
        "{report}"
    );

    let target_text = target_dir("step3f-shared-block-one-sound-out-text");
    let text_output = extract_claim_text(&repo, &foot_claim.to_string(), &target_text);
    support::ok(&text_output, "extract --claim (text)");
    let text = stdout(&text_output);
    assert!(
        text.contains("also recognized by, not used:")
            && text.contains(&unverifiable_candidate.to_string()),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// **Test 7** (R2): `claim-chain-cycle` -- two claims whose declared parents loop back on each
/// other. Content-addressed claims cannot form this honestly; a hostile sender still can, so the
/// walk must still refuse cleanly rather than loop or panic.
#[test]
fn extract_claim_refuses_claim_chain_cycle() {
    let (repo, layout, _genesis_block) = admitted_repo("step3f-cycle");
    let mut store = FileObjectStore::new(layout.clone());
    let author = author_signer("step3f-cycle-author", 0xF1);
    let signer = maintainer_signer("step3f-cycle-maintainer", 0xF2);
    let block_x = ObjectId::from_bytes([0xF3; 32]);
    let block_y = ObjectId::from_bytes([0xF4; 32]);

    let patch_g = write_simple_create_file_patch(&mut store, &author, "g.txt", 0xF5, b"g\n");
    let claim_g = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: block_x,
            patch_ids: vec![patch_g],
            parent_block_ids: vec![block_y],
        },
        &signer,
    );
    let patch_f = write_simple_create_file_patch(&mut store, &author, "f.txt", 0xF6, b"f\n");
    let claim_f = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: block_y,
            patch_ids: vec![patch_f],
            parent_block_ids: vec![block_x],
        },
        &signer,
    );

    let target = target_dir("step3f-cycle-out");
    let output = extract_claim_json(&repo, &claim_f.to_string(), &target);
    assert_refusal_json(&output, "claim-chain-cycle", None, "loops back on itself");
    let stderr_text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr_text.contains(&claim_f.to_string()), "{stderr_text}");
    assert!(stderr_text.contains(&claim_g.to_string()), "{stderr_text}");

    let _ = std::fs::remove_dir_all(&repo);
}
