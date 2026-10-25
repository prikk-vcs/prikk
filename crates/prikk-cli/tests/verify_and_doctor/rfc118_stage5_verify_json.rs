//! RFC 118 stage 5: `prikk verify --format json` controls.
//!
//! `prikk-cli` has no third-party dependencies (RFC 118 §10 prerequisite 4), so there is no
//! `serde_json` here either -- [`assert_valid_json`] below is a small, hand-written recursive-
//! descent syntax check, written only to prove the emitted document parses, not a general-purpose
//! parser. It covers exactly the five JSON forms `print_verify_report_json` can emit: objects,
//! arrays, strings (with the same escapes `escape_json_string` writes), the `true`/`false`
//! literals, and (since RFC 108 increment 3b's `active_sessions.count`/`verified_count`) bare
//! non-negative-integer numbers.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Output;

#[path = "../support/mod.rs"]
mod support;

use prikk_object::{ObjectEnvelope, ObjectType};
use prikk_store::{FileObjectStore, ObjectWriter, RepositoryLayout};

fn ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn fail(output: &Output, what: &str) {
    assert!(
        !output.status.success(),
        "{what} unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn commit_index_path(repo: &Path) -> PathBuf {
    repo.join(".prikk").join("cache").join("commit-index.v1")
}

/// Same "stray non-file entry inside an object prefix directory" technique
/// `stage_containment.rs` (`prikk-store`'s own tests) uses to force a structural `Objects`-stage
/// failure -- reused here from `prikk-cli`'s side via the `test-support` feature. `name` is the
/// stray entry's own file name, chosen by each control to embed whatever bytes that control needs
/// to reach the resulting error message.
fn plant_stray_object_entry(layout: &RepositoryLayout, name: &str) {
    let mut objects = FileObjectStore::new(layout.clone());
    let stray_id = objects
        .write_object(&ObjectEnvelope::unsigned(
            ObjectType::Blob,
            1,
            b"payload".to_vec(),
        ))
        .expect("write a real blob to create the prefix directory");
    let prefix_dir = layout
        .object_path(ObjectType::Blob, stray_id)
        .parent()
        .expect("object path has a parent")
        .to_path_buf();
    std::fs::create_dir_all(prefix_dir.join(name)).expect("create the stray non-file entry");
}

/// Windows hostile-test fix handoff: this control used to plant a directory named
/// `quote"back\\slash<LF>newline<TAB>tab<U+0001>control` -- a double quote, a backslash, a newline,
/// a tab, and a raw control byte, exactly the set Win32 forbids in a filename (`<>:"/\\|?*` and
/// every C0 control). The directory could never be created on Windows, so the control never ran
/// there at all (Windows mutation suite, 2026-08-25) -- `escape_json_string`'s hostile-byte proof
/// now lives in a filesystem-free unit test (`output/verification/tests.rs`), which can use bytes
/// no filesystem would ever accept.
///
/// **A portable name cannot replace that proof**, and this is stated rather than silently assumed:
/// every character JSON requires escaping (`"`, `\\`, and every C0 control) is also filesystem-illegal
/// on Windows, so no filename-legal-everywhere string can exercise the escaper's hostile-input
/// path. What a portable name *can* still prove, on every platform: a real structural `Objects`-stage
/// failure's message -- which embeds the full stray path via `Path::display`, including Windows'
/// own `\\` path separators -- reaches `--format json` intact, the document still parses, and
/// ordinary punctuation in the planted name (which needs no escaping) survives unmangled. That is a
/// real, if narrower, end-to-end proof, not a placeholder.
#[test]
fn stage_failure_message_with_ordinary_punctuation_reaches_valid_json() {
    let repo = support::unique_repo("rfc118-stage5-portable-message");
    support::init(&repo);
    let layout = RepositoryLayout::open(&repo).expect("layout opens");
    let portable_name = "can't (be here)";
    plant_stray_object_entry(&layout, portable_name);

    let out = support::prikk(&repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    fail(
        &out,
        "verify --format json against a structurally broken repository",
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_valid_json(&stdout);
    assert!(
        stdout.contains(r#""ok": false"#),
        "a structural stage failure must report ok: false: {stdout}"
    );
    assert!(
        stdout.contains(r#""id": "stage-failure""#),
        "a structural stage failure must name the stage-failure condition: {stdout}"
    );
    assert!(
        stdout.contains(portable_name),
        "ordinary punctuation must survive unescaped and unmangled: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2 (RFC 118 stage 5 handoff §6.2): the verdict must catch a condition **outside**
/// `has_stage_failure`/`has_item_failure` -- this is the control that actually proves §2 was
/// solved, since a JSON emitter built on `has_blocking_defect()` would report `ok: true` here. Uses
/// `dc56_commit_index.rs`'s own deliberately-stale-index-entry technique: same recorded stat as the
/// real file, a content hash that disagrees with the file's actual bytes.
#[test]
fn verdict_catches_a_commit_index_divergence_outside_stage_and_item_failure() {
    let repo = support::unique_repo("rfc118-stage5-commit-index-divergence");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "genesis commit",
    );

    let index_path = commit_index_path(&repo);
    let original = std::fs::read_to_string(&index_path).unwrap();
    let mut lines: Vec<&str> = original.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected one header line and one entry line"
    );
    let entry_fields: Vec<&str> = lines[1].split('\t').collect();
    assert_eq!(
        entry_fields.len(),
        7,
        "expected the documented 7-field entry format"
    );
    let fabricated_hash = "0".repeat(64);
    let corrupted_entry = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{fabricated_hash}",
        entry_fields[0],
        entry_fields[1],
        entry_fields[2],
        entry_fields[3],
        entry_fields[4],
        entry_fields[5],
    );
    lines[1] = &corrupted_entry;
    std::fs::write(&index_path, format!("{}\n{}\n", lines[0], lines[1])).unwrap();

    let out = support::prikk(&repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    fail(
        &out,
        "verify --format json with a deliberately stale commit-index entry",
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_valid_json(&stdout);
    assert!(
        !stdout.contains(r#""stage-failure""#) && !stdout.contains(r#""item-failure""#),
        "this control must trip a condition outside stage/item failure, not those two: {stdout}"
    );
    assert!(
        stdout.contains(r#""ok": false"#),
        "a commit-index divergence must report ok: false: {stdout}"
    );
    assert!(
        stdout.contains(r#""id": "commit-index-divergence""#),
        "the verdict must name commit-index-divergence specifically: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4 (RFC 118 stage 5 handoff §6.4): every `VerificationStage::ALL` entry is present in
/// the JSON `--stop-on-first-error`, including `halted` ones -- the path most likely to drop a
/// stage silently, per stage 4's own report.
#[test]
fn every_stage_is_present_in_json_under_stop_on_first_error() {
    let repo = support::unique_repo("rfc118-stage5-stop-on-first-error");
    support::init(&repo);
    let layout = RepositoryLayout::open(&repo).expect("layout opens");
    plant_stray_object_entry(&layout, "stray-directory");

    let out = support::prikk(&repo)
        .args(["verify", "--format", "json", "--stop-on-first-error"])
        .output()
        .unwrap();
    fail(&out, "verify --format json --stop-on-first-error");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_valid_json(&stdout);
    let stage_count = stdout.matches(r#"{"stage": "#).count();
    assert_eq!(
        stage_count,
        prikk_store::VerificationStage::ALL.len(),
        "expected one JSON stage entry per VerificationStage::ALL: {stdout}"
    );
    assert!(
        stdout.contains(r#""status": "halted""#),
        "a stop-on-first-error run with a real failure must halt at least one later stage: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Review fix (stage 5 review v1, condition 1): the emitter used to walk `report.stage_outcomes`
/// (pipeline order) rather than `VerificationStage::ALL` (declared order), so a healthy repository's
/// JSON silently emitted `received-refs` and `local-tag-trust` third and fourth instead of last --
/// visibly wrong against the handoff's own §1 instruction ("one entry per `VerificationStage::ALL`,
/// in `ALL` order") and against this module's own doc comment. Asserts the stage names appear in
/// exactly `ALL`'s order, not merely that all fourteen are present (control 4 already covers that).
#[test]
fn stages_are_emitted_in_verification_stage_all_order() {
    let repo = support::unique_repo("rfc118-stage5-all-order");
    support::init(&repo);

    let out = support::prikk(&repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    ok(&out, "verify --format json against a clean repository");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_valid_json(&stdout);

    let expected: Vec<&str> = prikk_store::VerificationStage::ALL
        .iter()
        .map(|stage| stage.label())
        .collect();
    let actual: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix(r#"{"stage": ""#)
                .map(|rest| rest.split('"').next().unwrap().to_string())
        })
        .collect();
    assert_eq!(
        actual, expected,
        "JSON stage order must match VerificationStage::ALL exactly: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Minimal JSON syntax validator -- see this file's own module doc for scope.
fn assert_valid_json(input: &str) {
    let mut chars = input.trim().chars().peekable();
    parse_value(&mut chars);
    skip_ws(&mut chars);
    assert!(
        chars.next().is_none(),
        "trailing content after the top-level JSON value"
    );
}

fn skip_ws(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while matches!(chars.peek(), Some(' ' | '\n' | '\t' | '\r')) {
        chars.next();
    }
}

fn parse_value(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
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
        // RFC 108 increment 3b: `active_sessions.count`/`verified_count` are this emitter's first
        // bare JSON numbers -- every prior form this validator covered was documented in the
        // module doc above as exactly objects/arrays/strings/true/false. Extending the validator to
        // recognize a fifth, genuinely new emitted form is not loosening an existing assertion.
        Some(character) if character == '-' || character.is_ascii_digit() => parse_number(chars),
        other => panic!("unexpected token starting a JSON value: {other:?}"),
    }
}

/// RFC 8259 §6 number grammar, minimally: optional leading `-`, one or more integer digits, an
/// optional `.` fraction, an optional `e`/`E` exponent. Sufficient for this emitter's own output
/// (plain non-negative integer counts today) without special-casing those specific values -- a
/// syntax validator that only accepted the exact digits currently emitted would stop being a syntax
/// validator.
fn parse_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    if chars.peek() == Some(&'-') {
        chars.next();
    }
    let mut has_digit = false;
    while matches!(chars.peek(), Some(character) if character.is_ascii_digit()) {
        chars.next();
        has_digit = true;
    }
    assert!(has_digit, "expected at least one digit in a JSON number");
    if chars.peek() == Some(&'.') {
        chars.next();
        let mut has_fraction_digit = false;
        while matches!(chars.peek(), Some(character) if character.is_ascii_digit()) {
            chars.next();
            has_fraction_digit = true;
        }
        assert!(
            has_fraction_digit,
            "expected digit(s) after the decimal point in a JSON number"
        );
    }
    if matches!(chars.peek(), Some('e' | 'E')) {
        chars.next();
        if matches!(chars.peek(), Some('+' | '-')) {
            chars.next();
        }
        let mut has_exponent_digit = false;
        while matches!(chars.peek(), Some(character) if character.is_ascii_digit()) {
            chars.next();
            has_exponent_digit = true;
        }
        assert!(
            has_exponent_digit,
            "expected digit(s) in a JSON number exponent"
        );
    }
}

fn parse_literal(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, literal: &str) {
    for expected in literal.chars() {
        assert_eq!(chars.next(), Some(expected), "expected literal {literal:?}");
    }
}

fn parse_object(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
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
        assert_eq!(chars.next(), Some(':'), "expected ':' in object");
        parse_value(chars);
        skip_ws(chars);
        match chars.next() {
            Some(',') => continue,
            Some('}') => break,
            other => panic!("expected ',' or '}}' in object, got {other:?}"),
        }
    }
}

fn parse_array(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
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

fn parse_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    assert_eq!(chars.next(), Some('"'), "expected opening quote");
    let mut value = String::new();
    loop {
        match chars.next() {
            Some('"') => break,
            Some('\\') => match chars.next() {
                Some('"') => value.push('"'),
                Some('\\') => value.push('\\'),
                Some('/') => value.push('/'),
                Some('n') => value.push('\n'),
                Some('r') => value.push('\r'),
                Some('t') => value.push('\t'),
                Some('b') => value.push('\u{8}'),
                Some('f') => value.push('\u{c}'),
                Some('u') => {
                    let hex: String = (0..4)
                        .map(|_| chars.next().expect("4 hex digits after \\u"))
                        .collect();
                    let code = u32::from_str_radix(&hex, 16).expect("valid hex escape");
                    value.push(char::from_u32(code).expect("valid unicode escape"));
                }
                other => panic!("invalid escape sequence: \\{other:?}"),
            },
            Some(character) if (character as u32) < 0x20 => panic!(
                "raw control character U+{:04X} in a JSON string -- must be escaped",
                character as u32
            ),
            Some(character) => value.push(character),
            None => panic!("unterminated JSON string"),
        }
    }
    value
}
