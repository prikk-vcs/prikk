//! **RFC 160 §9 R4 — every CI job has `timeout-minutes`.**
//!
//! None of the 15 jobs across `ci.yml`, `docs.yml`, `docs-pr.yml`, `release.yml` and `security-audit.yml` set `timeout-minutes`
//! before this round, so GitHub's own default of six hours applied to every one of them: a job stuck in a loop of the shape R2/R3
//! exist to prevent would have run for hours before anyone noticed, on someone else's CI minutes. This is a **test gate**, not a
//! CLI subcommand -- it runs as part of `cargo test -p prikk-release-policy`, inside the same `test`/`msrvtest` gates the round
//! already runs, rather than adding a 15th named local gate to a workflow that has settled on 14.
//!
//! **What counts as a job**: a key one level under `jobs:` (two-space indent) in a workflow file -- found the same way a human reads
//! the YAML, by indentation, not by a full YAML parser (this crate has none as a dependency, and adding one for one gate was not
//! judged worth it). A job "has" `timeout-minutes` when a line reading exactly that (at four-space indent, directly under the job,
//! any value) appears before the next job key or end of file.
//!
//! **A job that calls a reusable workflow (`uses:` at the same four-space indent, in place of `runs-on:`/`steps:`) is exempt**:
//! GitHub Actions does not accept `timeout-minutes` on a call job at all -- the called workflow's own job carries it. Letter 015's
//! N4 round added `release.yml`'s `ci-status-gate` job in this shape; its budget lives in `ci-status-gate.yml`'s own job, which
//! this same scan (`check_all`, over every `*.yml` in the directory) checks directly, so the property this gate exists for --
//! nothing runs unbounded -- still holds for that job, just not on the calling line.

use std::path::{Path, PathBuf};

/// One workflow's jobs and which of them are missing `timeout-minutes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowTimeouts {
    pub(crate) file: String,
    pub(crate) jobs_without_timeout: Vec<String>,
    pub(crate) job_count: usize,
}

/// Parse one workflow file's `jobs:` block into `(job key, has timeout-minutes)` pairs, by indentation.
pub(crate) fn parse_jobs(text: &str) -> Vec<(String, bool)> {
    let mut jobs: Vec<(String, bool)> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        // A two-space-indented `key:` (not four, not a comment, not a list item) starts a new job.
        if let Some(rest) = line.strip_prefix("  ") {
            if !rest.starts_with(' ')
                && !rest.starts_with('#')
                && !rest.starts_with('-')
                && rest.trim_end().ends_with(':')
                && !rest.trim().is_empty()
            {
                let key = rest.trim_end().trim_end_matches(':').to_string();
                jobs.push((key, false));
                continue;
            }
        }
        if line.trim_start().starts_with("timeout-minutes:") {
            if let Some(last) = jobs.last_mut() {
                last.1 = true;
            }
        }
        // A four-space-indented `uses:` is a call to a reusable workflow, in place of `runs-on:`/`steps:` -- GitHub Actions
        // refuses `timeout-minutes` there outright, so this job is exempt (see the module doc). A step's own `uses:` (inside
        // `steps:`) is more deeply indented and a list item (`      - uses: ...`), so `strip_prefix("    ")` alone (no further
        // indent, no leading `-`) distinguishes the two the same way the job-key check above does.
        if let Some(rest) = line.strip_prefix("    ") {
            if !rest.starts_with(' ') && rest.starts_with("uses:") {
                if let Some(last) = jobs.last_mut() {
                    last.1 = true;
                }
            }
        }
    }
    jobs
}

/// Check one workflow file.
pub(crate) fn check_file(path: &Path) -> Result<WorkflowTimeouts, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let jobs = parse_jobs(&text);
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    Ok(WorkflowTimeouts {
        jobs_without_timeout: jobs
            .iter()
            .filter(|(_, has)| !has)
            .map(|(key, _)| key.clone())
            .collect(),
        job_count: jobs.len(),
        file,
    })
}

/// Check every workflow file in `workflows_dir` (`.github/workflows`). Returns one [`WorkflowTimeouts`] per `*.yml` file found,
/// sorted by file name, so the result is deterministic and a report can be generated from it directly.
pub(crate) fn check_all(workflows_dir: &Path) -> Result<Vec<WorkflowTimeouts>, String> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(workflows_dir)
        .map_err(|error| format!("{}: {error}", workflows_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("yml"))
        .collect();
    paths.sort();
    paths.iter().map(|path| check_file(path)).collect()
}

mod tests;
