//! RFC 168 §3.1–§3.2, end to end: the recovery log, its three `doctor` commands, and `verify`'s own log line.
//!
//! These run the real binary on real repositories and check the text a user reads, because RFC 168 is about what a person
//! can do with the saved bytes, and the words are the contract (RFC 168 §3.2 quotes them).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

/// The same as [`run`], returning the whole output for `support::ok`.
fn run_in(repo: &Path, args: &[&str]) -> std::process::Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn run(repo: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = support::prikk(repo).args(args).output().unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// A repository with two queued commits and seven torn bytes after them: a WAL tail `doctor --repair-wal-tail` removes.
fn torn_wal(tag: &str) -> PathBuf {
    use std::io::Write;
    let repo = support::unique_repo(tag);
    support::init(&repo);
    for (index, name) in ["one.txt", "two.txt"].iter().enumerate() {
        std::fs::write(repo.join(name), format!("queued commit {index}\n")).unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("queued {index}")),
            "commit",
        );
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(repo.join(".prikk/active/default/queue.wal"))
        .unwrap();
    file.write_all(b"partial").unwrap();
    repo
}

fn wal(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/queue.wal")
}

/// The entry a repair names, from `recovery/log, entry <id>`.
fn named_entry(output: &str) -> String {
    let start = output
        .find("recovery/log, run ")
        .unwrap_or_else(|| panic!("the repair names its entry\n{output}"))
        + "recovery/log, run ".len();
    output[start..].chars().take(16).collect()
}

fn repair(repo: &Path) -> String {
    let (code, text) = run(repo, &["doctor", "--repair-wal-tail"]);
    assert_eq!(code, Some(0), "{text}");
    text
}

#[test]
fn an_empty_log_lists_no_entries_and_says_how_to_judge_one() {
    let repo = support::unique_repo("rfc168-list-empty");
    support::init(&repo);
    let (code, text) = run(&repo, &["doctor", "--recovery-list"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("recovery log: 0 entries in 0 runs in .prikk/recovery/log"),
        "{text}"
    );
    assert!(
        text.trim_end().ends_with(
            "a listing judges no entry; `prikk doctor --recovery-restore <id> --plan-only` says whether one can be restored"
        ),
        "the listing's last line says how an entry is judged:\n{text}"
    );
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn a_repair_is_listed_under_the_id_it_named() {
    let repo = torn_wal("rfc168-list-after-repair");
    let id = named_entry(&repair(&repo));
    let (code, text) = run(&repo, &["doctor", "--recovery-list"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("recovery log: 1 entry in 1 run in .prikk/recovery/log"),
        "{text}"
    );
    assert!(text.contains(&id), "{text}");
    assert!(text.contains("active/default/queue.wal"), "{text}");
    assert!(text.contains("7 bytes"), "{text}");
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn a_restore_plan_names_each_condition_and_writes_nothing() {
    let repo = torn_wal("rfc168-plan-only");
    let id = named_entry(&repair(&repo));
    let after_repair = std::fs::read(wal(&repo)).unwrap();
    let (code, text) = run(&repo, &["doctor", "--recovery-restore", &id, "--plan-only"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("the source is 714 bytes, the length the repair left"),
        "{text}"
    );
    assert!(
        text.contains("the bytes before the offset are the bytes the repair left"),
        "{text}"
    );
    assert!(
        text.contains("active/default/ref-name is unchanged since the repair"),
        "{text}"
    );
    assert!(text.contains("plan only -- nothing written"), "{text}");
    assert!(
        text.contains(
            "After this, the file holds what it held before the repair, damage included."
        ),
        "the plan says what follows:\n{text}"
    );
    assert_eq!(
        std::fs::read(wal(&repo)).unwrap(),
        after_repair,
        "a plan writes nothing"
    );
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn a_restore_writes_the_removed_bytes_back_at_their_offset() {
    let repo = torn_wal("rfc168-restore");
    let before_repair = std::fs::read(wal(&repo)).unwrap();
    let id = named_entry(&repair(&repo));
    assert_ne!(
        std::fs::read(wal(&repo)).unwrap(),
        before_repair,
        "the repair removed the tail"
    );
    let (code, text) = run(&repo, &["doctor", "--recovery-restore", &id]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("wrote 7 bytes in 1 step"), "{text}");
    assert_eq!(
        std::fs::read(wal(&repo)).unwrap(),
        before_repair,
        "byte-identical to before the repair"
    );
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn a_restore_is_refused_and_writes_nothing_when_the_wal_was_written_since() {
    use std::io::Write;
    let repo = torn_wal("rfc168-restore-refused");
    let id = named_entry(&repair(&repo));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(wal(&repo))
        .unwrap();
    file.write_all(b"x").unwrap();
    drop(file);
    let written = std::fs::read(wal(&repo)).unwrap();
    let (code, text) = run(&repo, &["doctor", "--recovery-restore", &id]);
    assert_ne!(code, Some(0), "a refused restore exits non-zero:\n{text}");
    assert!(
        text.contains("no    the source is 715 bytes; the repair left 714"),
        "{text}"
    );
    assert_eq!(
        std::fs::read(wal(&repo)).unwrap(),
        written,
        "the refused restore wrote nothing"
    );
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn a_malformed_id_is_refused_with_the_length_it_needs() {
    let repo = torn_wal("rfc168-id-length");
    repair(&repo);
    let (code, text) = run(
        &repo,
        &["doctor", "--recovery-restore", "abc", "--plan-only"],
    );
    assert_ne!(code, Some(0), "{text}");
    assert!(text.contains("a run id is 16 hex characters"), "{text}");
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn clear_plans_then_empties_the_log_and_keeps_its_name() {
    let repo = torn_wal("rfc168-clear");
    repair(&repo);
    let (code, plan) = run(&repo, &["doctor", "--recovery-clear", "--plan-only"]);
    assert_eq!(code, Some(0), "{plan}");
    assert!(plan.contains("would remove 1 entry"), "{plan}");
    assert!(plan.contains("plan only -- nothing written"), "{plan}");
    let before = std::fs::read(repo.join(".prikk/recovery/log")).unwrap();
    assert!(!before.is_empty());

    let (code, text) = run(&repo, &["doctor", "--recovery-clear"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("removed 1 entry"), "{text}");
    assert!(
        text.contains("Older .bytes files are not touched."),
        "{text}"
    );
    assert_eq!(
        std::fs::read(repo.join(".prikk/recovery/log")).unwrap(),
        Vec::<u8>::new(),
        "the log is empty and its name is kept"
    );
    let (_, listing) = run(&repo, &["doctor", "--recovery-list"]);
    assert!(listing.contains("recovery log: 0 entries"), "{listing}");
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn the_recovery_commands_refuse_bad_combinations_with_usage_errors() {
    let repo = support::unique_repo("rfc168-combinations");
    support::init(&repo);
    for args in [
        vec!["doctor", "--recovery-list", "--repair-tails"],
        vec!["doctor", "--recovery-list", "--plan-only"],
        vec!["doctor", "--recovery-list", "--recovery-clear"],
        vec!["doctor", "--recovery-clear", "--rebuild-pointer-index"],
        vec![
            "doctor",
            "--plan-only",
            "--recovery-clear",
            "--discard-damaged-commits",
        ],
    ] {
        let (code, text) = run(&repo, &args);
        assert_eq!(code, Some(2), "{args:?} is a usage error:\n{text}");
    }
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn verify_reports_damage_in_the_log_on_its_own_line_and_keeps_its_exit_status() {
    use std::io::Write;
    let repo = torn_wal("rfc168-verify-line");
    repair(&repo);
    let (clean_code, _) = run(&repo, &["verify"]);
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(repo.join(".prikk/recovery/log"))
        .unwrap();
    log.write_all(b"damaged-region-that-is-not-a-frame-and-is-followed-by-nothing-sound")
        .unwrap();
    drop(log);
    // A sound entry after the damage keeps the damage a damaged region rather than a torn tail.
    let id = named_entry(&repair_again_after_damage(&repo));
    assert!(!id.is_empty());
    let (code, text) = run(&repo, &["verify"]);
    assert_eq!(
        code, clean_code,
        "verify's exit status is unchanged by the log: {text}"
    );
    assert!(
        text.contains("recovery log: 1 damaged region; a save there cannot be restored (`prikk doctor --recovery-list`)"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// Writes one more sound entry after the damage: a second WAL tail, repaired.
fn repair_again_after_damage(repo: &Path) -> String {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(wal(repo))
        .unwrap();
    file.write_all(b"again").unwrap();
    drop(file);
    repair(repo)
}

/// RFC 168 F1: a log that cannot be read is one line, and `verify`'s exit status is the one it has without the log, in prose and
/// in JSON. The log is made a directory here.
#[test]
fn an_unreadable_log_directory_keeps_verify_exit_status_in_prose_and_json() {
    let repo = torn_wal("rfc168-f1-directory");
    repair(&repo);
    let (clean_prose, _) = run(&repo, &["verify"]);
    let (clean_json, _) = run(&repo, &["verify", "--format", "json"]);
    let log = repo.join(".prikk/recovery/log");
    std::fs::remove_file(&log).unwrap();
    std::fs::create_dir(&log).unwrap();
    let (prose, text) = run(&repo, &["verify"]);
    assert_eq!(
        prose, clean_prose,
        "verify's exit status is unchanged: {text}"
    );
    assert!(text.contains("recovery log: cannot be read"), "{text}");
    let (json, _) = run(&repo, &["verify", "--format", "json"]);
    assert_eq!(json, clean_json, "the JSON exit status is unchanged too");
    let _ = std::fs::remove_dir_all(repo);
}

/// RFC 168 F1: the same for a log that is a symbolic link.
#[cfg(unix)]
#[test]
fn a_symlinked_log_keeps_verify_exit_status_and_prints_one_line() {
    let repo = torn_wal("rfc168-f1-symlink");
    repair(&repo);
    let (clean_prose, _) = run(&repo, &["verify"]);
    let log = repo.join(".prikk/recovery/log");
    let elsewhere = repo.join("elsewhere.log");
    std::fs::write(&elsewhere, b"not a log").unwrap();
    std::fs::remove_file(&log).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &log).unwrap();
    let (prose, text) = run(&repo, &["verify"]);
    assert_eq!(prose, clean_prose, "{text}");
    assert!(text.contains("recovery log: cannot be read"), "{text}");
    let _ = std::fs::remove_dir_all(repo);
}

/// RFC 168 F2: a plan whose condition fails says so, exits 1, and carries no "After this" sentence.
#[test]
fn a_refused_plan_exits_one_and_does_not_say_after_this() {
    use std::io::Write;
    let repo = torn_wal("rfc168-f2-refused-plan");
    let id = named_entry(&repair(&repo));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(wal(&repo))
        .unwrap();
    file.write_all(b"x").unwrap();
    drop(file);
    let (code, text) = run(&repo, &["doctor", "--recovery-restore", &id, "--plan-only"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("plan only -- this restore would be refused"),
        "{text}"
    );
    assert!(!text.contains("After this"), "{text}");
    assert!(!text.contains("would write"), "{text}");
    let _ = std::fs::remove_dir_all(repo);
}

/// RFC 168 F9: `--recovery-clear` lists the entries it removes, in the plan and in the run.
#[test]
fn clear_lists_each_entry_it_removes() {
    let repo = torn_wal("rfc168-f9-clear-list");
    let id = named_entry(&repair(&repo));
    let (code, plan) = run(&repo, &["doctor", "--recovery-clear", "--plan-only"]);
    assert_eq!(code, Some(0), "{plan}");
    assert!(plan.contains(&id), "the plan lists the entry: {plan}");
    let (code, done) = run(&repo, &["doctor", "--recovery-clear"]);
    assert_eq!(code, Some(0), "{done}");
    assert!(
        done.contains(&id),
        "the run lists the entry it removed: {done}"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// RFC 168 §3.4 and Addendum 1 item 10: the residual-(a) route, run as the docs will quote it. The state: a switch completed, one
/// file reverted to the other branch's bytes, the marker clear. The route: move the file `status` names out of the way, then run
/// `prikk checkout --patch-materialize --ref <the current branch>`. The user's copy is kept.
#[test]
fn the_residual_a_route_is_run_and_keeps_the_users_copy() {
    let repo = support::unique_repo("rfc168-route-a");
    support::init(&repo);
    std::fs::write(repo.join("shared.txt"), b"main\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "m1"), "commit main");
    support::ok(&support::seal(&repo, "heads/main"), "seal main");
    support::ok(
        &support::branch_create(&repo, "heads/other", "heads/main"),
        "branch create",
    );
    support::ok(
        &run_in(&repo, &["branch", "switch", "heads/other"]),
        "switch to other",
    );
    std::fs::write(repo.join("shared.txt"), b"other\n").unwrap();
    support::ok(&support::commit(&repo, "heads/other", "o1"), "commit other");
    support::ok(&support::seal(&repo, "heads/other"), "seal other");
    support::ok(
        &run_in(&repo, &["branch", "switch", "heads/main"]),
        "switch back to main",
    );
    support::ok(
        &run_in(&repo, &["branch", "switch", "heads/other"]),
        "switch to other",
    );
    // The residual: one file holds the other branch's bytes again (its rename was lost).
    std::fs::write(repo.join("shared.txt"), b"main\n").unwrap();
    let (_, before) = run(&repo, &["worktree-status"]);
    assert!(
        before.contains("shared.txt"),
        "worktree-status names the file: {before}"
    );

    // The route, as the docs quote it.
    std::fs::rename(repo.join("shared.txt"), repo.join("shared.txt.aside")).unwrap();
    let (route, route_text) = run(
        &repo,
        &["checkout", "--patch-materialize", "--ref", "heads/other"],
    );
    assert_eq!(route, Some(0), "the route runs: {route_text}");
    assert_eq!(
        std::fs::read(repo.join("shared.txt")).unwrap(),
        b"other\n",
        "the branch's file is back"
    );
    assert_eq!(
        std::fs::read(repo.join("shared.txt.aside")).unwrap(),
        b"main\n",
        "the user's copy is kept"
    );
    let (_, after) = run(&repo, &["worktree-status"]);
    assert!(
        !after.contains("shared.txt  "),
        "status is clean for the branch's own files: {after}"
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// RFC 168 §3.4, the two other cases of the residual-(a) route, run before they are quoted: a created file lost in the power loss
/// is written by the same checkout; a deleted file that came back (it shows as extra) is removed by the user, then the checkout runs.
#[test]
fn the_residual_a_route_writes_a_lost_creation_and_keeps_the_users_removal_choice() {
    let repo = support::unique_repo("rfc168-route-a-cases");
    support::init(&repo);
    std::fs::write(repo.join("old.txt"), b"old\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "m1"), "commit main");
    support::ok(&support::seal(&repo, "heads/main"), "seal main");
    support::ok(
        &support::branch_create(&repo, "heads/other", "heads/main"),
        "branch create",
    );
    support::ok(
        &run_in(&repo, &["branch", "switch", "heads/other"]),
        "switch to other",
    );
    std::fs::remove_file(repo.join("old.txt")).unwrap();
    std::fs::write(repo.join("new.txt"), b"new\n").unwrap();
    support::ok(&support::commit(&repo, "heads/other", "o1"), "commit other");
    support::ok(&support::seal(&repo, "heads/other"), "seal other");
    support::ok(
        &run_in(&repo, &["branch", "switch", "heads/main"]),
        "switch back to main",
    );
    support::ok(
        &run_in(&repo, &["branch", "switch", "heads/other"]),
        "switch to other",
    );
    // The residual: the created file was lost; the deleted file came back.
    std::fs::remove_file(repo.join("new.txt")).unwrap();
    std::fs::write(repo.join("old.txt"), b"old\n").unwrap();
    let (_, before) = run(&repo, &["worktree-status"]);
    assert!(
        before.contains("new.txt") && before.contains("old.txt"),
        "status names both: {before}"
    );

    // The user removes the file that came back; the checkout writes the one that was lost.
    std::fs::remove_file(repo.join("old.txt")).unwrap();
    let (route, route_text) = run(
        &repo,
        &["checkout", "--patch-materialize", "--ref", "heads/other"],
    );
    assert_eq!(route, Some(0), "the route runs: {route_text}");
    assert_eq!(
        std::fs::read(repo.join("new.txt")).unwrap(),
        b"new\n",
        "the lost file is written"
    );
    assert!(!repo.join("old.txt").exists(), "the user's removal stands");
    let (_, after) = run(&repo, &["worktree-status"]);
    assert!(
        !after.contains("new.txt") && !after.contains("old.txt"),
        "worktree-status is clean: {after}"
    );
    let _ = std::fs::remove_dir_all(repo);
}
