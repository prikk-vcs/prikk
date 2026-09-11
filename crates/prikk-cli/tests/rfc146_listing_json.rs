//! RFC 146: `--format json` for `log`, `branch` and `tag`, driven through the compiled binary.
//!
//! Control 1 (RFC 146 §4): every test that reads a repository state reads *both* forms and
//! compares their facts against each other, not against two independently-asserted expectations.
//! Control 2: `log_control2_genesis_previous_ref_state_is_null` and
//! `log_control2_missing_block_object_fails_both_forms` are rule 1/2's perturbation, together —
//! the first shows an *expected* absence (a genesis block's own `previous_ref_state_id: None`)
//! degrades to `null` at exit `0` in both forms; the second shows a *missing object* still fails
//! both forms alike, so JSON does not silently swallow a real corruption into a null the way a
//! careless implementation could.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use prikk_object::ObjectType;
use prikk_store::{ContainerSlot, RepositoryLayout};

/// Full hand-written recursive-descent JSON value parser (objects, arrays, strings, numbers,
/// `true`/`false`/`null`), mirroring `rfc140_status_json_and_enumeration.rs`'s own -- this crate
/// has no third-party dependencies, so there is no `serde_json` to lean on (RFC 118 §10
/// prerequisite 4, restated by RFC 146 §1).
mod json {
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

        pub(crate) fn as_bool(&self) -> bool {
            match self {
                Value::Bool(value) => *value,
                other => panic!("expected a bool, got {other:?}"),
            }
        }

        pub(crate) fn is_null(&self) -> bool {
            matches!(self, Value::Null)
        }
    }

    pub(crate) fn parse(input: &str) -> Value {
        let mut chars = input.trim().chars().peekable();
        let value = parse_value(&mut chars);
        skip_ws(&mut chars);
        assert!(
            chars.next().is_none(),
            "trailing content after the top-level JSON value: {input}"
        );
        value
    }

    fn skip_ws(chars: &mut Peekable<Chars<'_>>) {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
    }

    fn parse_value(chars: &mut Peekable<Chars<'_>>) -> Value {
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
            Some(c) if c == '-' || c.is_ascii_digit() => Value::Number(parse_number(chars)),
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

    fn parse_number(chars: &mut Peekable<Chars<'_>>) -> String {
        let mut text = String::new();
        if chars.peek() == Some(&'-') {
            text.push(chars.next().unwrap());
        }
        while matches!(chars.peek(), Some(c) if c.is_ascii_digit() || *c == '.') {
            text.push(chars.next().unwrap());
        }
        text
    }

    fn parse_object(chars: &mut Peekable<Chars<'_>>) -> Value {
        assert_eq!(chars.next(), Some('{'));
        let mut map = BTreeMap::new();
        skip_ws(chars);
        if chars.peek() == Some(&'}') {
            chars.next();
            return Value::Object(map);
        }
        loop {
            skip_ws(chars);
            let key = parse_string(chars);
            skip_ws(chars);
            assert_eq!(chars.next(), Some(':'), "expected ':' after object key");
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
        let mut items = Vec::new();
        skip_ws(chars);
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
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn json_of(output: &std::process::Output) -> json::Value {
    json::parse(&stdout_of(output))
}

// ---------------------------------------------------------------------------------------------
// log
// ---------------------------------------------------------------------------------------------

#[test]
fn log_prose_and_json_agree_across_two_blocks() {
    let repo = support::unique_repo("rfc146-log-agree");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"hello\n", "add a.txt");
    support::generation(&repo, "heads/main", "a.txt", b"hello, edited\n", "edit a");

    let prose = support::prikk(&repo).args(["log"]).output().unwrap();
    support::ok(&prose, "log prose");
    let json_out = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json_out, "log json");

    let value = json_of(&json_out);
    assert_eq!(value.get("schema_version").as_str(), "log-report-v1");
    assert_eq!(value.get("ref").as_str(), "heads/main");
    let blocks = value.get("blocks").as_array();
    assert_eq!(blocks.len(), 2, "two sealed generations must be two blocks");

    // Same facts, read from prose, not re-derived independently (control 1).
    let prose_text = stdout_of(&prose);
    let prose_block_ids: Vec<&str> = prose_text
        .lines()
        .filter_map(|line| line.strip_prefix("block "))
        .collect();
    let json_block_ids: Vec<&str> = blocks.iter().map(|b| b.get("block_id").as_str()).collect();
    assert_eq!(
        prose_block_ids, json_block_ids,
        "block order and identity must agree between prose and JSON"
    );

    // Newest first: index 0 has a real previous_ref_state_id, the oldest (genesis) is null.
    assert!(!blocks[0].get("previous_ref_state_id").is_null());
    assert!(blocks[1].get("previous_ref_state_id").is_null());
    assert_eq!(blocks[1].get("kind").as_str(), "Root");
    assert!(prose_text.contains("kind: Root"));

    for (index, block) in blocks.iter().enumerate() {
        let messages = block.get("patch_messages").as_array();
        assert_eq!(
            messages.len(),
            1,
            "each generation authored exactly one patch"
        );
        let message = messages[0].get("message").as_str();
        assert!(
            prose_text.contains(&format!(": {message}")),
            "JSON message {message:?} at block {index} must appear in prose too"
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2, half one (RFC 146 rule 2): an *expected* absence -- the genesis block's own
/// `previous_ref_state_id: None` -- degrades to `null`, not an error, and the command still
/// exits `0` in both forms. Verified this fails loudly if regressed: temporarily replacing the
/// `None => push "null"` arm in `push_history_entry` (`output/worktree.rs`) with a panic/`expect`
/// makes this test fail immediately; reverted before landing, per RFC 146 §4 control 2.
#[test]
fn log_control2_genesis_previous_ref_state_is_null() {
    let repo = support::unique_repo("rfc146-log-control2-null");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"hello\n", "genesis");

    let prose = support::prikk(&repo).args(["log"]).output().unwrap();
    support::ok(&prose, "log prose");
    assert!(stdout_of(&prose).contains("previous-ref-state: <none>"));

    let json_out = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        json_out.status.success(),
        "an expected absence must not fail the command: {}",
        String::from_utf8_lossy(&json_out.stderr)
    );
    let value = json_of(&json_out);
    let blocks = value.get("blocks").as_array();
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].get("previous_ref_state_id").is_null());
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2, half two: the *other* rule-2 case -- a genuinely missing object -- must **not**
/// degrade at all. Both forms fail alike; JSON must not silently render a `null`/absent field for
/// a real corruption the way it correctly does for an expected absence above.
#[test]
fn log_control2_missing_block_object_fails_both_forms() {
    let repo = support::unique_repo("rfc146-log-control2-missing");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"hello\n", "genesis");

    // Format-3 stores objects in per-type A/B containers, not loose files (`layout.object_path`
    // is the retired format-1/2 loose-object path, kept only for format-transition fixtures --
    // see its own doc comment). Truncating the Block container to zero bytes makes every
    // previously-recorded container offset point past EOF, so the genesis block this repository's
    // single ref-state chain names becomes genuinely unreadable, the same class of "missing
    // object" RFC 146 rule 2 names, without needing container-record surgery.
    let layout = RepositoryLayout::open(&repo).expect("repository must open");
    std::fs::write(
        layout.container_slot_path(ObjectType::Block, ContainerSlot::A),
        b"",
    )
    .expect("truncate the block container");

    let prose = support::prikk(&repo).args(["log"]).output().unwrap();
    assert!(
        !prose.status.success(),
        "a missing block object must fail the prose form, not degrade"
    );

    let json_out = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        !json_out.status.success(),
        "a missing block object must fail the JSON form identically, not silently null it out"
    );
    assert_eq!(
        json_out.status.code(),
        prose.status.code(),
        "both forms must fail with the same exit code"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn log_empty_history_is_a_value() {
    let repo = support::unique_repo("rfc146-log-empty");
    support::init(&repo);

    let prose = support::prikk(&repo).args(["log"]).output().unwrap();
    support::ok(&prose, "log prose on unborn ref");
    assert!(stdout_of(&prose).contains("history: <empty>"));

    let json_out = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    assert!(json_out.status.success(), "empty history must exit 0");
    let value = json_of(&json_out);
    assert_eq!(value.get("blocks").as_array().len(), 0);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn log_exit_codes_on_format_flag() {
    let repo = support::unique_repo("rfc146-log-exit-codes");
    support::init(&repo);

    let bad = support::prikk(&repo)
        .args(["log", "--format", "yaml"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2), "malformed --format must exit 2");

    let repeated = support::prikk(&repo)
        .args(["log", "--format", "json", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        repeated.status.code(),
        Some(2),
        "repeated --format must exit 2"
    );

    let empty_ok = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(empty_ok.status.code(), Some(0), "empty listing must exit 0");
    let _ = std::fs::remove_dir_all(&repo);
}

// ---------------------------------------------------------------------------------------------
// branch
// ---------------------------------------------------------------------------------------------

#[test]
fn branch_prose_and_json_agree_open_and_closed() {
    let repo = support::unique_repo("rfc146-branch-agree");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"hello\n", "genesis");
    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create heads/topic",
    );
    support::ok(
        &support::branch_close(&repo, "heads/topic"),
        "branch close heads/topic",
    );
    // RFC 146 §8a: a tag ref in the fixture, so its *absence* from `branch` is asserted rather
    // than merely incidental -- `list_ref_pointers` returns every kind and the listing must filter.
    support::ok(
        &support::tag_create(&repo, "tags/v1", "heads/main"),
        "tag create tags/v1",
    );

    // Without --all: closed branch omitted in both forms.
    let prose = support::prikk(&repo).args(["branch"]).output().unwrap();
    support::ok(&prose, "branch prose");
    let prose_text = stdout_of(&prose);
    assert!(!prose_text.contains("heads/topic"));
    assert!(
        !prose_text.contains("tags/v1"),
        "a tag ref must not be listed as a branch: {prose_text}"
    );

    let json_out = support::prikk(&repo)
        .args(["branch", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json_out, "branch json");
    let value = json_of(&json_out);
    assert_eq!(value.get("schema_version").as_str(), "branch-list-v1");
    let branches = value.get("branches").as_array();
    assert_eq!(
        branches.len(),
        1,
        "the closed branch and the tag must both be omitted: {branches:?}"
    );
    assert_eq!(branches[0].get("ref_name").as_str(), "heads/main");
    assert!(!branches[0].get("closed").as_bool());

    // With --all: closed branch present in both, JSON carries a boolean not a marker string.
    let prose_all = support::prikk(&repo)
        .args(["branch", "--all"])
        .output()
        .unwrap();
    support::ok(&prose_all, "branch --all prose");
    assert!(
        stdout_of(&prose_all).contains("heads/topic") && stdout_of(&prose_all).contains("(closed)")
    );
    // `--all` widens *closed-ness*, never kind: the tag stays out of the branch listing either way.
    assert!(
        !stdout_of(&prose_all).contains("tags/v1"),
        "--all must not admit a tag ref into the branch listing: {}",
        stdout_of(&prose_all)
    );

    let json_all = support::prikk(&repo)
        .args(["branch", "--format", "json", "--all"])
        .output()
        .unwrap();
    support::ok(&json_all, "branch --all json");
    let value_all = json_of(&json_all);
    let branches_all = value_all.get("branches").as_array();
    assert_eq!(
        branches_all.len(),
        2,
        "--all admits the closed branch and nothing else: {branches_all:?}"
    );
    let topic = branches_all
        .iter()
        .find(|b| b.get("ref_name").as_str() == "heads/topic")
        .expect("heads/topic must be present with --all");
    assert!(topic.get("closed").as_bool());
    assert!(
        branches_all
            .iter()
            .all(|b| b.get("ref_name").as_str() != "tags/v1"),
        "--all must not admit a tag ref into `branches`: {branches_all:?}"
    );
    assert!(
        !stdout_of(&json_all).contains("(closed)"),
        "JSON must never carry the prose marker text"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 146 §8a control 2: the kind filter belongs to `branch` alone. A filter that leaked into the
/// wrong command -- `tag` dropping the very refs it exists to list -- is the failure this round
/// could introduce, so the same fixture that proves `branch` excludes a tag proves `tag` includes
/// it, in both forms.
#[test]
fn tag_still_lists_what_branch_now_filters_out() {
    let repo = support::unique_repo("rfc146-kind-filter-no-leak");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"hello\n", "genesis");
    support::ok(
        &support::tag_create(&repo, "tags/v1", "heads/main"),
        "tag create tags/v1",
    );

    let branch_json = support::prikk(&repo)
        .args(["branch", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&branch_json, "branch json");
    let branches = json_of(&branch_json);
    let branches = branches.get("branches").as_array();
    assert!(
        branches
            .iter()
            .all(|b| b.get("ref_name").as_str() != "tags/v1"),
        "branch must exclude the tag: {branches:?}"
    );
    assert_eq!(branches.len(), 1, "heads/main alone: {branches:?}");

    let tag_prose = support::prikk(&repo).args(["tag"]).output().unwrap();
    support::ok(&tag_prose, "tag prose");
    assert!(
        stdout_of(&tag_prose).contains("tags/v1"),
        "tag must still list it: {}",
        stdout_of(&tag_prose)
    );

    let tag_json = support::prikk(&repo)
        .args(["tag", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&tag_json, "tag json");
    let tags = json_of(&tag_json);
    let tags = tags.get("tags").as_array();
    assert_eq!(tags.len(), 1, "tag must still list it: {tags:?}");
    assert_eq!(tags[0].get("ref_name").as_str(), "tags/v1");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_received_ref_appears_in_its_own_array() {
    let origin = support::unique_repo("rfc146-branch-received-origin");
    support::init(&origin);
    support::generation(&origin, "heads/main", "a.txt", b"hello\n", "genesis");

    let bundle_path = origin.with_extension("bundle");
    support::ok(
        &support::prikk(&origin)
            .args([
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle_path.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "bundle export",
    );

    let receiver = support::unique_repo("rfc146-branch-received-receiver");
    support::init(&receiver);
    support::ok(
        &support::prikk(&receiver)
            .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
            .output()
            .unwrap(),
        "bundle import",
    );

    let prose = support::prikk(&receiver).args(["branch"]).output().unwrap();
    support::ok(&prose, "branch prose on receiver");
    assert!(stdout_of(&prose).contains("(received)"));

    let json_out = support::prikk(&receiver)
        .args(["branch", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json_out, "branch json on receiver");
    let value = json_of(&json_out);
    let received = value.get("received").as_array();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].get("ref_name").as_str(), "remotes/heads/main");
    assert!(received[0].get("ref_state_id").as_str().len() == 64);

    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&receiver);
    let _ = std::fs::remove_file(&bundle_path);
}

#[test]
fn branch_empty_is_a_value() {
    let repo = support::unique_repo("rfc146-branch-empty");
    support::init(&repo);

    let prose = support::prikk(&repo).args(["branch"]).output().unwrap();
    support::ok(&prose, "branch prose on fresh repository");
    assert!(stdout_of(&prose).contains("no branches"));

    let json_out = support::prikk(&repo)
        .args(["branch", "--format", "json"])
        .output()
        .unwrap();
    assert!(json_out.status.success());
    let value = json_of(&json_out);
    assert_eq!(value.get("branches").as_array().len(), 0);
    assert_eq!(value.get("received").as_array().len(), 0);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_exit_codes_on_format_flag() {
    let repo = support::unique_repo("rfc146-branch-exit-codes");
    support::init(&repo);

    let bad = support::prikk(&repo)
        .args(["branch", "--format", "yaml"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));

    let repeated = support::prikk(&repo)
        .args(["branch", "--format", "json", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(repeated.status.code(), Some(2));

    let empty_ok = support::prikk(&repo)
        .args(["branch", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(empty_ok.status.code(), Some(0));
    let _ = std::fs::remove_dir_all(&repo);
}

// ---------------------------------------------------------------------------------------------
// tag
// ---------------------------------------------------------------------------------------------

#[test]
fn tag_prose_and_json_agree() {
    let repo = support::unique_repo("rfc146-tag-agree");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"hello\n", "genesis");
    support::ok(
        &support::tag_create(&repo, "tags/v1", "heads/main"),
        "tag create tags/v1",
    );

    let prose = support::prikk(&repo).args(["tag"]).output().unwrap();
    support::ok(&prose, "tag prose");
    let prose_text = stdout_of(&prose);

    let json_out = support::prikk(&repo)
        .args(["tag", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json_out, "tag json");
    let value = json_of(&json_out);
    assert_eq!(value.get("schema_version").as_str(), "tag-list-v1");
    let tags = value.get("tags").as_array();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].get("ref_name").as_str(), "tags/v1");
    let target = tags[0].get("target_block_id").as_str();
    assert!(
        prose_text.contains(&format!("tags/v1 {target}")),
        "the JSON target block id must be the exact one prose printed: {prose_text}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_empty_is_a_value() {
    let repo = support::unique_repo("rfc146-tag-empty");
    support::init(&repo);

    let prose = support::prikk(&repo).args(["tag"]).output().unwrap();
    support::ok(&prose, "tag prose on fresh repository");
    assert!(stdout_of(&prose).contains("no tags"));

    let json_out = support::prikk(&repo)
        .args(["tag", "--format", "json"])
        .output()
        .unwrap();
    assert!(json_out.status.success());
    let value = json_of(&json_out);
    assert_eq!(value.get("tags").as_array().len(), 0);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_exit_codes_on_format_flag() {
    let repo = support::unique_repo("rfc146-tag-exit-codes");
    support::init(&repo);

    let bad = support::prikk(&repo)
        .args(["tag", "--format", "yaml"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));

    let repeated = support::prikk(&repo)
        .args(["tag", "--format", "json", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(repeated.status.code(), Some(2));

    let empty_ok = support::prikk(&repo)
        .args(["tag", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(empty_ok.status.code(), Some(0));
    let _ = std::fs::remove_dir_all(&repo);
}

/// `prikk tag list --format json` (the explicit subcommand form) must behave identically to the
/// implicit bare `prikk tag --format json` above -- both reach `run_list`.
#[test]
fn tag_explicit_list_subcommand_also_accepts_format() {
    let repo = support::unique_repo("rfc146-tag-explicit-list");
    support::init(&repo);
    let out = support::prikk(&repo)
        .args(["tag", "list", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "tag list --format json");
    let value = json_of(&out);
    assert_eq!(value.get("schema_version").as_str(), "tag-list-v1");
    let _ = std::fs::remove_dir_all(&repo);
}
