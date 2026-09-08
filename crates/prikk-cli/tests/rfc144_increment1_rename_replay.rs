//! RFC 144 increment 1: `patch_replay` applies `RenamePath` -- controls 4 and 5, at the CLI
//! surfaces increment 1 unblocks. `commit` never authors a `RenamePath` (that stays true through
//! increment 3), so this needs the raw-patch-then-seal technique `rfc143_content_at_a_point.rs`'s
//! own control 3 established, exposed cross-crate through `prikk-store`'s `test-support` feature.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use prikk_object::{
    CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectType, Operation, OperationKind,
    PatchPayload, PatchPurpose, RenamePath,
};
use prikk_store::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, ObjectWriteSession, ObjectWriter,
    RepositoryLayout, Wal, add_trusted_maintainer, author_signature,
    simulate_one_seal_for_test_support, write_active_ref_metadata,
};

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Full hand-written recursive-descent JSON syntax checker, mirroring
/// `rfc143_content_at_a_point.rs`'s own (this crate has no `serde_json` -- RFC 118 §10
/// prerequisite 4).
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

/// One node, created at `a.txt` then renamed to `b.txt`, in one sealed patch -- the same
/// raw-patch-then-seal shape `rfc143_content_at_a_point.rs::control3_unsupported_operation_still_errors`
/// used when `RenamePath` was still unsupported. Returns the repo root.
fn create_then_rename_fixture(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    let layout = RepositoryLayout::init(repo.clone()).unwrap();
    add_trusted_maintainer(
        &layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .unwrap();
    let author = Ed25519AuthorSigner::from_seed(format!("{tag}-author"), &[0x61; 32]).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();

    let node_id = NodeId::from_bytes([0x75; 32]);
    let blob_id = {
        use prikk_object::{BlobKind, BlobPayload};
        let payload = BlobPayload::new(BlobKind::Text, b"renamed content\n".to_vec());
        let bytes = payload.to_canonical_bytes().unwrap();
        let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes);
        let mut session = ObjectWriteSession::open(&layout).unwrap();
        session.write_object(&envelope).unwrap()
    };
    std::fs::write(repo.join("b.txt"), "renamed content\n").unwrap();
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
                kind: OperationKind::RenamePath(RenamePath {
                    node_id,
                    old_path: "a.txt".to_string(),
                    new_path: "b.txt".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
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
    repo
}

/// Control 4: `checkout --patch-materialize` over rename-containing history writes the node's
/// content at its *new* path, and the old path does not exist.
#[test]
fn control4_patch_materialize_writes_content_at_new_path() {
    let repo = create_then_rename_fixture("rfc144-inc1-control4-materialize");

    let materialized_root =
        support::rebuild_from_sealed_history(&repo, "rfc144-inc1-control4-materialize");
    assert!(
        !materialized_root.join("a.txt").exists(),
        "the old path must not exist after materialization"
    );
    let bytes = std::fs::read(materialized_root.join("b.txt")).unwrap();
    assert_eq!(bytes, b"renamed content\n");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized_root);
}

/// Control 4 (other half): `checkout --patch-plan --format json --content-path` over
/// rename-containing history reports the node's content at its *new* path; the old path reports
/// `not_found`.
#[test]
fn control4_patch_plan_content_path_reports_new_path() {
    let repo = create_then_rename_fixture("rfc144-inc1-control4-plan");

    let out = support::prikk(&repo)
        .args([
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "b.txt",
            "--content-path",
            "a.txt",
        ])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let entries = value.get("content").as_array();
    assert_eq!(entries.len(), 1, "expected exactly one entry: {entries:?}");
    assert_eq!(
        entries[0].get("path").as_str(),
        "b.txt",
        "content must be reported at the new path"
    );
    assert_eq!(
        entries[0].get("content").get("text").as_str(),
        "renamed content\n"
    );
    let not_found: Vec<&str> = value
        .get("not_found")
        .as_array()
        .iter()
        .map(|v| v.as_str())
        .collect();
    assert_eq!(
        not_found,
        vec!["a.txt"],
        "the old path must no longer be live, and must report as not_found"
    );
    let kinds: Vec<&str> = value
        .get("coverage")
        .get("applied_operation_kinds")
        .as_array()
        .iter()
        .map(|v| v.as_str())
        .collect();
    assert!(
        kinds.contains(&"rename-path"),
        "coverage must record rename-path as an applied operation kind: {kinds:?}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5 (RFC 143's own control 1, extended to rename-containing history): content from
/// `--patch-plan --format json` is byte-identical to what `--patch-materialize` writes, at the
/// node's new path.
#[test]
fn control5_byte_identity_holds_over_rename_containing_history() {
    let repo = support::unique_repo("rfc144-inc1-control5");
    let layout = RepositoryLayout::init(repo.clone()).unwrap();
    add_trusted_maintainer(
        &layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .unwrap();
    let author =
        Ed25519AuthorSigner::from_seed("rfc144-inc1-control5-author", &[0x61; 32]).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();

    let node_id = NodeId::from_bytes([0x76; 32]);
    let blob_id = {
        use prikk_object::{BlobKind, BlobPayload};
        let payload = BlobPayload::new(BlobKind::Text, b"byte identity check\n".to_vec());
        let bytes = payload.to_canonical_bytes().unwrap();
        let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes);
        let mut session = ObjectWriteSession::open(&layout).unwrap();
        session.write_object(&envelope).unwrap()
    };
    std::fs::write(repo.join("new-name.txt"), "byte identity check\n").unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    let payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "old-name.txt".to_string(),
                    node_id,
                    blob_id,
                    mode: 0o100_644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id,
                    old_path: "old-name.txt".to_string(),
                    new_path: "new-name.txt".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
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
            "new-name.txt",
        ])
        .output()
        .unwrap();
    support::ok(&out, "checkout --patch-plan --format json");
    let value = assert_valid_json(&stdout_of(&out));
    let entries = value.get("content").as_array();
    assert_eq!(entries.len(), 1, "expected exactly one entry: {entries:?}");
    let planned_text = entries[0].get("content").get("text").as_str();

    let materialized_root = support::rebuild_from_sealed_history(&repo, "rfc144-inc1-control5");
    let materialized_bytes = std::fs::read(materialized_root.join("new-name.txt")).unwrap();
    assert_eq!(
        planned_text.as_bytes(),
        materialized_bytes.as_slice(),
        "checkout --patch-plan --format json content must be byte-identical to \
         checkout --patch-materialize's own written file, over rename-containing history"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized_root);
}
