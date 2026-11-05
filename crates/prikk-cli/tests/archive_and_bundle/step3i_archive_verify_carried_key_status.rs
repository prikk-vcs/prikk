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
//!
//! 0.56.0 step 3i-b corrects one defect in 3i's own classification: a destination that *holds*
//! `.prikk` but can't actually be read (a corrupted `FORMAT` marker, or a corrupted trust-policy
//! container) used to either stay silent about it (the open-repository case) or, worse, make the
//! whole `archive verify` command fail and never print the archive's own, already-computed report
//! at all (the load-trust-policy case). Both cases below must still print the full report, still
//! report the fourth status for the carried key, still exit `0` on the archive's own
//! `is_internally_consistent()`, and now print exactly one explanatory line to stderr. The
//! "no repository at all" case (`outside_any_repository_reports_not_checked_against_a_repository`,
//! below) must keep behaving exactly as before, including staying silent on stderr.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::{MAINTAINER_KEY_ID, maintainer_public_key_hex, ok};
use prikk_store::{ContainerSlot, RepositoryLayout};

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

/// Like `verify_both_forms`, but also returns each form's stderr -- step 3i-b needs this: its own
/// defect was that the two "no policy to check against" cases disagreed about whether anything was
/// printed to stderr, and the fix adds exactly one explanatory line for the "a repository is here
/// but could not be read" case while the "no repository at all" case must keep printing none.
/// `verify_both_forms` itself never needed stderr, so it stays as-is for the other tests above.
fn verify_both_forms_with_stderr(
    dir: &std::path::Path,
    archive_path: &std::path::Path,
) -> (String, String, String, String) {
    let text = support::prikk(dir)
        .args(["archive", "verify"])
        .arg(archive_path)
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "archive verify (text) must exit 0 -- the archive's own `is_internally_consistent()` \
         decides the exit code, never the receiving repository's own health: {}",
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
        String::from_utf8_lossy(&text.stderr).into_owned(),
        String::from_utf8_lossy(&json.stdout).into_owned(),
        String::from_utf8_lossy(&json.stderr).into_owned(),
    )
}

/// Outside any repository at all: a plain directory with no `.prikk`. The planeter letter's
/// fourth, repository-less status. 3i-b: this is the one case that must stay exactly as it was,
/// including staying silent on stderr -- there genuinely is no repository here to report as
/// broken.
#[test]
fn outside_any_repository_reports_not_checked_against_a_repository() {
    let archive_path = export_one_generation("outside");
    let no_repo_dir = support::unique_repo("step3i-no-repo");

    let (text, text_err, json, json_err) =
        verify_both_forms_with_stderr(&no_repo_dir, &archive_path);
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
    assert!(
        text_err.is_empty(),
        "no `.prikk` at all must print nothing to stderr (text form): {text_err:?}"
    );
    assert!(
        json_err.is_empty(),
        "no `.prikk` at all must print nothing to stderr (json form): {json_err:?}"
    );

    let _ = std::fs::remove_file(&archive_path);
}

/// 3i-b: a destination that holds `.prikk`, but whose `FORMAT` marker is corrupted, so
/// `open_repository` itself fails. The archive's own report must still print in full, the carried
/// key must still get the fourth status (it's literally true: nothing *was* checked), the command
/// must still exit `0` on the archive's own consistency, and exactly one line naming the broken
/// repository must reach stderr -- the behaviour 3i itself got wrong (it stayed silent here).
#[test]
fn a_repository_with_a_corrupted_format_marker_still_prints_the_full_report_and_warns_once() {
    let archive_path = export_one_generation("format-corrupt");
    let dest = support::unique_repo("step3i-b-dest-format-corrupt");
    support::init(&dest);
    std::fs::write(
        dest.join(".prikk").join("FORMAT"),
        b"not a format number at all\n",
    )
    .expect("corrupt the FORMAT marker");

    let (text, text_err, json, json_err) = verify_both_forms_with_stderr(&dest, &archive_path);

    // The archive's own report is still printed in full.
    assert!(
        text.contains("structurally sound:"),
        "the full archive report must still print when the repository is unreadable: {text}"
    );
    assert!(
        text.contains("reference closure sound:"),
        "the full archive report must still print when the repository is unreadable: {text}"
    );
    assert!(
        text.contains("signatures checked:"),
        "the full archive report must still print when the repository is unreadable: {text}"
    );
    assert!(
        text.contains(&format!(
            "maintainer key {MAINTAINER_KEY_ID} (carried, not checked against a repository):"
        )),
        "a repository present but unreadable must still classify as the planeter's fourth, \
         literally-true status: {text}"
    );
    assert!(
        json.contains("\"status\": \"carried, not checked against a repository\""),
        "the json form must carry the identical status string: {json}"
    );
    assert!(
        json.contains("\"ok\": true"),
        "the archive's own internal consistency, never the repository's health, decides `ok`: \
         {json}"
    );

    for stderr in [&text_err, &json_err] {
        assert_eq!(
            stderr.lines().count(),
            1,
            "exactly one stderr line naming the unreadable repository: {stderr:?}"
        );
        assert!(
            stderr.contains("a repository is here"),
            "the stderr line must name that a repository is present: {stderr}"
        );
        assert!(
            stderr.contains("could not be read"),
            "the stderr line must name that it could not be read: {stderr}"
        );
        assert!(
            stderr.contains("carried keys were not checked against it"),
            "the stderr line must name that carried keys were not checked against it: {stderr}"
        );
    }

    let _ = std::fs::remove_file(&archive_path);
}

/// The same corruption shape `rfc138_trust_read_surface.rs`'s own
/// `corrupt_trust_policy_container` uses: a garbage prefix with no valid `PTRUPOL1` magic
/// anywhere in it, long enough to clear `TRUST_POLICY_HEADER_LEN` (so it decodes as `Invalid`, a
/// genuine item failure, not merely `TrailingPartial`, which `read_current_trust_policy_snapshot`
/// tolerates and folds into "no snapshot yet" -- a shorter garbage payload, such as
/// `trust/tests.rs`'s own `a_corrupt_policy_container_fails_closed_...` fixture, would silently
/// miss `load_maintainer_trust_policy_or_empty`'s error path entirely and instead resolve to an
/// empty policy), followed by one genuinely sound, empty snapshot record -- so the container is
/// RFC 164 Rule A interior damage (a sound record follows the garbage), which this read path still
/// fails closed on.
fn corrupt_trust_policy_container(repo: &std::path::Path) {
    let layout =
        RepositoryLayout::open(repo).expect("repository must open to corrupt its container");
    let mut bytes: Vec<u8> = b"this container has been damaged beyond its own header length and carries no valid magic anywhere in it".to_vec();
    const MAGIC: &[u8; 8] = b"PTRUPOL1";
    const VERSION: u16 = 1;
    let body = 0_u32.to_be_bytes().to_vec();
    let body_len = (body.len() as u64).to_be_bytes();
    let mut preimage = MAGIC.to_vec();
    preimage.extend_from_slice(&VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len);
    preimage.extend_from_slice(&body);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_be_bytes());
    bytes.extend_from_slice(&body_len);
    bytes.extend_from_slice(&prikk_hash::sha256(&preimage));
    bytes.extend_from_slice(&body);
    std::fs::write(
        layout.trust_policy_container_slot_path(ContainerSlot::A),
        bytes,
    )
    .expect("write corrupt trust policy bytes");
}

/// 3i-b: a destination that holds `.prikk` and opens fine, but whose trust-policy container is
/// corrupted, so `load_maintainer_trust_policy_or_empty` fails -- the second of the two cases 3i-b
/// unifies. Before this round this made the *whole command* fail and the archive's own, already
/// computed report was never printed at all. Same assertions as the `FORMAT`-corruption case
/// above: full report, fourth status, exit 0, one stderr line.
#[test]
fn a_repository_with_a_corrupted_trust_policy_container_still_prints_the_full_report_and_warns_once()
 {
    let archive_path = export_one_generation("policy-corrupt");
    let dest = support::unique_repo("step3i-b-dest-policy-corrupt");
    support::init(&dest);
    support::trust_maintainer(&dest);
    corrupt_trust_policy_container(&dest);

    let (text, text_err, json, json_err) = verify_both_forms_with_stderr(&dest, &archive_path);

    assert!(
        text.contains("structurally sound:"),
        "the full archive report must still print when the repository is unreadable: {text}"
    );
    assert!(
        text.contains("reference closure sound:"),
        "the full archive report must still print when the repository is unreadable: {text}"
    );
    assert!(
        text.contains("signatures checked:"),
        "the full archive report must still print when the repository is unreadable: {text}"
    );
    assert!(
        text.contains(&format!(
            "maintainer key {MAINTAINER_KEY_ID} (carried, not checked against a repository):"
        )),
        "a repository that opens but whose trust policy can't load must still classify as the \
         planeter's fourth, literally-true status: {text}"
    );
    assert!(
        json.contains("\"status\": \"carried, not checked against a repository\""),
        "the json form must carry the identical status string: {json}"
    );
    assert!(
        json.contains("\"ok\": true"),
        "the archive's own internal consistency, never the repository's health, decides `ok`: \
         {json}"
    );

    for stderr in [&text_err, &json_err] {
        assert_eq!(
            stderr.lines().count(),
            1,
            "exactly one stderr line naming the unreadable repository: {stderr:?}"
        );
        assert!(
            stderr.contains("a repository is here"),
            "the stderr line must name that a repository is present: {stderr}"
        );
        assert!(
            stderr.contains("could not be read"),
            "the stderr line must name that it could not be read: {stderr}"
        );
        assert!(
            stderr.contains("carried keys were not checked against it"),
            "the stderr line must name that carried keys were not checked against it: {stderr}"
        );
    }

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
