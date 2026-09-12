//! CLI end-to-end regression for `prikk branch` (DC-60 Branch Management Surface).
//!
//! Drives the compiled `prikk` binary, matching the style of `genesis_end_to_end.rs` and
//! `seal_recovery.rs`. `branch create --from` must reproduce DC-13's exact ref-state shape
//! (`update_seq = 1`, `previous_ref_state_id = None`, maintainer-signed, `RefKind::Branch`) for a
//! name with no surviving log, so state is inspected directly through `RefStore`/`FileObjectStore`
//! rather than trusted from stdout alone.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

use prikk_object::{ObjectType, RefKind, RefStatePayload};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader, RefStore,
    RepositoryLayout,
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
    dir.push(format!("prikk-cli-dc60-{tag}-{}", support::unique_suffix()));
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

fn branch_list(repo: &Path) -> Output {
    prikk(repo).args(["branch"]).output().unwrap()
}

/// `init`, genesis-commit `readme.txt` on `heads/main`, and seal it. Returns the layout.
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

#[test]
fn branch_list_reports_no_branches_before_genesis() {
    let repo = unique_repo("list-empty");
    ok(&prikk(&repo).arg("init").output().unwrap(), "init");

    let out = branch_list(&repo);
    ok(&out, "branch list on empty repository");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "no branches");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_list_is_sorted_and_reports_ref_state_ids() {
    let (repo, layout) = seeded_repo("list-sorted");

    let out = branch_create(&repo, &["heads/zzz-topic", "--from", "heads/main"]);
    ok(&out, "branch create heads/zzz-topic");
    let out = branch_create(&repo, &["heads/aaa-topic", "--from", "heads/main"]);
    ok(&out, "branch create heads/aaa-topic");

    let out = branch_list(&repo);
    ok(&out, "branch list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected three branches; stdout: {stdout}");
    assert!(lines[0].starts_with("heads/aaa-topic "), "stdout: {stdout}");
    assert!(lines[1].starts_with("heads/main "), "stdout: {stdout}");
    assert!(lines[2].starts_with("heads/zzz-topic "), "stdout: {stdout}");

    let ref_store = RefStore::new(layout.clone());
    let main_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    assert!(
        lines[1].ends_with(&main_id.to_string()),
        "expected heads/main RefState id {main_id}; stdout: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_create_fails_closed_on_invalid_name() {
    let (repo, _layout) = seeded_repo("create-invalid-name");
    let out = branch_create(&repo, &["not-a-branch-ref"]);
    fail(&out, "branch create with invalid name");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("heads/"), "unexpected stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_create_fails_closed_on_existing_name() {
    let (repo, _layout) = seeded_repo("create-existing-name");
    let out = branch_create(&repo, &["heads/main"]);
    fail(&out, "branch create on an existing name");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_create_fails_closed_on_untrusted_signer() {
    let (repo, _layout) = seeded_repo("create-untrusted-signer");
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file("222233334444555566667777888899990000aaaabbbbccccddddeeeeffff1111"),
        )
        .args(["branch", "create", "heads/topic", "--from", "heads/main"])
        .output()
        .unwrap();
    fail(&out, "branch create with an untrusted maintainer signer");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_create_fails_closed_on_unresolvable_from() {
    let (repo, _layout) = seeded_repo("create-bad-from");
    let out = branch_create(&repo, &["heads/topic", "--from", "heads/does-not-exist"]);
    fail(&out, "branch create with unresolvable --from");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not resolve to a published ref"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_create_at_existing_target_matches_dc13_genesis_shape() {
    let (repo, layout) = seeded_repo("create-shape");

    let out = branch_create(&repo, &["heads/topic", "--from", "heads/main"]);
    ok(&out, "branch create heads/topic --from heads/main");

    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());

    let main_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let main_envelope = object_store
        .read_typed(main_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let main_payload = RefStatePayload::decode_canonical(
        &main_envelope.canonical_payload,
        main_envelope.schema_version,
    )
    .unwrap();

    let topic_ref_state_id = ref_store
        .read_current_ref_state_id("heads/topic")
        .unwrap()
        .unwrap();
    let topic_envelope = object_store
        .read_typed(topic_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    assert!(
        !topic_envelope.signatures.is_empty(),
        "RefState must be signed"
    );
    let topic_payload = RefStatePayload::decode_canonical(
        &topic_envelope.canonical_payload,
        topic_envelope.schema_version,
    )
    .unwrap();

    // DC-13 genesis shape: update_seq = 1, previous_ref_state_id = None, kind = Branch.
    assert_eq!(topic_payload.update_seq, 1);
    assert_eq!(topic_payload.previous_ref_state_id, None);
    assert_eq!(topic_payload.kind, RefKind::Branch);
    assert_eq!(topic_payload.ref_name, "heads/topic");
    // Same target block as heads/main --from, not a new block.
    assert_eq!(
        topic_payload.target_object_id,
        main_payload.target_object_id
    );

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify after branch create");

    let _ = std::fs::remove_dir_all(&repo);
}

/// AC3's fourth fail-closed condition: a name with a surviving ref log but no live pointer. This
/// state arises from an interrupted publication (there is no `branch delete` in this scope to
/// produce it deliberately), so it is constructed the same way
/// `seal_rejects_missing_pointer_with_ref_log_history` (`genesis_end_to_end.rs`) constructs it:
/// publish normally, then remove only the pointer file to simulate what a crash between "pointer
/// promoted" and "before the next read" would leave behind. `branch create` must refuse to publish
/// over it rather than let `publish` fail with a generic classification error, and must not offer
/// any way to continue it — that recovery is DC-61's, pending its ref-log tombstone design.
#[test]
fn branch_create_fails_closed_on_surviving_log_with_no_live_pointer() {
    let (repo, layout) = seeded_repo("create-surviving-log-no-pointer");
    ok(
        &branch_create(&repo, &["heads/topic", "--from", "heads/main"]),
        "branch create heads/topic",
    );

    let ref_store = RefStore::new(layout.clone());
    assert_eq!(
        ref_store.replay_log("heads/topic").unwrap().records.len(),
        1,
        "sanity: heads/topic should have exactly one ref-log record"
    );
    prikk_store::remove_ref_pointer_entry_for_test_support(&layout, "heads/topic").unwrap();
    assert!(
        ref_store
            .read_current_ref_state_id("heads/topic")
            .unwrap()
            .is_none(),
        "sanity: pointer must be gone before the fail-closed assertion"
    );

    let out = branch_create(&repo, &["heads/topic", "--from", "heads/main"]);
    fail(
        &out,
        "branch create must reject a name with a surviving log and no live pointer",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("surviving ref log") && stderr.contains("DC-61"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

// DC-61 supersedes this test's original name and its "no `branch delete`" assertion: deletion
// became closure (`branch close`), which does exist now. Renamed and updated rather than left
// asserting a claim DC-61 made false.
#[test]
fn branch_help_states_no_switch_support() {
    let out = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .arg("--help")
        .output()
        .unwrap();
    ok(&out, "--help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("branch"),
        "help must mention branch: {stdout}"
    );
    let lower = stdout.to_lowercase();
    assert!(
        lower.contains("no `branch switch`"),
        "help must state switching is unsupported: {stdout}"
    );
    assert!(
        lower.contains("branch close"),
        "help must mention branch close (DC-61): {stdout}"
    );
}
