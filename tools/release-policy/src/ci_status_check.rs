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

use std::process::Command;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Deserialize)]
pub(crate) struct Run {
    #[serde(rename = "databaseId")]
    database_id: u64,
    status: String,
    conclusion: Option<String>,
    url: String,
}

/// Run `gh run list --repo <repo> --commit <sha> --workflow ci.yml --json databaseId,status,conclusion,url --limit 20` and return
/// its parsed rows. A separate function from [`check`] so a test can feed it fixed JSON instead of shelling out.
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
            "databaseId,status,conclusion,url",
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

/// The verdict: the most recent CI run's `(status, conclusion, url)` when at least one run exists for the commit, `None` when
/// none does (which is refused the same as a run that did not succeed -- a release must not be cut from a commit CI never ran
/// against).
fn latest(runs: &[Run]) -> Option<&Run> {
    runs.iter().max_by_key(|run| run.database_id)
}

pub(crate) fn evaluate(runs: &[Run], sha: &str) -> Result<()> {
    let Some(run) = latest(runs) else {
        return Err(Error::new(format!(
            "no CI workflow run found for commit {sha} -- a release must not be cut from a commit CI has not run against"
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
