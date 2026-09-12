//! RFC 121 §3 (round 3): argument hygiene, end-to-end through the compiled binary.
//!
//! The handoff named two reproductions directly (`prikk status --nonsense` exits `0`;
//! `bundle export --ref X --ref Y` silently exports `Y`) and the v3 amendment added a third
//! (`prikk commit` with no `-m` exits `1`, not the ruled `2`). This file drives all three, plus a
//! representative sample of the same last-write-wins duplicate-flag shape found at every other
//! value-carrying flag in the crate during the full enumeration (`arg_scan.rs`'s own module doc),
//! and confirms `doctor --repair-main-ref` keeps its recognized-and-refused shape rather than being
//! reclassified as an unknown argument.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

fn assert_usage_error(output: &std::process::Output, needle: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected a usage error (exit 2): {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(needle), "stderr: {stderr}");
}

/// §3.1's own reproduction: `status` took no arguments into account at all.
#[test]
fn status_refuses_an_unknown_argument() {
    let repo = support::unique_repo("rfc121-hygiene-status");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["status", "--nonsense"])
        .output()
        .unwrap();
    assert_usage_error(&output, "unknown status argument: --nonsense");
}

/// §3.1: `init` took the first positional as the path and silently discarded the rest.
#[test]
fn init_refuses_an_extra_positional() {
    let repo = support::unique_repo("rfc121-hygiene-init");
    let output = support::prikk(&repo)
        .args(["init", "somewhere", "extra"])
        .output()
        .unwrap();
    assert_usage_error(&output, "unknown init argument: extra");
    // And a bare path with nothing extra still works.
    let repo2 = support::unique_repo("rfc121-hygiene-init-ok");
    support::init(&repo2);
}

/// The v3 amendment's own "good first probe": under §1's ruled contract, a missing required flag
/// is a usage error, not the operational-failure `1` this used to exit before this round.
#[test]
fn commit_with_no_message_exits_two() {
    let repo = support::unique_repo("rfc121-hygiene-commit");
    support::init(&repo);
    std::fs::write(repo.join("f.txt"), "hello").unwrap();
    let output = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .arg("commit")
        .output()
        .unwrap();
    assert_usage_error(&output, "commit requires -m <message>");
}

/// §3.2's named victim: `bundle export --ref X --ref Y` used to export `Y` silently. Caught at the
/// parse stage, before the repository is even opened -- no sealed history is needed to prove it.
#[test]
fn bundle_export_refuses_a_duplicate_ref_flag() {
    let repo = support::unique_repo("rfc121-hygiene-bundle-ref");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--ref",
            "heads/other",
            "--output",
            "out.bundle",
        ])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --ref flag");
    assert!(
        !repo.join("out.bundle").exists(),
        "a refused export must not write anything"
    );
}

/// The same last-write-wins shape as `bundle export --ref`, at `bundle export --output`.
#[test]
fn bundle_export_refuses_a_duplicate_output_flag() {
    let repo = support::unique_repo("rfc121-hygiene-bundle-output");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            "a.bundle",
            "--output",
            "b.bundle",
        ])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --output flag");
}

/// The same shape at `log --ref`, in `args.rs` (not `bundle.rs`) -- one mechanism, not a
/// file-by-file fix.
#[test]
fn log_refuses_a_duplicate_ref_flag() {
    let repo = support::unique_repo("rfc121-hygiene-log-ref");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["log", "--ref", "heads/main", "--ref", "heads/other"])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --ref flag");
}

/// `tag create --target` duplicated.
#[test]
fn tag_create_refuses_a_duplicate_target_flag() {
    let repo = support::unique_repo("rfc121-hygiene-tag-target");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args([
            "tag",
            "create",
            "release",
            "--target",
            "heads/main",
            "--target",
            "heads/other",
        ])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --target flag");
}

/// `branch create --from` duplicated.
#[test]
fn branch_create_refuses_a_duplicate_from_flag() {
    let repo = support::unique_repo("rfc121-hygiene-branch-from");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args([
            "branch",
            "create",
            "topic",
            "--from",
            "heads/main",
            "--from",
            "heads/other",
        ])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --from flag");
}

/// `sync summary --output` duplicated -- `sync.rs` carries six `parse_*_args` functions; this is
/// the simplest of them.
#[test]
fn sync_summary_refuses_a_duplicate_output_flag() {
    let repo = support::unique_repo("rfc121-hygiene-sync-output");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args([
            "sync",
            "summary",
            "--output",
            "a.psyncsu1",
            "--output",
            "b.psyncsu1",
        ])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --output flag");
}

/// `unlock --lock` duplicated -- an inline loop, not a `parse_*_args` function, and the one place
/// in the crate whose flag value is a filesystem path rather than a ref/output name.
#[test]
fn unlock_refuses_a_duplicate_lock_flag() {
    let repo = support::unique_repo("rfc121-hygiene-unlock");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["unlock", "--lock", "a", "--lock", "b"])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --lock flag");
}

/// `compact`'s target flags used to accumulate into a `Vec` and `.dedup()`, which only removes
/// *adjacent* duplicates (`--pointer-index --all --pointer-index` would still have compacted
/// `PointerIndex` twice) -- refused outright now, the same as every other flag in the crate.
#[test]
fn compact_refuses_a_duplicate_target_flag() {
    let repo = support::unique_repo("rfc121-hygiene-compact");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["compact", "--pointer-index", "--pointer-index"])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --pointer-index flag");
}

/// `--all` combined with a named target is refused rather than silently compacting some targets
/// twice, in either order: a named target before `--all` hits `--all`'s own combination check;
/// `--all` before a named target hits that target's own already-present duplicate check -- both
/// refuse, worded for whichever flag comes second.
#[test]
fn compact_refuses_all_combined_with_a_named_target() {
    let repo = support::unique_repo("rfc121-hygiene-compact-all");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["compact", "--pointer-index", "--all"])
        .output()
        .unwrap();
    assert_usage_error(&output, "compact --all cannot be combined");

    let output = support::prikk(&repo)
        .args(["compact", "--all", "--pointer-index"])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --pointer-index flag");
}

/// A boolean flag repeated is refused too, not only value-carrying flags -- the ruled contract
/// (RFC 121 §1) lists "duplicate flag" as its own class with no carve-out for booleans.
///
/// AUD-10 fixed `main.rs::run_seal` to parse before building the MAINTAINER signer, so this test
/// no longer depends on it -- valid key material is supplied anyway, so a regression back to the
/// old signer-before-parse order would still hit this refusal rather than silently pass on a
/// masked "maintainer signing is required" instead. See `rfc121_aud10_signer_ordering.rs` for the
/// direct control (no key material supplied at all).
#[test]
fn seal_refuses_a_duplicate_allow_no_audit_flag() {
    let repo = support::unique_repo("rfc121-hygiene-seal-dup-bool");
    support::init(&repo);
    let output = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit", "--allow-no-audit"])
        .output()
        .unwrap();
    assert_usage_error(&output, "duplicate --allow-no-audit flag");
}

/// `doctor --repair-main-ref` must keep its recognized-and-refused shape: parsed as a real flag,
/// then refused later with a stated reason -- never demoted to "unknown argument" (exit `2`), and
/// never silently reclassified off its existing exit `1`.
#[test]
fn doctor_repair_main_ref_stays_a_recognized_operational_refusal() {
    let repo = support::unique_repo("rfc121-hygiene-doctor-repair-main-ref");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["doctor", "--repair-main-ref"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "a recognized, later-refused flag must stay an operational failure: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--repair-main-ref has no implemented repair"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown doctor argument"),
        "must not be reclassified as an unknown argument: {stderr}"
    );
}

/// An unrecognized subcommand under a multi-word command is a usage error too, the same as an
/// unrecognized top-level command name (RFC 121 round 1's own `main.rs::run` precedent) -- checked
/// on `branch`, `bundle`, `sync`, and `tag`, the four commands with their own subcommand dispatch.
#[test]
fn unrecognized_subcommands_are_usage_errors() {
    let repo = support::unique_repo("rfc121-hygiene-subcommands");
    support::init(&repo);
    for args in [
        ["branch", "nonsense"],
        ["bundle", "nonsense"],
        ["sync", "nonsense"],
        ["tag", "nonsense"],
    ] {
        let output = support::prikk(&repo).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?} -> {output:?}");
    }
}
