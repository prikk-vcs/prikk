//! CLI end-to-end regression for DC-57: active-patch warn/hard-block thresholds
//! (`PRIKK_ACTIVE_PATCH_WARN`/`PRIKK_ACTIVE_PATCH_LIMIT`), driving the compiled `prikk` binary exactly
//! as a real user would. Env vars are set per-child-process (`Command::env`), never via
//! `std::env::set_var` in this test binary itself, so these tests are safe under Rust's default
//! parallel test execution — matching the established pattern in `dc65_text_edit_baseline.rs`/
//! `dc66_multi_commit_queuing.rs`. See
//! `rfcs/handoffs/DC-57-active-patch-thresholds/implementation-handoff-v2.md`.

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
    dir.push(format!("prikk-cli-dc57-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const AUTHOR_KEY_ID: &str = "dc57-test-author";
const AUTHOR_SEED_HEX: &str = "2200334455667788990011223344556677889900112233445566778899001122";
const MAINTAINER_KEY_ID: &str = "dc57-test-maintainer";
const MAINTAINER_SEED: [u8; 32] = [
    0x61, 0x61, 0x72, 0x72, 0x83, 0x83, 0x94, 0x94, 0xa5, 0xa5, 0xb6, 0xb6, 0xc7, 0xc7, 0xd8, 0xd8,
    0xe9, 0xe9, 0xfa, 0xfa, 0x0b, 0x0b, 0x1c, 0x1c, 0x2d, 0x2d, 0x3e, 0x3e, 0x4f, 0x4f, 0x50, 0x50,
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

/// Commit with optional threshold env overrides. `None` leaves the variable unset (default applies).
fn commit_with_thresholds(
    repo: &Path,
    message: &str,
    warn: Option<&str>,
    limit: Option<&str>,
) -> Output {
    let mut cmd = prikk(repo);
    cmd.env("PRIKK_AUTHOR_KEY_ID", AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", message]);
    if let Some(warn) = warn {
        cmd.env("PRIKK_ACTIVE_PATCH_WARN", warn);
    }
    if let Some(limit) = limit {
        cmd.env("PRIKK_ACTIVE_PATCH_LIMIT", limit);
    }
    cmd.output().unwrap()
}

fn status_with_thresholds(repo: &Path, warn: Option<&str>, limit: Option<&str>) -> Output {
    let mut cmd = prikk(repo);
    cmd.arg("status");
    if let Some(warn) = warn {
        cmd.env("PRIKK_ACTIVE_PATCH_WARN", warn);
    }
    if let Some(limit) = limit {
        cmd.env("PRIKK_ACTIVE_PATCH_LIMIT", limit);
    }
    cmd.output().unwrap()
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

/// The full user-facing lifecycle at scaled-down thresholds (warn=1, limit=2): first queued commit
/// crosses warn, `status` recommends sealing; second reaches the hard limit; third is refused with an
/// actionable error naming `seal`; `seal` itself remains available and drains the queue.
#[test]
fn scaled_thresholds_warn_then_hard_block_then_seal_recovers() {
    let repo = unique_repo("scaled-thresholds");
    init(&repo);
    std::fs::write(repo.join("base.txt"), "base").unwrap();
    ok(
        &commit_with_thresholds(&repo, "genesis", None, None),
        "genesis commit",
    );
    seal(&repo);

    let before = status_with_thresholds(&repo, Some("1"), Some("2"));
    ok(&before, "status before any queued patch");
    let before_stdout = String::from_utf8_lossy(&before.stdout);
    assert!(
        !before_stdout.contains("warning: active patches"),
        "no warning expected with an empty queue: {before_stdout}"
    );

    std::fs::write(repo.join("a.txt"), "alpha").unwrap();
    ok(
        &commit_with_thresholds(&repo, "queue a", Some("1"), Some("2")),
        "first queued commit (below warn)",
    );

    let after_one = status_with_thresholds(&repo, Some("1"), Some("2"));
    ok(&after_one, "status after one queued patch");
    let after_one_stdout = String::from_utf8_lossy(&after_one.stdout);
    assert!(
        after_one_stdout.contains("queued patches: 1 targeting heads/main"),
        "{after_one_stdout}"
    );
    assert!(
        after_one_stdout.contains("recommended threshold (1)")
            && !after_one_stdout.contains("configured hard limit"),
        "expected a warn (not hard-limit) hint at count 1, limit 2: {after_one_stdout}"
    );

    std::fs::write(repo.join("b.txt"), "beta").unwrap();
    ok(
        &commit_with_thresholds(&repo, "queue b", Some("1"), Some("2")),
        "second queued commit (reaches the hard limit)",
    );

    let after_two = status_with_thresholds(&repo, Some("1"), Some("2"));
    ok(&after_two, "status after two queued patches");
    let after_two_stdout = String::from_utf8_lossy(&after_two.stdout);
    assert!(
        after_two_stdout.contains("configured hard limit (2)"),
        "expected the hard-limit hint, not merely the warn hint, at count 2, limit 2: {after_two_stdout}"
    );

    std::fs::write(repo.join("c.txt"), "gamma").unwrap();
    let blocked = commit_with_thresholds(&repo, "queue c", Some("1"), Some("2"));
    assert!(
        !blocked.status.success(),
        "a third commit at the hard limit must be refused"
    );
    let blocked_stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        blocked_stderr.contains("seal"),
        "the refusal must name `seal` as the remedy: {blocked_stderr}"
    );
    // RFC 132 part 2, control 5: a full active-patch queue is a caller precondition, not a lock --
    // nothing is held and no other writer is racing this one. Driven through a real repository at
    // the configured limit (this test's own setup), not unit-tested against the constructor.
    assert!(
        blocked_stderr.contains("precondition not met:"),
        "a full active-patch queue is a caller precondition, not a lock conflict: {blocked_stderr}"
    );

    // `seal` remains available at and above the hard bound and fully drains the queue.
    seal(&repo);
    let after_seal = status_with_thresholds(&repo, Some("1"), Some("2"));
    ok(&after_seal, "status after seal");
    assert!(
        String::from_utf8_lossy(&after_seal.stdout).contains("queued patches: 0"),
        "seal must fully drain the queue"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Defaults apply when both variables are unset: an ordinary commit succeeds and `status` prints no
/// threshold warning for a small queue.
#[test]
fn defaults_apply_when_unset() {
    let repo = unique_repo("defaults");
    init(&repo);
    std::fs::write(repo.join("base.txt"), "base").unwrap();
    ok(
        &commit_with_thresholds(&repo, "genesis", None, None),
        "genesis commit under default thresholds",
    );
    let out = status_with_thresholds(&repo, None, None);
    ok(&out, "status under default thresholds");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("warning: active patches"),
        "one queued patch must not trigger a warning against the 800 default"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Malformed configuration fails closed rather than silently keeping the default (criterion 6): a
/// non-numeric value, a warn threshold above the hard limit, and zero for either — all three must
/// refuse the command outright.
#[test]
fn malformed_thresholds_fail_closed_rather_than_defaulting() {
    let repo = unique_repo("malformed-thresholds");
    init(&repo);
    std::fs::write(repo.join("base.txt"), "base").unwrap();
    ok(
        &commit_with_thresholds(&repo, "genesis", None, None),
        "genesis commit under default thresholds",
    );

    std::fs::write(repo.join("a.txt"), "alpha").unwrap();
    let non_numeric = commit_with_thresholds(&repo, "a", None, Some("not-a-number"));
    assert!(
        !non_numeric.status.success(),
        "a non-numeric PRIKK_ACTIVE_PATCH_LIMIT must be refused, not silently defaulted"
    );

    let warn_above_limit = commit_with_thresholds(&repo, "a", Some("2000"), Some("1000"));
    assert!(
        !warn_above_limit.status.success(),
        "PRIKK_ACTIVE_PATCH_WARN above PRIKK_ACTIVE_PATCH_LIMIT must be refused"
    );

    let zero_limit = commit_with_thresholds(&repo, "a", None, Some("0"));
    assert!(
        !zero_limit.status.success(),
        "PRIKK_ACTIVE_PATCH_LIMIT=0 must be refused, not treated as unlimited or as the default"
    );

    let zero_warn = commit_with_thresholds(&repo, "a", Some("0"), None);
    assert!(
        !zero_warn.status.success(),
        "PRIKK_ACTIVE_PATCH_WARN=0 must be refused, not treated as always-warn or as the default"
    );

    // None of the refused attempts should have queued anything beyond the genesis commit already
    // queued above (never sealed in this test) — still exactly 1.
    let out = status_with_thresholds(&repo, None, None);
    ok(&out, "status after all refused attempts");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("queued patches: 1 targeting heads/main"),
        "no malformed-config attempt may have queued a patch beyond the genesis commit"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
