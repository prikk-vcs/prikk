//! RFC 146 §8e companion: the binary's own command list and the documented one must be the same list.
//!
//! **Two representations of one fact, both machine-readable** — which is what makes this a gate
//! rather than a style rule. `prikk --help` names every invocation the binary accepts;
//! `docs/src/reference/commands.md` opens with an inventory that claims to be "every command prikk
//! accepts, in one place". Neither is derived from the other, so they can only agree by someone
//! noticing, and at the 0.41.0 readiness sweep they did not: `bundle preview` had shipped in 0.37.0
//! and was absent from the inventory for two releases, and `prikk --version` had never been listed.
//!
//! **Heads only, deliberately.** The comparison is over invocation *heads* — `prikk key status`,
//! `prikk bundle preview` — and not over argument shapes or descriptions. Argument shapes are
//! checked by running the parser, which is the release-prep sweep's job and found four wrong ones
//! the same day; descriptions are prose with no second representation to compare against, which is
//! why the architect refused a gate for them. This check is the part that is mechanical.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::process::Command;

/// The literal words after `prikk`, e.g. `["trust", "maintainer", "list"]`.
///
/// The same parser `rfc146_help_synopsis_advertises_format_json.rs` uses, kept identical
/// deliberately: two checks reading one help text must agree on what an invocation *is*, or a line
/// one of them skips is a line the other silently enforces against nothing.
fn invocation_path(line: &str) -> Option<Vec<String>> {
    let rest = line.trim_start().strip_prefix("prikk ")?;
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

fn heads(lines: impl Iterator<Item = String>) -> BTreeSet<String> {
    lines
        .filter_map(|line| invocation_path(&line))
        .map(|path| path.join(" "))
        .collect()
}

fn help_heads(help: &str) -> BTreeSet<String> {
    // Only the indented synopsis lines are invocations; prose mentioning `prikk commit` is not.
    heads(
        help.lines()
            .filter(|line| line.starts_with("  prikk "))
            .map(str::to_string),
    )
}

/// The inventory block: the fenced list that opens the page. Lines elsewhere on the page mention
/// commands inside prose and are not claims of inventory membership.
fn documented_heads(markdown: &str) -> BTreeSet<String> {
    heads(
        markdown
            .lines()
            .filter(|line| line.starts_with("prikk "))
            .map(str::to_string),
    )
}

fn commands_md() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root")
        .join("docs/src/reference/commands.md");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn binary_help() -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success(), "prikk --help must succeed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn the_help_inventory_and_the_documented_inventory_are_the_same_list() {
    let from_help = help_heads(&binary_help());
    let from_docs = documented_heads(&commands_md());

    // A check that parses nothing passes vacuously; both sides must have found a real list.
    assert!(
        from_help.len() > 20,
        "parsed {} invocations from --help -- the parser is probably broken",
        from_help.len()
    );
    assert!(
        from_docs.len() > 20,
        "parsed {} invocations from commands.md -- the parser is probably broken",
        from_docs.len()
    );

    let undocumented: Vec<&String> = from_help.difference(&from_docs).collect();
    let unshipped: Vec<&String> = from_docs.difference(&from_help).collect();
    assert!(
        undocumented.is_empty(),
        "the binary accepts these but `docs/src/reference/commands.md` does not list them: {undocumented:?}"
    );
    assert!(
        unshipped.is_empty(),
        "`docs/src/reference/commands.md` lists these but the binary does not accept them: {unshipped:?}"
    );
}

/// The check must fail in **both** directions, and the perturbation goes through the **parser**, not
/// past it: the markdown text itself is edited in memory and re-parsed, so a change that the reader
/// would silently skip cannot pass as a change that was detected.
#[test]
fn both_directions_fail_when_the_lists_disagree() {
    let help = binary_help();
    let markdown = commands_md();
    let from_help = help_heads(&help);
    assert_eq!(
        from_help,
        documented_heads(&markdown),
        "precondition: they agree today"
    );
    // The two sides really do carry the entry that went missing for two releases -- otherwise the
    // perturbations below would be removing and adding nothing.
    assert!(from_help.contains("bundle preview"), "{from_help:?}");

    // Direction 1: documented list loses a shipped command.
    let without = markdown
        .lines()
        .filter(|line| !line.starts_with("prikk bundle preview"))
        .collect::<Vec<_>>()
        .join("\n");
    let thinned = documented_heads(&without);
    assert!(
        !thinned.contains("bundle preview"),
        "the perturbation must actually remove it from the parse, not merely from the file"
    );
    let undocumented: Vec<&String> = from_help.difference(&thinned).collect();
    assert_eq!(
        undocumented,
        vec![&"bundle preview".to_owned()],
        "and the difference names it, which is what the failure message prints"
    );

    // Direction 2: documented list gains a command nothing ships.
    let invented = format!("{markdown}\nprikk teleport sideways\n");
    let padded = documented_heads(&invented);
    let unshipped: Vec<&String> = padded.difference(&from_help).collect();
    assert_eq!(unshipped, vec![&"teleport sideways".to_owned()]);
}

/// `prikk --version` is not an invocation head on either side — both parsers stop at the first
/// `-`-prefixed token, so it is skipped consistently rather than counted on one side only. Stated as
/// a test because "both sides skip it" is the property that makes the comparison sound; if one
/// parser ever started counting flag-only lines, this fails rather than the parity check going red
/// for a reason nobody could read.
#[test]
fn flag_only_invocations_are_outside_this_check() {
    assert!(invocation_path("  prikk --version").is_none());
    assert!(invocation_path("prikk --version").is_none());
    assert!(invocation_path("prikk <command> --help").is_none());
}
