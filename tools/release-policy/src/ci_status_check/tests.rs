#![allow(clippy::expect_used)]

//! RFC 160 §9 D3 controls, and letter 015's N4 fix: only a `push` run on `main` counts as a commit's own CI.

use super::{Run, evaluate};

fn run(id: u64, status: &str, conclusion: Option<&str>, event: &str, head_branch: &str) -> Run {
    Run {
        database_id: id,
        status: status.to_string(),
        conclusion: conclusion.map(str::to_string),
        url: format!("https://github.com/example/example/actions/runs/{id}"),
        event: event.to_string(),
        head_branch: head_branch.to_string(),
    }
}

/// **The control the handoff names**: a green run on `main` passes, a red one on `main` refuses. Both fixtures below reproduce
/// the shape of the two real commits the round measured against (`85d699af…`, CI run `36204467050`, `conclusion: "failure"`;
/// `bb81b0fb…`, CI run `36280930300`, `conclusion: "success"`) -- the exact numbers are in the round's report, since a unit test
/// must not depend on live network access or the account's `gh` auth.
#[test]
fn a_failed_ci_run_refuses_and_a_succeeded_one_passes() {
    let red = [run(
        36204467050,
        "completed",
        Some("failure"),
        "push",
        "main",
    )];
    assert!(
        evaluate(&red, "85d699af").is_err(),
        "a failed CI run refuses the release"
    );

    let green = [run(
        36280930300,
        "completed",
        Some("success"),
        "push",
        "main",
    )];
    assert!(
        evaluate(&green, "bb81b0fb").is_ok(),
        "a succeeded CI run allows the release"
    );
}

/// No run at all for the commit: refused, not treated as "nothing to check."
#[test]
fn no_ci_run_for_the_commit_refuses() {
    assert!(evaluate(&[], "0000000").is_err());
}

/// A run still in progress (`status` not yet `completed`) refuses, even with no `conclusion` yet.
#[test]
fn an_in_progress_run_refuses() {
    let runs = [run(1, "in_progress", None, "push", "main")];
    assert!(evaluate(&runs, "abc123").is_err());
}

/// **The most recent run, among the ones on `main`, is the one that counts.** An earlier failed run followed by a later
/// successful one (a re-run, or a push that fixed a flake) passes; the reverse (an earlier success, then a later failure)
/// refuses.
#[test]
fn the_most_recent_run_by_database_id_is_the_one_evaluated() {
    let fixed = [
        run(1, "completed", Some("failure"), "push", "main"),
        run(2, "completed", Some("success"), "push", "main"),
    ];
    assert!(
        evaluate(&fixed, "abc123").is_ok(),
        "the later, successful run wins"
    );

    let regressed = [
        run(1, "completed", Some("success"), "push", "main"),
        run(2, "completed", Some("failure"), "push", "main"),
    ];
    assert!(
        evaluate(&regressed, "abc123").is_err(),
        "the later, failed run wins"
    );
}

/// **Letter 015, N4 -- rows modelled on 0.47.0's own real GitHub record.** The tag's own run of the same commit
/// (`event: push`, `headBranch: "0.47.0"`) is created in the same second the release workflow fires and is still `queued` when
/// the gate runs; it must never be the run evaluated.
#[test]
fn the_tags_own_run_of_the_same_commit_is_ignored_when_mains_run_succeeded() {
    let runs = [
        run(36078094314, "completed", Some("success"), "push", "main"),
        run(36079577518, "queued", None, "push", "0.47.0"),
    ];
    assert!(
        evaluate(&runs, "21895f46").is_ok(),
        "main's own completed, successful run is what counts, even though the tag's later run is still queued"
    );
}

#[test]
fn a_failed_main_run_refuses_even_with_a_successful_tag_run_of_the_same_commit() {
    let runs = [
        run(1, "completed", Some("failure"), "push", "main"),
        run(2, "completed", Some("success"), "push", "0.47.0"),
    ];
    assert!(
        evaluate(&runs, "21895f46").is_err(),
        "the tag's own run does not rescue a failed main run"
    );
}

#[test]
fn only_a_tag_run_refuses() {
    let runs = [run(1, "completed", Some("success"), "push", "0.47.0")];
    assert!(
        evaluate(&runs, "21895f46").is_err(),
        "a commit that never went through main's own CI is refused, whatever its tag run says"
    );
}

#[test]
fn two_main_runs_the_newer_failed_refuses() {
    let runs = [
        run(1, "completed", Some("success"), "push", "main"),
        run(2, "completed", Some("failure"), "push", "main"),
    ];
    assert!(evaluate(&runs, "21895f46").is_err());
}

/// **Control (letter 015, N4): remove the filter, and the first of the four rows above goes red.** Reproducing the unfiltered
/// selection this gate used before the fix -- the highest run id over every run of the commit, `push` or not, `main` or not --
/// on the exact same fixture as `the_tags_own_run_of_the_same_commit_is_ignored_when_mains_run_succeeded` picks the tag's own
/// run, `36079577518`, not `main`'s `36078094314`; and that run is `queued`, not `completed`. This is the failure the external
/// architect's review reproduced from GitHub's real record for 0.47.0, and it is why the fix filters in Rust rather than only
/// trusting `gh`'s own ordering.
#[test]
fn without_the_branch_and_event_filter_the_tags_own_still_queued_run_is_picked_instead() {
    let runs = [
        run(36078094314, "completed", Some("success"), "push", "main"),
        run(36079577518, "queued", None, "push", "0.47.0"),
    ];
    let unfiltered_pick = runs
        .iter()
        .max_by_key(|run| run.database_id)
        .expect("a run exists");
    assert_eq!(
        unfiltered_pick.database_id, 36079577518,
        "without the filter, the highest run id is the tag's own run, not main's"
    );
    assert_ne!(
        unfiltered_pick.status, "completed",
        "and that run is still queued -- the pre-fix gate would have refused a release day it should have allowed"
    );
}
