//! 0.56.0 step 1 part 1a: `sync pending --format json` (`sync-pending-v1`), driven end to end
//! through the CLI across two repositories -- the same real `sync build`/`sync accept` flow
//! `rfc116_sync_cli.rs` already exercises, used here to prove the new flag's own JSON shape and
//! that the existing text form is unaffected.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};

fn sync_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rfc146-sync-pending-json-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

/// Build repo A (one sealed patch) and repo B (empty), and hand B an accepted-but-unsealed patch
/// through the ordinary `sync summary`/`have`/`build`/`accept` loop. Returns the claim id `sync
/// accept` printed.
fn accept_one_pending_patch(repo_a: &Path, repo_b: &Path) -> String {
    support::init(repo_a);
    support::generation(
        repo_a,
        "heads/main",
        "a.txt",
        b"sync pending json fixture\n",
        "first",
    );
    support::init(repo_b);

    let summary_file = sync_file("summary");
    support::ok(
        &support::prikk(repo_a)
            .args([
                "sync",
                "summary",
                "--output",
                summary_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync summary",
    );

    let have_file = sync_file("have");
    support::ok(
        &support::prikk(repo_b)
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

    let artifact_file = sync_file("artifact");
    let build = support::prikk(repo_a)
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

    // Trust the maintainer key in B *before* accepting, so the claim's signature outcome reads
    // `sound` here -- `pending_patches_with_claims` re-verifies against B's current trust policy,
    // not whatever held at accept time (RFC 115 Stage 3's own accept path verifies once too, but
    // this query's answer can differ later if the policy changes; see this round's report).
    support::trust_maintainer(repo_b);

    let accept = support::prikk(repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let accept_stdout = String::from_utf8_lossy(&accept.stdout).into_owned();
    let claim_line = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .unwrap_or_else(|| panic!("sync accept must print a claim line: {accept_stdout}"));
    claim_line
        .trim_start()
        .strip_prefix("claim ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("could not parse a claim id from: {claim_line}"))
        .trim()
        .to_string()
}

#[test]
fn sync_pending_format_json_reports_the_claim_naming_the_pending_patch() {
    let repo_a = support::unique_repo("sync-pending-json-a");
    let repo_b = support::unique_repo("sync-pending-json-b");
    let claim_id = accept_one_pending_patch(&repo_a, &repo_b);

    // The existing text form (a count and one id per line) must be unaffected by this round.
    let pending_text = support::prikk(&repo_b)
        .args(["sync", "pending"])
        .output()
        .unwrap();
    support::ok(&pending_text, "sync pending");
    let pending_text_stdout = String::from_utf8_lossy(&pending_text.stdout);
    assert!(
        pending_text_stdout.contains("pending (accepted, unsealed) patches: 1"),
        "text form must be unchanged by this round: {pending_text_stdout}"
    );

    let pending_json = support::prikk(&repo_b)
        .args(["sync", "pending", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&pending_json, "sync pending --format json");
    let stdout = String::from_utf8_lossy(&pending_json.stdout).into_owned();
    let value = support::json::parse(&stdout);
    assert_eq!(
        value.get("schema_version").as_str(),
        "sync-pending-v1",
        "stdout: {stdout}"
    );
    let patches = value.get("patches").as_array();
    assert_eq!(patches.len(), 1, "exactly one pending patch: {stdout}");
    let claims = patches[0].get("claims").as_array();
    assert_eq!(claims.len(), 1, "exactly one claim names it: {stdout}");
    assert_eq!(claims[0].get("claim_id").as_str(), claim_id);
    let signature = claims[0].get("signature");
    assert_eq!(signature.get("outcome").as_str(), "sound");
    assert_eq!(signature.get("key_id").as_str(), support::MAINTAINER_KEY_ID);
}

#[test]
fn sync_pending_format_json_on_an_empty_queue_is_an_empty_patches_array() {
    let repo = support::unique_repo("sync-pending-json-empty");
    support::init(&repo);

    let pending_json = support::prikk(&repo)
        .args(["sync", "pending", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&pending_json, "sync pending --format json");
    let stdout = String::from_utf8_lossy(&pending_json.stdout).into_owned();
    let value = support::json::parse(&stdout);
    assert_eq!(value.get("schema_version").as_str(), "sync-pending-v1");
    assert!(
        value.get("patches").as_array().is_empty(),
        "an empty queue must report an empty patches array: {stdout}"
    );
}

#[test]
fn sync_pending_format_json_refuses_an_unsupported_format_value() {
    let repo = support::unique_repo("sync-pending-json-bad-format");
    support::init(&repo);

    let output = support::prikk(&repo)
        .args(["sync", "pending", "--format", "yaml"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "an unsupported --format value must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--format"),
        "the refusal must name --format: {stderr}"
    );
}
