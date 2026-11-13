//! 0.57.0 release prep P3c (external review 050, section 4): plain `prikk doctor` writes nothing under `.prikk/`.
//!
//! The defect: with the current branch's worktree record absent, damaged or empty, or with a queued commit, `doctor` rewrote
//! `.prikk/cache/lifecycle-state.v1` on every run (`steps_since_reanchor` went 1, 2, 3). 0.56.0's `doctor` left it alone, and
//! `platform-support.md` says plain `doctor` is read-only. The test is not about that one file: every file under `.prikk/`
//! (path, length, content) must be identical after three runs, in each shape the reviewer found and in the clean case.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every file under `.prikk/` except lock files (which `doctor` takes and releases), as path -> bytes.
fn digest(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("lock"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn doctor(repo: &Path) -> std::process::Output {
    support::prikk(repo).arg("doctor").output().unwrap()
}

/// A repository with two sealed generations, so the lifecycle cache holds a baseline.
fn sealed_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "g1.txt", b"one\n", "one");
    support::generation(&repo, "heads/main", "g2.txt", b"two\n", "two");
    repo
}

/// Run `doctor` once to settle anything a first run may legitimately create, then three more times, and require every file under
/// `.prikk/` to be identical after each of the three. (A first run that writes is itself the defect, so it is not exempt: the
/// digest is taken before the first run, and the three runs are compared with it.)
fn assert_plain_doctor_writes_nothing(repo: &Path, shape: &str) -> String {
    let before = digest(repo);
    let mut last = String::new();
    for run in 1..=3 {
        let output = doctor(repo);
        last = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let after = digest(repo);
        for (path, bytes) in &after {
            match before.get(path) {
                None => panic!("{shape}: doctor run {run} created {}", path.display()),
                Some(old) if old != bytes => panic!(
                    "{shape}: doctor run {run} changed {} (length {} -> {})",
                    path.display(),
                    old.len(),
                    bytes.len()
                ),
                Some(_) => {}
            }
        }
        for path in before.keys() {
            assert!(
                after.contains_key(path),
                "{shape}: doctor run {run} removed {}",
                path.display()
            );
        }
    }
    last
}

#[test]
fn a_clean_repository() {
    let repo = sealed_repo("p3c-clean");
    assert_plain_doctor_writes_nothing(&repo, "clean");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_record_absent() {
    let repo = sealed_repo("p3c-absent");
    std::fs::remove_file(repo.join(".prikk/worktree.baseline")).unwrap();
    let said = assert_plain_doctor_writes_nothing(&repo, "record absent");
    // doctor's finding does not change: it still says the record is ignored and names the way out.
    assert!(
        said.contains("PRIKK-DOCTOR-WORKTREE-RECORD-IGNORED"),
        "{said}"
    );
    assert!(said.contains("prikk branch switch heads/main"), "{said}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_record_damaged() {
    let repo = sealed_repo("p3c-damaged");
    std::fs::write(
        repo.join(".prikk/worktree.baseline"),
        b"this is not a record\n",
    )
    .unwrap();
    let said = assert_plain_doctor_writes_nothing(&repo, "record damaged");
    assert!(
        said.contains("PRIKK-DOCTOR-WORKTREE-RECORD-IGNORED"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_record_empty() {
    let repo = sealed_repo("p3c-empty");
    std::fs::write(repo.join(".prikk/worktree.baseline"), b"").unwrap();
    let said = assert_plain_doctor_writes_nothing(&repo, "record empty");
    assert!(
        said.contains("PRIKK-DOCTOR-WORKTREE-RECORD-IGNORED"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_queued_commit_with_the_record_present() {
    let repo = sealed_repo("p3c-queued");
    std::fs::write(repo.join("q.txt"), b"queued\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued"), "commit");
    assert!(repo.join(".prikk/worktree.baseline").is_file());
    assert_plain_doctor_writes_nothing(&repo, "queued commit, record present");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_queued_commit_with_the_record_absent() {
    let repo = sealed_repo("p3c-queued-absent");
    std::fs::write(repo.join("q.txt"), b"queued\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued"), "commit");
    std::fs::remove_file(repo.join(".prikk/worktree.baseline")).unwrap();
    assert_plain_doctor_writes_nothing(&repo, "queued commit, record absent");
    let _ = std::fs::remove_dir_all(&repo);
}

/// A second path to the same status derivation: `doctor --discard-damaged-commits --plan-only` computes whether the working tree
/// may still hold the discarded content, and a plan writes nothing. (Built as `rfc166_round2_refusals_speak_first` builds it: a
/// queued, witnessed commit with one byte of its record flipped.)
#[test]
fn discard_damaged_commits_plan_only_writes_nothing() {
    let repo = sealed_repo("p3c-discard-plan");
    std::fs::write(repo.join("queued.txt"), b"queued\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued"), "commit");
    let wal = repo.join(".prikk/active/default/queue.wal");
    let mut bytes = std::fs::read(&wal).unwrap();
    let at = bytes.len() - 5;
    bytes[at] ^= 0xFF;
    std::fs::write(&wal, &bytes).unwrap();
    // The record absent too, so the cache is not warm in the step the plan takes.
    std::fs::remove_file(repo.join(".prikk/worktree.baseline")).unwrap();
    let before = digest(&repo);
    let output = support::prikk(&repo)
        .args(["doctor", "--discard-damaged-commits", "--plan-only"])
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(said.contains("plan"), "the verb ran as a plan: {said}");
    let after = digest(&repo);
    for (path, bytes) in &after {
        assert_eq!(
            before.get(path),
            Some(bytes),
            "a plan changed or created {}\n{said}",
            path.display()
        );
    }
    assert_eq!(before.len(), after.len(), "a plan removed a file\n{said}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Every `--plan-only` verb that can run on a healthy repository, in the shapes that reached the refreshing status (record absent;
/// a queued commit): a plan writes nothing under `.prikk/`, `--rebuild-caches`'s plan included.
#[test]
fn the_plan_only_verbs_write_nothing_on_a_repository_with_the_record_absent_and_a_queued_commit() {
    for verb in [
        &["--repair-tails", "--plan-only"][..],
        &["--rebuild-pointer-index", "--plan-only"],
        &["--rebuild-caches", "--plan-only"],
        &["--recovery-clear", "--plan-only"],
    ] {
        let repo = sealed_repo("p3c-plans");
        std::fs::write(repo.join("q.txt"), b"queued\n").unwrap();
        support::ok(&support::commit(&repo, "heads/main", "queued"), "commit");
        std::fs::remove_file(repo.join(".prikk/worktree.baseline")).unwrap();
        let before = digest(&repo);
        let output = support::prikk(&repo)
            .arg("doctor")
            .args(verb)
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let after = digest(&repo);
        for (path, bytes) in &after {
            assert_eq!(
                before.get(path),
                Some(bytes),
                "doctor {verb:?} changed or created {}\n{said}",
                path.display()
            );
        }
        assert_eq!(
            before.len(),
            after.len(),
            "doctor {verb:?} removed a file\n{said}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}
