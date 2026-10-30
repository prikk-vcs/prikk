//! `archive import --format json` (0.56.0 step 3c2, review ruling R2): `prepo-import-report-v1`'s
//! own per-landing `class`/`ref_state_chain` fields, and a `refusal-v1` document on exactly the
//! three connectivity-based refusals this round and 3c classify structurally. Driven through the
//! compiled binary, like `step3b_bundle_import_json.rs`'s own `bundle import --format json` tests.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;

/// Full hand-written recursive-descent JSON syntax checker (this crate has no `serde_json`), the
/// same copy `step3b_bundle_import_json.rs`/`rfc144_bundle_preview.rs` each already carry.
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

        pub(crate) fn is_null(&self) -> bool {
            matches!(self, Value::Null)
        }
    }

    pub(crate) fn skip_ws(chars: &mut Peekable<Chars<'_>>) {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
    }

    pub(crate) fn parse_value(chars: &mut Peekable<Chars<'_>>) -> Value {
        skip_ws(chars);
        match chars.peek() {
            Some('{') => parse_object(chars),
            Some('[') => parse_array(chars),
            Some('"') => Value::String(parse_string(chars)),
            Some('t') => {
                for _ in 0.."true".len() {
                    chars.next();
                }
                Value::Bool(true)
            }
            Some('f') => {
                for _ in 0.."false".len() {
                    chars.next();
                }
                Value::Bool(false)
            }
            Some('n') => {
                for _ in 0.."null".len() {
                    chars.next();
                }
                Value::Null
            }
            Some(_) => {
                let mut number = String::new();
                while matches!(chars.peek(), Some(c) if c.is_ascii_digit() || *c == '-' || *c == '.')
                {
                    number.push(chars.next().unwrap());
                }
                Value::Number(number)
            }
            None => panic!("unexpected end of input"),
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

/// `init`, commit `path` with `content`, seal -- one genesis block with the given content.
fn genesis(tag: &str, path: &str, content: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join(path), content).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");
    repo
}

/// One more generation on `repo`'s own `heads/main`: write `path`, commit, seal.
fn advance(repo: &std::path::Path, path: &str, content: &str) {
    std::fs::write(repo.join(path), content).unwrap();
    support::ok(&support::commit(repo, "heads/main", "advance"), "advance");
    support::ok(&support::seal(repo, "heads/main"), "seal advance");
}

fn export(repo: &std::path::Path, archive_path: &std::path::Path) {
    support::ok(
        &support::prikk(repo)
            .args(["archive", "export", archive_path.to_str().unwrap()])
            .output()
            .unwrap(),
        "archive export",
    );
}

fn import_json(target: &std::path::Path, archive_path: &std::path::Path) -> std::process::Output {
    support::prikk(target)
        .args([
            "archive",
            "import",
            archive_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap()
}

fn import_json_allow_non_fast_forward(
    target: &std::path::Path,
    archive_path: &std::path::Path,
) -> std::process::Output {
    support::prikk(target)
        .args([
            "archive",
            "import",
            archive_path.to_str().unwrap(),
            "--allow-non-fast-forward",
            "--format",
            "json",
        ])
        .output()
        .unwrap()
}

fn import_plain(target: &std::path::Path, archive_path: &std::path::Path) -> std::process::Output {
    support::prikk(target)
        .args(["archive", "import", archive_path.to_str().unwrap()])
        .output()
        .unwrap()
}

fn cleanup(paths: &[&std::path::Path]) {
    for path in paths {
        let _ = std::fs::remove_dir_all(path);
    }
}

#[test]
fn first_import_reports_no_local_history_per_landing() {
    let source = genesis("step3c2-nlh-source", "a.txt", "a\n");
    let archive = source.join("a.prepo");
    export(&source, &archive);
    let target = support::unique_repo("step3c2-nlh-target");
    support::init(&target);

    let out = import_json(&target, &archive);
    support::ok(&out, "archive import --format json");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(
        value.get("schema_version").as_str(),
        "prepo-import-report-v1"
    );
    let landings = value.get("landings").as_array();
    assert_eq!(landings.len(), 1);
    assert_eq!(landings[0].get("class").as_str(), "no-local-history");
    assert!(landings[0].get("ref_state_chain").is_null());

    cleanup(&[&source, &target]);
}

#[test]
fn fast_forward_reports_class_and_chain_per_landing() {
    let source = genesis("step3c2-ff-source", "a.txt", "a\n");
    let archive1 = source.join("a1.prepo");
    export(&source, &archive1);
    let target = support::unique_repo("step3c2-ff-target");
    support::init(&target);
    support::ok(&import_plain(&target, &archive1), "first import");

    advance(&source, "b.txt", "b\n");
    let archive2 = source.join("a2.prepo");
    export(&source, &archive2);

    let out = import_json(&target, &archive2);
    support::ok(&out, "archive import --format json (fast-forward)");
    let value = assert_valid_json(&stdout_of(&out));
    let landings = value.get("landings").as_array();
    assert_eq!(landings.len(), 1);
    assert_eq!(landings[0].get("class").as_str(), "fast-forward");
    assert_eq!(landings[0].get("ref_state_chain").as_str(), "extends");

    cleanup(&[&source, &target]);
}

#[test]
fn rollback_without_the_flag_refuses_with_refusal_v1_and_writes_nothing() {
    let source = genesis("step3c2-rb-source", "a.txt", "a\n");
    let archive1 = source.join("a1.prepo");
    export(&source, &archive1);
    advance(&source, "b.txt", "b\n");
    let archive2 = source.join("a2.prepo");
    export(&source, &archive2);

    let target = support::unique_repo("step3c2-rb-target");
    support::init(&target);
    support::ok(&import_plain(&target, &archive2), "land the tip first");

    let before = support::store_bytes(&target);
    let out = import_json(&target, &archive1);
    assert!(!out.status.success(), "a rollback must refuse: {out:?}");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(value.get("command").as_str(), "archive-import");
    assert_eq!(value.get("refusal").as_str(), "rollback");
    let after = support::store_bytes(&target);
    assert_eq!(
        before, after,
        "a refused archive import (0.56.0 step 3c2, R1) must write nothing at all"
    );

    cleanup(&[&source, &target]);
}

#[test]
fn rollback_with_the_flag_succeeds_and_reports_the_class() {
    let source = genesis("step3c2-rb-flag-source", "a.txt", "a\n");
    let archive1 = source.join("a1.prepo");
    export(&source, &archive1);
    advance(&source, "b.txt", "b\n");
    let archive2 = source.join("a2.prepo");
    export(&source, &archive2);

    let target = support::unique_repo("step3c2-rb-flag-target");
    support::init(&target);
    support::ok(&import_plain(&target, &archive2), "land the tip first");

    let out = import_json_allow_non_fast_forward(&target, &archive1);
    support::ok(
        &out,
        "archive import --format json --allow-non-fast-forward (rollback)",
    );
    let value = assert_valid_json(&stdout_of(&out));
    let landings = value.get("landings").as_array();
    assert_eq!(landings.len(), 1);
    assert_eq!(landings[0].get("class").as_str(), "rollback");

    cleanup(&[&source, &target]);
}

#[test]
fn format_json_does_not_change_stderr_or_exit_code_on_refusal() {
    let source = genesis("step3c2-parity-source", "a.txt", "a\n");
    let archive1 = source.join("a1.prepo");
    export(&source, &archive1);
    advance(&source, "b.txt", "b\n");
    let archive2 = source.join("a2.prepo");
    export(&source, &archive2);

    let target_plain = support::unique_repo("step3c2-parity-target-plain");
    support::init(&target_plain);
    support::ok(
        &import_plain(&target_plain, &archive2),
        "land the tip first (plain)",
    );
    let plain = import_plain(&target_plain, &archive1);

    let target_json = support::unique_repo("step3c2-parity-target-json");
    support::init(&target_json);
    support::ok(
        &import_plain(&target_json, &archive2),
        "land the tip first (json)",
    );
    let json = import_json(&target_json, &archive1);

    assert_eq!(plain.status.code(), json.status.code());
    assert_eq!(
        String::from_utf8_lossy(&plain.stderr),
        String::from_utf8_lossy(&json.stderr)
    );
    assert!(
        stdout_of(&plain).is_empty(),
        "the plain run's own stdout must stay empty on a refusal"
    );

    cleanup(&[&source, &target_plain, &target_json]);
}
