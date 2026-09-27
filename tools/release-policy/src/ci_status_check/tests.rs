//! RFC 160 §9 D3 controls.

use super::{Run, evaluate};

fn run(id: u64, status: &str, conclusion: Option<&str>) -> Run {
    Run {
        database_id: id,
        status: status.to_string(),
        conclusion: conclusion.map(str::to_string),
        url: format!("https://github.com/example/example/actions/runs/{id}"),
    }
}

/// **The control the handoff names**: a green run passes, a red one refuses. Both fixtures below reproduce the shape of the two
/// real commits the round measured against (`85d699af…`, CI run `36204467050`, `conclusion: "failure"`; `bb81b0fb…`, CI run
/// `36280930300`, `conclusion: "success"`) -- the exact numbers are in the round's report, since a unit test must not depend on
/// live network access or the account's `gh` auth.
#[test]
fn a_failed_ci_run_refuses_and_a_succeeded_one_passes() {
    let red = [run(36204467050, "completed", Some("failure"))];
    assert!(
        evaluate(&red, "85d699af").is_err(),
        "a failed CI run refuses the release"
    );

    let green = [run(36280930300, "completed", Some("success"))];
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
    let runs = [run(1, "in_progress", None)];
    assert!(evaluate(&runs, "abc123").is_err());
}

/// **The most recent run is the one that counts.** An earlier failed run followed by a later successful one (a re-run, or a push
/// that fixed a flake) passes; the reverse (an earlier success, then a later failure) refuses.
#[test]
fn the_most_recent_run_by_database_id_is_the_one_evaluated() {
    let fixed = [
        run(1, "completed", Some("failure")),
        run(2, "completed", Some("success")),
    ];
    assert!(
        evaluate(&fixed, "abc123").is_ok(),
        "the later, successful run wins"
    );

    let regressed = [
        run(1, "completed", Some("success")),
        run(2, "completed", Some("failure")),
    ];
    assert!(
        evaluate(&regressed, "abc123").is_err(),
        "the later, failed run wins"
    );
}
