//! 0.51.0 step 1 Part C: `--keep-slot`'s own arg-level ruling (K9/K10), and one end-to-end run
//! through the real binary (the handoff's own smoke section 28, condensed).
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;

use support::{init, ok, prikk};

fn compact(repo: &Path, args: &[&str]) -> std::process::Output {
    prikk(repo)
        .arg("compact")
        .args(args)
        .output()
        .expect("spawn prikk compact")
}

#[test]
fn keep_slot_k9_refuses_with_pointer_index() {
    let repo = support::unique_repo("rfc165-step1-c-k9");
    std::fs::create_dir_all(&repo).unwrap();
    init(&repo);

    let output = compact(&repo, &["--pointer-index", "--keep-slot", "a"]);
    assert!(
        !output.status.success(),
        "--keep-slot must not apply to --pointer-index"
    );
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("--rebuild-pointer-index"),
        "must name the rebuild: {text}"
    );
    assert!(
        text.contains("the ref log decides"),
        "must say why: {text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn keep_slot_k10_refuses_with_all() {
    let repo = support::unique_repo("rfc165-step1-c-k10");
    std::fs::create_dir_all(&repo).unwrap();
    init(&repo);

    let output = compact(&repo, &["--all", "--keep-slot", "a"]);
    assert!(
        !output.status.success(),
        "--keep-slot must not apply to --all"
    );
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("exactly one container"),
        "must name one container: {text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn keep_slot_k10_refuses_with_more_than_one_explicit_target() {
    let repo = support::unique_repo("rfc165-step1-c-k10-explicit");
    std::fs::create_dir_all(&repo).unwrap();
    init(&repo);

    let output = compact(
        &repo,
        &[
            "--trust-policy",
            "--received-index",
            "--keep-slot",
            "a",
        ],
    );
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("exactly one container"), "{text}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn keep_slot_bad_value_refuses() {
    let repo = support::unique_repo("rfc165-step1-c-bad-value");
    std::fs::create_dir_all(&repo).unwrap();
    init(&repo);

    let output = compact(&repo, &["--trust-policy", "--keep-slot", "c"]);
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("expects a or b"), "{text}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Smoke section 28 (condensed): the trust-policy twin, built by commands through the real
/// binary, `--plan-only` first (both key sets printed, nothing written), then `--keep-slot a`
/// for real, then the chosen history reads back, then `--recovery-list` shows the run.
#[test]
fn smoke_section_28_trust_policy_keep_slot_end_to_end() {
    let repo = support::unique_repo("rfc165-step1-c-smoke28");
    std::fs::create_dir_all(&repo).unwrap();
    init(&repo);

    let k_key = maintainer_public_key_hex_for_seed(&[91_u8; 32]);
    let l_key = maintainer_public_key_hex_for_seed(&[92_u8; 32]);
    ok(
        &trust_add(&repo, "k", &k_key),
        "trust maintainer add k",
    );
    ok(
        &trust_add(&repo, "l", &l_key),
        "trust maintainer add l",
    );
    ok(
        &trust_remove(&repo, "l"),
        "trust maintainer remove l (revoked)",
    );
    ok(&compact(&repo, &["--trust-policy"]), "compact #1");
    ok(
        &trust_add(&repo, "l", &l_key),
        "trust maintainer add l again",
    );
    ok(&compact(&repo, &["--trust-policy"]), "compact #2");
    ok(
        &trust_remove(&repo, "l"),
        "trust maintainer remove l again (revoked, repeats the retired snapshot)",
    );
    // Lose the record the same way every other fixture in this suite does.
    std::fs::write(repo.join(".prikk/trust/policy-generation.log"), b"").unwrap();

    let plan = compact(&repo, &["--trust-policy", "--keep-slot", "a", "--plan-only"]);
    assert!(plan.status.success(), "{:?}", plan);
    let plan_text = String::from_utf8_lossy(&plan.stdout);
    assert!(plan_text.contains('k'), "{plan_text}");
    let before = store_bytes(&repo);

    let real = compact(&repo, &["--trust-policy", "--keep-slot", "a"]);
    ok(&real, "compact --trust-policy --keep-slot a");
    assert_ne!(before, store_bytes(&repo), "a real run must write something");

    let check = prikk(&repo)
        .args(["trust", "maintainer", "check", "--key-id", "l"])
        .output()
        .unwrap();
    ok(&check, "trust maintainer check l");
    let check_text = String::from_utf8_lossy(&check.stdout);
    assert!(
        check_text.contains("not trusted: l"),
        "l must read revoked after keeping slot a: {check_text}"
    );

    let recovery = prikk(&repo)
        .args(["doctor", "--recovery-list"])
        .output()
        .unwrap();
    ok(&recovery, "doctor --recovery-list");
    let recovery_text = String::from_utf8_lossy(&recovery.stdout);
    assert!(
        recovery_text.contains("keep-slot"),
        "the run must be listed: {recovery_text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

fn maintainer_public_key_hex_for_seed(seed: &[u8; 32]) -> String {
    use prikk_crypto::Ed25519KeyPair;
    support::hex(&Ed25519KeyPair::from_seed(seed).public_key_bytes())
}

fn trust_add(repo: &Path, key_id: &str, public_key_hex: &str) -> std::process::Output {
    prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            key_id,
            "--public-key",
            public_key_hex,
        ])
        .output()
        .expect("spawn trust maintainer add")
}

fn trust_remove(repo: &Path, key_id: &str) -> std::process::Output {
    prikk(repo)
        .args(["trust", "maintainer", "remove", "--key-id", key_id])
        .output()
        .expect("spawn trust maintainer remove")
}

fn store_bytes(repo: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    support::store_bytes(repo)
}
