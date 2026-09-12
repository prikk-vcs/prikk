//! RFC 146 §8e: every synopsis whose parser accepts `--format json` must say so — and every
//! synopsis that says so must have a parser that accepts it.
//!
//! **This check is derived, not listed.** Nothing here names `log`, `branch` or `tag`. The test
//! reads `prikk --help`, extracts every invocation it advertises, asks each one's *parser* whether
//! it accepts `--format json`, and compares the two answers. So the next command to gain the flag
//! cannot ship the gap this round fixes, and a synopsis cannot advertise a flag the parser refuses
//! either — the drift is caught in whichever direction it happens.
//!
//! **How a parser is asked.** Not by matching refusal wording — that is a hand-written list wearing
//! a different hat, and the first attempt at it misclassified six commands (`branch close`, five
//! `sync` subcommands) whose positional argument swallows `--format` and produces a refusal in
//! wording all its own. The question asked instead is the one that actually matters: **does this
//! parser distinguish `--format` from an arbitrary flag it has never heard of?** Run the invocation
//! twice in an empty directory that is *not* a repository, once with `--format json` and once with
//! a nonsense flag, then compare the two failures with the flag name normalised away. Identical
//! means `--format` was treated as just another unknown token — the parser does not know it.
//! Different means it does.
//!
//! That probe is side-effect free by construction, which was measured rather than assumed: the temp
//! directory is asserted empty afterwards. `init` and `setup` are the two that could create
//! something, and both refuse `--format` at parse time.
//!
//! **A succeeding invocation is not a broken probe.** The first version asserted that every
//! invocation *fails* outside a repository, reading that as the guarantee of side-effect freedom.
//! RFC 150's `key status` then shipped as a reader that answers outside a repository and exits `0`
//! by design, and the assertion fired on a command that had done nothing. Success and failure are
//! both fine here; what must hold is that nothing was written, and that is what the leftovers check
//! at the end actually measures. A command that succeeds has an empty first stderr line, which
//! differs from the unknown-flag arm exactly as a parser that knows `--format` should.
//!
//! The key environment is neutralised for every invocation: this file builds its own `Command`s, so
//! without the seam the probe would read whatever keys the person running it happens to have.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// One advertised invocation: the literal words after `prikk`, e.g. `["trust", "maintainer",
/// "list"]`. Flags, placeholders and prose are not part of the path.
fn invocation_path(help_line: &str) -> Option<Vec<String>> {
    let rest = help_line.trim_start().strip_prefix("prikk ")?;
    let path: Vec<String> = rest
        .split_whitespace()
        .take_while(|token| {
            token
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !token.starts_with('-')
        })
        .map(str::to_string)
        .collect();
    if path.is_empty() { None } else { Some(path) }
}

/// A flag no parser can possibly know, used as the control arm of the comparison below.
const UNKNOWN_FLAG: &str = "--zzz-not-a-real-flag";

fn first_stderr_line(cwd: &Path, path: &[String], flag: &str) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prikk"));
    support::isolate_key_environment(&mut command);
    let output = command
        .current_dir(cwd)
        .args(path)
        .args([flag, "json"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Ask the parser at `path` whether it knows `--format`, by whether it treats it differently from a
/// flag nothing could know.
fn parser_accepts_format_json(cwd: &Path, path: &[String]) -> bool {
    let with_format = first_stderr_line(cwd, path, "--format");
    let with_nonsense =
        first_stderr_line(cwd, path, UNKNOWN_FLAG).replace(UNKNOWN_FLAG, "--format");
    with_format != with_nonsense
}

#[test]
fn every_parser_that_accepts_format_json_advertises_it_and_vice_versa() {
    let help = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success(), "prikk --help must succeed");
    let help = String::from_utf8_lossy(&help.stdout).into_owned();

    // Group by invocation: `checkout` advertises several lines under one parser, and only some of
    // them mention the flag — the question is whether *any* line for that invocation does.
    let mut advertised: BTreeMap<Vec<String>, bool> = BTreeMap::new();
    for line in help.lines() {
        let Some(path) = invocation_path(line) else {
            continue;
        };
        let says_it = line.contains("--format json");
        let entry = advertised.entry(path).or_insert(false);
        *entry = *entry || says_it;
    }
    assert!(
        advertised.len() > 20,
        "the help text should advertise well over twenty invocations; parsed {} -- the parser \
         below is probably broken, and a check that parses nothing passes vacuously",
        advertised.len()
    );

    let probe_dir = std::env::temp_dir().join(format!(
        "prikk-cli-rfc146-help-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&probe_dir).unwrap();

    let mut missing_from_help = Vec::new();
    let mut missing_from_parser = Vec::new();
    for (path, says_it) in &advertised {
        let accepts = parser_accepts_format_json(&probe_dir, path);
        match (accepts, says_it) {
            (true, false) => missing_from_help.push(path.join(" ")),
            (false, true) => missing_from_parser.push(path.join(" ")),
            _ => {}
        }
    }

    // The probe must not have written anything, or a later run would measure the leftovers.
    let leftovers: Vec<_> = std::fs::read_dir(&probe_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "the probe wrote something into the scratch directory: {leftovers:?}"
    );
    let _ = std::fs::remove_dir_all(&probe_dir);

    assert!(
        missing_from_help.is_empty(),
        "these parsers accept `--format json` but their `--help` synopsis does not say so: {}",
        missing_from_help.join(", ")
    );
    assert!(
        missing_from_parser.is_empty(),
        "these synopses advertise `--format json` but the parser refuses it: {}",
        missing_from_parser.join(", ")
    );
}
