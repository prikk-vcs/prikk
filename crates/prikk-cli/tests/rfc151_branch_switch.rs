//! RFC 151 increment 2: `prikk branch switch heads/<name>`, end to end.
//!
//! The crash postures are store tests (`prikk_store`'s `branch_switch::tests`): failpoints exist only
//! in the store's own test build. Every command here runs through `support::prikk`, the isolation
//! seam.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn pointer(repo: &Path) -> String {
    std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap()
}

fn run_with_keys(repo: &Path, args: &[&str]) -> Output {
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(args)
        .output()
        .unwrap()
}

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
        .output()
        .unwrap()
}

fn assert_refused(output: &Output, what: &str, needles: &[&str]) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{what} must refuse\nstdout: {}\nstderr: {}",
        stdout(output),
        stderr(output)
    );
    for needle in needles {
        assert!(
            stderr(output).contains(needle),
            "{what}: stderr must contain {needle:?}\nstderr: {}",
            stderr(output)
        );
    }
}

/// `heads/main` holds `readme.txt`; `heads/other` holds `other.txt` and not `readme.txt`. Built
/// through the CLI itself -- the switch to `heads/other`, a commit and a seal that both take their
/// ref from the pointer -- and left on `heads/main`.
fn disjoint_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "readme.txt", b"hello\n", "genesis");
    support::ok(
        &support::branch_create(&repo, "heads/other", "heads/main"),
        "branch create heads/other",
    );
    support::ok(&switch(&repo, "heads/other"), "switch to heads/other");
    std::fs::remove_file(repo.join("readme.txt")).unwrap();
    std::fs::write(repo.join("other.txt"), b"other\n").unwrap();
    support::ok(
        &run_with_keys(&repo, &["commit", "-m", "other"]),
        "commit on the current branch",
    );
    support::ok(
        &run_with_keys(&repo, &["seal", "--allow-no-audit"]),
        "seal on the current branch",
    );
    support::ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    repo
}

#[test]
fn switching_between_two_branches_moves_their_files_and_the_pointer() {
    let repo = disjoint_repo("switch-disjoint");
    assert_eq!(pointer(&repo), "heads/main\n");
    assert_eq!(std::fs::read(repo.join("readme.txt")).unwrap(), b"hello\n");
    assert!(!repo.join("other.txt").exists());
    std::fs::write(repo.join("notes.txt"), b"mine\n").unwrap();

    let out = switch(&repo, "heads/other");
    support::ok(&out, "switch to heads/other");
    assert!(stdout(&out).contains("switched from heads/main to heads/other"));
    assert_eq!(pointer(&repo), "heads/other\n");
    assert!(!repo.join("readme.txt").exists());
    assert_eq!(std::fs::read(repo.join("other.txt")).unwrap(), b"other\n");
    assert_eq!(std::fs::read(repo.join("notes.txt")).unwrap(), b"mine\n");
    support::ok(&support::verify(&repo), "verify on heads/other");

    let out = switch(&repo, "heads/main");
    support::ok(&out, "switch to heads/main");
    assert_eq!(pointer(&repo), "heads/main\n");
    assert_eq!(std::fs::read(repo.join("readme.txt")).unwrap(), b"hello\n");
    assert!(!repo.join("other.txt").exists());
    assert_eq!(std::fs::read(repo.join("notes.txt")).unwrap(), b"mine\n");
    support::ok(&support::verify(&repo), "verify on heads/main");

    let again = switch(&repo, "heads/main");
    support::ok(&again, "switch to the current branch");
    assert!(stdout(&again).contains("already on heads/main"));
}

/// A tracked file modified on disk that the target lacks: the dirty refusal fires, not a deletion
/// one, and nothing moves.
#[test]
fn a_dirty_worktree_is_refused_and_nothing_is_written() {
    let repo = disjoint_repo("switch-dirty");
    std::fs::write(repo.join("readme.txt"), b"edited\n").unwrap();

    let out = switch(&repo, "heads/other");
    assert_refused(
        &out,
        "switch over a modified file",
        &[
            "precondition not met",
            "the worktree is not clean against heads/main",
            "readme.txt (modified)",
            "prikk worktree-status",
        ],
    );
    assert!(!stderr(&out).contains("in the way"));
    assert_eq!(pointer(&repo), "heads/main\n");
    assert_eq!(std::fs::read(repo.join("readme.txt")).unwrap(), b"edited\n");
    assert!(!repo.join("other.txt").exists());
}

#[test]
fn unsealed_work_for_another_branch_is_refused() {
    let repo = disjoint_repo("switch-unsealed");
    std::fs::write(repo.join("queued.txt"), b"queued\n").unwrap();
    support::ok(
        &run_with_keys(&repo, &["commit", "-m", "queued on main"]),
        "commit on heads/main",
    );

    let out = switch(&repo, "heads/other");
    assert_refused(
        &out,
        "switch with unsealed work for heads/main",
        &[
            "the active WAL holds unsealed work for heads/main",
            "prikk seal",
        ],
    );
    assert_eq!(pointer(&repo), "heads/main\n");
    assert!(!repo.join("other.txt").exists());
}

#[test]
fn a_missing_or_closed_target_is_refused() {
    let repo = disjoint_repo("switch-target");
    assert_refused(
        &switch(&repo, "heads/nowhere"),
        "switch to a missing branch",
        &["heads/nowhere does not exist", "prikk branch create"],
    );
    assert_refused(
        &switch(&repo, "tags/v1"),
        "switch to a tag ref",
        &["ref namespace is reserved: tags/v1"],
    );

    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create heads/topic",
    );
    support::ok(
        &support::branch_close(&repo, "heads/topic"),
        "branch close heads/topic",
    );
    assert_refused(
        &switch(&repo, "heads/topic"),
        "switch to a closed branch",
        &["heads/topic is closed"],
    );
    assert_eq!(pointer(&repo), "heads/main\n");
}

#[test]
fn an_untracked_file_in_the_way_is_refused_before_any_write() {
    let repo = disjoint_repo("switch-in-the-way");
    std::fs::write(repo.join("other.txt"), b"not the branch's\n").unwrap();
    assert_refused(
        &switch(&repo, "heads/other"),
        "switch onto an untracked file",
        &["in the way", "other.txt", "nothing was written"],
    );
    assert_eq!(pointer(&repo), "heads/main\n");
    assert_eq!(std::fs::read(repo.join("readme.txt")).unwrap(), b"hello\n");
    assert_eq!(
        std::fs::read(repo.join("other.txt")).unwrap(),
        b"not the branch's\n"
    );
}

/// `doctor`'s route out of a broken pointer works: with the pointer on a branch that does not exist,
/// the switch runs without an old baseline -- it writes what is absent and deletes nothing.
#[test]
fn a_broken_pointer_can_be_switched_away_from() {
    let repo = disjoint_repo("switch-broken-pointer");
    std::fs::write(repo.join(".prikk/current-branch"), b"heads/nowhere\n").unwrap();
    let out = switch(&repo, "heads/other");
    support::ok(&out, "switch away from a broken pointer");
    assert!(stdout(&out).contains("could not be resolved"));
    assert_eq!(pointer(&repo), "heads/other\n");
    assert!(
        repo.join("readme.txt").exists(),
        "nothing is deleted without an old baseline"
    );
    assert_eq!(std::fs::read(repo.join("other.txt")).unwrap(), b"other\n");
}

#[test]
fn status_reports_the_current_branch_and_help_names_the_switch() {
    let repo = disjoint_repo("switch-status");
    support::ok(&switch(&repo, "heads/other"), "switch to heads/other");

    let prose = support::prikk(&repo).arg("status").output().unwrap();
    support::ok(&prose, "status");
    assert!(stdout(&prose).contains("current branch: heads/other\n"));
    let json = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json, "status --format json");
    assert!(stdout(&json).contains("\"current_branch\": \"heads/other\""));

    let help = support::prikk(&repo)
        .args(["branch", "--help"])
        .output()
        .unwrap();
    let help = stdout(&help);
    assert!(help.contains("prikk branch switch heads/<name>"));
    assert!(!help.contains("note:"), "the note is replaced: {help}");
}
