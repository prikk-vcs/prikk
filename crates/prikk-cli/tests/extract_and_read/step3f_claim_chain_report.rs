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
//!
//! **0.56.0 step 3f2 addendum:** three further tests, below the original seven, close the same gap
//! for `outside-version-rule` -- the id R1 (`1fbba9fa`) gave `extract --claim`'s own chain-fold
//! classification, and 3f2 now also gives `extract --ref`'s own fallback classification for the
//! identical cause (a path outside this version's own rule, today non-ASCII), where it had
//! previously and wrongly read `damaged-repository`. These use a third hand-crafted shape beyond
//! the two above: a `Block`/`RefState`/`RefUpdate` triple written directly and published onto
//! `heads/main` through `RefStore::publish`, the same technique
//! `branch_and_tag/dc61_branch_closure.rs`'s own `reopening_a_closed_branch_is_an_ordinary_cas_update`
//! test uses to advance a ref by hand -- no honest `commit`/`seal` ever produces a non-ASCII or
//! empty repo path (0.56.0 step 3d2's own authoring-side refusal), so `extract --ref`'s own
//! coverage of this cause needs the identical bypass the claim-chain tests already use for their
//! own hand-crafted patches.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::extract::assert_refusal_json;
use crate::support;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, ChangePerm, CreateFile,
    NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose, RecognitionClaimPayload, RefStatePayload, RefUpdatePayload,
};
use prikk_store::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader,
    ObjectWriter, RefPublication, RefStore, RepositoryLayout, author_signature,
    compute_state_root, maintainer_signature,
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

// ---------------------------------------------------------------------------------------------
// 0.56.0 step 3f2: `outside-version-rule`, through both `extract --ref` and `extract --claim`.
// ---------------------------------------------------------------------------------------------

/// `extract --ref <point> --format json`, writing into a fresh target under `repo`'s own unique
/// temp area -- the CLI-level twin of `extract_claim_json`, above, kept local to this file rather
/// than imported from `extract_and_read/extract.rs`'s own (non-`pub(crate)`) `extract(...)`
/// helper, which a sibling module cannot reach.
fn extract_ref_json(repo: &Path, point: &str, target: &Path) -> Output {
    support::prikk(repo)
        .args([
            "extract",
            "--ref",
            point,
            "--format",
            "json",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

/// A single `CreateFile` patch naming `path` exactly as given, built by driving
/// `prikk_object::CanonicalWriter` directly -- bypassing `CreateFile`'s own validating
/// `encode_canonical`/`validate_repo_path`, the same bypass
/// `crates/prikk-store/src/extract/tests.rs::write_non_ascii_path_patch` uses one layer down, and
/// `crates/prikk-store/src/patch_replay/tests/proptest_round_trip.rs:832`'s own one-line technique
/// for an empty path -- ported here so this file's own CLI-level tests can drive both the
/// non-ASCII and the empty-path shape through the identical fixture code. No honest CLI `commit`
/// can author either path (3d2's own authoring-side refusal), so this is the only way to get one
/// into a chain this binary will read.
fn write_hand_crafted_create_file_patch(
    store: &mut FileObjectStore,
    author: &Ed25519AuthorSigner,
    path: &str,
    node_byte: u8,
) -> ObjectId {
    use prikk_object::{CanonicalWriter, WireType};

    // CreateFile tags (payload/patch/operations.rs): 1=path, 2=node_id, 3=blob_id, 4=mode.
    let mut create_file = CanonicalWriter::new();
    create_file.field_repo_path(1, path).unwrap();
    create_file
        .field_bytes(2, NodeId::from_bytes([node_byte; 32]).as_bytes())
        .unwrap();
    create_file
        .field_object_id(3, &ObjectId::from_bytes([node_byte.wrapping_add(1); 32]))
        .unwrap();
    create_file.field_u32(4, 0o100_644).unwrap();
    let create_file_bytes = create_file.finish();

    // Operation tags (payload/patch.rs): 1=op_seq, 10=CreateFile (nested Record).
    let mut operation = CanonicalWriter::new();
    operation.field_u32(1, 1).unwrap();
    operation
        .field_raw(10, WireType::Record, &create_file_bytes)
        .unwrap();
    let operation_bytes = operation.finish();

    // PatchPayload tag 1: one operation, framed as a `RecordListItem` (`repeated_record_list`'s
    // own wire shape).
    let mut payload = CanonicalWriter::new();
    payload
        .field_raw(1, WireType::RecordListItem, &operation_bytes)
        .unwrap();
    let canonical_payload = payload.finish();

    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, canonical_payload);
    let patch_id = patch.object_id();
    let patch_sig = author_signature(author, patch_id).unwrap();
    patch.add_signature(patch_sig).unwrap();
    store.write_object(&patch).unwrap()
}

/// Write a real, maintainer-signed `Block` carrying `patch_ids` over `parent_block_id`, then
/// publish it onto `ref_name` as the new CAS tip through `RefStore::publish` -- the identical
/// `Block`/`RefState`/`RefUpdate` triple `prikk seal` would publish for an honest generation, built
/// by hand here only because no honest `commit` can author the non-ASCII/empty-path patches this
/// file needs inside it (`branch_and_tag/dc61_branch_closure.rs`'s own
/// `reopening_a_closed_branch_is_an_ordinary_cas_update` test advances a ref by hand the same way).
/// `signer` must be a key this repository has already adopted (`admitted_repo`'s own genesis seal
/// adopts `support::MAINTAINER_KEY_ID`/`support::MAINTAINER_SEED`), or `RefStore::publish` itself
/// refuses before extract ever sees this block.
fn publish_block_with_patches(
    layout: &RepositoryLayout,
    ref_name: &str,
    parent_block_id: ObjectId,
    patch_ids: Vec<ObjectId>,
    signer: &Ed25519MaintainerSigner,
) -> ObjectId {
    let mut store = FileObjectStore::new(layout.clone());

    // `replay_for_extract` never reads a block's own `state_merkle_root` (it recomputes live
    // state from the patches themselves); `compute_state_root(&[])` is exactly what
    // `test_support.rs::signed_block`'s own default uses for the identical reason.
    let state_root = compute_state_root(&[]).unwrap();
    let block_payload = BlockPayload {
        parent_block_ids: vec![parent_block_id],
        kind: BlockKind::Normal,
        patch_ids,
        state_merkle_root: state_root,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block_envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, block_payload.to_canonical_bytes().unwrap());
    let block_id = block_envelope.object_id();
    let block_sig = maintainer_signature(signer, ObjectType::Block, block_id).unwrap();
    block_envelope.add_signature(block_sig).unwrap();
    store.write_object(&block_envelope).unwrap();

    let ref_store = RefStore::new(layout.clone());
    let current_ref_state_id = ref_store
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let current_envelope = store
        .read_typed(current_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let current_payload = RefStatePayload::decode_canonical(
        &current_envelope.canonical_payload,
        current_envelope.schema_version,
    )
    .unwrap();

    let new_ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: current_payload.kind,
        target_object_id: block_id,
        update_seq: current_payload.update_seq + 1,
        previous_ref_state_id: Some(current_ref_state_id),
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state_envelope = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        new_ref_state_payload.to_canonical_bytes().unwrap(),
    );
    let ref_state_id = ref_state_envelope.object_id();
    ref_state_envelope
        .add_signature(maintainer_signature(signer, ObjectType::RefState, ref_state_id).unwrap())
        .unwrap();

    let ref_update_payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: Some(current_ref_state_id),
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq: current_payload.update_seq + 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut ref_update_envelope = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes().unwrap(),
    );
    let ref_update_id = ref_update_envelope.object_id();
    ref_update_envelope
        .add_signature(maintainer_signature(signer, ObjectType::RefUpdate, ref_update_id).unwrap())
        .unwrap();

    let publication = RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: Some(current_ref_state_id),
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    ref_store.publish(&publication).unwrap();
    block_id
}

/// The trusted maintainer signer `admitted_repo`'s own genesis seal already adopts
/// (`support::MAINTAINER_KEY_ID`/`support::MAINTAINER_SEED`) -- reused here, rather than a fresh
/// never-adopted key, since the published ref itself (unlike a provisional claim chain) must carry
/// a signature this repository's own trust policy accepts before `extract --ref` ever reaches the
/// replay this test is actually proving.
fn admitted_maintainer_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
        .unwrap()
}

/// **Test 8** (3f2): `extract --ref` on a chain whose single non-genesis block carries the
/// hand-crafted non-ASCII patch gives `outside-version-rule` -- the bug the review found (3f2's own
/// motivating finding): before this round, `classify_prikk_error_fallback`'s only two arms (`Io`,
/// else `DamagedRepository`) put this exact cause under `damaged-repository`, contradicting the
/// `message` text that already correctly said this is not damage. Run twice: once against the ref
/// name, once against the bare block id (`extract --ref` accepts either, `is_point_name`'s own
/// dispatch, and both resolve to the identical `Point` before `prepare_extract_plan` ever runs, so
/// one assertion inside this test covers the bare-block-id path too without a second fixture).
#[test]
fn extract_ref_refuses_outside_version_rule_for_a_non_ascii_path_in_the_chain() {
    let (repo, layout, genesis_block) = admitted_repo("step3f2-ref-outside-version-rule");
    let mut store = FileObjectStore::new(layout.clone());
    let author = author_signer("step3f2-ref-outside-version-author", 0x91);
    let signer = admitted_maintainer_signer();

    let non_ascii_patch =
        write_hand_crafted_create_file_patch(&mut store, &author, "pages/日本語.md", 0x92);
    let tip_block = publish_block_with_patches(
        &layout,
        "heads/main",
        genesis_block,
        vec![non_ascii_patch],
        &signer,
    );

    let target = target_dir("step3f2-ref-outside-version-rule-out");
    let output = extract_ref_json(&repo, "heads/main", &target);
    assert_refusal_json(
        &output,
        "outside-version-rule",
        None,
        "outside what this version of prikk accepts",
    );

    // The identical cause, reached by naming the tip block directly rather than through the ref --
    // `classify_extract_plan_error`/`classify_prikk_error_fallback` run the same way regardless of
    // how the point was named.
    let target_by_block_id = target_dir("step3f2-ref-outside-version-rule-blockid-out");
    let output_by_block_id = extract_ref_json(&repo, &tip_block.to_string(), &target_by_block_id);
    assert_refusal_json(
        &output_by_block_id,
        "outside-version-rule",
        None,
        "outside what this version of prikk accepts",
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// **Test 9** (3f2): `extract --claim` on a one-link chain whose own (foot) patch is the
/// hand-crafted non-ASCII one also gives `outside-version-rule` -- the first test to assert the
/// actual printed `refusal-v1` id through the compiled binary for this cause (R1, `1fbba9fa`,
/// proved only the library-level `err.kind`/message text:
/// `prepare_extract_plan_for_claim_reads_the_outside_version_sentence_not_claim_does_not_fold_mid_chain`
/// in `crates/prikk-store/src/extract/tests.rs`).
#[test]
fn extract_claim_refuses_outside_version_rule_for_a_non_ascii_path() {
    let (repo, layout, genesis_block) = admitted_repo("step3f2-claim-outside-version-rule");
    let mut store = FileObjectStore::new(layout.clone());
    let author = author_signer("step3f2-claim-outside-version-author", 0x93);

    let non_ascii_patch =
        write_hand_crafted_create_file_patch(&mut store, &author, "pages/日本語.md", 0x94);
    let foot_claim = write_claim_signed_by(
        &mut store,
        RecognitionClaimPayload {
            block_id: ObjectId::from_bytes([0x95; 32]),
            patch_ids: vec![non_ascii_patch],
            parent_block_ids: vec![genesis_block],
        },
        &maintainer_signer("step3f2-claim-outside-version-maintainer", 0x96),
    );

    let target = target_dir("step3f2-claim-outside-version-rule-out");
    let output = extract_claim_json(&repo, &foot_claim.to_string(), &target);
    assert_refusal_json(
        &output,
        "outside-version-rule",
        None,
        "outside what this version of prikk accepts",
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// **Test 10** (3f2): an empty path through the identical `--ref` chain still gives
/// `damaged-repository`, never `outside-version-rule` -- proof that 3d2's own narrowing (only the
/// non-ASCII cause is version-relative; an empty path is permanently invalid, no future `prikk`
/// will ever accept it) stays narrow now that `outside-version-rule` exists as a real id a careless
/// classification could otherwise have swallowed this case into.
#[test]
fn extract_ref_still_refuses_an_empty_path_as_damaged_repository() {
    let (repo, layout, genesis_block) = admitted_repo("step3f2-ref-empty-path-damaged");
    let mut store = FileObjectStore::new(layout.clone());
    let author = author_signer("step3f2-ref-empty-path-author", 0x97);
    let signer = admitted_maintainer_signer();

    let empty_path_patch = write_hand_crafted_create_file_patch(&mut store, &author, "", 0x98);
    publish_block_with_patches(
        &layout,
        "heads/main",
        genesis_block,
        vec![empty_path_patch],
        &signer,
    );

    let target = target_dir("step3f2-ref-empty-path-damaged-out");
    let output = extract_ref_json(&repo, "heads/main", &target);
    assert_refusal_json(&output, "damaged-repository", None, "invalid name");

    let _ = std::fs::remove_dir_all(&repo);
}
