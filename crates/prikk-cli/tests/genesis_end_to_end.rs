//! CLI end-to-end regression for the genesis first-commit flow (DC-09 4.4b P2-1).
//!
//! Drives the exact release-facing path through the compiled `prikk` binary:
//! `init → commit (genesis) → seal → log → verify`. This complements the store-level genesis tests
//! by guarding the CLI wiring that the release documents.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

use prikk_store::{Ed25519MaintainerSigner, MaintainerSigner, RefStore, RepositoryLayout};

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
    dir.push(format!("prikk-cli-e2e-{tag}-{}", support::unique_suffix()));
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

#[test]
fn genesis_init_commit_seal_log_verify() {
    let repo = unique_repo("genesis");

    // init a fresh repository in the working directory.
    let out = prikk(&repo).arg("init").output().unwrap();
    ok(&out, "init");

    // two regular worktree files to author as the genesis commit.
    std::fs::write(repo.join("readme.txt"), b"hello prikk\n").unwrap();
    std::fs::write(repo.join("main.rs"), b"fn main() {}\n").unwrap();

    // genesis commit: node-addressed, role-bound Ed25519 AUTHOR-signed (key material via env).
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "-m", "genesis"])
        .output()
        .unwrap();
    ok(&out, "commit");
    let commit_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        commit_stdout.contains("operations: 2"),
        "expected two CreateFile operations; stdout: {commit_stdout}"
    );

    add_trusted_maintainer(&repo);

    // seal publishes the first (Root) block and advances heads/main.
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal");

    // log shows a Root block at update-seq 1.
    let out = prikk(&repo).arg("log").output().unwrap();
    ok(&out, "log");
    let log_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        log_stdout.contains("Root"),
        "expected a Root block; log stdout: {log_stdout}"
    );
    assert!(
        log_stdout.contains("update-seq: 1"),
        "expected update-seq 1; log stdout: {log_stdout}"
    );

    // verify reports a clean repository.
    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn non_default_ref_genesis_commit_seal_log_verify() {
    let repo = unique_repo("nondefault-genesis");
    let out = prikk(&repo).arg("init").output().unwrap();
    ok(&out, "init");

    std::fs::write(repo.join("topic.txt"), b"topic root\n").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", "heads/topic", "-m", "topic genesis"])
        .output()
        .unwrap();
    ok(&out, "commit --ref");
    let commit_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        commit_stdout.contains("baseline ref: heads/topic"),
        "expected heads/topic baseline; stdout: {commit_stdout}"
    );

    add_trusted_maintainer(&repo);
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit", "--ref", "heads/topic"])
        .output()
        .unwrap();
    ok(&out, "seal --ref");
    let seal_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        seal_stdout.contains("heads/topic RefState:"),
        "expected heads/topic publication; stdout: {seal_stdout}"
    );
    // RFC 102 Stage 5, design-v1.md §14.6: the file is permanent from `init` onward; "removed" is now
    // "truncated to empty."
    assert!(
        std::fs::read(repo.join(".prikk/active/default/ref-name"))
            .unwrap()
            .is_empty(),
        "seal should clear active ref metadata"
    );

    let out = prikk(&repo)
        .args(["log", "--ref", "heads/topic"])
        .output()
        .unwrap();
    ok(&out, "log --ref");
    let log_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(log_stdout.contains("Root"), "log stdout: {log_stdout}");
    assert!(
        log_stdout.contains("update-seq: 1"),
        "log stdout: {log_stdout}"
    );

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn seal_rejects_active_wal_owned_by_another_ref() {
    let repo = unique_repo("seal-ref-mismatch");
    let out = prikk(&repo).arg("init").output().unwrap();
    ok(&out, "init");

    std::fs::write(repo.join("topic.txt"), b"topic root\n").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", "heads/topic", "-m", "topic genesis"])
        .output()
        .unwrap();
    ok(&out, "commit --ref");

    add_trusted_maintainer(&repo);
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    fail(&out, "seal wrong ref");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("active WAL is owned by heads/topic"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn seal_rejects_missing_pointer_with_ref_log_history() {
    let repo = unique_repo("seal-missing-pointer-with-log");
    let out = prikk(&repo).arg("init").output().unwrap();
    ok(&out, "init");

    std::fs::write(repo.join("topic.txt"), b"topic root\n").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", "heads/topic", "-m", "topic genesis"])
        .output()
        .unwrap();
    ok(&out, "commit --ref");

    add_trusted_maintainer(&repo);
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit", "--ref", "heads/topic"])
        .output()
        .unwrap();
    ok(&out, "seal --ref");

    std::fs::write(repo.join("topic.txt"), b"topic update\n").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", "heads/topic", "-m", "topic update"])
        .output()
        .unwrap();
    ok(&out, "second commit --ref");

    let layout = RepositoryLayout::open(&repo).unwrap();
    prikk_store::remove_ref_pointer_entry_for_test_support(&layout, "heads/topic").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit", "--ref", "heads/topic"])
        .output()
        .unwrap();
    fail(&out, "seal missing pointer with log");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pointer/log state does not match the expected publication transition"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn seal_rejects_missing_pointer_with_partial_ref_log() {
    let repo = unique_repo("seal-missing-pointer-with-partial-log");
    let out = prikk(&repo).arg("init").output().unwrap();
    ok(&out, "init");

    std::fs::write(repo.join("topic.txt"), b"topic root\n").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", "heads/topic", "-m", "topic genesis"])
        .output()
        .unwrap();
    ok(&out, "commit --ref");

    add_trusted_maintainer(&repo);
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit", "--ref", "heads/topic"])
        .output()
        .unwrap();
    ok(&out, "seal --ref");

    std::fs::write(repo.join("topic.txt"), b"topic update\n").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", "heads/topic", "-m", "topic update"])
        .output()
        .unwrap();
    ok(&out, "second commit --ref");

    let layout = RepositoryLayout::open(&repo).unwrap();
    prikk_store::remove_ref_pointer_entry_for_test_support(&layout, "heads/topic").unwrap();
    support::append_torn_ref_log_tail(
        &layout.ref_log_container_slot_path(prikk_store::ContainerSlot::A),
    );

    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit", "--ref", "heads/topic"])
        .output()
        .unwrap();
    fail(&out, "seal missing pointer with partial log");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pointer is missing and its log has trailing partial bytes"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn seal_retry_drains_already_published_wal_without_duplicate_ref_update() {
    let repo = unique_repo("seal-retry-drains");
    let out = prikk(&repo).arg("init").output().unwrap();
    ok(&out, "init");

    std::fs::write(repo.join("readme.txt"), b"hello prikk\n").unwrap();
    let out = prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "-m", "genesis"])
        .output()
        .unwrap();
    ok(&out, "commit");

    let layout = RepositoryLayout::open(&repo).unwrap();
    let wal_bytes = std::fs::read(layout.default_queue_wal_path()).unwrap();
    let ref_metadata_bytes = std::fs::read(layout.default_active_ref_name_path()).unwrap();
    add_trusted_maintainer(&repo);
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal");
    let ref_store = RefStore::new(layout.clone());
    let before_retry_log = ref_store.replay_log("heads/main").unwrap();
    assert_eq!(before_retry_log.records.len(), 1);

    std::fs::write(layout.default_queue_wal_path(), wal_bytes).unwrap();
    std::fs::write(layout.default_active_ref_name_path(), ref_metadata_bytes).unwrap();
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal retry");
    let after_retry_log = ref_store.replay_log("heads/main").unwrap();
    assert_eq!(after_retry_log.records.len(), 1);
    assert_eq!(std::fs::read(layout.default_queue_wal_path()).unwrap(), b"");
    // RFC 102 Stage 5, design-v1.md §14.6: the file is permanent from `init` onward; "removed" is now
    // "truncated to empty."
    assert!(
        std::fs::read(layout.default_active_ref_name_path())
            .unwrap()
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(&repo);
}
