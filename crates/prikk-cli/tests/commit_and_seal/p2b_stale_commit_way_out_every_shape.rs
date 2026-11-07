//! 0.56.0 prep P2b: every command a stale-commit refusal names, run literally, is followed by
//! the same `commit` succeeding.
//!
//! **The measured defect.** `commit --ref heads/feature` from `heads/main`'s worktree, where
//! `heads/feature` diverged `a.txt`, refused naming `prikk branch switch heads/main` (the record's
//! own ref) and `prikk branch switch <the current branch>`. Both are no-ops there (the worktree is
//! already what the record says, and the current branch is already `heads/main`), so the same
//! commit refused again with the identical message. Only `branch switch heads/feature` let it
//! through. W3-0 had tested that the named switches *succeed*, never that the commit then does.
//!
//! **The rule this pins.** Three refs decide what a refusal can name: the ref the commit is onto
//! (`C`), the ref the worktree's record names (`R`), and the current branch (`B`). This file
//! enumerates the shapes by which of them are equal:
//!
//! | shape | C | R | B | fixture |
//! |---|---|---|---|---|
//! | 1 | = | = | = | `rollback-draft --append-inverse` then `seal` on the current branch (the tip moved on) |
//! | 2 | = | ≠ | = | the stuck state: `commit --ref heads/docs` + `seal` from `heads/main`'s worktree |
//! | 3 | = | = | ≠ | the stuck state, then `heads/docs`' own tip moved on; `commit --ref heads/docs` |
//! | 4 | ≠ | = | = | `heads/feature` diverged `a.txt`; `commit --ref heads/feature` from `heads/main` |
//! | 5 | ≠ | ≠ | ≠ | the stuck state plus a diverged `heads/feature`; `commit --ref heads/feature` |
//! | 6 | ≠ | = | = | `commit --ref heads/new` onto a ref never published: the named switch refuses with its own reason |
//!
//! (R = B and C = R with C ≠ B cannot be told apart from the table's rows 4 and 3; C = B = R with a
//! current branch that cannot be resolved keeps the old text and is not a shape of its own.)
//!
//! For each shape the test makes the commit refuse, reads every `prikk branch switch …` the message
//! names (substituting the documented placeholder `<the current branch>` with the branch the
//! pointer names), and on a fresh copy of the fixture runs each one, then the *same* commit. The
//! commit must succeed and author only the innocent file. Without the fix, shapes 3, 4 and 5 fail
//! (the refusal names the two no-ops); shapes 1 and 2 keep their old text and pass either way.

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

/// A `prikk` command signed by the fixed test author.
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

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
        .output()
        .unwrap()
}

/// One fixture: a repository whose worktree is clean against its record, an innocent new file
/// `z.txt` waiting to be committed, and the exact `commit` arguments that refuse as stale.
struct Shape {
    repo: PathBuf,
    commit_args: Vec<&'static str>,
}

fn commit_in(shape: &Shape) -> Output {
    authored(&shape.repo, &shape.commit_args)
}

/// Shape 4's base: `heads/main` published with `a.txt = a1`; `heads/feature` branched, switched
/// to, `a.txt = fx` committed and sealed there; back on `heads/main` (worktree holds `a1`).
fn diverged_feature_fixture(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a1\n", "genesis a1");
    ok(
        &support::branch_create(&repo, "heads/feature", "heads/main"),
        "branch create heads/feature",
    );
    ok(&switch(&repo, "heads/feature"), "switch to heads/feature");
    std::fs::write(repo.join("a.txt"), b"fx\n").unwrap();
    ok(
        &authored(&repo, &["commit", "-m", "feature diverges a.txt"]),
        "commit on heads/feature",
    );
    ok(&support::seal(&repo, "heads/feature"), "seal heads/feature");
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"a1\n");
    repo
}

/// The stuck state on top of `repo` (`heads/main` published, current): `heads/docs` branched off
/// it, then `commit --ref heads/docs` + `seal` from `heads/main`'s worktree. The record names
/// `heads/docs`; the pointer still names `heads/main`.
fn make_stuck(repo: &Path) {
    ok(
        &support::branch_create(repo, "heads/docs", "heads/main"),
        "branch create heads/docs",
    );
    std::fs::write(repo.join("readme.txt"), b"docs line\n").unwrap();
    ok(
        &authored(repo, &["commit", "--ref", "heads/docs", "-m", "docs line"]),
        "commit --ref heads/docs",
    );
    ok(&support::seal(repo, "heads/docs"), "seal heads/docs");
    let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
    assert!(
        record.starts_with("PRIKK-WORKTREE-BASELINE-v2 heads/docs "),
        "the record names heads/docs: {record}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/main\n"
    );
}

fn build(shape_number: u8, tag: &str) -> Shape {
    let repo;
    let commit_args;
    match shape_number {
        // C = R = B: the tip of the current branch moved on without the worktree following.
        1 => {
            repo = support::unique_repo(tag);
            support::init(&repo);
            support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
            ok(
                &authored(
                    &repo,
                    &["rollback-draft", "--append-inverse", "-m", "undo a.txt"],
                ),
                "rollback-draft --append-inverse",
            );
            ok(&support::seal(&repo, "heads/main"), "seal the inverse");
            assert!(repo.join("a.txt").exists(), "a.txt is still on disk");
            commit_args = vec!["commit", "-m", "innocent z"];
        }
        // C = B, R different: the stuck state, a plain commit.
        2 => {
            repo = support::unique_repo(tag);
            support::init(&repo);
            support::generation(&repo, "heads/main", "readme.txt", b"l1\n", "genesis l1");
            make_stuck(&repo);
            commit_args = vec!["commit", "-m", "innocent z"];
        }
        // C = R, B different: the record's own branch moved on, committed onto by --ref.
        3 => {
            repo = support::unique_repo(tag);
            support::init(&repo);
            support::generation(&repo, "heads/main", "readme.txt", b"l1\n", "genesis l1");
            make_stuck(&repo);
            ok(
                &authored(
                    &repo,
                    &[
                        "rollback-draft",
                        "--append-inverse",
                        "--ref",
                        "heads/docs",
                        "-m",
                        "undo the docs line",
                    ],
                ),
                "rollback-draft --append-inverse --ref heads/docs",
            );
            ok(&support::seal(&repo, "heads/docs"), "seal the docs inverse");
            commit_args = vec!["commit", "--ref", "heads/docs", "-m", "innocent z"];
        }
        // C different, R = B: the reported defect.
        4 => {
            repo = diverged_feature_fixture(tag);
            commit_args = vec!["commit", "--ref", "heads/feature", "-m", "innocent z"];
        }
        // C, R and B all different.
        5 => {
            repo = diverged_feature_fixture(tag);
            ok(
                &support::branch_create(&repo, "heads/docs", "heads/main"),
                "branch create heads/docs",
            );
            std::fs::write(repo.join("d.txt"), b"docs file\n").unwrap();
            ok(
                &authored(&repo, &["commit", "--ref", "heads/docs", "-m", "docs file"]),
                "commit --ref heads/docs",
            );
            ok(&support::seal(&repo, "heads/docs"), "seal heads/docs");
            let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
            assert!(
                record.starts_with("PRIKK-WORKTREE-BASELINE-v2 heads/docs "),
                "the record names heads/docs: {record}"
            );
            commit_args = vec!["commit", "--ref", "heads/feature", "-m", "innocent z"];
        }
        other => panic!("no shape {other}"),
    }
    std::fs::write(repo.join("z.txt"), b"innocent\n").unwrap();
    Shape { repo, commit_args }
}

/// Every `prikk branch switch <ref>` the message names, in order, as the `<ref>` argument, with the
/// documented placeholder `<the current branch>` replaced by the branch the pointer names.
fn named_switch_targets(message: &str, current_branch: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for piece in message.split('`').skip(1).step_by(2) {
        if let Some(rest) = piece.strip_prefix("prikk branch switch ") {
            let target = rest.trim();
            targets.push(if target == "<the current branch>" {
                current_branch.to_string()
            } else {
                target.to_string()
            });
        }
    }
    targets
}

fn run_shape(shape_number: u8) {
    let probe = build(shape_number, &format!("p2b-shape{shape_number}-probe"));
    let refused = commit_in(&probe);
    let message = stderr(&refused);
    assert!(
        !refused.status.success() && message.contains("refusing to commit:"),
        "shape {shape_number}: the commit must refuse as stale: stdout={} stderr={message}",
        stdout(&refused)
    );
    let current = std::fs::read_to_string(probe.repo.join(".prikk/current-branch"))
        .unwrap()
        .trim()
        .to_string();
    let targets = named_switch_targets(&message, &current);
    assert!(
        !targets.is_empty(),
        "shape {shape_number}: the refusal names no `prikk branch switch`: {message}"
    );
    let _ = std::fs::remove_dir_all(&probe.repo);

    for (index, target) in targets.iter().enumerate() {
        let shape = build(shape_number, &format!("p2b-shape{shape_number}-way{index}"));
        ok(
            &switch(&shape.repo, target),
            &format!("shape {shape_number}: the named `prikk branch switch {target}`"),
        );
        let again = commit_in(&shape);
        assert!(
            again.status.success(),
            "shape {shape_number}: after the named `prikk branch switch {target}` the same commit \
             must succeed, and it refused: stderr={}",
            stderr(&again)
        );
        let report = stdout(&again);
        assert!(
            report.contains("create-file z.txt") && report.contains("operations: 1"),
            "shape {shape_number}: the commit must author only z.txt: {report}"
        );
        let _ = std::fs::remove_dir_all(&shape.repo);
    }
}

#[test]
fn shape_1_commit_ref_record_and_current_branch_are_all_the_same() {
    run_shape(1);
}

#[test]
fn shape_2_the_record_names_another_branch_than_the_one_committed_onto() {
    run_shape(2);
}

#[test]
fn shape_3_the_commit_is_onto_the_records_branch_which_is_not_the_current_one() {
    run_shape(3);
}

#[test]
fn shape_4_the_commit_is_onto_another_ref_than_the_record_and_the_current_branch() {
    run_shape(4);
}

#[test]
fn shape_5_the_commit_ref_the_record_and_the_current_branch_all_differ() {
    run_shape(5);
}

/// When the commit is onto some other ref than the current branch, the only command named is the
/// switch to that ref, substituted literally: the record's ref and the self-switch are the two
/// no-ops the rule removes.
#[test]
fn a_commit_onto_another_ref_names_only_the_switch_to_that_ref() {
    for shape_number in [3, 4, 5] {
        let shape = build(shape_number, &format!("p2b-names-only-{shape_number}"));
        let refused = commit_in(&shape);
        let message = stderr(&refused);
        let wanted = if shape_number == 3 {
            "run `prikk branch switch heads/docs` to bring"
        } else {
            "run `prikk branch switch heads/feature` to bring"
        };
        assert!(
            message.contains(wanted),
            "shape {shape_number}: expected the single literal switch: {message}"
        );
        assert!(
            !message.contains("<the current branch>"),
            "shape {shape_number}: the self-switch placeholder must not be offered: {message}"
        );
        assert_eq!(
            message.matches("prikk branch switch").count(),
            1,
            "shape {shape_number}: exactly one command is named: {message}"
        );
        let _ = std::fs::remove_dir_all(&shape.repo);
    }
}

/// Shape 6 (the commit is onto a ref that has never been published, from another branch's
/// worktree): the refusal names `prikk branch switch heads/new`, and that switch refuses with its
/// own, different reason (`heads/new does not exist; run prikk branch create heads/new first`),
/// which is the second thing the rule allows. Following that reason, `branch create` and then the
/// same commit goes through. Before this round the refusal named the record's ref, a no-op, and
/// the same commit refused again with the identical message.
#[test]
fn shape_6_the_commit_is_onto_an_unborn_ref_and_the_named_switch_says_why_it_refuses() {
    let repo = support::unique_repo("p2b-shape6");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    std::fs::write(repo.join("z.txt"), b"innocent\n").unwrap();
    let args = ["commit", "--ref", "heads/new", "-m", "innocent z"];

    let refused = authored(&repo, &args);
    let message = stderr(&refused);
    assert!(
        !refused.status.success() && message.contains("refusing to commit:"),
        "the commit must refuse as stale: {message}"
    );
    assert!(
        message.contains("run `prikk branch switch heads/new` to bring"),
        "{message}"
    );
    assert!(!message.contains("<the current branch>"), "{message}");
    assert_eq!(
        message.matches("prikk branch switch").count(),
        1,
        "{message}"
    );

    let switched = switch(&repo, "heads/new");
    assert!(!switched.status.success(), "heads/new does not exist yet");
    assert!(
        stderr(&switched).contains("heads/new does not exist; run `prikk branch create heads/new`"),
        "the switch refuses for its own reason: {}",
        stderr(&switched)
    );

    ok(
        &support::branch_create(&repo, "heads/new", "heads/main"),
        "the step that refusal names",
    );
    let again = authored(&repo, &args);
    assert!(again.status.success(), "{}", stderr(&again));
    let report = stdout(&again);
    assert!(
        report.contains("create-file z.txt") && report.contains("operations: 1"),
        "{report}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
