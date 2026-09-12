//! DC-78 §D2/§D5 — the trust store becomes a set of adopted MAINTAINER keys, and adoption must never
//! break the receiver's own history. This file exercises the handoff v2 §4 negative controls that
//! are testable without the exchange/bundle machinery (controls 1 and 3); controls 2 and 4 land with
//! the import stage.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

mod support;

use prikk_store::{Ed25519MaintainerSigner, MaintainerSigner};

const SECOND_KEY_ID: &str = "dc78-second-maintainer";
const SECOND_SEED: [u8; 32] = [
    0x51, 0x51, 0x62, 0x62, 0x73, 0x73, 0x84, 0x84, 0x95, 0x95, 0xa6, 0xa6, 0xb7, 0xb7, 0xc8, 0xc8,
    0xd9, 0xd9, 0xea, 0xea, 0xfb, 0xfb, 0x0c, 0x0c, 0x1d, 0x1d, 0x2e, 0x2e, 0x3f, 0x3f, 0x40, 0x40,
];

fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    // RFC 148: prikk now reads key material from the user's own config directory, so a test that
    // does not neutralise it measures whoever is running it. See `support::isolate_key_environment`.
    support::isolate_key_environment(&mut cmd);
    cmd
}

fn second_public_key_hex() -> String {
    let signer = Ed25519MaintainerSigner::from_seed(SECOND_KEY_ID, &SECOND_SEED)
        .expect("fixed test seed derives a valid signer");
    signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn adopt_second_key(repo: &Path) -> Output {
    prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            SECOND_KEY_ID,
            "--public-key",
            &second_public_key_hex(),
        ])
        .output()
        .expect("trust maintainer add must run")
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

/// DC-78 handoff v2 §4, negative control 1 — the decisive test. "Adopting a second key must not
/// invalidate existing history... It must still pass. This is §D1's bug; if it survives, the
/// increment has failed regardless of what else works." Before DC-78, `verify/objects.rs:223`
/// checked every `Block`/`RefState` against a single-key policy — adopting any second key would
/// have replaced that policy and made every block this repository sealed itself untrusted.
#[test]
fn adopting_a_second_key_does_not_invalidate_the_repositorys_own_sealed_history() {
    let repo = support::unique_repo("second-key-preserves-history");
    support::init(&repo);
    support::generation(
        &repo,
        "heads/main",
        "README.md",
        b"hello\n",
        "first sealed generation",
    );

    // Sanity: the repository verifies cleanly before any second key is adopted.
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify before adoption",
    );

    ok(
        &adopt_second_key(&repo),
        "trust maintainer add (second key)",
    );

    // The decisive assertion: history sealed entirely under the first key must still verify after a
    // second, unrelated key is adopted. `ok()` already checks the exit status (verify's own exit
    // code is non-zero on any issue); this additionally confirms the trust-issue *count* specifically
    // is zero, not just that some other check happened to save the exit code.
    let after = prikk(&repo).arg("verify").output().unwrap();
    ok(&after, "verify after adopting a second key");
    let stdout = String::from_utf8_lossy(&after.stdout);
    assert!(
        stdout.contains("publication trust issues: 0"),
        "verify did not report zero publication trust issues after adopting a second key: {stdout}"
    );
    assert!(
        stdout.contains("checked blocks: 1"),
        "the block sealed before adoption must still be the one checked: {stdout}"
    );

    let _ = std::fs::remove_dir_all(repo);
}

/// DC-78 handoff v2 §4, negative control 3 — TOFU enforces, it does not re-prompt. A changed public
/// key for an already-adopted key id is refused, at the CLI surface, not just the library call.
#[test]
fn readopting_the_fixed_maintainer_key_id_with_a_different_key_is_refused_at_the_cli() {
    let repo = support::unique_repo("second-key-conflict-refused");
    support::init(&repo);
    support::generation(
        &repo,
        "heads/main",
        "README.md",
        b"hello\n",
        "first sealed generation",
    );

    // Re-adopting support::MAINTAINER_KEY_ID with a *different* public key (the second test key's)
    // must be refused, not silently accepted as a replacement.
    let conflicting = prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            support::MAINTAINER_KEY_ID,
            "--public-key",
            &second_public_key_hex(),
        ])
        .output()
        .unwrap();
    assert!(
        !conflicting.status.success(),
        "re-adopting an existing key id with a different key must be refused"
    );

    // And existing history must still verify — the refused attempt must not have touched anything.
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after refused re-adoption",
    );

    let _ = std::fs::remove_dir_all(repo);
}

/// Re-adopting the fixed maintainer key with its own, unchanged public key stays a no-op success —
/// `support::seal` already relies on this being safe to call repeatedly.
#[test]
fn readopting_the_fixed_maintainer_key_id_with_the_same_key_is_idempotent() {
    let repo = support::unique_repo("second-key-idempotent-readd");
    support::init(&repo);
    support::trust_maintainer(&repo);
    let repeat = prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            support::MAINTAINER_KEY_ID,
            "--public-key",
            &support::maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
    ok(&repeat, "idempotent re-adoption of the same key");
    let _ = std::fs::remove_dir_all(repo);
}
