//! CLI end-to-end regression for DC-65: editing the same text file across multiple separate sealed
//! commits, driving the compiled `prikk` binary exactly as a real user would. This is the scenario
//! the defect was originally found in — `crates/prikk-cli/tests/dc59_commit_benchmark.rs`'s Axis C
//! development hit it first, and it was confirmed independently with three bare manual CLI commands
//! before any store-level test existed. See
//! `rfcs/handoffs/DC-65-text-edit-baseline-content/prerequisite-questions-v1.md` for the root cause
//! and chosen invariant.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

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

fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-dc65-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const AUTHOR_KEY_ID: &str = "dc65-test-author";
const AUTHOR_SEED_HEX: &str = "0011223344556677889900112233445566778899001122334455667788990011";
const MAINTAINER_KEY_ID: &str = "dc65-test-maintainer";
const MAINTAINER_SEED: [u8; 32] = [
    0x41, 0x41, 0x52, 0x52, 0x63, 0x63, 0x74, 0x74, 0x85, 0x85, 0x96, 0x96, 0xa7, 0xa7, 0xb8, 0xb8,
    0xc9, 0xc9, 0xda, 0xda, 0xeb, 0xeb, 0xfc, 0xfc, 0x0d, 0x0d, 0x1e, 0x1e, 0x2f, 0x2f, 0x30, 0x30,
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn maintainer_public_key_hex() -> String {
    use prikk_store::MaintainerSigner;
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(MAINTAINER_KEY_ID, &MAINTAINER_SEED)
            .expect("fixed maintainer seed derives a valid signer");
    hex(&signer.public_key_bytes())
}

fn init(repo: &Path) {
    ok(&prikk(repo).arg("init").output().unwrap(), "init");
}

fn commit(repo: &Path, message: &str) -> Output {
    prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", message])
        .output()
        .unwrap()
}

fn seal(repo: &Path) {
    let out = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            MAINTAINER_KEY_ID,
            "--public-key",
            &maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
    let _ = out; // idempotent-enough for repeated calls within one test; only `seal` must succeed.

    let out = prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal");
}

/// The exact scenario from the RFC: `init; write; commit; seal` repeated, editing the same file each
/// time. Two was the boundary that was missed; this runs five real sealed edits, comfortably past
/// the N >= 3 minimum, and also exercises DC-64's incremental baseline cache (the third and later
/// commits are eligible for it) so the fix is proven under both full-replay and incremental baseline
/// resolution, not just the first.
#[test]
fn editing_the_same_text_file_across_five_sealed_commits_succeeds() {
    let repo = unique_repo("five-sealed-edits");
    init(&repo);
    std::fs::write(repo.join("f.txt"), "hello world").unwrap();
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);

    for generation in 1..=5 {
        std::fs::write(repo.join("f.txt"), format!("hello world, edit{generation}")).unwrap();
        let out = commit(&repo, &format!("edit {generation}"));
        ok(&out, &format!("edit {generation} commit"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("edit-text f.txt"),
            "expected an edit-text summary line for generation {generation}, got: {stdout}"
        );
        seal(&repo);
    }

    assert_eq!(
        std::fs::read_to_string(repo.join("f.txt")).unwrap(),
        "hello world, edit5"
    );

    // Checkout/materialization must independently reconstruct the same final content from the
    // sealed diff chain — proving the fix is consistent with the unaffected replay side (DC-65 Q1),
    // not merely making the commit command stop erroring.
    let materialize_root = unique_repo("five-sealed-edits-materialize");
    std::fs::create_dir_all(materialize_root.join(".prikk")).unwrap();
    copy_dir_recursive(&repo.join(".prikk"), &materialize_root.join(".prikk"));
    let out = prikk(&materialize_root)
        .arg("checkout")
        .arg("--patch-materialize")
        .output()
        .unwrap();
    ok(&out, "checkout --patch-materialize");
    assert_eq!(
        std::fs::read_to_string(materialize_root.join("f.txt")).unwrap(),
        "hello world, edit5"
    );

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lifecycle-cache divergences: 0"));
    assert!(stdout.contains("commit-index divergences: 0"));

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path);
        } else {
            std::fs::copy(entry.path(), &dst_path).unwrap();
        }
    }
}
