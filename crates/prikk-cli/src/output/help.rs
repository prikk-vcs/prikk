//! Top-level help output.
//!
//! RFC 118 stage 1: this holds no literal command text of its own. It renders
//! [`crate::commands::COMMANDS`] -- the single declaration of the command surface dispatch also
//! derives from -- plus the fixed header/`Usage:` lines and the `--version` meta line, which is a
//! meta-arm (`main.rs::run`'s own match still handles `--help`/`-h`/`--version`/`-V` directly), not
//! a registry entry.

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;

/// The documentation's address, printed as the top-level help's last line.
const DOCS_URL: &str = "https://prikk.org/docs/";

/// The top-level help (0.57.0 item 6): the commands grouped in the order of a day's work, one line each -- the name and the
/// registry's own `summary`. The full synopses are `prikk <command> --help` (also `prikk help <command>`). Built as text so a test
/// can read it without a process.
pub(crate) fn help_text(version: &str) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    let _ = writeln!(text, "prikk {version}\n");
    let _ = writeln!(text, "Usage: prikk <command> [arguments]");
    let _ = writeln!(
        text,
        "       prikk help <command>        (the same as: prikk <command> --help)"
    );
    let _ = writeln!(text, "       prikk --version");
    // The name column is as wide as the longest name, so every summary lines up.
    let width = crate::commands::COMMANDS
        .iter()
        .map(|command| command.name.len())
        .max()
        .unwrap_or(0);
    for group in crate::commands::Group::ALL {
        let _ = writeln!(text, "\n{}", group.title());
        for command in crate::commands::COMMANDS
            .iter()
            .filter(|command| command.group == group)
        {
            let _ = writeln!(text, "  {:<width$}  {}", command.name, command.summary);
        }
    }
    let _ = writeln!(text, "\nEvery command's arguments: prikk help <command>");
    let _ = writeln!(text, "Guides and reference: {DOCS_URL}");
    text
}

/// Print top-level help.
pub(crate) fn print_help(version: &str) {
    for line in help_text(version).lines() {
        println!("{line}");
    }
}

/// Print one command's own `--help`/`-h` output (RFC 121 §2.5) -- its `help_lines` verbatim, the
/// same text [`print_help`] already renders inline for every command. `COMMANDS` is a flat table of
/// top-level dispatchable names, not a per-subcommand tree, so `prikk bundle --help` and
/// `prikk bundle export --help` print the same thing: `bundle`'s full `help_lines`, covering
/// export/import/verify together -- there is no finer-grained table to route a subcommand to, and
/// adding one would be the second table RFC 118 forbids.
pub(crate) fn print_command_help(command: &crate::commands::Command) {
    for line in command.help_lines {
        println!("{line}");
    }
}
