//! 0.56.0 step W3c-3 (`.git-exclude/reviewed/0.56.0-step-3w3c-2-review-v1.md`'s own "W3c-3"
//! ruling): a file in the way of the `--from` target's own files must refuse `branch create
//! --from` *before* it publishes `canonical` -- not only at the pre-existing post-publish backstop
//! (`materialize_after_publish`'s own `in_the_way` check), which, by the time it runs, has already
//! published the ref.
//!
//! **Measured on the build of `959c290d`** (the defect this round fixes): `heads/side` holds
//! `o.txt` and `p.txt`; the worktree's own record is `heads/other`, which holds only `o.txt`; an
//! untracked `p.txt` sits in the worktree already. The clean check passes (an untracked file is
//! never a reason to refuse). `branch create heads/main --from heads/side` then published
//! `heads/main` and set the dirty marker *before* refusing with "... was published, but 1 file(s)
//! are in the way ...". This file's own fixture reproduces that exact shape and proves the new
//! pre-write check (`require_nothing_in_the_way_for_published_ref`, wired into
//! `crates/prikk-cli/src/branch.rs::run_create`) refuses first, writing nothing at all.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::collections::BTreeMap;
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

fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout);
    let ref_state_id = read_current_ref_state_id(repo, ref_name).unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}

fn verify_text(repo: &Path) -> String {
    let output = support::prikk(repo).arg("verify").output().unwrap();
    format!("{}{}", stdout(&output), stderr(&output))
}

fn read_current_ref_state_id(repo: &Path, ref_name: &str) -> Option<ObjectId> {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
}

/// Every file in the repository, worktree **and** `.prikk` both (except `.prikk/cache`), keyed by
/// its path relative to `repo` -- the identical idiom
/// `step3w3c1_merge_refusal_writes_nothing.rs`'s own `whole_repo` (and
/// `step3w3c2_branch_create_from_current_branch.rs`'s own copy of it) already established and
/// grounded, reused rather than re-derived. This also makes a separate "no marker was set" check
/// unnecessary: the dirty marker is itself a file under `.prikk/`, so a byte-identical comparison
/// against the pre-refusal snapshot already proves it stayed unset.
fn whole_repo(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == "cache")
                && path
                    .parent()
                    .is_some_and(|parent| parent == repo.join(".prikk"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(repo).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

/// Build `ref_name` as a real local branch holding every one of `files`, with content this
/// repository's own worktree never physically received -- generalizes
/// `step3w3c2_branch_create_from_current_branch.rs`'s own
/// `build_heads_other_with_content_never_physically_received` to carry more than one file at once
/// (needed here so the target, `heads/side`, can hold a file -- `p.txt` -- the worktree's own
/// *record*, `heads/other`, never does). A disposable `src` repository writes every file in one
/// worktree, commits and seals them together (`commit_worktree_changes_signed` diffs the whole
/// worktree, not a single named path, confirmed by reading `crates/prikk-cli/src/main.rs::run_commit`),
/// and `sync have`/`sync build`/`sync accept`/`sync seal --claim` carries the sealed content into
/// `repo` as `ref_name`, never touching `repo`'s own worktree.
fn build_ref_with_content_never_physically_received(
    repo: &Path,
    ref_name: &str,
    files: &[(&str, &[u8])],
) {
    let src = support::unique_repo("w3c3-branch-create-src");
    support::init(&src);
    for (path, content) in files {
        std::fs::write(src.join(path), content).unwrap();
    }
    ok(
        &support::commit(&src, "heads/main", "genesis"),
        "commit in src (every file, one patch)",
    );
    ok(&support::seal(&src, "heads/main"), "seal in src");

    let have_file =
        std::env::temp_dir().join(format!("w3c3-have-{}.bin", support::unique_suffix()));
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
    let artifact_file =
        std::env::temp_dir().join(format!("w3c3-artifact-{}.bin", support::unique_suffix()));
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
    let accept = support::prikk(repo)
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
    support::trust_maintainer(repo);
    let sync_seal = support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", ref_name, "--claim", &claim_id])
        .output()
        .unwrap();
    ok(
        &sync_seal,
        &format!("sync seal {ref_name} --claim (builds the local sibling)"),
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);
}

/// The ruling's own test list, end to end in one fixture (the same chained-narrative shape
/// `step3w2_worktree_baseline.rs`'s own case 5 already uses): an untracked file at a path the
/// target adds refuses before publish (1), leaving the whole repository byte-identical (2) --
/// which, per `whole_repo`'s own doc above, also proves no marker was set (4) -- and `heads/main`
/// unpublished (3); after the offending file is moved aside, the same command succeeds and
/// materializes (5).
#[test]
fn branch_create_from_an_untracked_file_in_the_way_refuses_before_publish_then_resyncs() {
    let repo = support::unique_repo("w3c3-create-from-in-the-way");
    support::init(&repo);

    // The record: `heads/other`, holding only `o.txt` -- built the same "content this worktree
    // never physically received" way `step3w3c2`'s own clean-worktree fixture uses, except this
    // time the current branch (`heads/main`) is left *current and unborn*, exactly as the
    // architect's own measurement names it. A plain `commit --ref heads/other`/`seal` cycle
    // without a `--from` create or switch leaves the standing worktree-baseline record pointed at
    // `heads/other`'s own sealed genesis block, with nothing physically written to this worktree.
    build_ref_with_content_never_physically_received(&repo, "heads/other", &[("o.txt", b"o\n")]);
    assert!(
        read_current_ref_state_id(&repo, "heads/main").is_none(),
        "fixture sanity: heads/main must still be unborn"
    );
    assert!(
        !repo.join("o.txt").exists(),
        "fixture sanity: heads/other's own content must not be physically present yet"
    );

    // The target: `heads/side`, holding both `o.txt` (the same bytes the record already has, so
    // the clean check -- which never even looks at `heads/side` -- is unaffected) and `p.txt`
    // (which the record does not have at all).
    build_ref_with_content_never_physically_received(
        &repo,
        "heads/side",
        &[("o.txt", b"o\n"), ("p.txt", b"side-p\n")],
    );

    // The untracked file already in the way, at the exact path the target adds -- different
    // content than `heads/side` will bring, so the collision is real, not a no-op "unchanged"
    // match.
    std::fs::write(repo.join("p.txt"), b"zzz\n").unwrap();

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );
    let before = whole_repo(&repo);

    let create = support::branch_create(&repo, "heads/main", "heads/side");
    assert!(
        !create.status.success(),
        "branch create --from must refuse while a file is in the way of the --from target's own \
         files: stdout={} stderr={}",
        stdout(&create),
        stderr(&create)
    );
    let message = stderr(&create);
    assert!(
        message.contains("refusing to creating heads/main")
            && message.contains("in the way")
            && message.contains("p.txt")
            && message.contains("heads/main's files")
            && message.contains("nothing was written"),
        "the new pre-write refusal must name the action, heads/main, p.txt, and say nothing was \
         written: {message}"
    );
    assert!(
        !message.contains("was published"),
        "the old post-publish backstop's own sentence ('... was published, but ...') must not be \
         the one that fires here -- the new pre-write check must refuse first: {message}"
    );

    // Test 1 + 3 (the ruling's own list): `heads/main` stays unpublished.
    assert!(
        read_current_ref_state_id(&repo, "heads/main").is_none(),
        "a refused create must not publish heads/main -- this is exactly the defect this round \
         fixes (measured on 959c290d: heads/main was published before the old refusal ever ran)"
    );
    // Test 2 (the ruling's own list), which also proves test 4 (no marker set): the whole
    // repository -- worktree AND `.prikk` -- is byte-identical. `p.txt` itself is covered too
    // (never overwritten).
    assert_eq!(
        whole_repo(&repo),
        before,
        "a refused create must leave every file -- worktree AND .prikk -- byte-identical; a stray \
         object written to the object store, a moved pointer, or a set marker would all show up \
         here"
    );
    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "a refused create must not leave an orphaned, unreferenced object behind: {}",
        verify_text(&repo)
    );
    assert_eq!(
        std::fs::read(repo.join("p.txt")).unwrap(),
        b"zzz\n",
        "the untracked file in the way must never be overwritten by a refused create"
    );

    // Test 5 (the ruling's own list): after the offending file is moved aside, the same command
    // succeeds and materializes.
    std::fs::rename(repo.join("p.txt"), repo.join("p.txt.aside")).unwrap();
    let retry = support::branch_create(&repo, "heads/main", "heads/side");
    assert!(
        retry.status.success(),
        "branch create --from must succeed once the file in the way is moved aside: stdout={} \
         stderr={}",
        stdout(&retry),
        stderr(&retry)
    );
    assert!(
        stdout(&retry).contains("materialized into the worktree"),
        "the CLI output must say the retried create materialized the worktree: {}",
        stdout(&retry)
    );
    assert_eq!(
        tip_block_id(&repo, "heads/main"),
        tip_block_id(&repo, "heads/side"),
        "heads/main must now point at exactly heads/side's own tip"
    );
    assert_eq!(std::fs::read(repo.join("o.txt")).unwrap(), b"o\n");
    assert_eq!(std::fs::read(repo.join("p.txt")).unwrap(), b"side-p\n");

    // The moved-aside file itself is untracked debris from this test's own fixture, not something
    // `worktree-status` should read as part of the materialized state -- remove it before the
    // final clean-worktree check below.
    std::fs::remove_file(repo.join("p.txt.aside")).unwrap();

    let status = support::prikk(&repo)
        .args(["worktree-status"])
        .output()
        .unwrap();
    ok(&status, "worktree-status after the retried create");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the retried create: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
