//! `prikk compact` end-to-end (RFC 102 Stage 6 Step 2, design-v1.md §15.7/§15.9): the CLI surface for
//! the compactor. Exercises target selection, `--all`, `--plan-only` (both its output and that it
//! never mutates), and the "no target" refusal -- everything `prikk_store::compact`'s own unit tests
//! don't cover, since those stop at the library boundary.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::Path;
use std::process::Output;

use support::{init, ok, prikk, unique_repo};

fn trust_a_key(repo: &Path) -> Output {
    prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "compact-test-key",
            "--public-key",
            &"07".repeat(32),
        ])
        .output()
        .unwrap()
}

#[test]
fn bare_compact_with_no_target_refuses_rather_than_defaulting_to_all() {
    let repo = unique_repo("compact-no-target");
    init(&repo);
    let output = prikk(&repo).arg("compact").output().unwrap();
    assert!(
        !output.status.success(),
        "a bare `compact` must refuse, not silently compact everything"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires a target"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn plan_only_reports_without_a_real_run_ever_being_needed_first() {
    let repo = unique_repo("compact-plan-only");
    init(&repo);
    let output = prikk(&repo)
        .args(["compact", "--pointer-index", "--plan-only"])
        .output()
        .unwrap();
    ok(&output, "compact --pointer-index --plan-only");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pointer-index:") && stdout.contains("would reclaim"),
        "stdout: {stdout}"
    );
}

#[test]
fn all_runs_every_target_and_a_real_run_reports_reclaimed_not_would_reclaim() {
    let repo = unique_repo("compact-all");
    init(&repo);
    ok(&trust_a_key(&repo), "trust maintainer add");

    let output = prikk(&repo).args(["compact", "--all"]).output().unwrap();
    ok(&output, "compact --all");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pointer-index:"), "stdout: {stdout}");
    assert!(stdout.contains("received-index:"), "stdout: {stdout}");
    assert!(stdout.contains("trust-policy:"), "stdout: {stdout}");
    assert!(
        stdout.contains("reclaimed") && !stdout.contains("would reclaim"),
        "a real run must say what it reclaimed, not what it would: {stdout}"
    );
}

/// The property `--plan-only` exists to prove: it never mutates, checked directly by running it twice
/// and confirming the second run reports the identical counts a real run would still see.
#[test]
fn plan_only_never_mutates_the_repository() {
    let repo = unique_repo("compact-plan-only-no-mutation");
    init(&repo);
    ok(&trust_a_key(&repo), "trust maintainer add");

    let first_plan = prikk(&repo)
        .args(["compact", "--trust-policy", "--plan-only"])
        .output()
        .unwrap();
    ok(&first_plan, "compact --trust-policy --plan-only (first)");

    let second_plan = prikk(&repo)
        .args(["compact", "--trust-policy", "--plan-only"])
        .output()
        .unwrap();
    ok(&second_plan, "compact --trust-policy --plan-only (second)");

    assert_eq!(
        first_plan.stdout, second_plan.stdout,
        "two consecutive plan-only runs over an unchanged repository must report identical counts"
    );

    let real = prikk(&repo)
        .args(["compact", "--trust-policy"])
        .output()
        .unwrap();
    ok(&real, "compact --trust-policy (real, after two previews)");
}
