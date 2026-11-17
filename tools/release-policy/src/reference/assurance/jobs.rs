//! The job map: which operating systems CI runs the test suites on, read from `ci.yml` and never typed into the gate.
//!
//! **What it reads.** A job is a *suite job* when one of its `run:` lines is exactly `cargo test --workspace --locked`, with or
//! without `--no-fail-fast`: no package, test, feature or name filter, so the whole workspace's tests run. The OS is the job's
//! `runs-on`, which must be a literal. **What it refuses (fails the gate, never assumes):** any other `cargo test` line (a
//! filter), a `matrix`/`strategy`, a job- or step-level `if:`, `continue-on-error`, or a `runs-on` that is not one of the three
//! hosted runners. A suite job is therefore always one whose suite is whole and whose result gates the merge; a new filter, a new
//! matrix, or a skipped job shows up as a gate failure, not as a silently narrower table.
//!
//! **One recognised exception.** A `cargo test` line that selects `--ignored` after `--` runs only `#[ignore]` tests (the macOS
//! `fsync` measurement is one). It adds no test to what runs, and the table cannot cite an ignored test, so it is skipped.

use std::collections::BTreeSet;

use super::Os;

/// The operating systems whose runner runs a whole, unfiltered `cargo test --workspace`.
pub(super) fn suite_oses(ci_yml: &str) -> Result<BTreeSet<Os>, String> {
    let mut oses = BTreeSet::new();
    for job in jobs(ci_yml) {
        let mut suite = false;
        for line in &job.lines {
            let Some(command) = test_command(line) else {
                continue;
            };
            match classify(&command) {
                Command::Suite => suite = true,
                Command::IgnoredOnly => {}
                Command::Unreadable => {
                    return Err(format!(
                        "job `{}` runs `{command}`, which the gate cannot read as a whole-workspace suite (a filter or an unknown form)",
                        job.key
                    ));
                }
            }
        }
        if !suite {
            continue;
        }
        for line in &job.lines {
            let trimmed = line.trim_start();
            let blocked = trimmed.starts_with("strategy:")
                || trimmed.starts_with("matrix:")
                || trimmed.starts_with("if:")
                || trimmed.starts_with("- if:")
                || trimmed.starts_with("continue-on-error:");
            if blocked {
                return Err(format!(
                    "suite job `{}` has `{}`: it may not run, or may not gate, so the gate cannot take its OS as a fact",
                    job.key,
                    trimmed.split(':').next().unwrap_or_default()
                ));
            }
        }
        let runs_on = job
            .lines
            .iter()
            .find_map(|line| line.strip_prefix("    runs-on:"))
            .map(str::trim);
        let os = match runs_on {
            Some("ubuntu-latest") => Os::Linux,
            Some("macos-latest") => Os::Macos,
            Some("windows-latest") => Os::Windows,
            other => {
                return Err(format!(
                    "suite job `{}` has `runs-on: {}`, not a literal hosted runner",
                    job.key,
                    other.unwrap_or("(none)")
                ));
            }
        };
        oses.insert(os);
    }
    if oses.is_empty() {
        return Err("no job runs `cargo test --workspace --locked`".to_owned());
    }
    Ok(oses)
}

enum Command {
    Suite,
    IgnoredOnly,
    Unreadable,
}

fn classify(command: &str) -> Command {
    let words: Vec<&str> = command.split_whitespace().collect();
    let suite = ["cargo", "test", "--workspace", "--locked"];
    if words == suite || words == [suite.as_slice(), &["--no-fail-fast"]].concat() {
        return Command::Suite;
    }
    let after_dashes = command.split(" -- ").nth(1).unwrap_or_default();
    if after_dashes
        .split_whitespace()
        .any(|word| word == "--ignored")
    {
        return Command::IgnoredOnly;
    }
    Command::Unreadable
}

/// The command on a line that runs a test suite (`cargo test`, `cargo nextest`, …), whatever its YAML prefix, or `None`.
fn test_command(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with('#') {
        return None;
    }
    let trimmed = trimmed.trim_start_matches("- ").trim_start();
    let command = trimmed.strip_prefix("run:").map_or(trimmed, str::trim);
    let runs_tests = command.starts_with("cargo test")
        || command.starts_with("cargo nextest")
        || command.starts_with("cargo t ")
        || command.starts_with("cargo llvm-cov")
        || (command.starts_with("cargo +") && command.contains(" test"));
    runs_tests.then(|| command.to_owned())
}

struct Job {
    key: String,
    lines: Vec<String>,
}

/// The `jobs:` block split into jobs by indentation (a two-space `key:`), the way `ci_timeouts::parse_jobs` splits it.
fn jobs(text: &str) -> Vec<Job> {
    let mut jobs: Vec<Job> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if let Some(rest) = line.strip_prefix("  ") {
            if !rest.starts_with(' ')
                && !rest.starts_with('#')
                && !rest.starts_with('-')
                && rest.trim_end().ends_with(':')
                && !rest.trim().is_empty()
            {
                jobs.push(Job {
                    key: rest.trim_end().trim_end_matches(':').to_owned(),
                    lines: Vec::new(),
                });
                continue;
            }
        }
        if let Some(last) = jobs.last_mut() {
            last.lines.push(line.to_owned());
        }
    }
    jobs
}
