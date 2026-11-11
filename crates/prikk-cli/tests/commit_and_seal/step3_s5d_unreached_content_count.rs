//! 0.57.0 step 3, part S5d: blobs and patches that nothing reaches are one count line in `verify` and `doctor`, not a warning
//! each. Nothing was published under them, so there is no trust question; a Block or a RefState keeps its warning.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support;
use crate::support::ok;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn repo_with(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    repo
}

fn commit_file(repo: &Path, path: &str, body: &[u8]) -> String {
    std::fs::write(repo.join(path), body).unwrap();
    let output = support::commit(repo, "heads/main", "m");
    ok(&output, "commit");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints its patch id")
        .to_string()
}

fn verify(repo: &Path) -> Output {
    support::prikk(repo).arg("verify").output().unwrap()
}

fn count_lines(said: &str) -> Vec<&str> {
    said.lines()
        .filter(|line| line.starts_with("unreached content: "))
        .collect()
}

/// What `uncommit` of a binary patch leaves: one blob. `verify` says so in one line that is not a warning, `doctor` in one
/// `info`, exit codes unchanged, and `--format json` still lists the blob by id and type.
#[test]
fn uncommit_of_a_binary_patch_leaves_one_count_line_and_the_json_lists_the_blob() {
    let repo = repo_with("s5d-uncommit");
    let id = commit_file(&repo, "d.bin", b"bin\0\x01\x02");
    ok(
        &support::prikk(&repo)
            .args(["uncommit", &id])
            .output()
            .unwrap(),
        "uncommit",
    );

    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(said.contains("unreached objects: 1"), "{said}");
    let lines = count_lines(&said);
    assert_eq!(lines.len(), 1, "exactly one count line: {said}");
    assert!(
        lines[0].contains("1 object(s) (0 patch(es), 1 blob(s))"),
        "{}",
        lines[0]
    );
    for part in [
        "harmless",
        "no command removes them yet",
        "--format json",
        "uncommit",
    ] {
        assert!(lines[0].contains(part), "{part}: {}", lines[0]);
    }
    assert!(
        !said.lines().any(|line| line.starts_with("warning: blob ")),
        "a blob is no longer a warning of its own: {said}"
    );

    let doctor = support::prikk(&repo).arg("doctor").output().unwrap();
    let doctor_said = text(&doctor);
    assert_eq!(doctor.status.code(), Some(0), "{doctor_said}");
    assert!(
        doctor_said.contains("info [PRIKK-DOCTOR-UNREACHED-CONTENT]"),
        "{doctor_said}"
    );
    assert!(
        !doctor_said.contains("PRIKK-DOCTOR-UNREACHED-OBJECT"),
        "{doctor_said}"
    );

    let json = support::prikk(&repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(0));
    let value = support::json::parse(&String::from_utf8_lossy(&json.stdout));
    let listed = value.get("unreached_objects").as_array();
    assert_eq!(listed.len(), 1, "the JSON lists every unreached object");
    assert_eq!(listed[0].get("object_type").as_str(), "blob");
    let _ = std::fs::remove_dir_all(&repo);
}

/// A repository with no unreached content prints no count line at all (a zero line would be noise).
#[test]
fn a_repository_with_nothing_unreached_prints_no_count_line() {
    let repo = repo_with("s5d-none");
    commit_file(&repo, "a.txt", b"a\n");
    let said = text(&verify(&repo));
    assert!(count_lines(&said).is_empty(), "{said}");
    assert!(said.contains("unreached objects: 0"), "{said}");
    let _ = std::fs::remove_dir_all(&repo);
}
