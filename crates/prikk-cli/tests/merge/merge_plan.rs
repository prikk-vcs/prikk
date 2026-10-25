//! CLI regression tests for the read-only `merge-plan` command.

// DC-84: `merge_evidence_support` now pulls in `tests/support/mod.rs` for `unique_suffix()`, and
// that shared file's own (pre-existing, unrelated) helpers use `.unwrap()` throughout — matching
// every other prikk-cli integration test file that already carries this allow.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "../merge_evidence_support/mod.rs"]
mod merge_evidence_support;

use merge_evidence_support::*;

#[test]
fn merge_plan_ref_targets_are_confluent_subset_and_read_only() -> TestResult {
    let repo = unique_repo("merge-plan-ref-targets")?;
    let baseline = init_with_sealed_genesis(&repo)?;
    let before = snapshot_files(&repo)?;

    let out = prikk(&repo)
        .args([
            "merge-plan",
            "--baseline-block",
            &baseline,
            "--left-ref",
            "heads/main",
            "--right-ref",
            "heads/main",
        ])
        .output()?;
    ok(&out, "merge-plan")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("merge plan\n"), "stdout: {stdout}");
    assert!(
        stdout.contains("status: ConfluentSubset"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("evidence outcome: Confluent"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("reason: proven_confluent"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("action: review the evidence, then run 'prikk merge' to execute"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("left selector: ref heads/main"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("right selector: ref heads/main"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("items: 1 displayed of 1"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("no merge commit, ref update, WAL write, object write, or worktree change"),
        "stdout: {stdout}"
    );
    assert_eq!(snapshot_files(&repo)?, before);
    let _ = std::fs::remove_dir_all(repo);
    Ok(())
}

#[test]
fn merge_plan_conflict_is_successful_blocked_plan() -> TestResult {
    let repo = unique_repo("merge-plan-conflict")?;
    let (baseline, left, right) = write_conflict_fixture(&repo)?;

    let out = prikk(&repo)
        .args([
            "merge-plan",
            "--baseline-block",
            &baseline,
            "--left-block",
            &left,
            "--right-block",
            &right,
        ])
        .output()?;
    ok(&out, "merge-plan conflict")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("status: BlockedConflict"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("evidence outcome: Conflict"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("reason: pair_conflict"), "stdout: {stdout}");
    assert!(
        stdout.contains("action: inspect evidence; conflict resolution is not implemented"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("cross:"), "stdout: {stdout}");
    assert!(
        stdout.contains("left[0] op_seq=1 ChangePerm"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("right[0] op_seq=1 ChangePerm"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("<->"),
        "stdout kept ambiguous cross-item renderer: {stdout}"
    );
    let _ = std::fs::remove_dir_all(repo);
    Ok(())
}

#[test]
fn merge_plan_missing_selector_is_command_failure() -> TestResult {
    let repo = unique_repo("merge-plan-missing-selector")?;
    let out = prikk(&repo).arg("init").output()?;
    ok(&out, "init")?;
    let out = prikk(&repo)
        .args(["merge-plan", "--baseline-block", &"0".repeat(64)])
        .output()?;
    fail(&out, "merge-plan missing selector")?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("merge-plan requires --left-block or --left-ref"),
        "stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(repo);
    Ok(())
}

#[test]
fn merge_plan_failure_path_is_read_only() -> TestResult {
    let repo = unique_repo("merge-plan-failure-read-only")?;
    let _baseline = init_with_sealed_genesis(&repo)?;
    let before = snapshot_files(&repo)?;

    let out = prikk(&repo)
        .args([
            "merge-plan",
            "--baseline-block",
            &"0".repeat(64),
            "--left-ref",
            "heads/main",
            "--right-ref",
            "heads/main",
        ])
        .output()?;
    fail(&out, "merge-plan missing baseline")?;
    assert_eq!(snapshot_files(&repo)?, before);
    let _ = std::fs::remove_dir_all(repo);
    Ok(())
}

#[test]
fn merge_plan_output_does_not_leak_file_content_or_host_paths() -> TestResult {
    let repo = unique_repo("merge-plan-privacy")?;
    let baseline = init_with_sealed_genesis(&repo)?;
    let secret = "MERGE_PLAN_SECRET_PAYLOAD_DO_NOT_PRINT";
    std::fs::write(repo.join("secret.txt"), format!("{secret}\n"))?;
    commit_worktree(&repo, "secret candidate")?;
    let _target = seal_current(&repo)?;

    let out = prikk(&repo)
        .args([
            "merge-plan",
            "--baseline-block",
            &baseline,
            "--left-ref",
            "heads/main",
            "--right-ref",
            "heads/main",
        ])
        .output()?;
    ok(&out, "merge-plan privacy")?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stdout.contains(secret), "stdout leaked content: {stdout}");
    assert!(!stderr.contains(secret), "stderr leaked content: {stderr}");
    let repo_path = repo.to_string_lossy();
    assert!(
        !stdout.contains(repo_path.as_ref()),
        "stdout leaked host path: {stdout}"
    );
    assert!(
        !stderr.contains(repo_path.as_ref()),
        "stderr leaked host path: {stderr}"
    );
    let _ = std::fs::remove_dir_all(repo);
    Ok(())
}
