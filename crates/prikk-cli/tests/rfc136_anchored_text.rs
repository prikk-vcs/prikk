//! RFC 136 increment 2c, end to end through the binary: editing the same text file across sealed commits takes
//! each edit's baseline text from a replay-verified anchor, says nothing when nothing is wrong, and **names on
//! stderr** a verified anchor that could not supply it (increment 2c addendum §3).
//!
//! The naming control poisons the DC-64 cache (`poison_lifecycle_cache_for_test_support`) so that the true text
//! the anchor yields cannot hash to what the cache claims: the commit must still succeed, by falling back to a
//! full replay, and must say so once. **Perturb:** remove `warn_anchor_fallbacks(prikk_store::
//! take_anchor_fallbacks()...)` from `main`: the warning test goes red.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    support::isolate_key_environment(&mut cmd);
    cmd
}

fn ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-2c-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const AUTHOR_KEY_ID: &str = "rfc136-2c-author";
const AUTHOR_SEED_HEX: &str = "0011223344556677889900112233445566778899001122334455667788990011";
const MAINTAINER_KEY_ID: &str = "rfc136-2c-maintainer";
const MAINTAINER_SEED: [u8; 32] = [0x2C; 32];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn commit(repo: &Path, message: &str) -> Output {
    prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", message])
        .output()
        .unwrap()
}

fn seal(repo: &Path) {
    use prikk_store::MaintainerSigner;
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(MAINTAINER_KEY_ID, &MAINTAINER_SEED)
            .unwrap();
    let _ = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            MAINTAINER_KEY_ID,
            "--public-key",
            &hex(&signer.public_key_bytes()),
        ])
        .output()
        .unwrap();
    let out = prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal");
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A repository whose one text file has been edited and sealed `edits` times.
fn repo_with_edits(tag: &str, edits: usize) -> PathBuf {
    let repo = unique_repo(tag);
    ok(&prikk(&repo).arg("init").output().unwrap(), "init");
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    for generation in 1..=edits {
        let lines = ["one", "two", "three", "four", "five", "six"];
        let mut text = String::new();
        for (index, line) in lines.iter().enumerate() {
            if index < generation {
                text.push_str(&format!("{line}-edit{generation}\n"));
            } else {
                text.push_str(&format!("{line}\n"));
            }
        }
        std::fs::write(repo.join("f.txt"), text).unwrap();
        let out = commit(&repo, &format!("edit {generation}"));
        ok(&out, &format!("edit {generation} commit"));
        seal(&repo);
    }
    repo
}

/// Nothing is wrong, so nothing is said: five sealed edits of one file, every commit's stderr free of a warning,
/// and the final text intact.
#[test]
fn a_run_of_edits_of_one_file_is_silent_on_stderr() {
    let repo = repo_with_edits("silent", 5);
    std::fs::write(
        repo.join("f.txt"),
        "one\ntwo\nthree\nfour\nfive\nsix-final\n",
    )
    .unwrap();
    let out = commit(&repo, "final");
    ok(&out, "final commit");
    assert!(
        !stderr(&out).contains("warning:"),
        "an anchored baseline text says nothing: {}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(repo);
}

/// A verified anchor whose text fails its hash against the cache's claim is named on stderr, and the commit
/// still succeeds (a full replay), with the answer correct.
#[test]
fn a_verified_anchor_that_could_not_supply_a_text_is_named_on_stderr() {
    let repo = repo_with_edits("named", 3);
    let layout = prikk_store::RepositoryLayout::open(repo.clone()).unwrap();
    assert!(
        prikk_store::poison_lifecycle_cache_for_test_support(&layout).unwrap(),
        "a cache to poison: the last commit persisted one"
    );
    std::fs::write(
        repo.join("f.txt"),
        "one\ntwo\nthree\nfour\nfive\nsix-poisoned\n",
    )
    .unwrap();
    let out = commit(&repo, "after a poisoned cache");
    ok(&out, "commit after a poisoned cache");
    let text = stderr(&out);
    assert_eq!(
        text.matches("could not supply a file's text").count(),
        1,
        "named exactly once: {text}"
    );
    assert!(text.contains("prikk verify"), "{text}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("edit-text f.txt"));
    let _ = std::fs::remove_dir_all(repo);
}

// ---- Addendum 2: a request for the cached baseline itself ------------------------------------------------

fn cache_steps(repo: &Path) -> Option<u32> {
    let layout = prikk_store::RepositoryLayout::open(repo.to_path_buf()).unwrap();
    prikk_store::lifecycle_cache_header_for_test_support(&layout).map(|h| h.steps_since_reanchor)
}

fn set_text(repo: &Path, last: &str) {
    std::fs::write(
        repo.join("f.txt"),
        format!("one\ntwo\nthree\nfour\nfive\n{last}\n"),
    )
    .unwrap();
}

/// Several `commit`s between two `seal`s, and `worktree-status` before a `commit`, resolve the same sealed tip:
/// each is a hit counted as a step -- the cache's step count rises by one each time, where it used to reset to 0
/// (a full replay). **Perturb:** remove the `same_baseline_hit` branch: the count resets and this goes red.
#[test]
fn commits_between_seals_and_a_status_before_a_commit_are_hits_not_full_replays() {
    let repo = repo_with_edits("hits", 3);
    set_text(&repo, "six-a");
    ok(&commit(&repo, "first after the seal"), "first commit");
    let first = cache_steps(&repo).expect("a cache");
    set_text(&repo, "six-b");
    ok(&commit(&repo, "second"), "second commit");
    assert_eq!(
        cache_steps(&repo),
        Some(first + 1),
        "the second commit at one tip is a hit"
    );
    set_text(&repo, "six-c");
    ok(&commit(&repo, "third"), "third commit");
    assert_eq!(cache_steps(&repo), Some(first + 2));

    ok(
        &prikk(&repo).arg("worktree-status").output().unwrap(),
        "worktree-status",
    );
    assert_eq!(
        cache_steps(&repo),
        Some(first + 3),
        "worktree-status is a hit too"
    );
    set_text(&repo, "six-d");
    let out = commit(&repo, "after a status");
    ok(&out, "commit after a status");
    assert_eq!(
        cache_steps(&repo),
        Some(first + 4),
        "and so is the commit after it"
    );
    assert!(!stderr(&out).contains("warning:"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(repo);
}

/// A poisoned cache at the baseline being asked for is still something `verify` reports.
#[test]
fn verify_reports_a_poisoned_cache_whose_baseline_is_the_tip() {
    let repo = repo_with_edits("verify-poison", 3);
    set_text(&repo, "six-a");
    ok(
        &commit(&repo, "a commit, so the cache is over the tip"),
        "commit",
    );
    let layout = prikk_store::RepositoryLayout::open(repo.clone()).unwrap();
    assert!(prikk_store::poison_lifecycle_cache_for_test_support(&layout).unwrap());
    let out = prikk(&repo).arg("verify").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("lifecycle-cache divergences: 1"), "{text}");
    let _ = std::fs::remove_dir_all(repo);
}
