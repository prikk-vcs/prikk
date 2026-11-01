//! 0.56.0 step W3-0, through the compiled binary
//! (`.git-exclude/reviewed/0.56.0-step-3w2f-review-v1.md`, "## Taken into W3 as its first item:
//! the way out a refusal names must work").
//!
//! **The measured hazard this closes.** `worktree-patch.md`'s own walkthrough (a fresh
//! repository, `commit --ref heads/topic -m "start topic"` on a genuinely unborn `heads/topic`,
//! then `seal`) leaves the pointer naming `heads/main` while the worktree-baseline record names
//! `heads/topic`. An ordinary `commit` afterward correctly refuses as stale and used to always
//! name the self-switch (`branch switch <the current branch>`) as the way out -- but `heads/main`
//! has never been published here (no `RefState` at all), so that self-switch itself refused at
//! `branch_switch.rs`'s refusal 1 (`require_open_branch`): `"heads/main does not exist; run
//! \`prikk branch create heads/main\` first"`. No command the refusal named could ever work --
//! the "users never confused" failure (W2f's own report found this, and the architect's review
//! took it into W3 as its first item).
//!
//! **The fix, in two parts:**
//! 1. `node_authoring.rs`'s stale-commit refusal now always names `branch switch <the record's own
//!    ref>` literally (read off `recorded_identity`, not the pointer) -- which R1's own invariant
//!    (the record always describes what the worktree physically holds) guarantees always works.
//!    It names the self-switch placeholder *additionally*, only when the ref actually being
//!    committed onto (`canonical_ref`) is itself published.
//! 2. `branch_switch.rs`'s refusal 1 now carries a narrow exemption: a self-switch whose own
//!    comparison reads `Behind` against a target with no published `RefState` is not refused
//!    there -- it materializes the target's empty (Genesis) state instead, judging "clean" against
//!    the record exactly as every other `Behind` case already does, via W2f's own generalized
//!    `from_files_against_record_or_live`/`require_clean` (unchanged by this round).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::Path;
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
        .output()
        .unwrap()
}

/// `commit --ref <ref_name>`, signed by the fixed test author.
fn commit_onto(repo: &Path, ref_name: &str, message: &str) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--ref", ref_name, "-m", message])
        .output()
        .unwrap()
}

fn plain_commit(repo: &Path, message: &str) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", message])
        .output()
        .unwrap()
}

/// `worktree-patch.md`'s own walkthrough, reproduced literally: a fresh repository,
/// `commit --ref heads/topic -m "start topic"` on a genuinely unborn `heads/topic` (never
/// `branch create`d first), then `seal`. The pointer stays `heads/main` throughout (never
/// published); the record names `heads/topic`.
fn unborn_topic_walkthrough_fixture(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);

    std::fs::write(repo.join("topic.txt"), b"topic content\n").unwrap();
    ok(
        &commit_onto(&repo, "heads/topic", "start topic"),
        "commit --ref heads/topic on an unborn ref",
    );
    ok(&support::seal(&repo, "heads/topic"), "seal heads/topic");

    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/main\n",
        "the pointer is still heads/main after commit --ref/seal"
    );
    let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
    assert!(
        record.starts_with("PRIKK-WORKTREE-BASELINE-v2 heads/topic "),
        "the record names heads/topic: {record}"
    );
    repo
}

/// Case 1 (the walkthrough's own stuck shape -- `heads/main` never published): the stale-commit
/// refusal names `branch switch heads/topic` literally -- the record's own ref -- and does *not*
/// also claim the self-switch, since the ref actually being committed onto (`heads/main`) is
/// unpublished. Running the one named switch succeeds.
#[test]
fn stuck_commit_names_the_records_ref_only_when_the_current_branch_is_unpublished() {
    let repo = unborn_topic_walkthrough_fixture("w3-0-case1-unpublished-main");

    // A plain commit (no --ref) authors onto the current branch, heads/main -- unpublished here.
    // No further edit: the worktree already physically holds exactly the record's own content
    // (topic.txt's post-seal bytes); the refusal fires purely from the record/canonical_ref
    // mismatch, exactly as the walkthrough measures it.
    let refused = plain_commit(&repo, "stale onto unpublished main");
    let message = stderr(&refused);
    assert!(
        !refused.status.success(),
        "the commit must refuse as stale: stdout={} stderr={}",
        stdout(&refused),
        message
    );
    assert!(
        message.contains("prikk branch switch heads/topic"),
        "the refusal must literally name the record's own ref: {message}"
    );
    assert!(
        !message.contains("<the current branch>"),
        "heads/main is unpublished, so the self-switch placeholder must not be offered as a way \
         out here: {message}"
    );

    // The one named way out works.
    let switched = switch(&repo, "heads/topic");
    ok(&switched, "the named branch switch heads/topic");
    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/topic\n"
    );
    assert_eq!(
        std::fs::read(repo.join("topic.txt")).unwrap(),
        b"topic content\n",
        "the worktree already held the record's own content; the resync changes nothing"
    );

    // Work on topic afterward is clean.
    std::fs::write(repo.join("topic.txt"), b"topic content\nmore\n").unwrap();
    let clean = plain_commit(&repo, "continue on topic");
    ok(&clean, "ordinary commit on topic after the resync");
    assert!(stderr(&clean).is_empty(), "no refusal: {}", stderr(&clean));

    let _ = std::fs::remove_dir_all(&repo);
}

/// Case 1b: `branch_switch.rs`'s own new exemption, exercised directly rather than only through
/// `node_authoring.rs`'s refusal message. From the same unpublished-`heads/main` walkthrough
/// fixture, the self-switch the stale-commit refusal above deliberately does **not** name
/// (`heads/main` has no `RefState` at all) still succeeds when run directly: refusal 1
/// (`require_open_branch`) no longer fires unconditionally for this shape. It materializes
/// `heads/main`'s true, empty (Genesis) state -- deleting `topic.txt`, which belongs only to the
/// record's own `heads/topic`, never to `heads/main` -- and records the baseline against
/// `heads/main` with no sealed block (the `-` genesis marker). This is the test the red control
/// below must break: it is the only one in this file that calls `branch switch heads/main` while
/// `heads/main` is unpublished.
#[test]
fn self_switch_to_the_unpublished_current_branch_resyncs_to_its_empty_state() {
    let repo = unborn_topic_walkthrough_fixture("w3-0-case1b-self-switch-unpublished");

    let switched = switch(&repo, "heads/main");
    assert!(
        switched.status.success(),
        "a self-switch to the unpublished, Behind-reading current branch must not refuse at \
         refusal 1: stdout={} stderr={}",
        stdout(&switched),
        stderr(&switched)
    );
    assert!(
        !stdout(&switched).contains("already"),
        "this must be a real resync (materializing the empty state), not a no-op: {}",
        stdout(&switched)
    );
    assert!(
        !repo.join("topic.txt").exists(),
        "topic.txt belongs to heads/topic's own content, not heads/main's true (empty) state -- \
         the self-switch must delete it"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/main\n"
    );
    let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
    assert!(
        record.starts_with("PRIKK-WORKTREE-BASELINE-v2 heads/main - "),
        "the record now names heads/main with no sealed block (genesis marker `-`): {record}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Reproduces `step3w2f_record_outlives_pointer.rs`'s own `stuck_fixture`: `heads/main` published
/// (a real genesis commit), `heads/docs` branched off it, then `commit --ref heads/docs` +
/// `seal --ref heads/docs` from `main`'s worktree without ever switching -- the pointer still
/// names `heads/main` (published), the record names `heads/docs`.
fn stuck_fixture_with_published_current_branch(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "readme.txt", b"l1\n", "genesis l1");
    ok(
        &support::branch_create(&repo, "heads/docs", "heads/main"),
        "branch create docs",
    );
    let mut content = std::fs::read(repo.join("readme.txt")).unwrap();
    content.extend_from_slice(b"l2\n");
    std::fs::write(repo.join("readme.txt"), &content).unwrap();
    ok(
        &commit_onto(&repo, "heads/docs", "l2 on docs"),
        "commit --ref heads/docs",
    );
    ok(&support::seal(&repo, "heads/docs"), "seal heads/docs");

    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/main\n",
        "the pointer must still name heads/main -- commit --ref/seal never move it"
    );
    let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
    assert!(
        record.starts_with("PRIKK-WORKTREE-BASELINE-v2 heads/docs "),
        "the record must name heads/docs: {record}"
    );
    repo
}

/// Case 2 (the current branch, `heads/main`, is published): the stale-commit refusal names
/// *both* the record's own ref (`branch switch heads/docs`) and the self-switch placeholder
/// (`branch switch <the current branch>`), and both named switches succeed -- run against two
/// separate copies of the same stuck fixture.
#[test]
fn stuck_commit_names_both_switches_when_the_current_branch_is_published() {
    let repo_for_message = stuck_fixture_with_published_current_branch("w3-0-case2-message");

    let refused = plain_commit(&repo_for_message, "stale onto published main");
    let message = stderr(&refused);
    assert!(
        !refused.status.success(),
        "the commit must refuse as stale: stdout={} stderr={}",
        stdout(&refused),
        message
    );
    assert!(
        message.contains("prikk branch switch heads/docs"),
        "the refusal must name the record's own ref: {message}"
    );
    assert!(
        message.contains("<the current branch>"),
        "heads/main is published, so the self-switch placeholder must also be offered: {message}"
    );
    let _ = std::fs::remove_dir_all(&repo_for_message);

    // The record's own ref switch succeeds (mirrors step3w2f's own case2).
    let repo_a = stuck_fixture_with_published_current_branch("w3-0-case2-switch-docs");
    let to_docs = switch(&repo_a, "heads/docs");
    ok(&to_docs, "branch switch heads/docs from the stuck state");
    assert_eq!(
        std::fs::read_to_string(repo_a.join(".prikk/current-branch")).unwrap(),
        "heads/docs\n"
    );
    let _ = std::fs::remove_dir_all(&repo_a);

    // The self-switch also succeeds (mirrors step3w2f's own case1).
    let repo_b = stuck_fixture_with_published_current_branch("w3-0-case2-switch-main");
    let to_main = switch(&repo_b, "heads/main");
    ok(
        &to_main,
        "branch switch heads/main (self) from the stuck state",
    );
    assert_eq!(
        std::fs::read_to_string(repo_b.join(".prikk/current-branch")).unwrap(),
        "heads/main\n"
    );
    let _ = std::fs::remove_dir_all(&repo_b);
}
