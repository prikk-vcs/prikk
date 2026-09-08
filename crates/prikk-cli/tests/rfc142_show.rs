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

/// Show-degradation handoff v2, control 1: build a real repository, corrupt a `CreateFile`-named
/// blob's *stored* bytes (via `support::swap_two_equal_length_blob_frames`, which reaches
/// `object_store.rs`'s own id-recomputation check rather than the container's earlier checksum
/// check -- see that helper's own doc comment) and show it. Exit `1`, with a message that names an
/// integrity problem, not exit `0` and render `<unavailable>`.
///
/// Shows the **patch** id, not the block id: `show` on a block first computes that block's
/// lifecycle state (`merge_evidence::lifecycle_state_at`), which performs its own, pre-existing
/// blob-resolvability replay check independent of anything this round touched (`show.rs`'s own
/// module doc: "content comes straight from the patch payloads -- no replay needed" for a bare
/// patch, `lifecycle: None`). A corrupted blob named by a *block*'s `CreateFile` fails there first,
/// regardless of what `show_blob_content` does -- confirmed empirically: perturbing
/// `show_blob_content` back to round 2's defect (degrade every `Err`) left a block-id version of
/// this fixture still exiting `1` unchanged, which means it would not have caught round 2's defect
/// and is not the control this round needs. Showing the *patch* id skips lifecycle replay entirely
/// and reaches `show_operation` -> `show_blob_content` directly, the code this round actually
/// changed; the same perturbation flips this version of the fixture to exit `0`.
#[test]
fn degradation_control1_a_corrupted_object_fails_loudly() {
    let repo = support::unique_repo("rfc142-degradation-control1-corrupt");
    support::init(&repo);
    // Two files, same byte length, distinct content -- two blobs whose container frames are
    // therefore identical total length (RFC 142 §6b handoff v2's own fixture requirement), so
    // `swap_two_equal_length_blob_frames` has a matching pair to swap.
    std::fs::write(repo.join("a.txt"), "AAAAAAAAAAAA").unwrap();
    std::fs::write(repo.join("b.txt"), "BBBBBBBBBBBB").unwrap();
    let commit_out = support::commit(&repo, "heads/main", "genesis");
    support::ok(&commit_out, "genesis");
    let patch_id = extract_patch_id(&commit_out);
    let genesis_seal = support::seal(&repo, "heads/main");
    support::ok(&genesis_seal, "seal genesis");

    support::swap_two_equal_length_blob_frames(&repo);

    let out = support::prikk(&repo)
        .args(["show", &patch_id])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("integrity"),
        "exit-1 message must name an integrity problem, got stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Show-degradation handoff v2, control 4: `ReplaceBinary`'s JSON shape is asserted directly,
/// against a block that actually contains one -- not via a block that lacks it, the way round 2's
/// `degradation_control4_ordinary_output_is_unperturbed` (an `EditText`-only block, which
/// dereferences no blob at all) let the shape change from flat `old_blob_id`/`old_size`/
/// `new_blob_id`/`new_size` to nested `old`/`new` objects ship unasserted and unmentioned.
///
/// `ReplaceBinary` is not reachable through ordinary `commit` (`show/tests.rs`'s own module doc:
/// `WorktreeChangeKind` never detects a binary content replacement), so this builds the block
/// directly against the library -- the same raw-patch-then-seal technique `prikk-store`'s own
/// `show/tests.rs` uses, exposed cross-crate via `prikk-store`'s `test-support` feature -- then
/// exercises the real binary's own JSON renderer, the only place this shape actually lives.
#[test]
fn degradation_control4_replace_binary_json_shape_is_asserted_directly() {
    use prikk_object::{
        BlobKind, BlobPayload, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId,
        ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, RefStatePayload,
        ReplaceBinary,
    };
    use prikk_store::{
        Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, ObjectReader,
        ObjectWriteSession, ObjectWriter, RepositoryLayout, Wal, add_trusted_maintainer,
        author_signature, simulate_one_seal_for_test_support, write_active_ref_metadata,
    };

    // `simulate_one_seal_for_test_support` returns the published RefState's own id (see
    // `rfc111_seal_drift_guard.rs`'s identical use), not the block id -- read the RefState back to
    // get the block id `show` actually wants.
    fn block_id_after_seal(layout: &RepositoryLayout, ref_state_id: ObjectId) -> ObjectId {
        let object_store = FileObjectStore::new(layout.clone());
        let envelope = object_store
            .read_typed(ref_state_id, ObjectType::RefState)
            .unwrap()
            .unwrap();
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
            .unwrap()
            .target_object_id
    }

    let repo = support::unique_repo("rfc142-degradation-control4-replace-binary");
    let layout = RepositoryLayout::init(repo.clone()).unwrap();
    add_trusted_maintainer(
        &layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .unwrap();
    let author = Ed25519AuthorSigner::from_seed("rfc142-control4-author", &[0x51; 32]).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();

    let write_blob = |kind: BlobKind, content: &[u8]| -> prikk_object::ObjectId {
        let payload = BlobPayload::new(kind, content.to_vec());
        let bytes = payload.to_canonical_bytes().unwrap();
        let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes);
        let mut session = ObjectWriteSession::open(&layout).unwrap();
        session.write_object(&envelope).unwrap()
    };

    // node's live `CreateFile`, sealed on its own -- `ReplaceBinary`'s target must already be a
    // live node at seal-time replay validation (found empirically, `show/tests.rs`'s own module
    // doc: an invented node id is rejected at `seal`, not at `show`).
    let node_id = NodeId::from_bytes([0x51; 32]);
    let old_content: &[u8] = b"\x00binary-old";
    std::fs::write(repo.join("bin.dat"), old_content).unwrap();
    let create_blob_id = write_blob(BlobKind::Binary, old_content);
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    let create_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "bin.dat".to_string(),
                node_id,
                blob_id: create_blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let create_bytes = create_payload.to_canonical_bytes().unwrap();
    let mut create_envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, create_bytes);
    let create_id = create_envelope.object_id();
    create_envelope
        .add_signature(author_signature(&author, create_id).unwrap())
        .unwrap();
    Wal::for_layout(&layout, prikk_store::DEFAULT_ACTIVE_NAME)
        .append_patch(&create_envelope)
        .unwrap();
    simulate_one_seal_for_test_support(&layout, "heads/main", &maintainer).unwrap();

    // A real `ReplaceBinary`: both sides must be sealed, binary-kind blobs (seal-time
    // `require_binary_blob` validation), so write the new-content blob first.
    let new_content: &[u8] = b"\x00binary-new";
    let replace_blob_id = write_blob(BlobKind::Binary, new_content);
    std::fs::write(repo.join("bin.dat"), new_content).unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    let replace_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ReplaceBinary(ReplaceBinary {
                node_id,
                old_blob_id: create_blob_id,
                new_blob_id: replace_blob_id,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let replace_bytes = replace_payload.to_canonical_bytes().unwrap();
    let mut replace_envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, replace_bytes);
    let replace_id = replace_envelope.object_id();
    replace_envelope
        .add_signature(author_signature(&author, replace_id).unwrap())
        .unwrap();
    Wal::for_layout(&layout, prikk_store::DEFAULT_ACTIVE_NAME)
        .append_patch(&replace_envelope)
        .unwrap();
    let replace_ref_state_id =
        simulate_one_seal_for_test_support(&layout, "heads/main", &maintainer).unwrap();
    let replace_block_id = block_id_after_seal(&layout, replace_ref_state_id);

    let out = support::prikk(&repo)
        .args(["show", &replace_block_id.to_string(), "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "show replace-binary block (json)");
    let value = assert_valid_json(&stdout_of(&out));
    let patches = value.get("patches").as_array();
    let operations = patches[0].get("operations").as_array();
    assert_eq!(operations[0].get("kind").as_str(), "replace-binary");
    let content = operations[0].get("content");
    let old = content.get("old");
    let new = content.get("new");
    assert_eq!(old.get("kind").as_str(), "binary");
    assert_eq!(new.get("kind").as_str(), "binary");
    assert_eq!(old.get("blob_id").as_str(), create_blob_id.to_string());
    assert_eq!(new.get("blob_id").as_str(), replace_blob_id.to_string());
    // `size` is a JSON number (`push_blob_content`'s `format!(", \"size\": {size}}}")`, no
    // quotes), so this asserts against the parser's `Number` variant directly rather than
    // `as_str()` (which would panic on a non-string value -- the point of asserting the shape
    // directly rather than trusting it).
    assert!(
        matches!(old.get("size"), serde_json_like::Value::Number(n) if n == &old_content.len().to_string()),
        "old.size: {:?}",
        old.get("size")
    );
    assert!(
        matches!(new.get("size"), serde_json_like::Value::Number(n) if n == &new_content.len().to_string()),
        "new.size: {:?}",
        new.get("size")
    );

    let _ = std::fs::remove_dir_all(&repo);
}
