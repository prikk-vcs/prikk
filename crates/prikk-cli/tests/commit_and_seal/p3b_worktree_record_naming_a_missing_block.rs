//! 0.56.0 prep P3b: a `.prikk/worktree.baseline` record that parses but names a Block this
//! repository does not hold reads as no record.
//!
//! **The measured defect.** The record is derived, worktree-local state. A record that does not
//! parse (a flipped non-hex byte, truncation, an empty file) already read as "no record" and healed
//! at the next write. A single flipped *hex digit* of its block id still parsed, so `commit` and
//! `branch switch` went on to load a Block that is not there and failed with `integrity error:
//! missing Block <id>` while `verify` and `doctor` exited 0 and the documented self-resync did not
//! clear it. The repository's own objects were intact; the error blamed them.
//!
//! **What this pins.** With one hex digit of the record's block id replaced by another valid hex
//! digit: `commit` succeeds and authors only the new file; `seal` then succeeds and the record names
//! the new tip; in a second fixture the self-switch exits 0; `worktree-status` exits 0 and does not
//! say the worktree is behind. A record that parses and names a Block the repository *does* hold
//! keeps its old meaning (the other tests of the W2 guard cover that). Without the change in
//! `node_authoring.rs` the `commit` assertion fails; without the one in
//! `worktree_baseline::compare_worktree_baseline_with_live` the switch and `worktree-status`
//! assertions fail.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::{Path, PathBuf};
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn authored(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(args)
        .output()
        .unwrap()
}

/// A repository with one sealed commit, so the record names a real tip block.
fn fixture(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    repo
}

fn record_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/worktree.baseline")
}

/// The record's block id field (the third space-separated field).
fn recorded_block_id(repo: &Path) -> String {
    let record = std::fs::read_to_string(record_path(repo)).unwrap();
    record.split(' ').nth(2).unwrap().to_string()
}

/// Replace one hex digit of the record's block id with a different valid hex digit, so the record
/// still parses and names a Block the repository does not hold. Returns the damaged id.
fn damage_block_id(repo: &Path) -> String {
    let record = std::fs::read_to_string(record_path(repo)).unwrap();
    let mut fields: Vec<String> = record.trim_end().split(' ').map(str::to_string).collect();
    let id = fields[2].clone();
    assert_eq!(id.len(), 64, "a published tip: {record}");
    let mut digits: Vec<char> = id.chars().collect();
    digits[10] = if digits[10] == '0' { '1' } else { '0' };
    let damaged: String = digits.into_iter().collect();
    assert_ne!(damaged, id);
    fields[2] = damaged.clone();
    std::fs::write(record_path(repo), format!("{}\n", fields.join(" "))).unwrap();
    damaged
}

#[test]
fn commit_and_seal_treat_a_record_naming_a_missing_block_as_no_record() {
    let repo = fixture("p3b-commit-seal");
    let damaged = damage_block_id(&repo);
    std::fs::write(repo.join("b.txt"), b"b\n").unwrap();

    let committed = authored(&repo, &["commit", "-m", "new file b"]);
    assert!(
        committed.status.success(),
        "a record naming a missing Block must read as no record, not fail the commit: {}",
        stderr(&committed)
    );
    let report = stdout(&committed);
    assert!(
        report.contains("create-file b.txt") && report.contains("operations: 1"),
        "the commit authors only the new file: {report}"
    );
    assert!(!report.contains("delete-file"), "{report}");

    ok(&support::seal(&repo, "heads/main"), "seal after the commit");
    let healed = recorded_block_id(&repo);
    assert_ne!(healed, damaged, "the record was rewritten");
    let log = stdout(&authored(&repo, &["log", "--ref", "heads/main"]));
    assert!(
        log.contains(&healed),
        "the record now names a block of heads/main's history (the new tip): {healed} not in {log}"
    );
    ok(
        &support::prikk(&repo).args(["verify"]).output().unwrap(),
        "verify after the heal",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_self_switch_and_worktree_status_treat_it_as_no_record() {
    let repo = fixture("p3b-switch");
    let damaged = damage_block_id(&repo);

    let status = support::prikk(&repo)
        .arg("worktree-status")
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", stderr(&status));
    assert!(
        !stdout(&status).contains("behind"),
        "a record naming a missing Block is not 'behind': {}",
        stdout(&status)
    );

    let switched = support::prikk(&repo)
        .args(["branch", "switch", "heads/main"])
        .output()
        .unwrap();
    assert!(
        switched.status.success(),
        "the self-switch must not fail with an integrity error about a Block the record names: {}",
        stderr(&switched)
    );
    // A no-op self-switch writes no record (the worktree was not changed), so the damaged one may
    // still be there; it still reads as none, and the next commit proceeds and replaces it.
    std::fs::write(repo.join("c.txt"), b"c\n").unwrap();
    let committed = authored(&repo, &["commit", "-m", "after the switch"]);
    assert!(committed.status.success(), "{}", stderr(&committed));
    assert_ne!(recorded_block_id(&repo), damaged);
    let _ = std::fs::remove_dir_all(&repo);
}
