//! CLI end-to-end regression for `prikk branch close` (DC-61 Branch Closure).
//!
//! Drives the compiled `prikk` binary, matching the style of `dc60_branch_management.rs` and
//! `dc63_tag_surface.rs`. Closure is not deletion: the pointer, its history, and every object stay
//! present. DC-60 tried deletion (pointer removed, log kept) and it bricked repository-wide commits
//! because the system treats "pointer absent, log present" as corruption with a repair function
//! (see `.git-exclude/reviewed/prikk-dc60-delete-divergence-ruling-v1.md`). These tests exist to
//! prove closure does not reproduce that failure and that the falsification-test functions
//! (`verify`, `publish`, `recoverable_missing_ref`, `doctor`) still classify every input exactly as
//! they did before DC-61.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, REF_STATE_CLOSED_SCHEMA, RefKind,
    RefStatePayload, RefUpdatePayload,
};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader, RefPublication,
    RefStore, RepositoryLayout, maintainer_signature, remove_active_ref_metadata,
};

fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    // RFC 148: prikk now reads key material from the user's own config directory, so a test that
    // does not neutralise it measures whoever is running it. See `support::isolate_key_environment`.
    support::isolate_key_environment(&mut cmd);
    cmd
}

fn ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn fail(output: &Output, what: &str) {
    assert!(
        !output.status.success(),
        "{what} unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-dc61-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn public_key_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn maintainer_seed() -> &'static str {
    "111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000"
}

fn maintainer_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed(
        "e2e-maintainer",
        &[
            0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x66, 0x66, 0x77, 0x77,
            0x88, 0x88, 0x99, 0x99, 0xaa, 0xaa, 0xbb, 0xbb, 0xcc, 0xcc, 0xdd, 0xdd, 0xee, 0xee,
            0xff, 0xff, 0x00, 0x00,
        ],
    )
    .unwrap()
}

fn add_trusted_maintainer(repo: &Path) {
    let signer = maintainer_signer();
    let out = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "e2e-maintainer",
            "--public-key",
            &public_key_hex(&signer.public_key_bytes()),
        ])
        .output()
        .unwrap();
    ok(&out, "trust maintainer add");
}

fn commit(repo: &Path, ref_name: &str, message: &str) -> Output {
    prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", ref_name, "-m", message])
        .output()
        .unwrap()
}

fn seal(repo: &Path, ref_name: &str) -> Output {
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit", "--ref", ref_name])
        .output()
        .unwrap()
}

fn branch_create(repo: &Path, args: &[&str]) -> Output {
    let mut full = vec!["branch", "create"];
    full.extend_from_slice(args);
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(full)
        .output()
        .unwrap()
}

fn branch_close(repo: &Path, name: &str) -> Output {
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["branch", "close", name])
        .output()
        .unwrap()
}

fn branch_list(repo: &Path, args: &[&str]) -> Output {
    let mut full = vec!["branch"];
    full.extend_from_slice(args);
    prikk(repo).args(full).output().unwrap()
}

/// `init`, genesis-commit `readme.txt` on `heads/main`, and seal it.
fn seeded_repo(tag: &str) -> (PathBuf, RepositoryLayout) {
    let repo = unique_repo(tag);
    ok(&prikk(&repo).arg("init").output().unwrap(), "init");
    std::fs::write(repo.join("readme.txt"), b"hello prikk\n").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "commit heads/main");
    add_trusted_maintainer(&repo);
    ok(&seal(&repo, "heads/main"), "seal heads/main");
    let layout = RepositoryLayout::open(&repo).unwrap();
    (repo, layout)
}

fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &Ed25519MaintainerSigner,
) -> ObjectEnvelope {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope
        .add_signature(maintainer_signature(signer, object_type, object_id).unwrap())
        .unwrap();
    envelope
}

fn decode_ref_state(object_store: &FileObjectStore, ref_state_id: ObjectId) -> RefStatePayload {
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version).unwrap()
}

/// Criterion 1: `branch close` publishes a maintainer-signed schema-2 ref state carrying `closed`;
/// the pointer stays present; `verify` passes.
#[test]
fn branch_close_publishes_and_verify_passes() {
    let (repo, layout) = seeded_repo("close-and-verify");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );
    let out = branch_close(&repo, "heads/topic");
    ok(&out, "branch close heads/topic");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("nothing was reclaimed") && stdout.contains("recoverable"),
        "stdout: {stdout}"
    );

    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout);
    let ref_state_id = ref_store
        .read_current_ref_state_id("heads/topic")
        .unwrap()
        .unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    assert_eq!(envelope.schema_version, REF_STATE_CLOSED_SCHEMA);
    let payload = decode_ref_state(&object_store, ref_state_id);
    assert!(payload.closed);
    assert_eq!(payload.update_seq, 2);

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify after branch close");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Criterion 2, the DC-60 regression: a commit to an unrelated ref succeeds after a closure.
#[test]
fn branch_close_does_not_block_commits_to_unrelated_refs() {
    let (repo, _layout) = seeded_repo("close-unrelated-ref-unaffected");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );
    ok(
        &branch_close(&repo, "heads/topic"),
        "branch close heads/topic",
    );

    std::fs::write(repo.join("readme.txt"), b"unrelated change\n").unwrap();
    let out = commit(&repo, "heads/main", "unrelated change after closure");
    ok(&out, "commit onto heads/main after closing heads/topic");
    let out = seal(&repo, "heads/main");
    ok(&out, "seal heads/main after closing heads/topic");

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify after unrelated commit");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Criterion 3: pointer-absent-log-present is still reported and blocking after DC-61, at both a
/// single-record and a multi-record log, exactly as before closure existed — closure must not have
/// weakened `verify`'s corruption classification. Constructed the same way
/// `seal_rejects_missing_pointer_with_ref_log_history` (`genesis_end_to_end.rs`) constructs it:
/// publish normally, then remove only the pointer file to simulate what an interrupted publication
/// leaves behind. This is deliberate corruption simulation, unrelated to `branch close`'s own
/// pointer-retaining behaviour — DC-61 gives no CLI path to reach this state.
#[test]
fn corruption_detection_for_pointer_loss_is_unchanged_by_dc61() {
    for record_count in [1_usize, 2] {
        let (repo, layout) = seeded_repo(&format!("corruption-unchanged-{record_count}"));
        ok(
            &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
            "branch create heads/topic",
        );
        if record_count == 2 {
            std::fs::write(repo.join("readme.txt"), b"topic update\n").unwrap();
            ok(
                &commit(&repo, "heads/topic", "topic update"),
                "commit onto heads/topic",
            );
            ok(&seal(&repo, "heads/topic"), "seal heads/topic");
        }

        let ref_store = RefStore::new(layout.clone());
        assert_eq!(
            ref_store.replay_log("heads/topic").unwrap().records.len(),
            record_count
        );
        prikk_store::remove_ref_pointer_entry_for_test_support(&layout, "heads/topic").unwrap();

        let out = prikk(&repo).arg("verify").output().unwrap();
        fail(
            &out,
            &format!("verify at record_count={record_count} with pointer lost"),
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.is_empty(),
            "verify must report something for pointer loss at record_count={record_count}"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn branch_close_fails_closed_on_missing_branch() {
    let (repo, _layout) = seeded_repo("close-missing");
    let out = branch_close(&repo, "heads/does-not-exist");
    fail(&out, "branch close on a missing branch");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not exist"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_close_fails_closed_on_untrusted_signer() {
    let (repo, _layout) = seeded_repo("close-untrusted-signer");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file("222233334444555566667777888899990000aaaabbbbccccddddeeeeffff1111"),
        )
        .args(["branch", "close", "heads/topic"])
        .output()
        .unwrap();
    fail(&out, "branch close with an untrusted maintainer signer");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_close_fails_closed_on_already_closed_branch() {
    let (repo, _layout) = seeded_repo("close-already-closed");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );
    ok(
        &branch_close(&repo, "heads/topic"),
        "branch close heads/topic",
    );
    let out = branch_close(&repo, "heads/topic");
    fail(&out, "branch close on an already-closed branch");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already closed"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_close_fails_closed_on_non_empty_active_wal() {
    let (repo, _layout) = seeded_repo("close-active-wal-owner");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );
    std::fs::write(repo.join("readme.txt"), b"unsealed change\n").unwrap();
    ok(
        &commit(&repo, "heads/main", "unsealed change"),
        "commit onto heads/main, unsealed",
    );

    let out = branch_close(&repo, "heads/main");
    fail(&out, "branch close while its active WAL is non-empty");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("non-empty active WAL"),
        "unexpected stderr: {stderr}"
    );

    // The branch that does not own the active WAL is unaffected by this rule.
    let out = branch_close(&repo, "heads/topic");
    ok(
        &out,
        "branch close of the non-owning branch should still succeed",
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// N1 (`prikk-dc61-implementation-review-v1.md`): a non-empty active WAL whose ownership metadata is
/// missing must refuse to close, not proceed. The pre-repair code tested `.is_ok()` on
/// `require_active_ref_for_non_empty_wal`, which collapsed "owned by a different ref" (proceed,
/// correct) and "ownership unknown due to an integrity error" (proceed, wrong) into the same
/// `Err` branch. `node_authoring.rs`'s commit path propagates this same error via `?`; `branch close`
/// must fail closed identically rather than treat unknown ownership as evidence it is uninvolved.
#[test]
fn branch_close_fails_closed_on_missing_active_ref_metadata() {
    let (repo, layout) = seeded_repo("close-missing-active-ref-metadata");
    std::fs::write(repo.join("readme.txt"), b"unsealed change\n").unwrap();
    ok(
        &commit(&repo, "heads/main", "unsealed change"),
        "commit onto heads/main, unsealed",
    );

    // Simulate an integrity condition: a non-empty active WAL whose ownership metadata is absent.
    let removed = remove_active_ref_metadata(&layout);
    assert!(removed.is_ok());

    let out = branch_close(&repo, "heads/main");
    fail(
        &out,
        "branch close must not proceed when active-ref ownership is unknown",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("active ref metadata is missing"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// No "last remaining branch" guard: closure is recoverable, so closing a repository's only branch
/// must succeed — the opposite of DC-60's deletion guard, and deliberately not reproduced here.
#[test]
fn branch_close_has_no_last_remaining_branch_guard() {
    let (repo, _layout) = seeded_repo("close-last-remaining-branch-allowed");
    let out = branch_close(&repo, "heads/main");
    ok(&out, "closing the last remaining branch must be permitted");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Criterion 5: `branch list` hides closed refs by default; `--all` shows them, marked.
#[test]
fn branch_list_hides_closed_by_default_and_shows_with_all() {
    let (repo, _layout) = seeded_repo("list-hides-closed");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );
    ok(
        &branch_close(&repo, "heads/topic"),
        "branch close heads/topic",
    );

    let out = branch_list(&repo, &[]);
    ok(&out, "branch list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|line| line.starts_with("heads/main ")),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("heads/topic"),
        "closed branch must be hidden by default; stdout: {stdout}"
    );

    let out = branch_list(&repo, &["--all"]);
    ok(&out, "branch list --all");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|line| line.starts_with("heads/main ")),
        "stdout: {stdout}"
    );
    let topic_line = stdout
        .lines()
        .find(|line| line.starts_with("heads/topic "))
        .unwrap_or_else(|| panic!("expected heads/topic under --all; stdout: {stdout}"));
    assert!(
        topic_line.contains("closed"),
        "closed branch must be marked under --all: {topic_line}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Criterion 6: reopening a closed branch is an ordinary CAS update (no `closed` field) and
/// succeeds; `verify` passes afterward. No `branch reopen` CLI verb exists in this increment (the
/// handoff's command surface is `close` and `list --all` only), so this is constructed directly
/// against `RefStore::publish`, exactly like `branch close` would if such a verb existed.
#[test]
fn reopening_a_closed_branch_is_an_ordinary_cas_update() {
    let (repo, layout) = seeded_repo("reopen-ordinary-cas-update");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );
    ok(
        &branch_close(&repo, "heads/topic"),
        "branch close heads/topic",
    );

    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());
    let closed_ref_state_id = ref_store
        .read_current_ref_state_id("heads/topic")
        .unwrap()
        .unwrap();
    let closed_payload = decode_ref_state(&object_store, closed_ref_state_id);
    assert!(
        closed_payload.closed,
        "sanity: branch must be closed before reopening"
    );

    let signer = maintainer_signer();
    let reopened_payload = RefStatePayload {
        ref_name: "heads/topic".to_string(),
        kind: RefKind::Branch,
        target_object_id: closed_payload.target_object_id,
        update_seq: closed_payload.update_seq + 1,
        previous_ref_state_id: Some(closed_ref_state_id),
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        1,
        reopened_payload.to_canonical_bytes().unwrap(),
        &signer,
    );
    let ref_state_id = ref_state_envelope.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: "heads/topic".to_string(),
        old_ref_state_id: Some(closed_ref_state_id),
        new_ref_state_id: ref_state_id,
        new_target_object_id: closed_payload.target_object_id,
        update_seq: closed_payload.update_seq + 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes().unwrap(),
        &signer,
    );
    let publication = RefPublication {
        ref_name: "heads/topic".to_string(),
        expected_previous_ref_state_id: Some(closed_ref_state_id),
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    let published = ref_store.publish(&publication);
    assert!(
        published.is_ok(),
        "reopening must succeed as an ordinary CAS update: {published:?}"
    );

    let reopened = decode_ref_state(&object_store, published.unwrap());
    assert!(
        !reopened.closed,
        "reopened branch must not carry the closed field"
    );

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify after reopening");

    let out = branch_list(&repo, &[]);
    ok(&out, "branch list after reopening");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|line| line.starts_with("heads/topic ")),
        "reopened branch must be visible under plain `branch list` again; stdout: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
