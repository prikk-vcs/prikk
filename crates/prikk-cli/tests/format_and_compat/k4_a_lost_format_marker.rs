//! 0.59.0 step K4: `init` never labels a repository it did not make, and every command that meets a lost `FORMAT` marker names a command that works.
//!
//! The states, by what `.prikk` holds and what the marker is: **(b)** an `init` that did not finish, no marker; **(c)** a populated repository, no
//! marker; **(d)** an `init` of a release before K4 killed after creating the marker, which is empty; **(e)** a populated repository whose marker is empty.
//! (State (a), nothing there, is `init`'s ordinary run.) For each: an ordinary command prints the sentence of its state, `init` and `format upgrade`
//! do what the state allows and refuse the rest writing nothing, the command the sentence names exits 0, and `verify` is then clean at format 8.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use crate::support;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn marker(repo: &Path) -> PathBuf {
    repo.join(".prikk/FORMAT")
}

fn run(repo: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = support::prikk(repo).args(args).output().unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn files(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    /// (b)
    KilledInitAbsent,
    /// (c)
    PopulatedAbsent,
    /// (d)
    KilledInitEmpty,
    /// (e)
    PopulatedEmpty,
}

fn make(state: State, tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    let populated = matches!(state, State::PopulatedAbsent | State::PopulatedEmpty);
    if populated {
        std::fs::write(repo.join("one.txt"), "first\n").unwrap();
        support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
        support::ok(&support::seal(&repo, "heads/main"), "seal");
    } else {
        // `prikk init` records the worktree baseline after the marker, so a killed `init` has none.
        std::fs::remove_file(repo.join(".prikk/worktree.baseline")).unwrap();
    }
    std::fs::remove_file(marker(&repo)).unwrap();
    if matches!(state, State::KilledInitEmpty | State::PopulatedEmpty) {
        std::fs::write(marker(&repo), b"").unwrap();
    }
    repo
}

/// The first command the sentence names in backticks.
fn named(sentence: &str) -> String {
    sentence
        .split('`')
        .skip(1)
        .step_by(2)
        .find_map(|inside| inside.strip_prefix("prikk "))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("the sentence names no command: {sentence}"))
}

#[test]
fn every_state_names_a_command_that_works_and_verify_is_then_clean_at_format_8() {
    for state in [
        State::KilledInitAbsent,
        State::PopulatedAbsent,
        State::KilledInitEmpty,
        State::PopulatedEmpty,
    ] {
        let repo = make(state, "k4-states");
        let populated = matches!(state, State::PopulatedAbsent | State::PopulatedEmpty);
        let before = files(&repo);
        // An ordinary command, and one with `--format json` (refusals are text on stderr in both): the sentence of the state, exit 1.
        for args in [
            &["status"][..],
            &["verify"],
            &["status", "--format", "json"],
        ] {
            let (code, text) = run(&repo, args);
            assert_eq!(code, Some(1), "{state:?} {args:?}: {text}");
            assert!(
                !text.contains("required root-relative"),
                "{state:?}: {text}"
            );
            assert!(!text.contains("holds \"\""), "{state:?}: {text}");
            if populated {
                assert!(
                    text.starts_with("error: integrity error: the format marker is lost"),
                    "{state:?}: {text}"
                );
                assert_eq!(named(&text), "format upgrade", "{state:?}: {text}");
                assert!(
                    text.contains("prikk before 0.57.0 does not open format 8"),
                    "{text}"
                );
                assert!(text.contains("restore a sound copy"), "{text}");
            } else {
                assert!(
                    text.starts_with("error: precondition not met: the format marker"),
                    "{state:?}: {text}"
                );
                assert_eq!(named(&text), "init", "{state:?}: {text}");
            }
        }
        assert_eq!(
            files(&repo),
            before,
            "{state:?}: a refused command wrote nothing"
        );
        // The command the state does not allow refuses, naming the one it does, and writes nothing.
        let (refused_args, refused_names): (&[&str], &str) = if populated {
            (&["init"], "format upgrade")
        } else {
            (&["format", "upgrade"], "init")
        };
        let (code, text) = run(&repo, refused_args);
        assert_eq!(code, Some(1), "{state:?} {refused_args:?}: {text}");
        assert_eq!(named(&text), refused_names, "{state:?}: {text}");
        assert_eq!(
            files(&repo),
            before,
            "{state:?}: {refused_args:?} wrote nothing"
        );
        // The one it names exits 0, and the repository is format 8 and verifies.
        let (_, sentence) = run(&repo, &["status"]);
        let command = named(&sentence);
        let words: Vec<&str> = command.split_whitespace().collect();
        let (code, text) = run(&repo, &words);
        assert_eq!(code, Some(0), "{state:?} `prikk {command}`: {text}");
        if populated {
            assert!(text.contains("the format marker was lost"), "{text}");
            assert!(text.contains("is now format 8"), "{text}");
            assert!(
                !text.contains("from"),
                "no earlier number is claimed: {text}"
            );
        }
        assert_eq!(std::fs::read(marker(&repo)).unwrap(), b"8\n", "{state:?}");
        let (code, text) = run(&repo, &["verify"]);
        assert_eq!(code, Some(0), "{state:?}: {text}");
        // And the repository kept everything it held: `format upgrade` moved nothing but the marker.
        if populated {
            let after = files(&repo);
            for (path, bytes) in &before {
                if path.ends_with("FORMAT") {
                    continue;
                }
                if path.to_string_lossy().contains("lock") {
                    continue;
                }
                assert_eq!(
                    after.get(path),
                    Some(bytes),
                    "{state:?}: {}",
                    path.display()
                );
            }
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn a_format_6_or_7_repository_without_its_marker_is_not_relabelled_by_init() {
    for number in ["6\n", "7\n"] {
        let repo = support::unique_repo("k4-older");
        support::init(&repo);
        std::fs::write(repo.join("one.txt"), "first\n").unwrap();
        support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
        support::ok(&support::seal(&repo, "heads/main"), "seal");
        // A new repository holds nothing a format-6 or format-7 repository could not, so this is one; then its marker is lost.
        std::fs::write(marker(&repo), number).unwrap();
        std::fs::remove_file(marker(&repo)).unwrap();
        let before = files(&repo);
        let (code, text) = run(&repo, &["init"]);
        assert_eq!(code, Some(1), "{text}");
        assert!(
            !marker(&repo).exists(),
            "init wrote a label over a repository it did not make"
        );
        assert_eq!(files(&repo), before);
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn an_init_killed_by_a_release_before_k4_is_finished_by_this_one() {
    // The empty marker the K3 sweep found at ordinal 221, on a fresh repository: `init` finishes it, and the history is empty.
    let repo = make(State::KilledInitEmpty, "k4-old-kill");
    let (code, text) = run(&repo, &["init"]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(std::fs::read(marker(&repo)).unwrap(), b"8\n");
    assert_eq!(run(&repo, &["verify"]).0, Some(0));
    let _ = std::fs::remove_dir_all(&repo);
}
