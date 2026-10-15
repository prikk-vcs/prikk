//! 0.53.0 step 1, item 3 (RFC 147 §2i): `checkout`'s four (now six, since the shared
//! `resolve_point` path also covers `--patch-delete-plan`/`--patch-materialize-delete`) refusing
//! modes stop saying a branch with queued, unsealed commits "does not exist" -- it exists, it just
//! has nothing sealed yet. `--plan-only` already answered this correctly before this round; its own
//! note now says "seal", not "publish a ref".
//!
//! **Three shapes that must stay exactly as they were, each a real regression this file's own
//! controls caught while this change was being written, not assumed safe**:
//! - a genuinely absent ref (a typo, never created) still says "does not exist" (`control_typo`);
//! - a destination that received a `remotes/` twin of the same local name, but never created the
//!   local branch at all, still gets the more specific received-ref-aware refusal, not this
//!   round's own generic wording (`control_received_twin`);
//! - a *truly* fresh repository (zero commits ever) still says "does not exist" for the six
//!   refusing modes -- RFC 147 §2i's own prior Addendum 3 ruling, not this round's own item to
//!   revisit (`control_zero_queued_stays_absent`).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Output;

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn ok(repo: &Path, args: &[&str]) -> Output {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    output
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const REFUSING_MODES: &[&str] = &[
    "--snapshot-plan",
    "--snapshot-materialize",
    "--patch-plan",
    "--patch-materialize",
    "--patch-delete-plan",
    "--patch-materialize-delete",
];

/// The new shared wording, for a branch with exactly one queued commit and nothing sealed.
#[test]
fn an_unpublished_branch_with_one_queued_commit_names_the_count_instead_of_does_not_exist() {
    let repo = support::unique_repo("csw-one-queued");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    ok(&repo, &["commit", "-m", "a"]);

    for mode in REFUSING_MODES {
        let output = run(&repo, &["checkout", mode, "--ref", "heads/main"]);
        assert!(
            !output.status.success(),
            "{mode} must still refuse: {}",
            text(&output)
        );
        assert_eq!(output.status.code(), Some(1), "{mode}: {}", text(&output));
        let message = text(&output);
        assert!(
            message.contains("heads/main has no sealed block yet (1 commit queued)"),
            "{mode}: {message}"
        );
        assert!(
            message.contains("`prikk seal` publishes it"),
            "{mode}: {message}"
        );
        assert!(
            !message.contains("does not exist"),
            "{mode} must not say \"does not exist\" for a branch that genuinely exists: {message}"
        );
    }

    // The implicit (no `--ref`) form answers identically, the same byte-for-byte expectation
    // RFC 147 §2i's own prior round already established for `--plan-only`.
    let explicit = text(&run(
        &repo,
        &["checkout", "--patch-plan", "--ref", "heads/main"],
    ));
    let implicit = text(&run(&repo, &["checkout", "--patch-plan"]));
    assert_eq!(
        explicit, implicit,
        "explicit --ref must match the implicit default"
    );
}

/// Two queued commits: the count is exact, and pluralized correctly ("commits", not "commit").
#[test]
fn an_unpublished_branch_with_two_queued_commits_pluralizes_the_count() {
    let repo = support::unique_repo("csw-two-queued");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    ok(&repo, &["commit", "-m", "a"]);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    ok(&repo, &["commit", "-m", "b"]);

    let message = text(&run(
        &repo,
        &["checkout", "--patch-plan", "--ref", "heads/main"],
    ));
    assert!(
        message.contains("heads/main has no sealed block yet (2 commits queued)"),
        "{message}"
    );
}

/// `--plan-only` keeps exiting 0 and keeps its own existing classification; only its note's own
/// wording changes, to use the user's own word, "seal".
#[test]
fn plan_only_keeps_exiting_zero_with_the_seal_wording() {
    let repo = support::unique_repo("csw-plan-only");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    ok(&repo, &["commit", "-m", "a"]);

    let output = run(&repo, &["checkout", "--plan-only", "--ref", "heads/main"]);
    support::ok(&output, "checkout --plan-only --ref heads/main");
    let stdout = text(&output);
    assert!(
        stdout.contains("materialization: unpublished-ref"),
        "{stdout}"
    );
    assert!(
        stdout.contains("note: `prikk seal` publishes a ref before checkout can target a block"),
        "{stdout}"
    );
}

/// Control: a genuinely absent ref (a typo, never created at all) still says "does not exist" --
/// this round's own new check must not fire for a name that was never real to begin with.
#[test]
fn control_typo_still_says_does_not_exist() {
    let repo = support::unique_repo("csw-typo");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    ok(&repo, &["commit", "-m", "a"]);

    for mode in REFUSING_MODES {
        let output = run(&repo, &["checkout", mode, "--ref", "heads/nope"]);
        assert!(!output.status.success(), "{mode}");
        assert_eq!(output.status.code(), Some(1), "{mode}");
        let message = text(&output);
        assert!(
            message.contains("ref heads/nope does not exist in this repository"),
            "{mode}: {message}"
        );
    }
}

/// Control: a destination holding a `remotes/heads/main` twin, with a *local, queued* commit also
/// made against `heads/main` (so `queued_commit_count` alone would be nonzero -- this is the part
/// a weaker version of this control missed), must still get the more specific received-ref-aware
/// refusal (`ref_resolution.rs::decide_ref_existence`'s own existing text), never this round's own
/// generic "no sealed block yet" wording -- a real regression an earlier draft of this change had,
/// caught by running `rfc132_absent_and_received_refs.rs`'s own
/// `control2_every_command_a_refusal_names_runs`.
#[test]
fn control_received_twin_keeps_the_more_specific_refusal() {
    let src = support::unique_repo("csw-twin-src");
    support::init(&src);
    support::generation(&src, "heads/main", "a.txt", b"a\n", "genesis");
    let bundle = src.join("main.bundle");
    ok(
        &src,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );

    let dst = support::unique_repo("csw-twin-dst");
    ok(&dst, &["init"]);
    ok(
        &dst,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    // A local, queued commit against heads/main in dst -- the received remotes/heads/main twin
    // stays untouched alongside it, so this exercises the case where `queued_commit_count` alone
    // is nonzero and the received-pointer guard is the only thing left distinguishing the two
    // refusals.
    std::fs::write(dst.join("c.txt"), "c\n").unwrap();
    support::ok(
        &support::commit(&dst, "heads/main", "local work"),
        "commit: local work",
    );

    let output = run(&dst, &["checkout", "--patch-plan", "--ref", "heads/main"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let message = text(&output);
    assert!(
        message.contains(
            "ref heads/main does not exist in this repository; remotes/heads/main exists"
        ),
        "{message}"
    );
    assert!(
        !message.contains("has no sealed block yet"),
        "the received-ref-aware refusal must win, not this round's own generic wording: {message}"
    );
}

/// Control: a truly fresh repository (zero commits ever) still says "does not exist" for the six
/// refusing modes -- RFC 147 §2i's own prior Addendum 3 ruling, which this round does not revisit.
#[test]
fn control_zero_queued_stays_absent() {
    let repo = support::unique_repo("csw-zero-queued");
    ok(&repo, &["init"]);

    for mode in REFUSING_MODES {
        let output = run(&repo, &["checkout", mode, "--ref", "heads/main"]);
        assert!(!output.status.success(), "{mode}");
        let message = text(&output);
        assert!(
            message.contains("ref heads/main does not exist in this repository"),
            "{mode}: {message}"
        );
    }

    // `--plan-only` on the same zero-commit repository still answers non-refusing, as Addendum 3
    // already established, now with the "seal" wording.
    let output = run(&repo, &["checkout", "--plan-only", "--ref", "heads/main"]);
    support::ok(
        &output,
        "checkout --plan-only --ref heads/main (zero queued)",
    );
    assert!(
        text(&output).contains("materialization: unpublished-ref"),
        "{}",
        text(&output)
    );
}
