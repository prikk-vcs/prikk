//! 0.56.0 step W2: the worktree's recorded baseline (R1) and the refusal/resync it enables (R2).
//!
//! Reproduces each of the design report's five hazard cases
//! (`.git-exclude/review-request/0.56.0-step-3w1-design-report.md` §2.1-§2.5) end to end through
//! the compiled binary: before this round each one left the worktree silently behind the ref it
//! moved, and the next unrelated `commit --from-worktree` authored a spurious `delete-file`. After
//! this round, that same commit refuses, `branch switch <the current branch>` resyncs, and the
//! following commit is clean.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn current_ref_state_id(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap()
}

fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout);
    let ref_state_id = current_ref_state_id(repo, ref_name);
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
        .output()
        .unwrap()
}

fn worktree_status(repo: &Path) -> Output {
    support::prikk(repo)
        .args(["worktree-status"])
        .output()
        .unwrap()
}

fn commit_unrelated(repo: &Path, path: &str, message: &str) -> Output {
    std::fs::write(repo.join(path), b"unrelated\n").unwrap();
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--from-worktree", "-m", message])
        .output()
        .unwrap()
}

fn merge_into_main(repo: &Path, baseline: ObjectId, from_ref: &str) -> Output {
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            from_ref,
        ])
        .output()
        .unwrap()
}

fn baseline_record_path(repo: &Path) -> PathBuf {
    repo.join(".prikk").join("worktree.baseline")
}

/// Set up `heads/main` (one file, sealed), create `sibling_name` from the same tip, switch to it,
/// write+commit+seal `extra_path` there, then switch back to `heads/main`. Real `branch switch`
/// round trips, not a bare `branch create` plus a worktree write left lying around -- the latter
/// would leave `extra_path` physically on disk the whole time (`branch create` never touches the
/// worktree, but writing a file by hand and never removing it does), which cannot reproduce "the
/// worktree genuinely lacks this file" the way the design report's own fixtures do. Returns
/// `heads/main`'s own genesis block id (for `merge --baseline-block`).
fn setup_main_and_sealed_sibling(repo: &Path, sibling_name: &str, extra_path: &str) -> ObjectId {
    support::generation(repo, "heads/main", "a.txt", b"a\n", "genesis");
    let baseline = tip_block_id(repo, "heads/main");
    ok(
        &support::branch_create(repo, sibling_name, "heads/main"),
        "branch create sibling",
    );
    ok(&switch(repo, sibling_name), "switch to sibling");
    support::generation(
        repo,
        sibling_name,
        extra_path,
        b"sibling content\n",
        "sibling adds its own file",
    );
    ok(&switch(repo, "heads/main"), "switch back to heads/main");
    assert!(
        !repo.join(extra_path).exists(),
        "switching back to heads/main must remove {extra_path}, which it never had"
    );
    baseline
}

/// §2.1: `merge --into heads/main` while `heads/main` is current -- the merge never touches the
/// worktree, so `c.txt` (brought in from `heads/feature`) is on `tree --ref heads/main` but not on
/// disk. Before this round, an unrelated commit silently authored `delete-file c.txt`; now it
/// refuses, and `branch switch heads/main` (the current branch) resyncs.
#[test]
fn case1_merge_into_refuses_then_resyncs() {
    let repo = support::unique_repo("w2-case1-merge");
    support::init(&repo);
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");

    let merge = merge_into_main(&repo, baseline, "heads/feature");
    ok(&merge, "merge --into heads/main");
    assert!(
        !repo.join("c.txt").exists(),
        "the merge must not touch the worktree -- c.txt stays absent on disk"
    );

    // The hazard: before this round, this commit silently authored `delete-file c.txt`.
    let refused = commit_unrelated(&repo, "extra1.txt", "unrelated edit after merge");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must now refuse: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit")
            && stderr(&refused).contains("c.txt")
            && stderr(&refused).contains("never were"),
        "the refusal must name c.txt as never held: {}",
        stderr(&refused)
    );
    // The resync.
    let resync = switch(&repo, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    assert!(
        repo.join("c.txt").exists(),
        "the resync must materialize c.txt: {}",
        stdout(&resync)
    );

    // `extra1.txt` is a real untracked file at this point (the refused commit above wrote it to
    // disk before refusing, since a refusal does not undo the worktree edit that preceded it) --
    // remove it to isolate what the resync itself did: everything else must be clean.
    std::fs::remove_file(repo.join("extra1.txt")).unwrap();
    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must be clean after the resync, apart from the refused commit's own \
         untracked leftover (removed above): {}",
        stdout(&status)
    );

    // The following commit is now clean -- no stray `delete-file`.
    let clean = commit_unrelated(&repo, "extra1.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("delete-file"),
        "no stray delete-file after the resync: {clean_stdout}"
    );
    assert!(
        clean_stdout.contains("create-file extra1.txt"),
        "the only authored operation is the real edit: {clean_stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// §2.2: `archive import --adopt` onto the unpublished current branch.
#[test]
fn case2_archive_import_adopt_refuses_then_resyncs() {
    let src = support::unique_repo("w2-case2-src");
    support::init(&src);
    support::generation(&src, "heads/main", "n.txt", b"n\n", "genesis");
    let archive_path =
        std::env::temp_dir().join(format!("prikk-w2-case2-{}.prepo", support::unique_suffix()));
    ok(
        &support::prikk(&src)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );

    let dest = support::unique_repo("w2-case2-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
    let import = support::prikk(&dest)
        .args(["archive", "import", "--adopt"])
        .arg(&archive_path)
        .output()
        .unwrap();
    ok(&import, "archive import --adopt");
    assert!(
        !dest.join("n.txt").exists(),
        "adopt never touches the worktree"
    );

    let refused = commit_unrelated(&dest, "extra2.txt", "unrelated edit after adopt");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("n.txt") && stderr(&refused).contains("never were"),
        "refusal names n.txt: {}",
        stderr(&refused)
    );

    let resync = switch(&dest, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    assert!(dest.join("n.txt").exists(), "resync materializes n.txt");

    let clean = commit_unrelated(&dest, "extra2.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");
    assert!(!stdout(&clean).contains("delete-file"));

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dest);
    let _ = std::fs::remove_file(&archive_path);
}

/// §2.3: `sync seal <current> --claim` while `heads/main` is current and unpublished.
#[test]
fn case3_sync_seal_claim_refuses_then_resyncs() {
    let repo_a = support::unique_repo("w2-case3-a");
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "s.txt", b"s\n", "genesis");

    let repo_b = support::unique_repo("w2-case3-b");
    support::init(&repo_b);

    let have_file =
        std::env::temp_dir().join(format!("w2-case3-have-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact_file = std::env::temp_dir().join(format!(
        "w2-case3-artifact-{}.bin",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&repo_a)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                artifact_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );
    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    ok(&accept, "sync accept");
    let accept_stdout = stdout(&accept);
    let claim_id = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .and_then(|line| line.trim_start().strip_prefix("claim "))
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .trim()
        .to_string();

    support::trust_maintainer(&repo_b);
    let seal = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/main", "--claim", &claim_id])
        .output()
        .unwrap();
    ok(&seal, "sync seal --claim");
    assert!(
        !repo_b.join("s.txt").exists(),
        "sync seal never touches the worktree"
    );

    let refused = commit_unrelated(&repo_b, "extra3.txt", "unrelated edit after sync seal");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse: {}",
        stdout(&refused)
    );
    assert!(stderr(&refused).contains("s.txt") && stderr(&refused).contains("never were"));

    let resync = switch(&repo_b, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    assert!(repo_b.join("s.txt").exists());

    let clean = commit_unrelated(&repo_b, "extra3.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");
    assert!(!stdout(&clean).contains("delete-file"));

    let _ = std::fs::remove_dir_all(&repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);
}

/// §2.4: `commit --ref heads/feature` from a worktree recorded against `heads/main` refuses
/// directly, at `commit` time -- it must never reach the point of authoring the bad patch at all
/// (the hazard this round fixes is the authoring itself, not just its later `branch switch`).
#[test]
fn case4_commit_ref_other_refuses_before_authoring() {
    let repo = support::unique_repo("w2-case4");
    support::init(&repo);
    let _baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "f.txt");

    std::fs::write(repo.join("q.txt"), b"q\n").unwrap();
    let refused = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args([
            "commit",
            "--from-worktree",
            "--ref",
            "heads/feature",
            "-m",
            "queued for feature",
        ])
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit --ref heads/feature must refuse outright: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("f.txt") && stderr(&refused).contains("never were"),
        "refusal names f.txt as never held by this worktree: {}",
        stderr(&refused)
    );

    // Confirm nothing was queued: status shows no patches, and an ordinary commit on main is
    // unaffected.
    let status = support::prikk(&repo).args(["status"]).output().unwrap();
    ok(&status, "status");
    assert!(
        !stdout(&status).contains("queued patches: 1"),
        "no patch was queued by the refused commit: {}",
        stdout(&status)
    );

    let clean = commit_unrelated(&repo, "q.txt", "ordinary edit on main");
    ok(&clean, "ordinary commit on main");

    let _ = std::fs::remove_dir_all(&repo);
}

/// §2.5: `branch create heads/main --from heads/other` onto the unborn current branch, where
/// `heads/other` holds content this worktree never physically received. `branch create` refuses a
/// *received* ref outright ("taken into a local branch by `merge --from`"), so `heads/other` must
/// be a real local branch -- built here via `sync seal --claim` (`seal_from_accepted.rs`, not this
/// round's `commit`/CLI-`seal` record-writers), the same mechanism case 3 uses, so its content
/// arrives from a second repository without ever being committed from *this* worktree. (Populating
/// it with a plain local `commit --ref heads/other` from this same worktree, as case 3's design-
/// report transcript for `heads/other` might suggest, is byte-for-byte the same shape
/// `dc74_merge_execution`'s legitimate sibling-branch commits already prove is safe -- see this
/// file's module doc and the `node_authoring.rs` deviation it documents.)
#[test]
fn case5_branch_create_from_onto_unborn_current_refuses_then_resyncs() {
    let src = support::unique_repo("w2-case5-src");
    support::init(&src);
    support::generation(&src, "heads/main", "o.txt", b"o\n", "genesis");

    let repo = support::unique_repo("w2-case5-dest");
    support::init(&repo);
    // heads/main stays unborn. heads/other is built by sync, never locally committed here.
    let have_file =
        std::env::temp_dir().join(format!("w2-case5-have-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(&repo)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact_file = std::env::temp_dir().join(format!(
        "w2-case5-artifact-{}.bin",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&src)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                artifact_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );
    let accept = support::prikk(&repo)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    ok(&accept, "sync accept");
    let accept_stdout = stdout(&accept);
    let claim_id = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .and_then(|line| line.trim_start().strip_prefix("claim "))
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .trim()
        .to_string();
    support::trust_maintainer(&repo);
    let sync_seal = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/other", "--claim", &claim_id])
        .output()
        .unwrap();
    ok(
        &sync_seal,
        "sync seal heads/other --claim (builds the local sibling)",
    );
    assert!(
        !repo.join("o.txt").exists(),
        "sync seal never touches the worktree"
    );

    let create = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["branch", "create", "heads/main", "--from", "heads/other"])
        .output()
        .unwrap();
    ok(&create, "branch create heads/main --from heads/other");
    assert!(
        !repo.join("o.txt").exists(),
        "branch create never touches the worktree"
    );

    let refused = commit_unrelated(
        &repo,
        "extra5.txt",
        "unrelated edit after branch create --from",
    );
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse: {}",
        stdout(&refused)
    );
    assert!(stderr(&refused).contains("o.txt") && stderr(&refused).contains("never were"));

    let resync = switch(&repo, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    assert!(repo.join("o.txt").exists());

    let clean = commit_unrelated(&repo, "extra5.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");
    assert!(!stdout(&clean).contains("delete-file"));

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);

    let _ = std::fs::remove_dir_all(&repo);
}

/// A legitimate deletion -- the record is up to date, and the user genuinely `rm`s a tracked file
/// -- still commits `delete-file`. Proves R1/R2 did not make every deletion refuse.
#[test]
fn a_legitimate_deletion_still_commits() {
    let repo = support::unique_repo("w2-legit-delete");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    std::fs::write(repo.join("b.txt"), b"b\n").unwrap();
    ok(
        &support::commit(&repo, "heads/main", "add b.txt"),
        "commit b.txt",
    );
    ok(&support::seal(&repo, "heads/main"), "seal");

    std::fs::remove_file(repo.join("b.txt")).unwrap();
    let commit = support::commit(&repo, "heads/main", "delete b.txt");
    ok(&commit, "commit the real deletion");
    assert!(
        stdout(&commit).contains("delete-file b.txt"),
        "the real deletion is still authored: {}",
        stdout(&commit)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// A repository whose record is absent (a pre-0.56.0 repository, or one an older binary wrote to)
/// behaves exactly as today: never spuriously refuses because a file is merely absent.
#[test]
fn a_repository_with_no_record_behaves_as_today() {
    let repo = support::unique_repo("w2-no-record");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");

    // Simulate a repository from before this round: remove the record this round's own `init`
    // just wrote.
    std::fs::remove_file(baseline_record_path(&repo)).unwrap();

    std::fs::write(repo.join("b.txt"), b"b\n").unwrap();
    let commit = support::commit(&repo, "heads/main", "add b.txt, no record");
    ok(&commit, "commit with no record present");
    assert!(!stdout(&commit).contains("delete-file"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// §2.7's deadlock, named: before this round, neither "switch to yourself" (a documented no-op)
/// nor "switch away and back" (refused by `require_clean`) could resync a stale worktree --
/// committing the loss was the only way back to "clean". After this round, both work.
#[test]
fn section_2_7_deadlock_is_dissolved() {
    let repo = support::unique_repo("w2-section-2-7");
    support::init(&repo);
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");
    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    assert!(!repo.join("c.txt").exists());

    // Half 1: "switch to yourself" -- today (pre-W2) a documented no-op (RFC 147 §2j) that leaves
    // the worktree exactly as stale as it found it. Now it resyncs.
    let self_switch = switch(&repo, "heads/main");
    ok(&self_switch, "branch switch heads/main (self, stale)");
    assert!(
        repo.join("c.txt").exists(),
        "self-switch must materialize c.txt once stale, not stay a no-op: {}",
        stdout(&self_switch)
    );
    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "clean after the self-switch resync"
    );

    // Half 2: "switch away and back" -- today (pre-W2) `require_clean`'s refusal 3 diffed the
    // branch being left against its *live* tip (which already has the new content), so leaving
    // was refused outright. Reproduce the staleness again, independently, and confirm leaving now
    // succeeds and coming back still leaves a clean worktree.
    let repo2 = support::unique_repo("w2-section-2-7-away");
    support::init(&repo2);
    let baseline2 = setup_main_and_sealed_sibling(&repo2, "heads/feature", "c.txt");
    ok(
        &support::branch_create(&repo2, "heads/other", "heads/main"),
        "branch create other",
    );
    ok(
        &merge_into_main(&repo2, baseline2, "heads/feature"),
        "merge --into heads/main",
    );
    assert!(!repo2.join("c.txt").exists());

    let away = switch(&repo2, "heads/other");
    ok(&away, "branch switch heads/other (away, from stale main)");

    let back = switch(&repo2, "heads/main");
    ok(&back, "branch switch heads/main (back)");
    assert!(
        repo2.join("c.txt").exists(),
        "coming back must land on main's real, current tip: {}",
        stdout(&back)
    );
    let status2 = worktree_status(&repo2);
    assert_eq!(
        status2.status.code(),
        Some(0),
        "clean after switching away and back"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&repo2);
}
