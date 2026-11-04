//! 0.56.0 step 3i (#6, planeter commitment): `archive verify` reports each carried maintainer
//! key's real status in the receiving repository, in one field, `status`, with exactly four fixed
//! strings: `"adopted"`, `"carried, not adopted"`, `"carried, conflicts with an adopted key"`, and
//! `"carried, not checked against a repository"` (run outside any repository). Before this round,
//! the text form printed the single hardcoded literal `"carried, not adopted"` for every key,
//! regardless of what the receiving repository actually had adopted, and `--format json`'s
//! `"status"` field (already present in the wire shape) was hardcoded the same way.
//!
//! The fixture repository is built with `support::generation`, which adopts the fixed
//! `support::MAINTAINER_KEY_ID` under `support::maintainer_public_key_hex()` and seals with it
//! (`support::trust_maintainer`/`support::seal`) -- so the exported archive carries exactly one
//! maintainer key, under that id and that public key, in every case below. Each destination
//! repository then adopts (or does not adopt) that same key id under the same or a different
//! public key, so every one of the four statuses is reached by an honest `archive export` /
//! `trust maintainer add` / `archive verify` round trip through the real binary, never by
//! hand-crafting the archive's own wire bytes.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::{MAINTAINER_KEY_ID, maintainer_public_key_hex, ok};

/// A different, but still valid, Ed25519 public key under the same key id -- the "conflicts" case
/// needs a key id already adopted locally with *different* key material, the same TOFU-collision
/// shape `add_trusted_maintainer`'s own tests use (`crates/prikk-store/src/trust/tests.rs`).
fn a_different_public_key_hex() -> String {
    use prikk_crypto::Ed25519KeyPair;
    support::hex(&Ed25519KeyPair::from_seed(&[0x5A_u8; 32]).public_key_bytes())
}

/// Build a source repository with one real generation under the fixed maintainer key, export it,
/// and return the archive's path. The caller owns cleanup.
fn export_one_generation(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(&format!("step3i-source-{tag}"));
    support::init(&repo);
    support::generation(
        &repo,
        "heads/main",
        "a.txt",
        b"the carried-key-status fixture's one file\n",
        "first generation",
    );
    let mut archive_path = std::env::temp_dir();
    archive_path.push(format!(
        "prikk-cli-step3i-{tag}-{}.prepo",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&repo)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );
    archive_path
}

/// Run `archive verify <archive_path>` from `dir` (its own current directory), in both text and
/// `--format json` form, asserting success and returning both stdouts.
fn verify_both_forms(dir: &std::path::Path, archive_path: &std::path::Path) -> (String, String) {
    let text = support::prikk(dir)
        .args(["archive", "verify"])
        .arg(archive_path)
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "archive verify (text) must exit 0 -- internally consistent regardless of the carried \
         key's repository status: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    let json = support::prikk(dir)
        .args(["archive", "verify", "--format", "json"])
        .arg(archive_path)
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "archive verify --format json must exit 0 for the identical reason: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    (
        String::from_utf8_lossy(&text.stdout).into_owned(),
        String::from_utf8_lossy(&json.stdout).into_owned(),
    )
}

/// Outside any repository at all: a plain directory with no `.prikk`. The planeter letter's
/// fourth, repository-less status.
#[test]
fn outside_any_repository_reports_not_checked_against_a_repository() {
    let archive_path = export_one_generation("outside");
    let no_repo_dir = support::unique_repo("step3i-no-repo");

    let (text, json) = verify_both_forms(&no_repo_dir, &archive_path);
    assert!(
        text.contains(&format!(
            "maintainer key {MAINTAINER_KEY_ID} (carried, not checked against a repository):"
        )),
        "the text form must report the repository-less status for the carried key: {text}"
    );
    assert!(
        json.contains("\"status\": \"carried, not checked against a repository\""),
        "the json form must carry the identical status string: {json}"
    );
    assert!(
        !text.contains("carried, not adopted"),
        "the old unconditional literal must not appear once the status is real: {text}"
    );

    let _ = std::fs::remove_file(&archive_path);
}

/// Inside a repository that adopted the identical key (same id, same public key): `"adopted"`.
#[test]
fn adopted_key_reports_adopted() {
    let archive_path = export_one_generation("adopted");
    let dest = support::unique_repo("step3i-dest-adopted");
    support::init(&dest);
    support::trust_maintainer(&dest);

    let (text, json) = verify_both_forms(&dest, &archive_path);
    assert!(
        text.contains(&format!("maintainer key {MAINTAINER_KEY_ID} (adopted):")),
        "the text form must report \"adopted\" once the destination holds the identical key: \
         {text}"
    );
    assert!(
        json.contains("\"status\": \"adopted\""),
        "the json form must carry the identical status string: {json}"
    );
    assert!(
        !text.contains("carried, not adopted"),
        "the old false claim must not appear for a key that is in fact adopted: {text}"
    );

    let _ = std::fs::remove_file(&archive_path);
}

/// Inside a repository that adopted the same key id under a *different* public key: "conflicts
/// with an adopted key".
#[test]
fn conflicting_key_material_under_the_same_id_reports_conflicts() {
    let archive_path = export_one_generation("conflicts");
    let dest = support::unique_repo("step3i-dest-conflicts");
    support::init(&dest);
    ok(
        &support::prikk(&dest)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                MAINTAINER_KEY_ID,
                "--public-key",
                &a_different_public_key_hex(),
            ])
            .output()
            .unwrap(),
        "trust maintainer add (a different key under the same id)",
    );

    let (text, json) = verify_both_forms(&dest, &archive_path);
    assert!(
        text.contains(&format!(
            "maintainer key {MAINTAINER_KEY_ID} (carried, conflicts with an adopted key):"
        )),
        "the text form must report the conflict once the destination's own key material under \
         this id differs: {text}"
    );
    assert!(
        json.contains("\"status\": \"carried, conflicts with an adopted key\""),
        "the json form must carry the identical status string: {json}"
    );

    let _ = std::fs::remove_file(&archive_path);
}

/// Inside a repository that never adopted that key id at all: `"carried, not adopted"` -- the
/// genuine case the old hardcoded literal happened to get right, now computed rather than assumed.
#[test]
fn never_adopted_key_id_reports_carried_not_adopted() {
    let archive_path = export_one_generation("never-adopted");
    let dest = support::unique_repo("step3i-dest-never-adopted");
    support::init(&dest);
    // A real, different key id adopted here, so the policy is non-empty but still never names
    // `MAINTAINER_KEY_ID` -- distinguishing "no policy at all" from "a policy that just doesn't
    // name this id" (both must classify identically, but only the former was ever exercised by
    // `load_maintainer_trust_policy_or_empty`'s own existing tests).
    ok(
        &support::prikk(&dest)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                "someone-elses-key",
                "--public-key",
                &a_different_public_key_hex(),
            ])
            .output()
            .unwrap(),
        "trust maintainer add (an unrelated key id)",
    );

    let (text, json) = verify_both_forms(&dest, &archive_path);
    assert!(
        text.contains(&format!(
            "maintainer key {MAINTAINER_KEY_ID} (carried, not adopted):"
        )),
        "the text form must still report \"carried, not adopted\" for a key id this \
         repository's own policy simply does not name: {text}"
    );
    assert!(
        json.contains("\"status\": \"carried, not adopted\""),
        "the json form must carry the identical status string: {json}"
    );

    let _ = std::fs::remove_file(&archive_path);
}

/// Red control: `maintainer_public_key_hex()` (the fixed fixture key, matching the archive's own
/// carried key exactly) and `a_different_public_key_hex()` (the conflicting case's key) must
/// actually differ -- otherwise `conflicting_key_material_under_the_same_id_reports_conflicts`
/// above would silently be testing the "adopted" case twice, under a different test name, and
/// would still pass even if the real conflict branch in
/// `prikk_store::classify_carried_key_against_repository` were deleted. Confirmed directly here,
/// not left implicit.
#[test]
fn red_control_fixture_keys_actually_differ() {
    assert_ne!(
        maintainer_public_key_hex(),
        a_different_public_key_hex(),
        "the fixed fixture key and the \"different\" key must not coincide, or the conflict test \
         above would not exercise the conflict branch at all"
    );
}
