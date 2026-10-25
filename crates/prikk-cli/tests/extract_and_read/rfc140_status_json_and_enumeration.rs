//! RFC 140: `prikk status --format json` and queued-patch enumeration, driven through the
//! compiled binary. §3 is the control this file exists for: a naive test that only queues
//! `CreateFile` (control 1's own trap) would pass while leaving the ordinary case -- editing an
//! existing file, which produces node-addressed `EditText` with no path at all -- silently
//! unresolved. Control 2's `rename-path` half needs a raw WAL record (`commit` never authors one:
//! `patch_replay.rs`'s own module doc) and is covered instead by a `prikk-store`-level test in
//! `worktree_status/tests.rs`, per the handoff's own steer not to manufacture a CLI path that does
//! not exist.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

/// Full hand-written recursive-descent JSON syntax checker (objects, arrays, strings, numbers,
/// `true`/`false`/`null`), mirroring `rfc118_stage5_verify_json.rs`'s own -- this crate has no
/// third-party dependencies, so there is no `serde_json` to lean on (RFC 118 §10 prerequisite 4).
/// `status-report-v1` is the first schema in this crate to emit numbers *and* `null` together, so
/// this copies the more complete of the two existing checkers rather than `rfc138`'s narrower one.
use crate::support;
fn assert_valid_json(input: &str) -> serde_json_like::Value {
    let mut chars = input.trim().chars().peekable();
    let value = serde_json_like::parse_value(&mut chars);
    serde_json_like::skip_ws(&mut chars);
    assert!(
        chars.next().is_none(),
        "trailing content after the top-level JSON value: {input}"
    );
    value
}

/// A minimal parsed-value tree, just enough for this file's own assertions to navigate into
/// `status-report-v1`'s nested `queue.patches[].operations[].paths[]` shape without re-parsing the
/// raw string with ad hoc substring checks.
mod serde_json_like {
    use std::collections::BTreeMap;
    use std::iter::Peekable;
    use std::str::Chars;

    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum Value {
        Null,
        Bool(bool),
        Number(String),
        String(String),
        Array(Vec<Value>),
        Object(BTreeMap<String, Value>),
    }

    impl Value {
        pub(crate) fn get(&self, key: &str) -> &Value {
            match self {
                Value::Object(map) => map.get(key).unwrap_or_else(|| {
                    panic!("missing key {key:?} in object with keys {:?}", map.keys())
                }),
                other => panic!("expected an object to look up {key:?}, got {other:?}"),
            }
        }

        pub(crate) fn as_array(&self) -> &[Value] {
            match self {
                Value::Array(items) => items,
                other => panic!("expected an array, got {other:?}"),
            }
        }

        pub(crate) fn as_str(&self) -> &str {
            match self {
                Value::String(text) => text,
                other => panic!("expected a string, got {other:?}"),
            }
        }

        pub(crate) fn as_number_str(&self) -> &str {
            match self {
                Value::Number(text) => text,
                other => panic!("expected a number, got {other:?}"),
            }
        }

        pub(crate) fn is_null(&self) -> bool {
            matches!(self, Value::Null)
        }
    }

    pub(crate) fn skip_ws(chars: &mut Peekable<Chars<'_>>) {
        while matches!(chars.peek(), Some(' ' | '\n' | '\t' | '\r')) {
            chars.next();
        }
    }

    pub(crate) fn parse_value(chars: &mut Peekable<Chars<'_>>) -> Value {
        skip_ws(chars);
        match chars.peek().copied() {
            Some('{') => parse_object(chars),
            Some('[') => parse_array(chars),
            Some('"') => Value::String(parse_string(chars)),
            Some('t') => {
                parse_literal(chars, "true");
                Value::Bool(true)
            }
            Some('f') => {
                parse_literal(chars, "false");
                Value::Bool(false)
            }
            Some('n') => {
                parse_literal(chars, "null");
                Value::Null
            }
            Some(character) if character == '-' || character.is_ascii_digit() => {
                Value::Number(parse_number(chars))
            }
            other => panic!("unexpected token starting a JSON value: {other:?}"),
        }
    }

    fn parse_number(chars: &mut Peekable<Chars<'_>>) -> String {
        let mut text = String::new();
        if chars.peek() == Some(&'-') {
            text.push(chars.next().unwrap());
        }
        let mut has_digit = false;
        while matches!(chars.peek(), Some(character) if character.is_ascii_digit()) {
            text.push(chars.next().unwrap());
            has_digit = true;
        }
        assert!(has_digit, "expected at least one digit in a JSON number");
        if chars.peek() == Some(&'.') {
            text.push(chars.next().unwrap());
            let mut has_fraction_digit = false;
            while matches!(chars.peek(), Some(character) if character.is_ascii_digit()) {
                text.push(chars.next().unwrap());
                has_fraction_digit = true;
            }
            assert!(
                has_fraction_digit,
                "expected digit(s) after the decimal point in a JSON number"
            );
        }
        text
    }

    fn parse_literal(chars: &mut Peekable<Chars<'_>>, literal: &str) {
        for expected in literal.chars() {
            assert_eq!(chars.next(), Some(expected), "expected literal {literal:?}");
        }
    }

    fn parse_object(chars: &mut Peekable<Chars<'_>>) -> Value {
        assert_eq!(chars.next(), Some('{'));
        skip_ws(chars);
        let mut map = BTreeMap::new();
        if chars.peek() == Some(&'}') {
            chars.next();
            return Value::Object(map);
        }
        loop {
            skip_ws(chars);
            let key = parse_string(chars);
            skip_ws(chars);
            assert_eq!(chars.next(), Some(':'), "expected ':' in object");
            let value = parse_value(chars);
            map.insert(key, value);
            skip_ws(chars);
            match chars.next() {
                Some(',') => continue,
                Some('}') => break,
                other => panic!("expected ',' or '}}' in object, got {other:?}"),
            }
        }
        Value::Object(map)
    }

    fn parse_array(chars: &mut Peekable<Chars<'_>>) -> Value {
        assert_eq!(chars.next(), Some('['));
        skip_ws(chars);
        let mut items = Vec::new();
        if chars.peek() == Some(&']') {
            chars.next();
            return Value::Array(items);
        }
        loop {
            items.push(parse_value(chars));
            skip_ws(chars);
            match chars.next() {
                Some(',') => continue,
                Some(']') => break,
                other => panic!("expected ',' or ']' in array, got {other:?}"),
            }
        }
        Value::Array(items)
    }

    fn parse_string(chars: &mut Peekable<Chars<'_>>) -> String {
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
                    other => panic!("invalid escape sequence: \\{other:?}"),
                },
                Some(other) => value.push(other),
                None => panic!("unterminated JSON string"),
            }
        }
        value
    }
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// RFC 140 control 8: the JSON parses and carries its `schema_version`.
#[test]
fn control8_json_parses_and_carries_schema_version() {
    let repo = support::unique_repo("rfc140-control8");
    support::init(&repo);
    let out = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "status --format json");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("schema_version").as_str(), "status-report-v1");
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 140 control 5: an empty queue is a valid, complete answer -- exit `0`, valid JSON, an empty
/// list, not an absent field.
#[test]
fn control5_empty_queue_is_a_valid_complete_answer() {
    let repo = support::unique_repo("rfc140-control5");
    support::init(&repo);
    let out = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "status --format json on an empty repository");
    let value = assert_valid_json(&stdout_of(&out));
    let queue = value.get("queue");
    assert_eq!(queue.get("count").as_number_str(), "0");
    assert!(queue.get("patches").as_array().is_empty());
    assert!(queue.get("target_ref").is_null());
    assert!(queue.get("threshold_status").is_null());
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 140 control 1 -- the control this whole increment exists for. Editing an *existing, sealed*
/// file is the ordinary case, and it produces node-addressed `EditText` with no path in its own
/// payload; a naive implementation (or an implementation only tested against a queue of
/// `CreateFile`) would report a node id here, not `a.txt`.
#[test]
fn control1_editing_an_existing_file_resolves_to_a_real_path() {
    let repo = support::unique_repo("rfc140-control1");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    std::fs::write(repo.join("a.txt"), "hello, edited\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "edit a.txt"),
        "edit a.txt",
    );

    let out = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "status --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let patches = value.get("queue").get("patches").as_array();
    assert_eq!(patches.len(), 1, "one queued patch: {value:?}");
    let operations = patches[0].get("operations").as_array();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].get("kind").as_str(), "edit-text");
    let paths = operations[0].get("paths").as_array();
    assert_eq!(paths.len(), 1);
    assert_eq!(
        paths[0].get("path").as_str(),
        "a.txt",
        "EditText must resolve to the real path, not a node id: {value:?}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 140 control 2 (the CLI-authorable three of its four kinds -- `rename-path` is covered by a
/// `prikk-store`-level test, see this file's own module doc). Create, edit, and delete in one
/// queue; all three report real paths.
#[test]
fn control2_create_edit_delete_all_report_paths() {
    let repo = support::unique_repo("rfc140-control2");
    support::init(&repo);
    std::fs::write(repo.join("edit-me.txt"), "hello\n").unwrap();
    std::fs::write(repo.join("delete-me.txt"), "bye\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    std::fs::write(repo.join("edit-me.txt"), "hello, edited\n").unwrap();
    std::fs::remove_file(repo.join("delete-me.txt")).unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "create+edit+delete"),
        "create+edit+delete",
    );

    let out = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "status --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let patches = value.get("queue").get("patches").as_array();
    assert_eq!(patches.len(), 1);
    let operations = patches[0].get("operations").as_array();

    let mut reported: Vec<(String, String)> = operations
        .iter()
        .map(|operation| {
            let kind = operation.get("kind").as_str().to_string();
            let paths = operation.get("paths").as_array();
            assert_eq!(paths.len(), 1, "{kind} must report exactly one path here");
            (kind, paths[0].get("path").as_str().to_string())
        })
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        vec![
            ("create-file".to_string(), "new.txt".to_string()),
            ("delete-node".to_string(), "delete-me.txt".to_string()),
            ("edit-text".to_string(), "edit-me.txt".to_string()),
        ],
        "{value:?}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 140 control 3: queue order is queue order -- not sorted by path, not by id. Three separate
/// commits, deliberately in reverse-alphabetical file order, must enumerate in the order they were
/// queued.
#[test]
fn control3_queue_order_is_queue_order_not_sorted() {
    let repo = support::unique_repo("rfc140-control3");
    support::init(&repo);
    for name in ["c.txt", "a.txt", "b.txt"] {
        std::fs::write(repo.join(name), format!("{name}\n")).unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("create {name}")),
            &format!("create {name}"),
        );
    }

    let out = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "status --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let patches = value.get("queue").get("patches").as_array();
    assert_eq!(patches.len(), 3);
    let order: Vec<String> = patches
        .iter()
        .map(|patch| {
            let operations = patch.get("operations").as_array();
            assert_eq!(operations.len(), 1);
            let paths = operations[0].get("paths").as_array();
            paths[0].get("path").as_str().to_string()
        })
        .collect();
    assert_eq!(
        order,
        vec![
            "c.txt".to_string(),
            "a.txt".to_string(),
            "b.txt".to_string()
        ],
        "queue order must be insertion order, not sorted: {value:?}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 140 control 4: an unresolvable node id does not fail the command. Editing a file and then
/// deleting that same file within the *same* unsealed queue leaves the earlier `EditText`'s own
/// node id no longer live in the folded state (the later `DeleteNode` removed it) -- a real,
/// ordinarily-reachable way to produce an unresolvable node id, with no raw WAL construction
/// needed.
#[test]
fn control4_unresolvable_node_id_does_not_fail_the_command() {
    let repo = support::unique_repo("rfc140-control4");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    std::fs::write(repo.join("a.txt"), "hello, edited\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "edit a"), "edit a");
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "delete a, same queue"),
        "delete a, same queue",
    );

    let out = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "an unresolvable node id must not fail the command: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value = assert_valid_json(&stdout_of(&out));
    let patches = value.get("queue").get("patches").as_array();
    assert_eq!(patches.len(), 2, "both patches must still be reported");

    let edit_operations = patches[0].get("operations").as_array();
    assert_eq!(edit_operations[0].get("kind").as_str(), "edit-text");
    let edit_paths = edit_operations[0].get("paths").as_array();
    assert!(
        edit_paths[0].get("unresolved_node_id").as_str().len() == 64,
        "the edit's own node id must be reported as unresolved (64 hex chars): {value:?}"
    );

    let delete_operations = patches[1].get("operations").as_array();
    assert_eq!(delete_operations[0].get("kind").as_str(), "delete-node");
    let delete_paths = delete_operations[0].get("paths").as_array();
    assert_eq!(delete_paths[0].get("path").as_str(), "a.txt");
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 140 control 7: `status --nonsense` still exits `2`; `--format yaml` exits `2`; a repeated
/// `--format` exits `2`. RFC 121 §3 is not loosened by this change.
#[test]
fn control7_argument_refusal_survives() {
    let repo = support::unique_repo("rfc140-control7");
    support::init(&repo);

    let nonsense = support::prikk(&repo)
        .args(["status", "--nonsense"])
        .output()
        .unwrap();
    assert_eq!(nonsense.status.code(), Some(2));

    let bad_format = support::prikk(&repo)
        .args(["status", "--format", "yaml"])
        .output()
        .unwrap();
    assert_eq!(bad_format.status.code(), Some(2));

    let repeated = support::prikk(&repo)
        .args(["status", "--format", "json", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(repeated.status.code(), Some(2));

    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 140 control 6, the structural half: the prose path (`run_status`'s own body, not
/// `run_status_json`) does not call `enumerate_queued_patches` at all -- so it cannot perform the
/// baseline derivation regardless of what `run_status_json` does. The functional half (byte-
/// identical prose output) is already covered by `dc57_active_patch_thresholds.rs` and
/// `dc66_multi_commit_queuing.rs`'s own existing prose-output assertions, unmodified by this round
/// and re-run in the same gate set -- not duplicated here.
#[test]
fn control6_prose_path_calls_no_enumeration() {
    let main_rs = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .unwrap();
    let start = main_rs
        .find("fn run_status(format_json: bool)")
        .expect("run_status must exist");
    let end = main_rs[start..]
        .find("fn run_status_json()")
        .map(|offset| start + offset)
        .expect("run_status_json must exist after run_status");
    let prose_body = &main_rs[start..end];
    assert!(
        !prose_body.contains("enumerate_queued_patches"),
        "run_status's own body (excluding run_status_json) must never call \
         enumerate_queued_patches -- the prose path must pay nothing new (RFC 140 §5)"
    );
}
