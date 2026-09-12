//! RFC 121 §2.5 (`command-discovery-handoff-v1.md`, v2 amendment): per-command `--help`, end to end
//! through the compiled binary. The v2 amendment changed this increment's premise -- round 3's
//! argument hygiene means every parser now refuses an unrecognized flag, `--help` included, so
//! `--help` must be recognized *before* a command's own parser ever runs. This file demonstrates
//! that routing directly, including the regression it closes: `verify --help` exited `2` before
//! this increment (round 3 made it a usage error), and must exit `0` now.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

fn help_output(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    support::prikk(repo).args(args).output().unwrap()
}

/// A plain, single-word command.
#[test]
fn help_for_a_plain_command() {
    let repo = support::unique_repo("rfc121-help-plain");
    let output = help_output(&repo, &["init", "--help"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        "  prikk init [path]                         Create a .prikk repository layout\n"
    );
}

/// `-h` is recognized too, the same as the top-level meta-arm.
#[test]
fn short_form_h_works_the_same_as_help() {
    let repo = support::unique_repo("rfc121-help-short");
    let output = help_output(&repo, &["init", "-h"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("prikk init [path]"), "{stdout}");
}

/// A multi-word command (`trust maintainer add`) -- `--help` is recognized regardless of how many
/// subcommand tokens precede it, and prints the whole `trust` entry's help_lines (both `add` and
/// `remove`), since `COMMANDS` has no finer-grained table to route a subcommand to.
#[test]
fn help_for_a_multi_word_command() {
    let repo = support::unique_repo("rfc121-help-multiword");
    let output = help_output(&repo, &["trust", "maintainer", "add", "--help"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("trust maintainer add --key-id"), "{stdout}");
    assert!(
        stdout.contains("trust maintainer remove --key-id"),
        "{stdout}"
    );
}

/// `bundle --help` and `bundle export --help` are deliberately the same question: both print the
/// full `bundle` entry (export/import/verify together), per the documented routing decision.
#[test]
fn command_level_and_subcommand_level_help_are_identical() {
    let repo = support::unique_repo("rfc121-help-subcommand");
    let bundle_only = help_output(&repo, &["bundle", "--help"]);
    let bundle_export = help_output(&repo, &["bundle", "export", "--help"]);
    assert!(bundle_only.status.success(), "{bundle_only:?}");
    assert!(bundle_export.status.success(), "{bundle_export:?}");
    assert_eq!(bundle_only.stdout, bundle_export.stdout);
    let stdout = String::from_utf8_lossy(&bundle_only.stdout);
    assert!(stdout.contains("bundle export"), "{stdout}");
    assert!(stdout.contains("bundle import"), "{stdout}");
    assert!(stdout.contains("bundle verify"), "{stdout}");
}

/// The regression the v2 amendment exists to close: `verify --help` used to be refused by
/// `parse_verify_args` itself (round 3 made `--format`/`--stop-on-first-error`/positional-path the
/// only recognized shapes, so `--help` fell to "unknown verify argument"). Before this increment
/// that was exit `2`; the routing in `main.rs::run` now intercepts it first.
#[test]
fn a_command_whose_parser_previously_rejected_help_now_accepts_it() {
    let repo = support::unique_repo("rfc121-help-regression");
    support::init(&repo);
    let output = help_output(&repo, &["verify", "--help"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "verify --help must now succeed, not be refused as an unknown argument: {output:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("prikk verify"), "{stdout}");
    // Confirms the routing happens *before* the parser: no repository is opened or touched -- a
    // bare `--help` works even with no `.prikk` at all (unlike a real `verify` invocation, which
    // would fail here since `uninit_repo` was never `init`ed).
    let uninit_repo = support::unique_repo("rfc121-help-regression-uninit");
    let output_no_repo = help_output(&uninit_repo, &["verify", "--help"]);
    assert!(
        output_no_repo.status.success(),
        "verify --help must not require an initialized repository: {output_no_repo:?}"
    );
}

/// `verify --format json --stop-on-first-error --help` -- `--help` short-circuits regardless of
/// what else is present, since it is recognized before the parser that would otherwise validate
/// (and, for a duplicate flag, refuse) the rest of the line ever runs.
#[test]
fn help_short_circuits_even_alongside_other_flags() {
    let repo = support::unique_repo("rfc121-help-alongside-flags");
    let output = help_output(
        &repo,
        &["verify", "--format", "json", "--format", "json", "--help"],
    );
    assert!(output.status.success(), "{output:?}");
}

/// Documented flags actually run, matching their own help line -- the four named in the handoff
/// (`verify --format json`, `verify --stop-on-first-error`, `unlock --yes|--force`,
/// `doctor --repair-main-ref`) plus the `-m`/`--message` alias sites found during the enumeration
/// (`commit`, `rollback-draft`, `tag create`).
#[test]
fn documented_flags_match_real_behavior() {
    let repo = support::unique_repo("rfc121-help-flags-real");
    support::init(&repo);

    let verify_json = support::prikk(&repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&verify_json, "verify --format json");
    assert!(
        String::from_utf8_lossy(&verify_json.stdout).contains("verify-report-v1"),
        "{verify_json:?}"
    );

    let doctor_repair = support::prikk(&repo)
        .args(["doctor", "--repair-main-ref"])
        .output()
        .unwrap();
    assert_eq!(doctor_repair.status.code(), Some(1), "{doctor_repair:?}");
    assert!(
        String::from_utf8_lossy(&doctor_repair.stderr).contains("no implemented repair"),
        "{doctor_repair:?}"
    );

    std::fs::write(repo.join("f.txt"), "hi").unwrap();
    let commit_long_message = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args([
            "commit",
            "--ref",
            "heads/main",
            "--message",
            "via --message",
        ])
        .output()
        .unwrap();
    support::ok(&commit_long_message, "commit --message");
}
