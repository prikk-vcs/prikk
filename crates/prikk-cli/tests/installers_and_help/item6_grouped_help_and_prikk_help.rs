//! 0.57.0 item (6), help that reads: the top-level help is a grouped summary, and `prikk help [command]` works. Through the binary:
//! the alias is exact for every command (the same bytes on both streams and the same exit code), `prikk help` is the top-level help,
//! an unknown name is the unknown-command error, and no top-level line is longer than 80 columns.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::process::Output;

use crate::support;

fn run(args: &[&str]) -> Output {
    let repo = support::unique_repo("item6-help");
    support::init(&repo);
    let output = support::prikk(&repo).args(args).output().unwrap();
    let _ = std::fs::remove_dir_all(&repo);
    output
}

fn same(a: &Output, b: &Output) -> bool {
    a.stdout == b.stdout && a.stderr == b.stderr && a.status.code() == b.status.code()
}

/// The command names the top-level help lists (the group lines: two spaces, then the name).
fn listed_names(top: &str) -> Vec<String> {
    top.lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[test]
fn the_top_level_help_is_a_grouped_summary_within_eighty_columns() {
    let top = run(&["--help"]);
    assert_eq!(top.status.code(), Some(0));
    let text = String::from_utf8_lossy(&top.stdout).into_owned();
    for line in text.lines() {
        assert!(
            line.chars().count() <= 80,
            "{} columns: {line}",
            line.chars().count()
        );
    }
    assert!(text.lines().count() < 80, "{} lines", text.lines().count());
    for heading in [
        "Start",
        "Record your work",
        "Read history and files",
        "Check and repair",
    ] {
        assert!(
            text.lines().any(|line| line == heading),
            "heading {heading}: {text}"
        );
    }
    assert!(text.contains("prikk help <command>"), "{text}");
    assert!(
        text.lines()
            .last()
            .is_some_and(|last| last.contains("https://prikk.org/docs/")),
        "{text}"
    );
    // `uncommit` sits under "Record your work", beside `commit`.
    let lines: Vec<&str> = text.lines().collect();
    let commit = lines
        .iter()
        .position(|l| l.trim_start().starts_with("commit "))
        .unwrap();
    assert!(
        lines[commit + 1].trim_start().starts_with("uncommit "),
        "{text}"
    );
    // The four ways in are one text.
    for args in [&[][..], &["-h"], &["help"]] {
        assert!(same(&run(args), &top), "{args:?} is the top-level help");
    }
}

#[test]
fn help_command_is_exactly_command_dash_dash_help_for_every_command() {
    let top = String::from_utf8_lossy(&run(&["--help"]).stdout).into_owned();
    let names = listed_names(&top);
    assert!(names.len() >= 37, "{} commands listed", names.len());
    for name in &names {
        let alias = run(&["help", name]);
        let direct = run(&[name, "--help"]);
        assert_eq!(direct.status.code(), Some(0), "{name} --help");
        assert!(!direct.stdout.is_empty(), "{name} --help prints something");
        assert!(
            same(&alias, &direct),
            "`prikk help {name}` differs from `prikk {name} --help`"
        );
    }
}

#[test]
fn help_of_an_unknown_command_is_the_unknown_command_error() {
    let unknown = run(&["help", "nonsense"]);
    let direct = run(&["nonsense"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert_eq!(direct.status.code(), Some(2));
    assert_eq!(unknown.stderr, direct.stderr, "the same error");
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown command: nonsense"));
    assert!(unknown.stdout.is_empty());
    // More than one name is a usage error too.
    let extra = run(&["help", "commit", "mv"]);
    assert_eq!(extra.status.code(), Some(2));
}
