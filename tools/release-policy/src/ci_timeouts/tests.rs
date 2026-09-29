#![allow(clippy::expect_used, clippy::unwrap_used)]

//! RFC 160 §9 R4 controls.

use super::{JobEntry, check_all, check_file, parse_jobs};

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

fn job(key: &str, has_timeout: bool, local_workflow_call: Option<&str>) -> JobEntry {
    JobEntry {
        key: key.to_string(),
        has_timeout,
        local_workflow_call: local_workflow_call.map(str::to_string),
    }
}

/// **The parser, checked against known-bad and known-good text**, so "no missing jobs" cannot come from a parser that stopped
/// matching. Covers a matrix job (`runs-on: ${{ matrix.os }}`), a job with no steps yet, a job whose `timeout-minutes` line
/// comes after several other keys, a job with a step-level `uses:` (which must NOT be read as a job-level call -- only a
/// job-level `uses:` is), a local reusable-workflow call (letter 015 N4's shape -- recorded as a candidate exemption, not
/// resolved here since `parse_jobs` sees only one file's text), and a call to another repository's workflow (never a
/// candidate exemption at all, part 1 review §3 item 1).
#[test]
fn parse_jobs_finds_every_job_and_records_a_local_workflow_call() {
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
      - uses: actions/checkout@v7
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

  calls-a-local-reusable-workflow:
    uses: ./.github/workflows/some-other.yml
    permissions:
      contents: read

  calls-a-remote-reusable-workflow:
    uses: owner/repo/.github/workflows/x.yml@v1
    permissions:
      contents: read
"#;
    let jobs = parse_jobs(text);
    assert_eq!(
        jobs,
        vec![
            job("has-timeout", true, None),
            job("missing-timeout", false, None),
            job("matrix-job", true, None),
            job("timeout-comes-late", true, None),
            job(
                "calls-a-local-reusable-workflow",
                false,
                Some("some-other.yml")
            ),
            job("calls-a-remote-reusable-workflow", false, None),
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
    let report =
        check_file(&path, &std::collections::HashSet::new()).expect("reading the perturbed file");
    assert_eq!(report.jobs_without_timeout, vec!["stable".to_string()]);
    let _ = std::fs::remove_dir_all(dir);
}

/// **Part 1 review, §3 item 1, control 1: a call to another repository's workflow is never exempt**, whatever `uses:` says --
/// this scan cannot see that workflow's own jobs, so there is no local file whose own `timeout-minutes` could stand in for
/// this job's. Reported as missing even when a same-named local file happens to exist (the workflow-file-names set passed
/// in is irrelevant here: the value never matched `LOCAL_WORKFLOW_PREFIX` in the first place).
#[test]
fn a_remote_workflow_call_stays_reported_as_missing() {
    let text = r#"
name: example

jobs:
  calls-elsewhere:
    uses: owner/repo/.github/workflows/x.yml@v1
"#;
    let dir = std::env::temp_dir().join(format!(
        "prikk-ci-timeouts-remote-call-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("caller.yml");
    std::fs::write(&path, text).unwrap();
    let present: std::collections::HashSet<String> = ["x.yml".to_string()].into_iter().collect();
    let report = check_file(&path, &present).expect("reading the file");
    assert_eq!(
        report.jobs_without_timeout,
        vec!["calls-elsewhere".to_string()]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **Part 1 review, §3 item 1, control 2: a local `uses:` naming a file that is not actually there stays reported too** -- a
/// typo or a since-deleted workflow file must not silently exempt the job that calls it.
#[test]
fn a_local_workflow_call_naming_a_missing_file_stays_reported() {
    let text = r#"
name: example

jobs:
  calls-a-typo:
    uses: ./.github/workflows/does-not-exist.yml
"#;
    let dir = std::env::temp_dir().join(format!(
        "prikk-ci-timeouts-missing-local-file-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("caller.yml");
    std::fs::write(&path, text).unwrap();
    // The scanned directory holds only `caller.yml` itself -- `does-not-exist.yml` is not among the names checked.
    let present: std::collections::HashSet<String> =
        ["caller.yml".to_string()].into_iter().collect();
    let report = check_file(&path, &present).expect("reading the file");
    assert_eq!(
        report.jobs_without_timeout,
        vec!["calls-a-typo".to_string()]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **The positive case, for contrast: a local `uses:` naming a file that IS present is exempt.**
#[test]
fn a_local_workflow_call_naming_a_present_file_is_exempt() {
    let text = r#"
name: example

jobs:
  calls-the-gate:
    uses: ./.github/workflows/ci-status-gate.yml
"#;
    let dir = std::env::temp_dir().join(format!(
        "prikk-ci-timeouts-present-local-file-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("caller.yml");
    std::fs::write(&path, text).unwrap();
    let present: std::collections::HashSet<String> =
        ["caller.yml".to_string(), "ci-status-gate.yml".to_string()]
            .into_iter()
            .collect();
    let report = check_file(&path, &present).expect("reading the file");
    assert!(
        report.jobs_without_timeout.is_empty(),
        "expected no missing jobs, got {:?}",
        report.jobs_without_timeout
    );
    let _ = std::fs::remove_dir_all(dir);
}
