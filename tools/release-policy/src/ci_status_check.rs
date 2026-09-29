//! **RFC 160 §9 D3 — a release is mechanically tied to a green CI run.**
//!
//! Before this round, `release.yml` fired on any matching tag, ran no tests of its own, and never asked whether the commit it was
//! releasing had passed CI at all. This is `cargo run -p prikk-release-policy -- ci-status-check <repo> <sha>`: the whole check as
//! one simple command line, so `release.yml`'s own `run:` step stays a single command the workflow's command-scan gate (which
//! cannot parse real shell control flow, `if`/`exit`/pipes) can classify, exactly like `release-notes` and `generate-installer`
//! before it. It finds the most recent `ci.yml` workflow run for the given commit and requires it to have **completed**
//! successfully -- which it can only do if every job in that run succeeded, the Windows and macOS mutation suites included, since
//! one failing job makes the whole run's own conclusion non-`success`. Shells out to `gh run list` (read-only; the workflow step
//! passes its own `GITHUB_TOKEN` as `GH_TOKEN`), the same way `size.rs` already shells out to `git`.
//!
//! **Letter 015, N4 — a tag push starts its own CI run of the same commit, and the first version of this gate could pick it.**
//! `ci.yml` triggers on every `push`, and a tag push is a push: 0.47.0's own commit (`21895f46`) has two runs, one on `main`
//! (`36078094314`) and one on the tag (`36079577518`), created in the same second the release workflow fired. The original
//! `latest()` took the highest run id over all of them, which on release day is that tag run, still `queued` -- the gate refused
//! the release it was meant to allow. The fix: only a run whose `event` is `push` and whose `headBranch` is `main` counts as this
//! commit's own CI. A tag push's run of the same commit is a different, later, and at release time irrelevant run of it.

use std::process::Command;

use serde::Deserialize;

use crate::error::{Error, Result};

/// The branch a release is cut from, and the only `headBranch` whose `push` run counts as a commit's own CI (letter 015, N4).
const RELEASE_BRANCH: &str = "main";

#[derive(Debug, Deserialize)]
pub(crate) struct Run {
    #[serde(rename = "databaseId")]
    database_id: u64,
    status: String,
    conclusion: Option<String>,
    url: String,
    event: String,
    #[serde(rename = "headBranch")]
    head_branch: String,
}

/// Run `gh run list --repo <repo> --commit <sha> --workflow ci.yml --json databaseId,status,conclusion,url,event,headBranch
/// --limit 20` and return its parsed rows. A separate function from [`check`] so a test can feed it fixed JSON instead of
/// shelling out. `event` and `headBranch` travel from `gh` itself -- filtering happens in Rust, in [`runs_on_release_branch`],
/// not by asking `gh` for fewer rows, so a test can see and exercise the filter directly.
fn list_ci_runs(repo: &str, sha: &str) -> Result<Vec<Run>> {
    let output = Command::new("gh")
        .args([
            "run",
            "list",
            "--repo",
            repo,
            "--commit",
            sha,
            "--workflow",
            "ci.yml",
            "--json",
            "databaseId,status,conclusion,url,event,headBranch",
            "--limit",
            "20",
        ])
        .output()
        .map_err(|error| Error::new(format!("spawning `gh run list`: {error}")))?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "`gh run list` failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| Error::new(format!("parsing `gh run list` output: {error}")))
}

/// Runs whose `event` is `push` and whose `headBranch` is [`RELEASE_BRANCH`] -- this commit's own CI, not a tag push's separate
/// run of the same commit (letter 015, N4: a tag push is a push, and `ci.yml` fires on every push).
fn runs_on_release_branch(runs: &[Run]) -> Vec<&Run> {
    runs.iter()
        .filter(|run| run.event == "push" && run.head_branch == RELEASE_BRANCH)
        .collect()
}

/// The verdict: the most recent `main`-push CI run's `(status, conclusion, url)` when at least one exists for the commit, `None`
/// when none does (which is refused the same as a run that did not succeed -- a release must not be cut from a commit that never
/// went through `main`'s own CI, whatever other runs of it exist).
fn latest_on_release_branch(runs: &[Run]) -> Option<&Run> {
    runs_on_release_branch(runs)
        .into_iter()
        .max_by_key(|run| run.database_id)
}

pub(crate) fn evaluate(runs: &[Run], sha: &str) -> Result<()> {
    let Some(run) = latest_on_release_branch(runs) else {
        return Err(Error::new(format!(
            "no CI workflow run on {RELEASE_BRANCH} found for commit {sha} -- a tag push starts its own, separate CI run of the \
             same commit, which does not count; that commit never went through {RELEASE_BRANCH}'s own CI"
        )));
    };
    println!(
        "most recent CI run for {sha}: {} (status={}, conclusion={})",
        run.url,
        run.status,
        run.conclusion.as_deref().unwrap_or("(none)")
    );
    if run.status != "completed" || run.conclusion.as_deref() != Some("success") {
        return Err(Error::new(format!(
            "CI run {} for commit {sha} is {}/{}, not a completed success -- refusing to release",
            run.url,
            run.status,
            run.conclusion.as_deref().unwrap_or("(none)")
        )));
    }
    println!(
        "CI run {} succeeded for {sha} -- release may proceed",
        run.url
    );
    Ok(())
}

/// `ci-status-check <repo> <sha>`: exits non-zero (via the returned `Err`) unless the most recent `ci.yml` run for `sha` in `repo`
/// completed successfully.
pub(crate) fn run(repo: &str, sha: &str) -> Result<()> {
    let runs = list_ci_runs(repo, sha)?;
    evaluate(&runs, sha)
}

#[cfg(test)]
mod tests;
