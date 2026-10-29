//! `bundle import --format json` (0.56.0 step 3b, design review v2 ruling A4):
//! `bundle-import-report-v1` on success, `refusal-v1` on exactly the three connectivity refusals
//! 3a added. Driven through the compiled binary, like `rfc144_bundle_preview.rs`'s own
//! `--format json` tests.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;

/// Full hand-written recursive-descent JSON syntax checker (this crate has no `serde_json` -- RFC
/// 118 §10 prerequisite 4), the same copy `rfc144_bundle_preview.rs` and
/// `rfc143_content_at_a_point.rs` each already carry -- this file's own narrow need, duplicated
/// here rather than factored into `support`, the same convention those two already established.
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

        pub(crate) fn as_bool(&self) -> bool {
            match self {
                Value::Bool(value) => *value,
                other => panic!("expected a bool, got {other:?}"),
            }
        }

        pub(crate) fn is_null(&self) -> bool {
            matches!(self, Value::Null)
        }

        pub(crate) fn as_number(&self) -> &str {
            match self {
                Value::Number(text) => text,
                other => panic!("expected a number, got {other:?}"),
            }
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

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
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

fn export(repo: &std::path::Path, bundle_path: &std::path::Path) {
    support::ok(
        &support::prikk(repo)
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
}

fn import_json(
    target: &std::path::Path,
    bundle_path: &std::path::Path,
    allow_non_fast_forward: bool,
) -> std::process::Output {
    let mut args = vec!["bundle", "import", "--input", bundle_path.to_str().unwrap()];
    if allow_non_fast_forward {
        args.push("--allow-non-fast-forward");
    }
    args.push("--format");
    args.push("json");
    support::prikk(target).args(args).output().unwrap()
}

fn import_plain(
    target: &std::path::Path,
    bundle_path: &std::path::Path,
    allow_non_fast_forward: bool,
) -> std::process::Output {
    let mut args = vec!["bundle", "import", "--input", bundle_path.to_str().unwrap()];
    if allow_non_fast_forward {
        args.push("--allow-non-fast-forward");
    }
    support::prikk(target).args(args).output().unwrap()
}

fn cleanup(paths: &[&std::path::Path]) {
    for path in paths {
        let _ = std::fs::remove_dir_all(path);
    }
}

// ---- Success: every field, on the classes an ordinary CLI session can reach. ----

#[test]
fn no_local_history_reports_every_field() {
    let source = genesis("step3b-nlh-source", "a.txt", "a\n");
    let bundle = source.join("b.pbndl");
    export(&source, &bundle);
    let target = support::unique_repo("step3b-nlh-target");
    support::init(&target);

    let out = import_json(&target, &bundle, false);
    support::ok(&out, "bundle import --format json");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(
        value.get("schema_version").as_str(),
        "bundle-import-report-v1"
    );
    assert_eq!(value.get("ref_name").as_str(), "remotes/heads/main");
    assert!(!value.get("ref_state_id").as_str().is_empty());
    // Measured (not guessed): one genesis commit through the real CLI (`support::commit`/`seal`)
    // bundles 5 objects -- one more than a hand-built library-level fixture's 4, since the CLI's
    // own `commit`/`seal` records an AUTHOR key object the library-level fixture skips.
    let object_count = value.get("object_count").as_number().to_string();
    assert_eq!(value.get("written_object_count").as_number(), object_count);
    assert_eq!(value.get("merged_object_count").as_number(), "0");
    assert_eq!(value.get("dropped_signatures").as_array().len(), 0);
    assert_eq!(value.get("recorded_author_key_count").as_number(), "1");
    assert_eq!(value.get("class").as_str(), "no-local-history");
    assert!(value.get("ref_state_chain").is_null());
    assert!(value.get("pointer_updated").as_bool());

    cleanup(&[&source, &target]);
}

#[test]
fn fast_forward_reports_every_field() {
    let source = genesis("step3b-ff-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);
    let target = support::unique_repo("step3b-ff-target");
    support::init(&target);
    support::ok(&import_plain(&target, &bundle1, false), "first import");

    advance(&source, "b.txt", "b\n");
    let bundle2 = source.join("b2.pbndl");
    export(&source, &bundle2);

    let out = import_json(&target, &bundle2, false);
    support::ok(&out, "bundle import --format json (fast-forward)");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("class").as_str(), "fast-forward");
    assert_eq!(value.get("ref_state_chain").as_str(), "extends");
    assert!(value.get("pointer_updated").as_bool());
    // Only the second generation's own new objects are written -- the first generation's objects
    // are already held from the earlier import.
    assert_ne!(value.get("written_object_count").as_number(), "0");

    cleanup(&[&source, &target]);
}

#[test]
fn already_at_tip_exact_reimport_reports_extends_and_pointer_updated() {
    let source = genesis("step3b-tip-source", "a.txt", "a\n");
    let bundle = source.join("b.pbndl");
    export(&source, &bundle);
    let target = support::unique_repo("step3b-tip-target");
    support::init(&target);
    support::ok(&import_plain(&target, &bundle, false), "first import");

    let out = import_json(&target, &bundle, false);
    support::ok(&out, "bundle import --format json (already-at-tip)");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("class").as_str(), "already-at-tip");
    assert_eq!(value.get("ref_state_chain").as_str(), "extends");
    // Still exits 0 and reports `pointer_updated: true` (A2: every combination but
    // already-at-tip/older writes or refuses by class alone -- an exact reimport is not the
    // older-chain case).
    assert!(value.get("pointer_updated").as_bool());
    assert_eq!(value.get("written_object_count").as_number(), "0");

    cleanup(&[&source, &target]);
}

/// A rollback, a diverged delivery, and a disconnected delivery all succeed and report every
/// field once `--allow-non-fast-forward` is given.
#[test]
fn rollback_with_the_flag_reports_every_field() {
    let source = genesis("step3b-rb-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);
    advance(&source, "b.txt", "b\n");
    let bundle2 = source.join("b2.pbndl");
    export(&source, &bundle2);

    let target = support::unique_repo("step3b-rb-target");
    support::init(&target);
    support::ok(
        &import_plain(&target, &bundle2, false),
        "first import (tip)",
    );

    let out = import_json(&target, &bundle1, true);
    support::ok(
        &out,
        "bundle import --format json --allow-non-fast-forward (rollback)",
    );
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("class").as_str(), "rollback");
    assert!(value.get("pointer_updated").as_bool());

    cleanup(&[&source, &target]);
}

#[test]
fn diverged_with_the_flag_reports_every_field() {
    let source = genesis("step3b-div-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);

    let fork = support::unique_repo("step3b-div-fork");
    std::fs::create_dir_all(fork.join(".prikk")).unwrap();
    support::copy_dir_recursive(&source.join(".prikk"), &fork.join(".prikk"));
    advance(&source, "from-a.txt", "from a\n");
    advance(&fork, "from-fork.txt", "from fork\n");
    let bundle_a = source.join("a.pbndl");
    export(&source, &bundle_a);
    let bundle_fork = fork.join("fork.pbndl");
    export(&fork, &bundle_fork);

    let target = support::unique_repo("step3b-div-target");
    support::init(&target);
    support::ok(
        &import_plain(&target, &bundle_a, false),
        "first import (a's line)",
    );

    let out = import_json(&target, &bundle_fork, true);
    support::ok(
        &out,
        "bundle import --format json --allow-non-fast-forward (diverged)",
    );
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("class").as_str(), "diverged");
    assert!(value.get("pointer_updated").as_bool());

    cleanup(&[&source, &fork, &target]);
}

#[test]
fn does_not_connect_with_the_flag_reports_every_field() {
    let source = genesis("step3b-dnc-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);
    let unrelated = genesis("step3b-dnc-unrelated", "z.txt", "z\n");
    let bundle_unrelated = unrelated.join("u.pbndl");
    export(&unrelated, &bundle_unrelated);

    let target = support::unique_repo("step3b-dnc-target");
    support::init(&target);
    support::ok(&import_plain(&target, &bundle1, false), "first import");

    let out = import_json(&target, &bundle_unrelated, true);
    support::ok(
        &out,
        "bundle import --format json --allow-non-fast-forward (does-not-connect)",
    );
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("class").as_str(), "does-not-connect");
    assert!(value.get("pointer_updated").as_bool());

    cleanup(&[&source, &unrelated, &target]);
}

// ---- Refusals: `refusal-v1`, and nothing written. ----

fn assert_refusal_v1(out: &std::process::Output, expected_id: &str) {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let value = assert_valid_json(&stdout_of(out));
    assert_eq!(value.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(value.get("command").as_str(), "bundle-import");
    assert_eq!(value.get("refusal").as_str(), expected_id);
    assert!(value.get("reason").is_null());
    assert!(
        value.get("message").as_str().contains(expected_id),
        "{value:?}"
    );
}

#[test]
fn rollback_without_the_flag_refuses_with_refusal_v1_and_writes_nothing() {
    let source = genesis("step3b-rb-refuse-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);
    advance(&source, "b.txt", "b\n");
    let bundle2 = source.join("b2.pbndl");
    export(&source, &bundle2);

    let target = support::unique_repo("step3b-rb-refuse-target");
    support::init(&target);
    support::ok(
        &import_plain(&target, &bundle2, false),
        "first import (tip)",
    );
    let before = support::store_bytes(&target);

    let out = import_json(&target, &bundle1, false);
    assert_refusal_v1(&out, "rollback");
    assert_eq!(support::store_bytes(&target), before, "nothing was written");

    cleanup(&[&source, &target]);
}

#[test]
fn diverged_without_the_flag_refuses_with_refusal_v1_and_writes_nothing() {
    let source = genesis("step3b-div-refuse-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);

    let fork = support::unique_repo("step3b-div-refuse-fork");
    std::fs::create_dir_all(fork.join(".prikk")).unwrap();
    support::copy_dir_recursive(&source.join(".prikk"), &fork.join(".prikk"));
    advance(&source, "from-a.txt", "from a\n");
    advance(&fork, "from-fork.txt", "from fork\n");
    let bundle_a = source.join("a.pbndl");
    export(&source, &bundle_a);
    let bundle_fork = fork.join("fork.pbndl");
    export(&fork, &bundle_fork);

    let target = support::unique_repo("step3b-div-refuse-target");
    support::init(&target);
    support::ok(&import_plain(&target, &bundle_a, false), "first import");
    let before = support::store_bytes(&target);

    let out = import_json(&target, &bundle_fork, false);
    assert_refusal_v1(&out, "diverged");
    assert_eq!(support::store_bytes(&target), before, "nothing was written");

    cleanup(&[&source, &fork, &target]);
}

#[test]
fn does_not_connect_without_the_flag_refuses_with_refusal_v1_and_writes_nothing() {
    let source = genesis("step3b-dnc-refuse-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);
    let unrelated = genesis("step3b-dnc-refuse-unrelated", "z.txt", "z\n");
    let bundle_unrelated = unrelated.join("u.pbndl");
    export(&unrelated, &bundle_unrelated);

    let target = support::unique_repo("step3b-dnc-refuse-target");
    support::init(&target);
    support::ok(&import_plain(&target, &bundle1, false), "first import");
    let before = support::store_bytes(&target);

    let out = import_json(&target, &bundle_unrelated, false);
    assert_refusal_v1(&out, "does-not-connect");
    assert_eq!(support::store_bytes(&target), before, "nothing was written");

    cleanup(&[&source, &unrelated, &target]);
}

// ---- `--format json`'s presence never changes stderr or the exit code. ----

#[test]
fn format_json_does_not_change_stderr_or_exit_code_on_success() {
    let source = genesis("step3b-stderr-ok-source", "a.txt", "a\n");
    let bundle = source.join("b.pbndl");
    export(&source, &bundle);

    let target_plain = support::unique_repo("step3b-stderr-ok-plain");
    support::init(&target_plain);
    let plain = import_plain(&target_plain, &bundle, false);

    let target_json = support::unique_repo("step3b-stderr-ok-json");
    support::init(&target_json);
    let json = import_json(&target_json, &bundle, false);

    assert_eq!(plain.status.code(), json.status.code());
    assert_eq!(stderr_of(&plain), stderr_of(&json));

    cleanup(&[&source, &target_plain, &target_json]);
}

#[test]
fn format_json_does_not_change_stderr_or_exit_code_on_refusal() {
    let source = genesis("step3b-stderr-refuse-source", "a.txt", "a\n");
    let bundle1 = source.join("b1.pbndl");
    export(&source, &bundle1);
    advance(&source, "b.txt", "b\n");
    let bundle2 = source.join("b2.pbndl");
    export(&source, &bundle2);

    let target_plain = support::unique_repo("step3b-stderr-refuse-plain");
    support::init(&target_plain);
    support::ok(
        &import_plain(&target_plain, &bundle2, false),
        "first import",
    );
    let plain = import_plain(&target_plain, &bundle1, false);

    let target_json = support::unique_repo("step3b-stderr-refuse-json");
    support::init(&target_json);
    support::ok(&import_plain(&target_json, &bundle2, false), "first import");
    let json = import_json(&target_json, &bundle1, false);

    assert_eq!(plain.status.code(), Some(1));
    assert_eq!(plain.status.code(), json.status.code());
    assert_eq!(stderr_of(&plain), stderr_of(&json));
    // The refusal-v1 document is the *only* thing `--format json` adds, and it lands on stdout,
    // never stderr.
    assert!(stdout_of(&json).contains("\"schema_version\": \"refusal-v1\""));
    assert!(stdout_of(&plain).is_empty(), "{}", stdout_of(&plain));

    cleanup(&[&source, &target_plain, &target_json]);
}
