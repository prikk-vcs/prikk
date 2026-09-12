//! RFC 147 §2d, under RFC 132's per-site practice: three trust-path refusals that report a caller
//! precondition, reached through the compiled binary.
//!
//! **The store-level tests in `prikk-store/src/trust/tests.rs` pin the `PrikkError` variant; these
//! pin what a person actually reads.** Both halves are needed: the variant decides the rendered
//! prefix (`precondition not met:` rather than `integrity error:`/`invalid signature:`), and a
//! variant change that never reached the rendered line would be invisible to every consumer.
//! Exit codes are asserted alongside, because RFC 121's `0`/`1`/`2` contract must not move when a
//! variant does — all three of these were `1` before this reclassification and must still be.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Public key hex for one of the fixed test seeds, derived rather than pasted.
fn public_key_hex(seed: &[u8; 32]) -> String {
    use prikk_store::MaintainerSigner;
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed("probe", seed)
        .expect("fixed test seed derives a valid signer");
    support::hex(&signer.public_key_bytes())
}

/// `init`, commit, then seal with a maintainer key configured but nothing adopted — the documented
/// second-project route (`docs/src/guide/first-run.md`), not a mocked snapshot.
fn repo_with_one_queued_patch(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    repo
}

fn seal_with(repo: &Path, key_id: &str, seed: &[u8; 32]) -> std::process::Output {
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", key_id)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(seed)),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap()
}

/// Site 1: nothing adopted yet. Reached by the documented route, per the handoff's control 2.
#[test]
fn site1_seal_with_nothing_adopted_renders_a_precondition() {
    let repo = repo_with_one_queued_patch("rfc132-site1");

    let out = seal_with(&repo, "maintainer", &support::MAINTAINER_SEED);
    let stderr = stderr_of(&out);
    assert_eq!(out.status.code(), Some(1), "RFC 121: unchanged at 1");
    assert!(
        stderr.contains(
            "error: precondition not met: no maintainer key is adopted in this repository yet"
        ),
        "the rendered prefix must follow the variant: {stderr}"
    );
    assert!(
        stderr.contains("trust maintainer add") && stderr.contains("doctor"),
        "both halves of the message must reach the reader: {stderr}"
    );
    assert!(
        !stderr.contains("integrity error"),
        "nothing here is damaged, and the rendered line must not say so: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Site 2: a key id already adopted with different material — a trust-on-first-use collision,
/// refused before anything is verified.
#[test]
fn site2_readopting_an_id_with_new_material_renders_a_precondition() {
    let repo = support::unique_repo("rfc132-site2");
    support::init(&repo);
    support::trust_maintainer(&repo);

    let out = support::prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            support::MAINTAINER_KEY_ID,
            "--public-key",
            &public_key_hex(&[0x5a_u8; 32]),
        ])
        .output()
        .unwrap();
    let stderr = stderr_of(&out);
    assert_eq!(out.status.code(), Some(1), "RFC 121: unchanged at 1");
    assert!(
        stderr.contains("error: precondition not met:")
            && stderr.contains("is already adopted with a different public key"),
        "the rendered prefix must follow the variant: {stderr}"
    );
    assert!(
        !stderr.contains("invalid signature"),
        "no signature was checked, and the rendered line must not claim one failed: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Site 3: policy membership. The exported key id is simply not among the adopted ones.
#[test]
fn site3_sealing_under_an_unadopted_key_id_renders_a_precondition() {
    let repo = repo_with_one_queued_patch("rfc132-site3");
    support::trust_maintainer(&repo);

    let out = seal_with(&repo, "some-other-id", &support::MAINTAINER_SEED);
    let stderr = stderr_of(&out);
    assert_eq!(out.status.code(), Some(1), "RFC 121: unchanged at 1");
    assert!(
        stderr.contains("error: precondition not met:")
            && stderr.contains("is not trusted by policy"),
        "the rendered prefix must follow the variant: {stderr}"
    );
    assert!(
        !stderr.contains("invalid signature"),
        "membership is not a signature check: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// The adjacent site this round deliberately left alone: the id *is* adopted and the material does
/// not match it. That is a real key-binding failure and must still render `invalid signature:` —
/// the half a reclassification round is most likely to take with it by accident.
#[test]
fn the_adjacent_material_mismatch_still_renders_invalid_signature() {
    let repo = repo_with_one_queued_patch("rfc132-adjacent");
    support::trust_maintainer(&repo);

    let out = seal_with(&repo, support::MAINTAINER_KEY_ID, &[0x5a_u8; 32]);
    let stderr = stderr_of(&out);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr.contains("error: invalid signature:")
            && stderr.contains("does not match trusted key"),
        "a trusted id with the wrong material stays a signature failure: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
