//! RFC 146 §8f and RFC 147 §2j (stikk letter 017, answering our letter 018): `branch list` names an
//! unpublished current branch instead of leaving it invisible next to `no branches`, and
//! `branch switch <the current branch>` stops claiming it does not exist when it hasn't been sealed
//! yet.
//!
//! Fixture invariant (handoff §1, asserted by control 5): at most one unpublished branch can exist,
//! it is always the current branch, and only before the first seal -- `branch create` refuses
//! entirely before that point and publishes immediately after.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Output;

use support::json::{self, Value};

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn ok(repo: &Path, args: &[&str]) -> Output {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    output
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn seal_one_commit(repo: &Path, path: &str) {
    std::fs::write(repo.join(path), b"content\n").unwrap();
    ok(repo, &["commit", "-m", path]);
    ok(repo, &["seal", "--allow-no-audit"]);
}

/// Whether `line` could be mistaken for a branch row: `[* ]<ref-name> <64 lowercase hex>`, optionally
/// followed by ` (closed)`. Control 3's own failure mode: a reader that refuses any row whose id
/// column is not 64 hex characters would fail every ref read if the prose line here matched this
/// shape, rather than merely misreporting one field.
fn looks_like_a_branch_row(line: &str) -> bool {
    let rest = line.strip_prefix("* ").unwrap_or(line);
    let rest = rest.strip_suffix(" (closed)").unwrap_or(rest);
    let Some((_name, id)) = rest.rsplit_once(' ') else {
        return false;
    };
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Control 1 (present and correct in both states) and control 2 (`branches[]` byte-identical, the
/// array itself asserted, not just its length) and control 3 (the exact prose line; no line parses
/// as a branch row).
#[test]
fn the_field_and_the_prose_line_are_correct_in_both_states() {
    let repo = support::unique_repo("rfc146-listing-both-states");
    ok(&repo, &["setup", "."]);

    let fresh_prose = stdout(&ok(&repo, &["branch", "list"]));
    assert_eq!(
        fresh_prose,
        "no branches\ncurrent branch heads/main has no published history yet; the first `prikk seal` \
         publishes it\n"
    );
    for line in fresh_prose.lines() {
        assert!(
            !looks_like_a_branch_row(line),
            "{line:?} parses as a branch row"
        );
    }

    let fresh_json = json::parse(&stdout(&ok(&repo, &["branch", "list", "--format", "json"])));
    assert_eq!(fresh_json.get("branches"), &Value::Array(Vec::new()));
    assert_eq!(
        fresh_json.get("unpublished_current_branch").as_str(),
        "heads/main"
    );

    seal_one_commit(&repo, "a.txt");

    let sealed_prose = stdout(&ok(&repo, &["branch", "list"]));
    assert!(sealed_prose.starts_with("* heads/main "), "{sealed_prose}");
    assert!(
        !sealed_prose.contains("has no published history yet"),
        "{sealed_prose}"
    );

    let sealed_json = json::parse(&stdout(&ok(&repo, &["branch", "list", "--format", "json"])));
    let branches = sealed_json.get("branches").as_array();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].get("ref_name").as_str(), "heads/main");
    assert!(branches[0].get("current").as_bool());
    assert!(sealed_json.get("unpublished_current_branch").is_null());

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: `branch switch <the current branch>` names neither `branch create` nor "does not
/// exist", and says the branch is already current, whether or not it has been sealed. A genuinely
/// absent target keeps today's message untouched.
#[test]
fn branch_switch_to_the_current_branch_never_says_it_does_not_exist() {
    let repo = support::unique_repo("rfc146-listing-switch");
    ok(&repo, &["setup", "."]);

    let fresh_switch = ok(&repo, &["branch", "switch", "heads/main"]);
    let fresh_text = text(&fresh_switch);
    assert!(!fresh_text.contains("branch create"), "{fresh_text}");
    assert!(!fresh_text.contains("does not exist"), "{fresh_text}");
    assert!(
        fresh_text.contains("heads/main is already the current branch"),
        "{fresh_text}"
    );

    let absent = run(&repo, &["branch", "switch", "heads/nope"]);
    assert_eq!(absent.status.code(), Some(1), "{}", text(&absent));
    assert!(
        text(&absent)
            .contains("precondition not met: heads/nope does not exist; run `prikk branch create heads/nope` first"),
        "{}",
        text(&absent)
    );

    seal_one_commit(&repo, "a.txt");
    let sealed_switch = ok(&repo, &["branch", "switch", "heads/main"]);
    assert_eq!(stdout(&sealed_switch), "already on heads/main\n");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5: the invariant the whole design rests on, asserted so a later change cannot move it
/// silently. `branch create` refuses entirely before the first seal and publishes immediately after.
#[test]
fn branch_create_still_refuses_before_the_first_seal_and_publishes_after() {
    let repo = support::unique_repo("rfc146-listing-create-invariant");
    ok(&repo, &["setup", "."]);

    let refused = run(&repo, &["branch", "create", "heads/side"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains("precondition not met"),
        "{}",
        text(&refused)
    );

    seal_one_commit(&repo, "a.txt");
    ok(&repo, &["branch", "create", "heads/side"]);
    let json = json::parse(&stdout(&ok(&repo, &["branch", "list", "--format", "json"])));
    assert!(
        json.get("branches")
            .as_array()
            .iter()
            .any(|branch| branch.get("ref_name").as_str() == "heads/side"),
        "{json:?}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
