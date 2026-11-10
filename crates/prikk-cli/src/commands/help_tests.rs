//! RFC 121 §2.5 (command-discovery-handoff-v1.md control 5): a static guard that every registry
//! entry actually has help text to route to. `main.rs::run`'s `--help`/`-h` routing prints
//! `command.help_lines` verbatim (`output::print_command_help`) -- it cannot detect an empty slice
//! at runtime and refuse, since an empty slice is a perfectly valid (if useless) value for the
//! field. This is the check that catches a future `Command` added to [`super::COMMANDS`] with
//! `help_lines: &[]`, so it fails a test rather than silently shipping a command whose `--help`
//! prints nothing.
//!
//! End-to-end routing itself (`prikk <command> --help` actually reaching this text, for a plain, a
//! multi-word, and a subcommand-dispatching command) is driven against the real binary in
//! `tests/rfc121_command_help.rs` -- this file only guards the data `COMMANDS` itself carries.

use super::COMMANDS;

#[test]
fn every_command_has_non_empty_help_lines() {
    for command in COMMANDS {
        assert!(
            !command.help_lines.is_empty(),
            "`{}` has no help_lines -- `prikk {} --help` would print nothing",
            command.name,
            command.name
        );
    }
}

/// At least one line per command must actually show `prikk <name>` usage, not only a wrapped
/// continuation or a `note:` aside -- so a command whose `help_lines` accidentally holds only
/// commentary (no usage line at all) is still caught, even though it is technically non-empty.
#[test]
fn every_command_has_at_least_one_usage_line_naming_itself() {
    let prefix_for = |name: &str| format!("  prikk {name}");
    for command in COMMANDS {
        let prefix = prefix_for(command.name);
        assert!(
            command
                .help_lines
                .iter()
                .any(|line| line.starts_with(&prefix)),
            "`{}`'s help_lines has no line starting with {prefix:?}",
            command.name
        );
    }
}

// ---- 0.57.0 item 6: the grouped top-level help -----------------------------------------------------------------------

use super::Group;

/// Every group has at least one command, and every command's group is one the help prints. (Each entry has exactly one group by
/// construction: `group` is a required field of an enum, so a command cannot lack one or spell it wrongly.)
#[test]
fn every_group_is_non_empty_and_every_command_is_in_a_printed_group() {
    for group in Group::ALL {
        assert!(
            COMMANDS.iter().any(|command| command.group == group),
            "the group {:?} has no command, so the help would print an empty heading",
            group.title()
        );
    }
    for command in COMMANDS {
        assert!(
            Group::ALL.contains(&command.group),
            "`{}`'s group is not in Group::ALL, so the help would never print it",
            command.name
        );
    }
}

/// The top-level help names every command exactly once, in its group, and no line is longer than 80 columns.
#[test]
fn the_top_level_help_lists_every_command_once_within_eighty_columns() {
    let text = crate::output::help_text("0.0.0");
    for line in text.lines() {
        assert!(
            line.chars().count() <= 80,
            "{} columns: {line}",
            line.chars().count()
        );
    }
    for command in COMMANDS {
        let listed = text
            .lines()
            .filter(|line| {
                line.starts_with("  ")
                    && !line.starts_with("   ")
                    && line.split_whitespace().next() == Some(command.name)
            })
            .count();
        assert_eq!(listed, 1, "`{}` is listed {listed} times", command.name);
    }
    assert!(text.contains("prikk help <command>") && text.contains("https://prikk.org/docs/"));
}

/// A summary is one short line: not empty, at most 60 columns, a capital first letter, no full stop. (Whether it is *true* of the
/// whole command is read by a person against the command's own help: the report lists each beside its source.)
#[test]
fn every_summary_is_one_short_line() {
    for command in COMMANDS {
        let summary = command.summary;
        assert!(!summary.is_empty(), "`{}` has no summary", command.name);
        assert!(
            summary.chars().count() <= 60,
            "`{}`'s summary is {} columns",
            command.name,
            summary.chars().count()
        );
        assert!(
            !summary.contains('\n') && !summary.ends_with('.'),
            "`{}`: {summary}",
            command.name
        );
        assert!(
            summary.chars().next().is_some_and(char::is_uppercase),
            "`{}`: {summary}",
            command.name
        );
    }
}
