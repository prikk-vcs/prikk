//! `prikk show` (RFC 142), driven through the compiled binary. §2's finding (a patch is
//! self-describing) and §4's path resolution are both `prikk-store`-level tests
//! (`crates/prikk-store/src/show/tests.rs`); this file covers what only the real binary can show:
//! the prose/JSON rendering (control 6/7) and the exit-code contract (control 8).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

/// Full hand-written recursive-descent JSON syntax checker, mirroring `rfc140_status_json_and_
/// enumeration.rs`'s own (this crate has no `serde_json` -- RFC 118 §10 prerequisite 4).
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

fn extract_block_id(output: &std::process::Output) -> String {
    stdout_of(output)
        .lines()
        .find_map(|line| line.strip_prefix("block id: "))
        .expect("seal output must include a block id line")
        .trim()
        .to_string()
}

fn extract_patch_id(output: &std::process::Output) -> String {
    stdout_of(output)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit output must include a patch id line")
        .trim()
        .to_string()
}

/// Build one genesis block ("a.txt": "before\n") and one edit block ("a.txt": "after\n"),
/// returning (genesis block id, edit block id, edit patch id).
fn edit_fixture(repo: &std::path::Path) -> (String, String, String) {
    support::init(repo);
    std::fs::write(repo.join("a.txt"), "before\n").unwrap();
    support::ok(&support::commit(repo, "heads/main", "genesis"), "genesis");
    let genesis_seal = support::seal(repo, "heads/main");
    support::ok(&genesis_seal, "seal genesis");
    let genesis_block_id = extract_block_id(&genesis_seal);

    std::fs::write(repo.join("a.txt"), "after\n").unwrap();
    let commit_out = support::commit(repo, "heads/main", "edit a.txt");
    support::ok(&commit_out, "edit a.txt");
    let edit_patch_id = extract_patch_id(&commit_out);
    let edit_seal = support::seal(repo, "heads/main");
    support::ok(&edit_seal, "seal edit");
    let edit_block_id = extract_block_id(&edit_seal);

    (genesis_block_id, edit_block_id, edit_patch_id)
}

/// Controls 1/2, end to end through the real binary: a text edit shows its before and after, and
/// the node-addressed operation resolves to a real path.
#[test]
fn control1_and_2_text_edit_prose_shows_before_after_and_resolved_path() {
    let repo = support::unique_repo("rfc142-control1-2");
    let (_genesis, edit_block_id, _patch) = edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["show", &edit_block_id])
        .output()
        .unwrap();
    support::ok(&out, "show edit block");
    let stdout = stdout_of(&out);
    assert!(stdout.contains("edit-text"), "{stdout}");
    assert!(stdout.contains("path: a.txt"), "{stdout}");
    assert!(stdout.contains("before"), "{stdout}");
    assert!(stdout.contains("after"), "{stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6: nothing is a synthetic diff. No hunk header, no line-number gutter, in either
/// rendering.
#[test]
fn control6_no_synthetic_diff_in_prose_or_json() {
    let repo = support::unique_repo("rfc142-control6");
    let (_genesis, edit_block_id, _patch) = edit_fixture(&repo);

    let prose = support::prikk(&repo)
        .args(["show", &edit_block_id])
        .output()
        .unwrap();
    support::ok(&prose, "show edit block (prose)");
    let prose_stdout = stdout_of(&prose);
    assert!(!prose_stdout.contains("@@"), "{prose_stdout}");
    // A unified diff's own line-number gutter looks like "12,3 +12,4" or "12c12" -- neither
    // pattern (digit, separator, digit) appears anywhere `show` writes a number (only in "mode:
    // NNN" and "operation N:", neither of which is a hunk range).
    assert!(
        !prose_stdout.contains(" +") && !prose_stdout.contains(" -"),
        "a line looked like a unified-diff hunk marker: {prose_stdout}"
    );

    let json = support::prikk(&repo)
        .args(["show", &edit_block_id, "--format", "json"])
        .output()
        .unwrap();
    support::ok(&json, "show edit block (json)");
    let json_stdout = stdout_of(&json);
    assert!(!json_stdout.contains("@@"), "{json_stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 7: the JSON parses and carries its `schema_version`.
#[test]
fn control7_json_parses_and_carries_schema_version() {
    let repo = support::unique_repo("rfc142-control7");
    let (_genesis, edit_block_id, _patch) = edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["show", &edit_block_id, "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "show --format json");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("schema_version").as_str(), "show-report-v1");
    let patches = value.get("patches").as_array();
    assert_eq!(patches.len(), 1);
    let operations = patches[0].get("operations").as_array();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].get("kind").as_str(), "edit-text");
    let paths = operations[0].get("paths").as_array();
    assert_eq!(paths[0].get("path").as_str(), "a.txt");
    let content = operations[0].get("content");
    assert_eq!(content.get("old_span_text").as_str(), "before");
    assert_eq!(content.get("replacement_text").as_str(), "after");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 8 (half): exit `2` for a malformed id or a missing one -- both are usage errors,
/// caught before any repository work begins (RFC 121's own contract).
#[test]
fn control8_exit_2_for_malformed_or_missing_id() {
    let repo = support::unique_repo("rfc142-control8-usage");
    support::init(&repo);

    let malformed = support::prikk(&repo)
        .args(["show", "not-a-real-id"])
        .output()
        .unwrap();
    assert_eq!(malformed.status.code(), Some(2), "{malformed:?}");

    let missing = support::prikk(&repo).args(["show"]).output().unwrap();
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Show-degradation handoff, control 5: degrading a missing *blob* must not soften the
/// missing-*target* path, which is a different failure and stays a failure. A well-formed but
/// nonexistent object id still exits `1`.
#[test]
fn control8_exit_1_for_a_well_formed_nonexistent_target_id() {
    let repo = support::unique_repo("rfc142-control8-nonexistent");
    support::init(&repo);

    let out = support::prikk(&repo)
        .args(["show", &"0".repeat(64)])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 8 (half): exit `0` whichever way `show` resolves -- a genesis block (a `CreateFile`,
/// not a change against anything) is still a valid, complete answer, not a failure.
#[test]
fn control8_exit_0_for_a_resolved_block() {
    let repo = support::unique_repo("rfc142-control8-ok");
    let (genesis_block_id, _edit, _patch) = edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["show", &genesis_block_id])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5: `show` on a patch id and on a block id both work.
#[test]
fn control5_show_on_a_patch_id_works_too() {
    let repo = support::unique_repo("rfc142-control5");
    let (_genesis, _edit_block, edit_patch_id) = edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["show", &edit_patch_id])
        .output()
        .unwrap();
    support::ok(&out, "show edit patch");
    let stdout = stdout_of(&out);
    assert!(stdout.contains("edit-text"), "{stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Create `a.txt`, seal; edit it, seal; delete it, seal (the show-degradation handoff's own §2
/// sequence). Returns the delete block id.
fn edit_then_delete_fixture(repo: &std::path::Path) -> String {
    let (_genesis, _edit_block, _edit_patch) = edit_fixture(repo);
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    support::ok(
        &support::commit(repo, "heads/main", "delete a.txt"),
        "delete a.txt",
    );
    let delete_seal = support::seal(repo, "heads/main");
    support::ok(&delete_seal, "seal delete");
    extract_block_id(&delete_seal)
}

/// Show-degradation handoff control 1 (round 1's control 3, rebuilt as originally specified):
/// create/edit/delete `a.txt`, seal each. `show` on the delete block exits `0` and renders every
/// operation -- this is the exact sequence round 1 exited `1` on.
#[test]
fn degradation_control1_exits_0_and_renders_every_operation() {
    let repo = support::unique_repo("rfc142-degradation-control1");
    let delete_block_id = edit_then_delete_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["show", &delete_block_id])
        .output()
        .unwrap();
    support::ok(&out, "show delete block");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = stdout_of(&out);
    assert!(stdout.contains("delete-node"), "{stdout}");
    assert!(
        stdout.contains("unavailable"),
        "delete-node's preimage content must degrade, not vanish: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Show-degradation handoff control 2: JSON exposes the degraded state as a field a consumer can
/// branch on (a distinct `"kind": "unavailable"`), never confusable with real empty text content
/// (`"kind": "text", "text": ""`).
#[test]
fn degradation_control2_json_exposes_a_branchable_unavailable_state() {
    let repo = support::unique_repo("rfc142-degradation-control2");
    let delete_block_id = edit_then_delete_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["show", &delete_block_id, "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "show delete block (json)");
    let value = assert_valid_json(&stdout_of(&out));
    let patches = value.get("patches").as_array();
    let delete_patch = patches
        .iter()
        .find(|patch| patch.get("operations").as_array()[0].get("kind").as_str() == "delete-node")
        .expect("a delete-node patch");
    let operation = &delete_patch.get("operations").as_array()[0];
    let preimage = operation.get("content").get("preimage");
    assert_eq!(preimage.get("kind").as_str(), "file");
    let content = preimage.get("content");
    assert_eq!(content.get("kind").as_str(), "unavailable");
    // Distinct key from the successful-text shape -- a consumer checking for `"text"` never sees
    // it here, and checking for `"kind": "text"` never matches an unavailable blob either.
    assert!(matches!(content, serde_json_like::Value::Object(map) if map.contains_key("blob_id")));

    let _ = std::fs::remove_dir_all(&repo);
}

/// Show-degradation handoff control 4: a block whose blobs are all readable renders no
/// `"unavailable"` anywhere -- the fix adds a state, it does not perturb the ordinary path.
#[test]
fn degradation_control4_ordinary_output_is_unperturbed() {
    let repo = support::unique_repo("rfc142-degradation-control4");
    let (_genesis, edit_block_id, _patch) = edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["show", &edit_block_id, "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "show edit block (json)");
    let stdout = stdout_of(&out);
    assert!(!stdout.contains("unavailable"), "{stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}
