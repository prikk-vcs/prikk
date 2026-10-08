//! 0.50.0 step 1, A6 item 4 (019 §5.10): `--repair-tails` is the one repair that may cut across ten
//! files in one run, and the last of the five recovery verbs to gain `--plan-only`. The plan it
//! prints must be exactly what a real run then does, and must write nothing itself.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

#[test]
fn plan_only_writes_nothing_and_matches_the_real_runs_own_report() {
    let repo = support::unique_repo("a6-item4-repair-tails-plan-only");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");

    let wal = repo.join(".prikk/active/default/queue.wal");
    let mut bytes = std::fs::read(&wal).unwrap();
    bytes.extend_from_slice(b"PWALR00\x01\x02\x03\x04\x05");
    std::fs::write(&wal, &bytes).unwrap();

    let before = support::store_bytes(&repo);

    let mut plan_cmd = support::prikk(&repo);
    let plan_output = plan_cmd
        .args(["doctor", "--repair-tails", "--plan-only"])
        .output()
        .unwrap();
    assert!(plan_output.status.success(), "{plan_output:?}");
    let plan_text = String::from_utf8_lossy(&plan_output.stdout).to_string();
    // 0.50.0 P2b, F1: a plan has not truncated anything yet -- "WAL: truncated ..." in plan mode
    // read as a claim the file was already cut, with the correction ("plan only -- nothing
    // written") arriving only afterward. The plan must say "would truncate" instead.
    assert!(
        plan_text.contains("WAL: would truncate 12 trailing byte(s)"),
        "{plan_text}"
    );
    assert!(
        !plan_text.contains("WAL: truncated"),
        "the plan must never say the past tense: {plan_text}"
    );
    assert!(
        plan_text.contains("plan only -- nothing written"),
        "{plan_text}"
    );

    let after_plan = support::store_bytes(&repo);
    assert_eq!(
        before, after_plan,
        "a plan-only run must write nothing at all"
    );

    let mut real_cmd = support::prikk(&repo);
    let real_output = real_cmd
        .args(["doctor", "--repair-tails"])
        .output()
        .unwrap();
    assert!(real_output.status.success(), "{real_output:?}");
    let real_text = String::from_utf8_lossy(&real_output.stdout).to_string();
    assert!(
        real_text.contains("WAL: truncated 12 trailing byte(s)"),
        "the real run must truncate exactly what the plan named: {real_text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control: before this fix, `--plan-only` combined with `--repair-tails` was rejected by argument
/// parsing entirely (not in the closed list of five verbs that accepted it) -- confirmed by this
/// test passing now and by the five-verb error message naming `--repair-tails` explicitly.
#[test]
fn plan_only_is_now_in_the_accepted_list_for_repair_tails() {
    let repo = support::unique_repo("a6-item4-repair-tails-plan-only-accepted");
    support::init(&repo);
    let mut cmd = support::prikk(&repo);
    let output = cmd
        .args(["doctor", "--repair-tails", "--plan-only"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        !text.contains("--plan-only is only accepted alongside"),
        "repair-tails must now be in the accepted list: {text}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
