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

/// Move `repo`'s own `heads/main` tip forward with a sibling's real content, without ever writing
/// `repo`'s worktree -- `heads/main` stays current throughout, and `sync seal`'s own R3 is **not**
/// landed this round (the W3b report names it deferred), so this reliably reproduces "the current
/// branch moved without the worktree's knowledge," the exact shape `merge --into` itself reproduced
/// before 0.56.0 step W3b closed that gap for merges specifically.
///
/// Mechanism: clone `repo`'s exact `heads/main` genesis into a disposable `mirror` (`archive
/// export` + `archive import --adopt`, byte-identical ids), build `sibling_name` **inside `mirror`
/// only** (so the resulting patch is genuinely new to `repo`'s own object store, not already
/// present under a different ref -- `sync accept`/`sync seal` otherwise read it as "already have
/// this," a no-op, confirmed by running this with the sibling built in `repo` instead), merge it
/// into `mirror`'s own `heads/main` there (a real `BlockKind::Merge` chained onto the shared
/// genesis -- `mirror`'s own worktree outcome is irrelevant and never inspected), then `sync` the
/// two new blocks back into `repo` by `have`/`build`/`accept --claims-out`/`seal --claims`, the same
/// cross-repository shape `case3_sync_seal_claim_refuses_then_resyncs` already uses, just sharing
/// real lineage instead of an unrelated genesis.
fn make_main_stale_via_sync_seal(repo: &Path, sibling_name: &str, extra_path: &str) {
    let archive_file = std::env::temp_dir().join(format!(
        "w2-section-2-7-archive-{}.bin",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(repo)
            .args(["archive", "export"])
            .arg(&archive_file)
            .output()
            .unwrap(),
        "archive export",
    );

    let mirror = support::unique_repo(&format!("w2-section-2-7-mirror-{}", support::unique_suffix()));
    support::init(&mirror);
    support::trust_maintainer(&mirror);
    ok(
        &support::prikk(&mirror)
            .args(["archive", "import", "--adopt"])
            .arg(&archive_file)
            .output()
            .unwrap(),
        "archive import --adopt (clone repo's exact genesis objects)",
    );
    // Materialize `mirror`'s own `heads/main` through the ordinary path first, so the merge below
    // judges a genuinely clean, properly recorded worktree -- not an edge case of its own.
    ok(
        &switch(&mirror, "heads/main"),
        "branch switch heads/main in mirror (normalize the adopted ref)",
    );
    let baseline = tip_block_id(&mirror, "heads/main");
    ok(
        &support::branch_create(&mirror, sibling_name, "heads/main"),
        "branch create sibling in mirror",
    );
    ok(&switch(&mirror, sibling_name), "switch to sibling in mirror");
    support::generation(
        &mirror,
        sibling_name,
        extra_path,
        b"sibling content\n",
        "sibling adds its own file, in mirror only",
    );
    ok(
        &switch(&mirror, "heads/main"),
        "switch back to heads/main in mirror",
    );
    ok(
        &merge_into_main(&mirror, baseline, sibling_name),
        "merge --into heads/main in mirror (builds the new blocks to sync back)",
    );

    let have_file =
        std::env::temp_dir().join(format!("w2-section-2-7-have-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(repo)
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
        "w2-section-2-7-artifact-{}.bin",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&mirror)
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
    let claims_file = std::env::temp_dir().join(format!(
        "w2-section-2-7-claims-{}.txt",
        support::unique_suffix()
    ));
    let accept = support::prikk(repo)
        .args([
            "sync",
            "accept",
            artifact_file.to_str().unwrap(),
            "--claims-out",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    ok(&accept, "sync accept");

    // Sealing a mirrored sibling's own content, not an unborn ref, carries **two** claims here (the
    // shared genesis the merge's own mainline parent names, and the merge block itself) -- `--claims
    // <file>`, not a single `--claim <id>`, seals every claim the file names, ordered parent block
    // first, exactly as the real `receiver-accept` CI job's own `sync seal <ref> --claims <file>`
    // does (`.github/workflows/ci.yml`).
    support::trust_maintainer(repo);
    let seal = support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "seal",
            "heads/main",
            "--claims",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    ok(&seal, "sync seal --claims");
    let _ = std::fs::remove_file(&claims_file);

    let _ = std::fs::remove_dir_all(&mirror);
    let _ = std::fs::remove_file(&archive_file);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);
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

/// §2.1, **closed by 0.56.0 step W3b**: `merge --into heads/main` while `heads/main` is current and
/// the worktree is clean now brings the merge along -- `c.txt` (brought in from `heads/feature`) is
/// materialized into the worktree by the merge itself, so there is no gap left for the next
/// unrelated commit to read as a spurious deletion. Before W3b, the merge never touched the
/// worktree at all, and this same unrelated commit silently authored `delete-file c.txt` until R2's
/// own refusal (and `branch switch heads/main`'s own resync) closed that symptom; W3b closes the
/// cause instead, for this exact shape (`merge --into` the current, clean branch).
#[test]
fn case1_merge_into_materializes_so_nothing_is_left_to_resync() {
    let repo = support::unique_repo("w2-case1-merge");
    support::init(&repo);
    let baseline = setup_main_and_sealed_sibling(&repo, "heads/feature", "c.txt");

    let merge = merge_into_main(&repo, baseline, "heads/feature");
    ok(&merge, "merge --into heads/main");
    assert!(
        repo.join("c.txt").exists(),
        "0.56.0 step W3b: the merge itself must materialize c.txt into the worktree"
    );
    assert_eq!(
        std::fs::read(repo.join("c.txt")).unwrap(),
        b"sibling content\n"
    );
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"a\n");

    // Nothing is stale: worktree-status reads clean immediately after the merge, with no resync
    // needed at all.
    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must already be clean right after the merge: {}",
        stdout(&status)
    );

    // An unrelated commit afterward is ordinary -- no stray `delete-file`, because there is no gap
    // left for it to misread.
    let clean = commit_unrelated(&repo, "extra1.txt", "unrelated edit after merge");
    ok(&clean, "commit after merge");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("delete-file"),
        "no stray delete-file after a materializing merge: {clean_stdout}"
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
    // `merge --into heads/main` itself is excluded (0.56.0 step W3b closed that gap for it):
    // `sync seal --claims`, not yet changed this round, reproduces the identical staleness instead.
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    make_main_stale_via_sync_seal(&repo, "heads/feature", "c.txt");
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
    support::generation(&repo2, "heads/main", "a.txt", b"a\n", "genesis");
    ok(
        &support::branch_create(&repo2, "heads/other", "heads/main"),
        "branch create other",
    );
    make_main_stale_via_sync_seal(&repo2, "heads/feature", "c.txt");
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
