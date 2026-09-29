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
//! **A job that calls a *local* reusable workflow (`uses: ./.github/workflows/<file>.yml` at the same four-space indent, in
//! place of `runs-on:`/`steps:`) is exempt, when that file is actually present in the scanned directory**: GitHub Actions
//! does not accept `timeout-minutes` on a call job at all -- the called workflow's own job carries it. Letter 015's N4 round
//! added `release.yml`'s `ci-status-gate` job in this shape; its budget lives in `ci-status-gate.yml`'s own job, which this
//! same scan (`check_all`, over every `*.yml` in the directory) checks directly, so the property this gate exists for --
//! nothing runs unbounded -- still holds for that job, just not on the calling line.
//!
//! **Part 1 review, §3 item 1: the exemption is scoped to a local call this scan can itself verify.** A job that calls
//! *another repository's* workflow (`uses: owner/repo/.github/workflows/x.yml@ref`) is not exempt -- this scan has no way
//! to check that workflow's own timeouts, so exempting it on `uses:` alone would let a job run unbounded with nobody
//! noticing (the exact "a guard that sees less than its name says" shape the external review named, D11). Nor is a local
//! `uses:` that names a file not actually present in the directory: a typo or a since-deleted file must not silently pass.
//! Only `./.github/workflows/<name>` naming a file this same `check_all` call also scans is exempt.

use std::path::{Path, PathBuf};

/// One workflow's jobs and which of them are missing `timeout-minutes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowTimeouts {
    pub(crate) file: String,
    pub(crate) jobs_without_timeout: Vec<String>,
    pub(crate) job_count: usize,
}

/// One job found under a workflow's `jobs:` key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobEntry {
    pub(crate) key: String,
    pub(crate) has_timeout: bool,
    /// `Some(name)` when this job's own `uses:` (job-level, not a step's) names a local workflow file
    /// (`./.github/workflows/<name>`) -- unresolved here, since `parse_jobs` sees only one file's text.
    /// `check_file`/`check_all` resolve it against what is actually present in the scanned directory.
    pub(crate) local_workflow_call: Option<String>,
}

/// The prefix a job-level `uses:` value must have to be a *local* reusable-workflow call, as opposed to one in another
/// repository (`owner/repo/.github/workflows/x.yml@ref`, never exempt: see the module doc).
const LOCAL_WORKFLOW_PREFIX: &str = "./.github/workflows/";

/// Parse one workflow file's `jobs:` block into [`JobEntry`] rows, by indentation.
pub(crate) fn parse_jobs(text: &str) -> Vec<JobEntry> {
    let mut jobs: Vec<JobEntry> = Vec::new();
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
                jobs.push(JobEntry {
                    key,
                    has_timeout: false,
                    local_workflow_call: None,
                });
                continue;
            }
        }
        if line.trim_start().starts_with("timeout-minutes:") {
            if let Some(last) = jobs.last_mut() {
                last.has_timeout = true;
            }
        }
        // A four-space-indented `uses:` is a call to a reusable workflow, in place of `runs-on:`/`steps:`. A step's own
        // `uses:` (inside `steps:`) is more deeply indented and a list item (`      - uses: ...`), so
        // `strip_prefix("    ")` alone (no further indent, no leading `-`) distinguishes the two the same way the
        // job-key check above does. Only a `./.github/workflows/` value is recorded as a candidate exemption; anything
        // else (another repository's workflow) is left `None` and so stays reported as missing.
        if let Some(rest) = line.strip_prefix("    ") {
            if !rest.starts_with(' ') {
                if let Some(value) = rest.strip_prefix("uses:") {
                    if let Some(last) = jobs.last_mut() {
                        let value = value.trim();
                        if let Some(name) = value.strip_prefix(LOCAL_WORKFLOW_PREFIX) {
                            last.local_workflow_call = Some(name.to_string());
                        }
                    }
                }
            }
        }
    }
    jobs
}

/// Whether `job` counts as satisfying the timeout requirement: it literally has `timeout-minutes`, or it calls a local
/// reusable workflow that `workflow_file_names` (every `*.yml`/`*.yaml` file name actually present in the scanned
/// directory) confirms exists.
fn satisfied(job: &JobEntry, workflow_file_names: &std::collections::HashSet<String>) -> bool {
    job.has_timeout
        || job
            .local_workflow_call
            .as_deref()
            .is_some_and(|name| workflow_file_names.contains(name))
}

/// Check one workflow file. `workflow_file_names` is every workflow file name present in the same directory (for
/// resolving a local `uses:` exemption); pass an empty set to treat every `uses:` as unresolved (reported as missing).
pub(crate) fn check_file(
    path: &Path,
    workflow_file_names: &std::collections::HashSet<String>,
) -> Result<WorkflowTimeouts, String> {
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
            .filter(|job| !satisfied(job, workflow_file_names))
            .map(|job| job.key.clone())
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
    let workflow_file_names: std::collections::HashSet<String> = paths
        .iter()
        .filter_map(|path| path.file_name())
        .filter_map(|name| name.to_str())
        .map(str::to_string)
        .collect();
    paths
        .iter()
        .map(|path| check_file(path, &workflow_file_names))
        .collect()
}

mod tests;
