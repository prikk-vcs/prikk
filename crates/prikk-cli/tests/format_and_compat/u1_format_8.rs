//! 0.57.0 U1 (Unicode repository paths), through the compiled binary: format 8, the upgrade from 7, and the one place prikk
//! suggests it -- the refusal to author a non-ASCII path in a repository that does not admit one.
//!
//! A repository this binary created holds nothing a format-6 or format-7 one could not, so setting its marker to 6 or 7 makes a
//! genuine older repository (the technique `rfc156_format_7.rs` uses for format 6). Linux and macOS only for the non-ASCII names:
//! the names are created by the tests themselves, so a filesystem that refuses one fails loudly rather than skipping.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::json;
use std::path::{Path, PathBuf};
use std::process::Output;

const NON_ASCII: &str = "日本語.md";

fn run(repo: &Path, args: &[&str]) -> Output {
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

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn marker(repo: &Path) -> Vec<u8> {
    std::fs::read(repo.join(".prikk").join("FORMAT")).unwrap()
}

fn set_marker(repo: &Path, number: u8) {
    std::fs::write(repo.join(".prikk").join("FORMAT"), [b'0' + number, b'\n']).unwrap();
}

/// Every file under `.prikk` except lock files: what "nothing was written" is compared against.
fn store_bytes(repo: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
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

/// A repository at `format` with `a.txt` committed and a non-ASCII file waiting in the worktree.
fn repository_with_a_non_ascii_file(tag: &str, format: u8) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&run(&repo, &["commit", "-m", "add a.txt"]), "commit a.txt");
    set_marker(&repo, format);
    std::fs::write(repo.join(NON_ASCII), "nihongo\n").unwrap();
    repo
}

/// **The suggestion, in a format-7 and a format-6 repository.** `commit` refuses, naming the path, the format it needs, the format
/// the repository is at, the command, and what the upgrade costs; and it writes nothing.
#[test]
fn an_older_format_refuses_a_non_ascii_path_and_names_the_upgrade() {
    for format in [6u8, 7] {
        let repo = repository_with_a_non_ascii_file(&format!("u1-refuse-{format}"), format);
        let before = store_bytes(&repo);
        let refused = run(&repo, &["commit", "-m", "add the non-ASCII file"]);
        let said = text(&refused);
        assert_eq!(refused.status.code(), Some(1), "format {format}: {said}");
        for part in [
            NON_ASCII,
            "need repository format 8",
            &format!("this repository is format {format}"),
            "run `prikk format upgrade`",
            "cannot open a format-8 repository",
        ] {
            assert!(
                said.contains(part),
                "format {format}: missing {part:?} in: {said}"
            );
        }
        assert!(
            store_bytes(&repo) == before,
            "format {format}: a refused commit writes nothing"
        );
        assert_eq!(marker(&repo), [b'0' + format, b'\n']);
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// `worktree-status` reports the same decision, with the string `commit` prints: one rule rendered twice.
#[test]
fn status_reports_the_same_refusal_commit_prints() {
    let repo = repository_with_a_non_ascii_file("u1-status-agrees", 7);
    let refused = run(&repo, &["commit", "-m", "add the non-ASCII file"]);
    let printed = String::from_utf8_lossy(&refused.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("error: "))
        .expect("commit prints an error line")
        .to_string();

    let status = run(&repo, &["worktree-status", "--format", "json"]);
    let report = json::parse(&String::from_utf8_lossy(&status.stdout));
    let entry = report
        .get("changes")
        .as_array()
        .iter()
        .find(|change| change.get("path").as_str() == NON_ASCII)
        .unwrap_or_else(|| panic!("no entry for {NON_ASCII}: {report:?}"))
        .clone();
    assert_eq!(entry.get("kind").as_str(), "untracked");
    assert_eq!(entry.get("authoring").as_str(), "refused");
    assert!(
        printed.ends_with(entry.get("refusal").as_str()),
        "commit printed:\n{printed}\nstatus said:\n{entry:?}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// A format-8 repository never suggests the upgrade -- it is already there -- whatever else it does with the name.
#[test]
fn a_format_8_repository_does_not_suggest_the_upgrade() {
    let repo = repository_with_a_non_ascii_file("u1-format-8-silent", 8);
    let attempt = run(&repo, &["commit", "-m", "add the non-ASCII file"]);
    assert!(
        !text(&attempt).contains("format upgrade"),
        "{}",
        text(&attempt)
    );
    let status = text(&run(&repo, &["worktree-status"]));
    assert!(!status.contains("format upgrade"), "{status}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// An ASCII-only repository at format 7 is never prompted to upgrade: not by `commit`, not by `worktree-status`.
#[test]
fn an_ascii_only_repository_is_never_prompted_to_upgrade() {
    let repo = support::unique_repo("u1-ascii-silent");
    support::init(&repo);
    set_marker(&repo, 7);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    let committed = run(&repo, &["commit", "-m", "add a.txt"]);
    support::ok(&committed, "an ASCII commit in a format-7 repository");
    for output in [
        text(&committed),
        text(&run(&repo, &["worktree-status"])),
        text(&run(&repo, &["status"])),
        text(&run(&repo, &["verify"])),
    ] {
        assert!(!output.contains("format upgrade"), "{output}");
    }
    assert_eq!(marker(&repo), b"7\n", "nothing moved the format");
    let _ = std::fs::remove_dir_all(&repo);
}

/// `prikk format upgrade` moves a verified format-7 repository to 8 in place, once, and the older-format refusal is then gone.
#[test]
fn format_upgrade_moves_format_7_to_8_and_the_suggestion_is_followed() {
    let repo = repository_with_a_non_ascii_file("u1-upgrade-7", 7);
    let refused = run(&repo, &["commit", "-m", "add the non-ASCII file"]);
    assert!(text(&refused).contains("run `prikk format upgrade`"));

    let upgraded = run(&repo, &["format", "upgrade"]);
    support::ok(&upgraded, "format upgrade");
    assert!(
        text(&upgraded).contains("repository format: upgraded from 7 to 8"),
        "{}",
        text(&upgraded)
    );
    assert!(
        text(&upgraded).contains("prikk 0.56.0 and earlier refuse a format-8 repository"),
        "the cost is stated: {}",
        text(&upgraded)
    );
    assert_eq!(marker(&repo), b"8\n");
    support::ok(&run(&repo, &["verify"]), "verify at format 8");

    let after = run(&repo, &["commit", "-m", "add the non-ASCII file"]);
    assert!(
        !text(&after).contains("format upgrade"),
        "after the upgrade the refusal no longer points at the upgrade: {}",
        text(&after)
    );

    let again = run(&repo, &["format", "upgrade"]);
    support::ok(&again, "second format upgrade");
    assert!(
        text(&again).contains("repository format: already 8; nothing changed"),
        "{}",
        text(&again)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Re-running `init` on a format-7 repository keeps it at 7: the upgrade is explicit, never a side effect.
#[test]
fn init_never_upgrades_an_existing_repository() {
    let repo = support::unique_repo("u1-init-keeps");
    support::init(&repo);
    assert_eq!(marker(&repo), b"8\n", "a new repository is format 8");
    set_marker(&repo, 7);
    support::init(&repo);
    assert_eq!(marker(&repo), b"7\n");
    let _ = std::fs::remove_dir_all(&repo);
}
