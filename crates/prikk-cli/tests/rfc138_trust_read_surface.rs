//! RFC 138 §7: `prikk trust maintainer list`/`check` controls, driven through the compiled binary.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::iter::Peekable;
use std::path::Path;
use std::str::Chars;

use prikk_store::{ContainerSlot, RepositoryLayout};

/// Small hand-written recursive-descent JSON syntax check, mirroring
/// `rfc118_stage5_verify_json.rs`'s own (this crate has no third-party dependencies, so there is
/// no `serde_json` to lean on -- RFC 118 §10 prerequisite 4). Covers exactly what
/// `trust-list-v1`/`trust-check-v1` can emit: objects, arrays, strings, `true`/`false`, and
/// `null` -- neither schema emits a bare number.
fn assert_valid_json(input: &str) {
    let mut chars = input.trim().chars().peekable();
    parse_value(&mut chars);
    skip_ws(&mut chars);
    assert!(
        chars.next().is_none(),
        "trailing bytes after the JSON value in: {input}"
    );
}

fn skip_ws(chars: &mut Peekable<Chars<'_>>) {
    while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
        chars.next();
    }
}

fn parse_value(chars: &mut Peekable<Chars<'_>>) {
    skip_ws(chars);
    match chars.peek().copied() {
        Some('{') => parse_object(chars),
        Some('[') => parse_array(chars),
        Some('"') => {
            parse_string(chars);
        }
        Some('t') => parse_literal(chars, "true"),
        Some('f') => parse_literal(chars, "false"),
        Some('n') => parse_literal(chars, "null"),
        other => panic!("unexpected JSON token starting with {other:?}"),
    }
}

fn parse_literal(chars: &mut Peekable<Chars<'_>>, literal: &str) {
    for expected in literal.chars() {
        assert_eq!(chars.next(), Some(expected), "expected literal {literal}");
    }
}

fn parse_string(chars: &mut Peekable<Chars<'_>>) -> String {
    assert_eq!(chars.next(), Some('"'), "expected opening quote");
    let mut value = String::new();
    loop {
        match chars.next() {
            Some('"') => break,
            Some('\\') => value.push(chars.next().expect("dangling escape at end of string")),
            Some(other) => value.push(other),
            None => panic!("unterminated JSON string"),
        }
    }
    value
}

fn parse_object(chars: &mut Peekable<Chars<'_>>) {
    assert_eq!(chars.next(), Some('{'));
    skip_ws(chars);
    if chars.peek() == Some(&'}') {
        chars.next();
        return;
    }
    loop {
        skip_ws(chars);
        parse_string(chars);
        skip_ws(chars);
        assert_eq!(chars.next(), Some(':'), "expected ':' after object key");
        parse_value(chars);
        skip_ws(chars);
        match chars.next() {
            Some(',') => continue,
            Some('}') => break,
            other => panic!("expected ',' or '}}' in object, got {other:?}"),
        }
    }
}

fn parse_array(chars: &mut Peekable<Chars<'_>>) {
    assert_eq!(chars.next(), Some('['));
    skip_ws(chars);
    if chars.peek() == Some(&']') {
        chars.next();
        return;
    }
    loop {
        parse_value(chars);
        skip_ws(chars);
        match chars.next() {
            Some(',') => continue,
            Some(']') => break,
            other => panic!("expected ',' or ']' in array, got {other:?}"),
        }
    }
}

fn add(repo: &Path, key_id: &str, public_key_hex: &str) {
    support::ok(
        &support::prikk(repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                key_id,
                "--public-key",
                public_key_hex,
            ])
            .output()
            .unwrap(),
        &format!("trust maintainer add {key_id}"),
    );
}

/// A structurally-corrupt trust policy container, forcing `read_current_trust_policy_snapshot` to
/// fail closed on a bad-magic record (`TrustPolicyFrameAttempt::Invalid`, `trust_index.rs`) rather
/// than resolving to a stale or empty policy. The container always resolves to slot `A` here,
/// since nothing in this test appends a generation record.
///
/// **Must be at least `TRUST_POLICY_HEADER_LEN` (50) bytes.** A shorter payload decodes as
/// `TrailingPartial`, not `Invalid` -- indistinguishable, from this read alone, from a container
/// that was never written at all, and `read_current_trust_policy_snapshot` folds both into the
/// same `Ok(None)`. That is a real, narrower and pre-existing ambiguity (shared by `seal`'s own
/// `load_maintainer_trust_policy`), not what this control is testing: this writes 96 bytes with no
/// valid magic anywhere in them, which can only decode as a genuine item failure.
fn corrupt_trust_policy_container(repo: &Path) {
    let layout =
        RepositoryLayout::open(repo).expect("repository must open to corrupt its container");
    std::fs::write(
        layout.trust_policy_container_slot_path(ContainerSlot::A),
        b"this container has been damaged beyond its own header length and carries no valid magic anywhere in it",
    )
    .expect("write corrupt trust policy bytes");
}

/// Control 3: an empty policy is a successful empty result, not an error.
#[test]
fn list_on_a_fresh_repository_succeeds_with_no_keys_adopted() {
    let repo = support::unique_repo("rfc138-list-empty");
    support::init(&repo);
    let out = support::prikk(&repo)
        .args(["trust", "maintainer", "list"])
        .output()
        .unwrap();
    support::ok(&out, "trust maintainer list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no maintainer keys adopted"),
        "an empty policy must say so plainly: {stdout}"
    );

    let json_out = support::prikk(&repo)
        .args(["trust", "maintainer", "list", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json_out, "trust maintainer list --format json");
    let json_stdout = String::from_utf8_lossy(&json_out.stdout);
    assert_valid_json(&json_stdout);
    assert!(json_stdout.contains("\"schema_version\": \"trust-list-v1\""));
    assert!(json_stdout.contains("\"keys\": []"));
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: `list` after two adds reports both, in adoption order, with the public keys passed
/// in. Also checks §4's output constraints: no threshold reported, and the object-trust-not-ref-
/// authority distinction is stated.
#[test]
fn list_reports_two_adopted_keys_in_adoption_order() {
    let repo = support::unique_repo("rfc138-list-two");
    support::init(&repo);
    let first_key = "1".repeat(64);
    let second_key = "2".repeat(64);
    add(&repo, "first", &first_key);
    add(&repo, "second", &second_key);

    let out = support::prikk(&repo)
        .args(["trust", "maintainer", "list"])
        .output()
        .unwrap();
    support::ok(&out, "trust maintainer list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first_pos = stdout.find("first").expect("first key id must appear");
    let second_pos = stdout.find("second").expect("second key id must appear");
    assert!(
        first_pos < second_pos,
        "keys must list in adoption order: {stdout}"
    );
    assert!(
        stdout.contains(&first_key),
        "the exact public key passed in must appear: {stdout}"
    );
    assert!(
        stdout.contains(&second_key),
        "the exact public key passed in must appear: {stdout}"
    );
    assert!(
        !stdout.contains("required="),
        "a listing must never report a threshold as policy: {stdout}"
    );
    assert!(
        stdout.contains("not ref authority"),
        "a listing must state the object-trust/ref-authority distinction: {stdout}"
    );

    let json_out = support::prikk(&repo)
        .args(["trust", "maintainer", "list", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json_out, "trust maintainer list --format json");
    let json_stdout = String::from_utf8_lossy(&json_out.stdout);
    assert_valid_json(&json_stdout);
    assert!(json_stdout.contains(&format!(
        "\"key_id\": \"first\", \"public_key\": \"{first_key}\""
    )));
    assert!(json_stdout.contains(&format!(
        "\"key_id\": \"second\", \"public_key\": \"{second_key}\""
    )));
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 1 (negative half) and RFC 121's exit-code ruling: a negative answer is a successful
/// query, exit `0`, not an operational failure.
#[test]
fn check_reports_not_trusted_and_exits_zero() {
    let repo = support::unique_repo("rfc138-check-negative");
    support::init(&repo);
    let out = support::prikk(&repo)
        .args(["trust", "maintainer", "check", "--key-id", "nobody"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "a negative answer must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("not trusted: nobody"), "{stdout}");

    let json_out = support::prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "check",
            "--key-id",
            "nobody",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(json_out.status.code(), Some(0));
    let json_stdout = String::from_utf8_lossy(&json_out.stdout);
    assert_valid_json(&json_stdout);
    assert!(json_stdout.contains("\"schema_version\": \"trust-check-v1\""));
    assert!(json_stdout.contains("\"trusted\": false"));
    assert!(json_stdout.contains("\"public_key\": null"));
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 1 (positive half): an affirmative answer is also exit `0`, and states the same
/// object-trust distinction `list` does.
#[test]
fn check_reports_trusted_and_exits_zero() {
    let repo = support::unique_repo("rfc138-check-positive");
    support::init(&repo);
    let key = "a".repeat(64);
    add(&repo, "someone", &key);

    let out = support::prikk(&repo)
        .args(["trust", "maintainer", "check", "--key-id", "someone"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("trusted: someone"), "{stdout}");
    assert!(stdout.contains(&key), "{stdout}");
    assert!(stdout.contains("not ref authority"), "{stdout}");

    let json_out = support::prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "check",
            "--key-id",
            "someone",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let json_stdout = String::from_utf8_lossy(&json_out.stdout);
    assert_valid_json(&json_stdout);
    assert!(json_stdout.contains("\"trusted\": true"));
    assert!(json_stdout.contains(&format!("\"public_key\": \"{key}\"")));
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2 (usage half): a missing or malformed `--key-id` is `2`, RFC 121's ruled usage-error
/// exit code -- distinct from the `0` a resolved query gets, whichever way it resolves.
#[test]
fn check_exits_two_on_missing_or_malformed_key_id() {
    let repo = support::unique_repo("rfc138-check-usage");
    support::init(&repo);

    let missing = support::prikk(&repo)
        .args(["trust", "maintainer", "check"])
        .output()
        .unwrap();
    assert_eq!(
        missing.status.code(),
        Some(2),
        "missing --key-id must be a usage error"
    );

    let malformed = support::prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "check",
            "--key-id",
            "bad id with spaces",
        ])
        .output()
        .unwrap();
    assert_eq!(
        malformed.status.code(),
        Some(2),
        "a malformed --key-id must be a usage error"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2 (integrity half): an unreadable policy is `1` for both subcommands, not `0` and not
/// `2` -- corruption is an operational failure, distinct from a valid negative answer.
#[test]
fn check_and_list_exit_one_on_an_unreadable_policy() {
    let repo = support::unique_repo("rfc138-unreadable-policy");
    support::init(&repo);
    add(&repo, "someone", &"a".repeat(64));
    corrupt_trust_policy_container(&repo);

    let check_out = support::prikk(&repo)
        .args(["trust", "maintainer", "check", "--key-id", "someone"])
        .output()
        .unwrap();
    assert_eq!(
        check_out.status.code(),
        Some(1),
        "an unreadable policy must be an operational failure for check"
    );

    let list_out = support::prikk(&repo)
        .args(["trust", "maintainer", "list"])
        .output()
        .unwrap();
    assert_eq!(
        list_out.status.code(),
        Some(1),
        "an unreadable policy must be an operational failure for list too"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5: `check` cannot disagree with `seal`, in either direction. Sealing once with the
/// fixed test maintainer key, confirming `check` agrees, then revoking it (via a second key so
/// the last-key-removal refusal does not intervene) and confirming both `check` and a subsequent
/// `seal` flip together.
#[test]
fn check_agrees_with_seal_before_and_after_revocation() {
    let repo = support::unique_repo("rfc138-check-agrees-with-seal");
    support::init(&repo);
    std::fs::write(repo.join("f.txt"), b"hello").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "commit: genesis",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal: genesis");

    let trusted_before = support::prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "check",
            "--key-id",
            support::MAINTAINER_KEY_ID,
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&trusted_before.stdout).starts_with("trusted:"));

    // A second key, so removing the fixed test maintainer key is not a last-key removal (which
    // `remove_trusted_maintainer` refuses outright, independent of anything this RFC changes).
    add(&repo, "backup", &"b".repeat(64));
    support::ok(
        &support::prikk(&repo)
            .args([
                "trust",
                "maintainer",
                "remove",
                "--key-id",
                support::MAINTAINER_KEY_ID,
            ])
            .output()
            .unwrap(),
        "trust maintainer remove",
    );

    let trusted_after = support::prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "check",
            "--key-id",
            support::MAINTAINER_KEY_ID,
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&trusted_after.stdout).starts_with("not trusted:"),
        "check must flip once the key is revoked"
    );

    std::fs::write(repo.join("f.txt"), b"hello again").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "second"),
        "commit: second",
    );
    // Deliberately not `support::seal`, which re-adopts the fixed key as a side effect -- this
    // seals directly under the now-revoked key to prove the refusal `check` just predicted.
    let seal_after_revocation = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit", "--ref", "heads/main"])
        .output()
        .unwrap();
    assert!(
        !seal_after_revocation.status.success(),
        "seal must refuse under a key `check` just reported as not trusted"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
