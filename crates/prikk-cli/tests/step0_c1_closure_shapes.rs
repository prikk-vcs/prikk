//! 0.52.0 step 0, Part C1: the archive's own closure now follows the store's real rule, through the
//! compiled binary, for the finding's own two reported shapes (`.git-exclude/reviewed/
//! archive-closure-finding-v1.md`) -- both built with real commands, never hand-crafted bytes.
//! Before this part, `archive export` on either shape wrote a file its own `archive verify`/
//! `archive import` refused, even though the source repository's own `prikk verify` passed cleanly.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn archive_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "prikk-cli-0520-c1-{tag}-{}.prepo",
        support::unique_suffix()
    ))
}

fn commit_and_seal(repo: &Path, message: &str) {
    support::ok(&support::commit(repo, "heads/main", message), message);
    support::ok(&support::seal(repo, "heads/main"), "seal");
}

/// Export, verify and import `repo`'s own archive, asserting every step succeeds -- the shape of
/// assertion both this file's own tests share.
fn assert_archive_round_trips_cleanly(repo: &Path, tag: &str) {
    let path = archive_path(tag);

    let export = support::prikk(repo)
        .args(["archive", "export"])
        .arg(&path)
        .output()
        .unwrap();
    support::ok(&export, "archive export");

    let verify = support::prikk(repo)
        .args(["archive", "verify"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "archive verify must exit 0 for a source repository `prikk verify` already passes: {}\n{}",
        stdout(&verify),
        String::from_utf8_lossy(&verify.stderr)
    );
    assert!(
        stdout(&verify).contains("internally consistent"),
        "verify's own stdout: {}",
        stdout(&verify)
    );

    let dest = support::unique_repo(&format!("0520-c1-{tag}-dest"));
    support::init(&dest);
    let import = support::prikk(&dest)
        .args(["archive", "import"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "archive import must succeed for an archive whose own verify just passed: {}\n{}",
        stdout(&import),
        String::from_utf8_lossy(&import.stderr)
    );

    // C1-5: the two shapes must not stop at `import` succeeding -- the destination's own `prikk
    // verify` must also pass, over the identical content the source repository's own `verify`
    // already passed. The destination adopts the same maintainer key a real receiver would decide
    // to trust (R2: trust is the receiver's own decision, never carried by import itself) -- with
    // that one row settled the same way on both sides, the destination's own verify must agree with
    // the source's.
    support::trust_maintainer(&dest);
    let source_verify = support::verify(repo);
    assert!(
        source_verify.status.success(),
        "the source repository's own prikk verify must pass: {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );
    let dest_verify = support::verify(&dest);
    assert!(
        dest_verify.status.success(),
        "the destination's own prikk verify must pass after import, not merely `archive import` \
         succeeding: {}\n{}",
        stdout(&dest_verify),
        String::from_utf8_lossy(&dest_verify.stderr)
    );
    assert!(
        !stdout(&dest_verify).contains("damaged:"),
        "the destination must carry no damage of its own: {}",
        stdout(&dest_verify)
    );
}

/// Shape 1 of the finding: a repository that ran `sync accept` carries a `RecognitionClaim`
/// referencing another repository's own blocks -- never existence-checked, by standing design
/// (RFC 155 §9.7-adjacent: a claim is never trust-conferring, never existence-checked), so the
/// archive must not require it either.
#[test]
fn a_repository_that_ran_sync_accept_exports_verifies_and_imports_cleanly() {
    let repo_a = support::unique_repo("0520-c1-sync-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"sync accept shape\n",
        "first",
    );

    let repo_b = support::unique_repo("0520-c1-sync-b");
    support::init(&repo_b);

    let summary_file = archive_path("sync-summary");
    support::ok(
        &support::prikk(&repo_a)
            .args([
                "sync",
                "summary",
                "--output",
                summary_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync summary",
    );

    let have_file = archive_path("sync-have");
    support::ok(
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

    let artifact_file = archive_path("sync-artifact");
    let build = support::prikk(&repo_a)
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
        .unwrap();
    support::ok(&build, "sync build");

    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let accept_stdout = stdout(&accept);
    let claim_id = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .and_then(|line| line.trim_start().strip_prefix("claim "))
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("sync accept must print a claim line: {accept_stdout}"))
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
    support::ok(&seal, "sync seal");

    // The source itself must already pass -- confirming the finding's own premise, not assuming it.
    let source_verify = support::prikk(&repo_b).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "repo B's own prikk verify must already pass (the finding's own premise): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );

    assert_archive_round_trips_cleanly(&repo_b, "sync-accept");
}

/// Shape 2 of the finding: a text file edited (so its current content is an `EditText` result
/// nothing stores), sealed, then deleted -- the sealed `Patch`'s own deletion preimage names a Blob
/// id the repository never wrote (DC-65). Never re-derived once sealed, by standing design, so the
/// archive must not require it either.
#[test]
fn an_edited_then_deleted_file_exports_verifies_and_imports_cleanly() {
    let repo = support::unique_repo("0520-c1-edited-deleted");
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("notes.txt"), b"line one\nline two\n").unwrap();
    commit_and_seal(&repo, "create notes.txt");
    std::fs::write(repo.join("notes.txt"), b"line one\nline two, edited\n").unwrap();
    commit_and_seal(&repo, "edit notes.txt");
    std::fs::remove_file(repo.join("notes.txt")).unwrap();
    commit_and_seal(&repo, "delete notes.txt");

    let source_verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "the source repository's own prikk verify must already pass (the finding's own premise): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );

    assert_archive_round_trips_cleanly(&repo, "edited-deleted");
}
