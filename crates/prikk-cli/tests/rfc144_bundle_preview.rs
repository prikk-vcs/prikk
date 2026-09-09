//! RFC 144 §4m: `prikk bundle preview --input <file>`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

/// Full hand-written recursive-descent JSON syntax checker, mirroring `rfc143_content_at_a_point.rs`'s
/// own (this crate has no `serde_json` -- RFC 118 §10 prerequisite 4).
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

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Build a source repo with one committed+sealed file, fork it (so `local` starts byte-identical),
/// then advance `source` by one more file -- a clean fast-forward shape. Returns (source, local,
/// bundle file path).
fn fast_forward_fixture(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let source = support::unique_repo(&format!("{tag}-source"));
    support::init(&source);
    std::fs::write(source.join("a.txt"), "shared\n").unwrap();
    support::ok(
        &support::commit(&source, "heads/main", "genesis"),
        "genesis",
    );
    support::ok(&support::seal(&source, "heads/main"), "seal genesis");

    let local = support::unique_repo(&format!("{tag}-local"));
    std::fs::create_dir_all(local.join(".prikk")).unwrap();
    support::copy_dir_recursive(&source.join(".prikk"), &local.join(".prikk"));

    std::fs::write(source.join("b.txt"), "new in source\n").unwrap();
    support::ok(
        &support::commit(&source, "heads/main", "advance"),
        "advance",
    );
    support::ok(&support::seal(&source, "heads/main"), "seal advance");

    let bundle_path = support::unique_repo(&format!("{tag}-bundle")).join("bundle.pbndl");
    let export = support::prikk(&source)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export, "bundle export");

    (source, local, bundle_path)
}

#[test]
fn bundle_preview_reports_fast_forward_and_the_new_file() {
    let (source, local, bundle_path) = fast_forward_fixture("rfc144-preview-cli-ff");

    let out = support::prikk(&local)
        .args([
            "bundle",
            "preview",
            "--input",
            bundle_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&out, "bundle preview");
    let stdout = stdout_of(&out);
    assert!(stdout.contains("fast-forward"), "{stdout}");
    assert!(stdout.contains("b.txt"), "{stdout}");

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&local);
    let _ = std::fs::remove_dir_all(bundle_path.parent().unwrap());
}

#[test]
fn bundle_preview_format_json_is_valid_and_machine_branchable() {
    let (source, local, bundle_path) = fast_forward_fixture("rfc144-preview-cli-json");

    let out = support::prikk(&local)
        .args([
            "bundle",
            "preview",
            "--input",
            bundle_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    support::ok(&out, "bundle preview --format json");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("connectivity").as_str(), "fast-forward");
    assert_eq!(value.get("conflict").as_str(), "applies-cleanly");
    let effects = value.get("effects").as_array();
    assert_eq!(effects.len(), 1, "{effects:?}");
    assert_eq!(effects[0].get("path").as_str(), "b.txt");
    assert_eq!(effects[0].get("kind").as_str(), "created");
    // Machine-branchable per §4m.4: a caller reads the `connectivity`/`conflict` fields, never
    // greps stdout prose or infers from the exit code.
    assert_eq!(out.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&local);
    let _ = std::fs::remove_dir_all(bundle_path.parent().unwrap());
}

/// §4m.4: a bundle that does not connect, or that would conflict, still exits `0` -- the command
/// was asked what would happen and it answered. Built directly on `does-not-connect` since that
/// shape needs no divergence machinery to set up through the ordinary CLI.
#[test]
fn bundle_preview_exits_zero_even_when_disconnected() {
    let source = support::unique_repo("rfc144-preview-cli-disconnect-source");
    support::init(&source);
    std::fs::write(source.join("a.txt"), "source\n").unwrap();
    support::ok(
        &support::commit(&source, "heads/main", "genesis"),
        "genesis",
    );
    support::ok(&support::seal(&source, "heads/main"), "seal genesis");
    let bundle_path =
        support::unique_repo("rfc144-preview-cli-disconnect-bundle").join("bundle.pbndl");
    support::ok(
        &support::prikk(&source)
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

    let local = support::unique_repo("rfc144-preview-cli-disconnect-local");
    support::init(&local);
    std::fs::write(local.join("unrelated.txt"), "no shared history\n").unwrap();
    support::ok(&support::commit(&local, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(&local, "heads/main"), "seal genesis");

    let out = support::prikk(&local)
        .args([
            "bundle",
            "preview",
            "--input",
            bundle_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("connectivity").as_str(), "does-not-connect");

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&local);
    let _ = std::fs::remove_dir_all(bundle_path.parent().unwrap());
}

#[test]
fn bundle_preview_requires_input() {
    let repo = support::unique_repo("rfc144-preview-cli-missing-input");
    support::init(&repo);
    let out = support::prikk(&repo)
        .args(["bundle", "preview"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(stderr_of(&out).contains("--input"), "{}", stderr_of(&out));

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn bundle_preview_refuses_a_duplicate_input_flag() {
    let repo = support::unique_repo("rfc144-preview-cli-duplicate-input");
    support::init(&repo);
    let out = support::prikk(&repo)
        .args([
            "bundle", "preview", "--input", "a.pbndl", "--input", "b.pbndl",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn bundle_preview_help_matches_the_bundle_command_help() {
    let repo = support::unique_repo("rfc144-preview-cli-help");
    let bundle_help = support::prikk(&repo)
        .args(["bundle", "--help"])
        .output()
        .unwrap();
    let preview_help = support::prikk(&repo)
        .args(["bundle", "preview", "--help"])
        .output()
        .unwrap();
    support::ok(&bundle_help, "bundle --help");
    support::ok(&preview_help, "bundle preview --help");
    assert_eq!(bundle_help.stdout, preview_help.stdout);
    let stdout = stdout_of(&bundle_help);
    assert!(stdout.contains("bundle preview"), "{stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}
