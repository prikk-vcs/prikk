#![allow(clippy::expect_used, clippy::unwrap_used)]

//! RFC 160 §9 R4 controls.

use super::{check_all, check_file, parse_jobs};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root")
        .to_path_buf()
}

/// **The gate itself, on the real repository.** Every job in every workflow file has `timeout-minutes`.
/// **Perturb:** drop one job's `timeout-minutes` line in `ci.yml`: this test fails naming the file and the job (the next control
/// proves it, without touching the real file).
#[test]
fn every_ci_job_has_a_timeout() {
    let dir = repo_root().join(".github/workflows");
    let results = check_all(&dir).expect("reading the workflow files");
    assert!(
        results.len() >= 5,
        "expected at least the five known workflow files: {results:?}"
    );
    let total_jobs: usize = results.iter().map(|report| report.job_count).sum();
    assert!(
        total_jobs >= 18,
        "expected at least 18 jobs across every workflow, found {total_jobs}: {results:?}"
    );
    let missing: Vec<String> = results
        .iter()
        .flat_map(|report| {
            report
                .jobs_without_timeout
                .iter()
                .map(move |job| format!("{}: {job}", report.file))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "jobs with no timeout-minutes: {missing:#?}"
    );
}

/// **The parser, checked against known-bad and known-good text**, so "no missing jobs" cannot come from a parser that stopped
/// matching. Covers a matrix job (`runs-on: ${{ matrix.os }}`), a job with no steps yet, and a job whose `timeout-minutes` line
/// comes after several other keys.
#[test]
fn parse_jobs_finds_every_job_and_whether_it_has_a_timeout() {
    let text = r#"
name: example

jobs:
  has-timeout:
    name: has timeout
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: echo hi

  missing-timeout:
    name: missing timeout
    runs-on: ubuntu-latest
    steps:
      - run: echo hi

  matrix-job:
    strategy:
      matrix:
        os: [windows-latest, macos-latest]
    name: matrix
    runs-on: ${{ matrix.os }}
    timeout-minutes: 10
    steps:
      - run: echo hi

  timeout-comes-late:
    name: late
    needs: has-timeout
    permissions:
      contents: read
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - run: echo hi
"#;
    let jobs = parse_jobs(text);
    assert_eq!(
        jobs,
        vec![
            ("has-timeout".to_string(), true),
            ("missing-timeout".to_string(), false),
            ("matrix-job".to_string(), true),
            ("timeout-comes-late".to_string(), true),
        ]
    );
}

/// **A perturbed copy of the real `ci.yml`, with one job's `timeout-minutes` removed, is reported missing** -- proving the gate
/// itself would have caught the very thing R4 fixed, without touching the tracked file.
#[test]
fn a_workflow_missing_one_jobs_timeout_is_reported() {
    let real = repo_root().join(".github/workflows/ci.yml");
    let text = std::fs::read_to_string(&real).expect("reading ci.yml");
    let perturbed = text.replacen("    timeout-minutes: 25\n", "", 1);
    assert_ne!(
        perturbed, text,
        "fixture: the line to remove must exist in ci.yml"
    );
    let dir = std::env::temp_dir().join(format!(
        "prikk-ci-timeouts-gate-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ci.yml");
    std::fs::write(&path, &perturbed).unwrap();
    let report = check_file(&path).expect("reading the perturbed file");
    assert_eq!(report.jobs_without_timeout, vec!["stable".to_string()]);
    let _ = std::fs::remove_dir_all(dir);
}
