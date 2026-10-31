//! 0.56.0 step W2f, through the compiled binary
//! (`.git-exclude/reviewed/0.56.0-step-3w2e-review-v1.md`, "## W2f").
//!
//! **The measured hazard this closes.** `from_files_against_record_or_live` and `require_clean`
//! (`crates/prikk-store/src/branch_switch.rs`) used to consult the worktree's recorded baseline
//! only when `recorded.ref_name == from` -- `from` being the **current-branch pointer**
//! (`current_branch::displayed_current_branch`), not the ref the record actually names. After
//! `commit --ref heads/docs` from `main`'s own worktree (the pointer never moves -- only `branch
//! switch`/`init` ever write it), followed by `seal --ref heads/docs` (which records the ref it
//! authored onto, per `node_authoring.rs`'s and `seal.rs`'s own write sites), the pointer names
//! `heads/main` while the record names `heads/docs` -- they never match, for *either* switch
//! target, so both functions always fell back to comparing against the pointer's own **live**
//! state, which the worktree (physically holding `docs`' own content) genuinely disagrees with.
//! Neither `branch switch heads/main` (a self-switch) nor `branch switch heads/docs` could ever
//! succeed, and an ordinary `commit` refused as stale and named the self-switch that had just
//! refused -- no command reached a clean worktree (§2.7's deadlock again, this time reachable
//! through `commit --ref <other>`, which `docs/src/guide/patches/worktree-patch.md`'s own
//! documented walkthrough puts a real user through).
//!
//! **The fix:** both functions now use the recorded replay whenever `compare_worktree_baseline`
//! reads `Behind` **at all**, regardless of whether `recorded.ref_name` equals `from` -- the
//! record is always an accurate description of the worktree's own physical files as of its last
//! write (R1), so using it is never less correct than trusting `from`'s own live state, only
//! falling back to the live comparison under `NoRecord`/`Current`.

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

/// Reproduces the measured stuck state and leaves the repo with the pointer naming `heads/main`
/// and the worktree-baseline record naming `heads/docs`, the worktree physically holding
/// `readme.txt` = `l1\nl2\n`.
fn stuck_fixture(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "readme.txt", b"l1\n", "genesis l1");
    ok(
        &support::branch_create(&repo, "heads/docs", "heads/main"),
        "branch create docs",
    );
    // Append l2 directly to the worktree -- the pointer is never moved to `heads/docs` here,
    // exactly the shape `commit --ref <other>` lets a user reach without ever running `branch
    // switch`.
    let mut content = std::fs::read(repo.join("readme.txt")).unwrap();
    content.extend_from_slice(b"l2\n");
    std::fs::write(repo.join("readme.txt"), &content).unwrap();
    ok(
        &commit_onto(&repo, "heads/docs", "l2 on docs"),
        "commit --ref heads/docs",
    );
    ok(&support::seal(&repo, "heads/docs"), "seal heads/docs");

    // Confirm the stuck shape before asserting the fix, exactly as the ruling measured it.
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

/// Case 1: from the stuck state, the self-switch to `heads/main` (what `commit`'s own refusal
/// tells the user to run) now succeeds instead of refusing "not clean against heads/main".
#[test]
fn case1_self_switch_to_pointer_target_resyncs_from_the_stuck_state() {
    let repo = stuck_fixture("w2f-case1-self-switch-main");

    let result = switch(&repo, "heads/main");
    ok(&result, "branch switch heads/main from the stuck state");
    assert!(
        !stdout(&result).contains("already"),
        "this must be a real resync, not a no-op: {}",
        stdout(&result)
    );
    assert_eq!(
        std::fs::read(repo.join("readme.txt")).unwrap(),
        b"l1\n",
        "main's own content is materialized"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/main\n"
    );
    let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
    assert!(
        record.starts_with("PRIKK-WORKTREE-BASELINE-v2 heads/main "),
        "the record now names heads/main, the switch target: {record}"
    );

    // The following ordinary commit is clean -- no stale refusal, no stray authored operation.
    std::fs::write(repo.join("readme.txt"), b"l1\nl3\n").unwrap();
    let clean = plain_commit(&repo, "l3 on main");
    ok(&clean, "ordinary commit after the resync must be clean");
    let out = stdout(&clean);
    assert!(
        out.contains("edit-text readme.txt") && !out.contains("create-file"),
        "only the real edit is authored: {out}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Case 2: from the same stuck state (a separately derived fixture), switching to the ref the
/// record actually names (`heads/docs`) also succeeds, and -- since the worktree already
/// physically holds `docs`' own content -- writes nothing.
#[test]
fn case2_switch_to_the_ref_the_record_names_also_resyncs_from_the_stuck_state() {
    let repo = stuck_fixture("w2f-case2-switch-docs");

    let result = switch(&repo, "heads/docs");
    ok(&result, "branch switch heads/docs from the stuck state");
    assert!(
        !stdout(&result).contains("already"),
        "this must be a real resync, not a no-op: {}",
        stdout(&result)
    );
    assert!(
        stdout(&result).contains("files written: 0")
            && stdout(&result).contains("files deleted: 0"),
        "nothing needed writing or deleting -- the worktree already held docs' own content: {}",
        stdout(&result)
    );
    assert_eq!(
        std::fs::read(repo.join("readme.txt")).unwrap(),
        b"l1\nl2\n",
        "docs' own content, already on disk, is untouched"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/docs\n"
    );
    let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
    assert!(
        record.starts_with("PRIKK-WORKTREE-BASELINE-v2 heads/docs "),
        "the record now names heads/docs, the switch target: {record}"
    );

    // The following ordinary commit on docs is clean too.
    std::fs::write(repo.join("readme.txt"), b"l1\nl2\nl3\n").unwrap();
    let clean = plain_commit(&repo, "l3 on docs");
    ok(&clean, "ordinary commit after the resync must be clean");
    let out = stdout(&clean);
    assert!(
        out.contains("edit-text readme.txt") && !out.contains("create-file"),
        "only the real edit is authored: {out}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `docs/src/guide/patches/worktree-patch.md`'s own documented walkthrough (`commit --ref
/// heads/topic -m "start topic"` on an unborn ref, then `seal`) ends usable.
///
/// **Reproduced literally**, per the brief: a fresh repository, `commit --ref heads/topic -m
/// "start topic"` on a genuinely unborn `heads/topic` (never `branch create`d first -- the doc's
/// own "Explicit unborn local branch genesis"), then `seal`. The pointer (`heads/main`) is never
/// moved by either -- `commit --ref`/`seal` write only the worktree-baseline record, which now
/// names `heads/topic`.
///
/// **What this measures, found while writing this test, reported plainly rather than papered
/// over:** an ordinary `commit` afterward correctly refuses as stale and names the self-switch
/// (`branch switch heads/main`) as the way out -- but in this specific fixture `heads/main` has
/// *never* been published (no `RefState` exists for it at all), so that self-switch itself refuses
/// with a wholly different message, `"heads/main does not exist; run \`prikk branch create
/// heads/main\` first"`. That refusal fires in `require_open_branch` (refusal 1, before refusal 3
/// -- the `from_files_against_record_or_live`/`require_clean` comparison this round's fix
/// touches, is ever reached), and **is unaffected by this round's fix either way** (confirmed
/// directly: identical message with the pre-W2f `recorded.ref_name == from` gate restored). It is
/// a separate, pre-existing gap (`branch switch`'s refusal 1 has no exemption for a self-switch to
/// an unpublished-but-current, `Behind`-reading branch, unlike the no-op path above it, which does)
/// -- not one this round's named functions can reach or were asked to fix, and not claimed fixed
/// here.
///
/// **What this round's fix actually does for this exact fixture, and what genuinely ends it
/// usable:** `branch switch heads/topic` -- the ref the record (and the worktree's real files)
/// already name -- succeeds and leaves the worktree ready for ordinary work on `heads/topic`,
/// which is the realistic next step for a user who just ran `commit --ref heads/topic -m "start
/// topic"` in the first place.
#[test]
fn worktree_patch_doc_walkthrough_ends_usable() {
    let repo = support::unique_repo("w2f-worktree-patch-walkthrough");
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

    // The actual command that ends this usable: switch to the ref the worktree already holds.
    let to_topic = switch(&repo, "heads/topic");
    ok(
        &to_topic,
        "branch switch heads/topic after seal heads/topic",
    );
    assert!(
        !stdout(&to_topic).contains("already"),
        "a real resync, naming the pointer move: {}",
        stdout(&to_topic)
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap(),
        "heads/topic\n"
    );

    // Ordinary work on topic afterward is clean.
    std::fs::write(repo.join("topic.txt"), b"topic content\nmore\n").unwrap();
    let clean = plain_commit(&repo, "continue on topic");
    ok(&clean, "ordinary commit on topic after the resync");
    assert!(stderr(&clean).is_empty(), "no refusal: {}", stderr(&clean));

    let _ = std::fs::remove_dir_all(&repo);
}
