//! RFC 143: `checkout --patch-plan --format json` -- content at a replayed point, for exactly the
//! requested paths.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

/// Full hand-written recursive-descent JSON syntax checker, mirroring `rfc142_show.rs`'s own
/// (this crate has no `serde_json` -- RFC 118 §10 prerequisite 4).
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

/// A digest of every regular file under `root` (path relative to `root`, plus content), sorted by
/// path -- used to prove a read-only command changed nothing on disk, repository or worktree
/// alike, rather than reasoning about it.
fn directory_digest(root: &std::path::Path) -> String {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                let bytes = std::fs::read(&path).unwrap();
                out.push((relative, bytes));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut preimage = Vec::new();
    for (path, bytes) in files {
        preimage.extend_from_slice(path.as_bytes());
        preimage.push(0);
        preimage.extend_from_slice(&bytes);
        preimage.push(0);
    }
    prikk_hash::to_hex(&prikk_hash::sha256(&preimage))
}

/// Two generations of a text file ("a.txt": "hello world\n" -> "hello prikk\n"), sealed each time.
fn text_edit_fixture(repo: &std::path::Path) {
    support::init(repo);
    std::fs::write(repo.join("a.txt"), "hello world\n").unwrap();
    support::ok(&support::commit(repo, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(repo, "heads/main"), "seal genesis");

    std::fs::write(repo.join("a.txt"), "hello prikk\n").unwrap();
    support::ok(&support::commit(repo, "heads/main", "edit a.txt"), "edit");
    support::ok(&support::seal(repo, "heads/main"), "seal edit");
}

/// Control 1, the one that matters most: content returned is byte-identical to what
/// `--patch-materialize` writes, for the same ref and path.
#[test]
fn control1_content_is_byte_identical_to_patch_materialize() {
    let repo = support::unique_repo("rfc143-control1");
    text_edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
        ])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let content = &value.get("content").as_array()[0];
    assert_eq!(content.get("path").as_str(), "a.txt");
    let text = content.get("content").get("text").as_str();

    let materialized_root = support::rebuild_from_sealed_history(&repo, "rfc143-control1");
    let materialized_bytes = std::fs::read(materialized_root.join("a.txt")).unwrap();
    assert_eq!(
        text.as_bytes(),
        materialized_bytes.as_slice(),
        "checkout --patch-plan --format json content must be byte-identical to \
         checkout --patch-materialize's own written file"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized_root);
}

/// Binary content byte-identity, the same control applied to a binary file: the JSON never
/// carries the bytes, but the declared size must match the materialized file's real size, and the
/// blob id must be the CURRENT one (post-`ReplaceBinary`), not the replaced one.
///
/// `ReplaceBinary` is reached here through *ordinary* `commit --from-worktree`: a new file whose
/// bytes are not valid UTF-8 is classified `BlobKind::Binary` at create time
/// (`node_authoring.rs::classify_new`), and editing an already-binary node's bytes plans
/// `ReplaceBinary` — found by trying it, not assumed from an older note that it needed a raw-patch
/// fixture (see the report).
#[test]
fn control1_binary_size_is_byte_identical_to_patch_materialize() {
    let repo = support::unique_repo("rfc143-control1-binary");
    support::init(&repo);
    std::fs::write(repo.join("data.bin"), b"\xff\xfe\x00\x01old-binary").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    std::fs::write(
        repo.join("data.bin"),
        b"\xff\xfe\x00\x01\x02new-binary-longer",
    )
    .unwrap();
    let commit_out = support::commit(&repo, "heads/main", "replace");
    support::ok(&commit_out, "replace");
    assert!(
        stdout_of(&commit_out).contains("replace-binary"),
        "fixture must actually exercise ReplaceBinary, not silently fall back to something else: {}",
        stdout_of(&commit_out)
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal replace");

    let out = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "data.bin",
        ])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let content = value.get("content").as_array()[0].get("content");
    assert_eq!(content.get("kind").as_str(), "binary");
    let new_binary: &[u8] = b"\xff\xfe\x00\x01\x02new-binary-longer";
    let expected_size = new_binary.len().to_string();
    assert!(
        matches!(content.get("size"), serde_json_like::Value::Number(n) if *n == expected_size),
        "size: {:?}, expected {expected_size}",
        content.get("size")
    );

    let materialized_root = support::rebuild_from_sealed_history(&repo, "rfc143-control1-binary");
    let materialized_bytes = std::fs::read(materialized_root.join("data.bin")).unwrap();
    assert_eq!(materialized_bytes, new_binary);

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized_root);
}

/// Control 2: `--patch-plan` still writes nothing, with `--format json` too. Repository and
/// worktree byte-identical before and after -- asserted, not reasoned.
#[test]
fn control2_writes_nothing_with_format_json() {
    let repo = support::unique_repo("rfc143-control2");
    text_edit_fixture(&repo);

    let before = directory_digest(&repo);
    let out = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
        ])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let after = directory_digest(&repo);
    assert_eq!(
        before, after,
        "checkout --patch-plan --format json must write nothing to the repository or worktree"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: an unsupported operation (`CreateSymlink`) still errors and does not appear as a
/// degraded field. `CreateSymlink` is unauthored today (no symlink authoring path exists), so this
/// needs the raw-patch-then-seal technique the `show` rounds established, exposed cross-crate
/// through `prikk-store`'s `test-support` feature.
///
/// This control used to exercise `RenamePath` for the same purpose; RFC 144 increment 1 made
/// `patch_replay`'s own apply path accept `RenamePath` (it stays unauthored -- `commit` still never
/// emits one -- but it is no longer *unsupported*, so it stopped being a valid fixture for "still
/// errors"). `CreateSymlink` remains refused for a different, unrelated reason (no authoring path)
/// and was out of that increment's scope, so it is still a genuine unsupported-operation fixture.
#[test]
fn control3_unsupported_operation_still_errors() {
    use prikk_object::{
        CreateFile, CreateSymlink, NodeId, ObjectEnvelope, ObjectType, Operation, OperationKind,
        PatchPayload, PatchPurpose,
    };
    use prikk_store::{
        Ed25519AuthorSigner, Ed25519MaintainerSigner, ObjectWriteSession, ObjectWriter,
        RepositoryLayout, Wal, add_trusted_maintainer, author_signature,
        simulate_one_seal_for_test_support, write_active_ref_metadata,
    };

    let repo = support::unique_repo("rfc143-control3");
    let layout = RepositoryLayout::init(repo.clone()).unwrap();
    add_trusted_maintainer(
        &layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .unwrap();
    let author = Ed25519AuthorSigner::from_seed("rfc143-control3-author", &[0x61; 32]).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();

    let node_id = NodeId::from_bytes([0x71; 32]);
    let blob_id = {
        use prikk_object::{BlobKind, BlobPayload, CanonicalEncode};
        let payload = BlobPayload::new(BlobKind::Text, b"hello\n".to_vec());
        let bytes = payload.to_canonical_bytes().unwrap();
        let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes);
        let mut session = ObjectWriteSession::open(&layout).unwrap();
        session.write_object(&envelope).unwrap()
    };
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    let payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "a.txt".to_string(),
                    node_id,
                    blob_id,
                    mode: 0o100_644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateSymlink(CreateSymlink {
                    path: "link".to_string(),
                    node_id: NodeId::from_bytes([0x72; 32]),
                    target: "a.txt".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    use prikk_object::CanonicalEncode;
    let bytes = payload.to_canonical_bytes().unwrap();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, bytes);
    let id = envelope.object_id();
    envelope
        .add_signature(author_signature(&author, id).unwrap())
        .unwrap();
    Wal::for_layout(&layout, prikk_store::DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
    simulate_one_seal_for_test_support(&layout, "heads/main", &maintainer).unwrap();

    let out = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = stderr_of(&out).to_lowercase();
    assert!(
        stderr.contains("symlink") || stderr.contains("unsupported"),
        "error must name the unsupported operation, got: {stderr}"
    );
    assert!(
        stdout_of(&out).is_empty(),
        "an error must produce no partial JSON on stdout: {}",
        stdout_of(&out)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: the coverage field is branchable -- a consumer can read it without string-matching
/// prose, and it names the single-parent walk.
#[test]
fn control4_coverage_is_branchable() {
    let repo = support::unique_repo("rfc143-control4");
    text_edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["checkout", "--patch-plan", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let coverage = value.get("coverage");
    assert_eq!(coverage.get("walk").as_str(), "single-parent");
    let kinds = coverage.get("applied_operation_kinds").as_array();
    let kind_strs: Vec<&str> = kinds.iter().map(|v| v.as_str()).collect();
    assert!(kind_strs.contains(&"create-file"));
    assert!(kind_strs.contains(&"edit-text"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5: binary content is never emitted as bytes -- the JSON's own `content` object for a
/// binary path never carries a `"text"` key, and the size is a JSON number, not a string.
#[test]
fn control5_binary_content_never_emitted_as_bytes() {
    let repo = support::unique_repo("rfc143-control5");
    support::init(&repo);
    std::fs::write(repo.join("data.bin"), b"\xff\xfe\x00binary").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    let out = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "data.bin",
        ])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let stdout = stdout_of(&out);
    assert!(
        !stdout.contains("\"text\""),
        "binary content must never carry a text field: {stdout}"
    );
    let value = assert_valid_json(&stdout);
    let content = value.get("content").as_array()[0].get("content");
    assert_eq!(content.get("kind").as_str(), "binary");
    assert!(matches!(
        content.get("blob_id"),
        serde_json_like::Value::String(_)
    ));
    assert!(matches!(
        content.get("size"),
        serde_json_like::Value::Number(_)
    ));

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6 (part): exit codes hold. `2` for a malformed argument (caught before repository work
/// begins), `1` for an operational failure, `0` when the plan resolves.
#[test]
fn control6_exit_codes_hold() {
    let repo = support::unique_repo("rfc143-control6");
    text_edit_fixture(&repo);

    let malformed_path = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "../escape",
        ])
        .output()
        .unwrap();
    assert_eq!(malformed_path.status.code(), Some(2), "{malformed_path:?}");

    let content_path_without_json = support::prikk(&repo)
        .args(["checkout", "--patch-plan", "--content-path", "a.txt"])
        .output()
        .unwrap();
    assert_eq!(
        content_path_without_json.status.code(),
        Some(2),
        "{content_path_without_json:?}"
    );

    let format_on_wrong_mode = support::prikk(&repo)
        .args(["checkout", "--plan-only", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        format_on_wrong_mode.status.code(),
        Some(2),
        "{format_on_wrong_mode:?}"
    );

    let nonexistent_ref = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--ref",
            "heads/nonexistent",
        ])
        .output()
        .unwrap();
    assert_eq!(
        nonexistent_ref.status.code(),
        Some(1),
        "{nonexistent_ref:?}"
    );

    let resolved = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
        ])
        .output()
        .unwrap();
    assert_eq!(resolved.status.code(), Some(0), "{resolved:?}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// A bare `--format json` (no `--content-path`) reports metadata without content (RFC 143 §5's
/// own open question, answered here): `content` and `not_found` both empty, `coverage` still
/// populated.
#[test]
fn bare_format_json_reports_metadata_only() {
    let repo = support::unique_repo("rfc143-bare-json");
    text_edit_fixture(&repo);

    let out = support::prikk(&repo)
        .args(["checkout", "--patch-plan", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("content").as_array().len(), 0);
    assert_eq!(value.get("not_found").as_array().len(), 0);
    assert!(
        !value
            .get("coverage")
            .get("applied_operation_kinds")
            .as_array()
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Absence degrades to `not_found`, not an error: a path that never existed, and a path that was
/// deleted before the target point.
#[test]
fn absent_paths_report_not_found() {
    let repo = support::unique_repo("rfc143-not-found");
    text_edit_fixture(&repo);
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "delete a.txt"),
        "delete",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal delete");

    let out = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
            "--content-path",
            "never-existed.txt",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let value = assert_valid_json(&stdout_of(&out));
    assert_eq!(value.get("content").as_array().len(), 0);
    let not_found = value.get("not_found").as_array();
    let not_found_strs: Vec<&str> = not_found.iter().map(|v| v.as_str()).collect();
    assert!(not_found_strs.contains(&"a.txt"));
    assert!(not_found_strs.contains(&"never-existed.txt"));

    let _ = std::fs::remove_dir_all(&repo);
}
