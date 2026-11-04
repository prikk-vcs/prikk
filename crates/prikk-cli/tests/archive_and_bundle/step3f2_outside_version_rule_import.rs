//! 0.56.0 F2 item 3 (review `.git-exclude/reviewed/0.56.0-step-3-f-review-v1.md`, "F2 item 3"):
//! `refusal-v1`'s own `"outside-version-rule"` id, wired into both `bundle import` and `archive
//! import` since the 0.56.0 step F round, had no test for either importer -- the step F report
//! skipped it as "low-value", which the review rejects outright: "it is exactly the class
//! 'measure the JSON id, not only the sentence' exists for" (3d and 3d2 shipped the wrong id
//! once, caught only by asserting the printed document itself).
//!
//! **`archive import` only**: this file covers `archive import`'s own `refusal-v1` document end
//! to end, through the compiled binary. `bundle import`'s own twin lives instead at
//! `crates/prikk-store/src/bundle/tests.rs::bundle_import_refuses_outside_version_rule_for_a_non_
//! ascii_path` -- a `prikk-store`-internal test, not a CLI one, for a structural reason found
//! while building this file: `export_bundle` itself decodes every carried `Patch`'s own
//! operations (`decode_patch_operations`, to discover referenced Blobs) as part of its own
//! closure walk, so **no local repository holding this fixture can ever be exported to a bundle
//! at all** -- confirmed directly, by trying it: `export_bundle` itself fails with the identical
//! `NonAsciiRepoPath`, before any bundle file exists to import. A CLI-level `bundle import
//! --format json` test for this id is therefore not constructible through the public API at all
//! (there is no way to get this exact byte shape into a `.pbndl` except `prikk-store`'s own
//! `pub(crate) encode_bundle`, which serializes already-built envelopes verbatim with no decode
//! of its own -- unreachable from here). The `prikk-store`-internal test asserts the identical
//! underlying classification (`PrikkError::NonAsciiRepoPath`/`DecodeFailureKind::
//! OutsideVersionRule`) and byte identity; it does not assert the CLI's own printed JSON text,
//! which is produced by a simple, pre-existing, unchanged-by-this-round match arm in
//! `crates/prikk-cli/src/bundle.rs` (`DecodeFailureKind::OutsideVersionRule =>
//! "outside-version-rule"`) -- the identical arm this file's own `archive import` test below
//! exercises for `crates/prikk-cli/src/archive.rs`'s own twin.
//!
//! The fixture is a path outside this version's own rule (a non-ASCII `CreateFile` path), built
//! the same way `crates/prikk-store/src/archive/tests.rs::seal_one_block_with_an_outside_version_
//! rule_patch` and `crates/prikk-cli/tests/extract_and_read/step3f_claim_chain_report.rs::write_
//! hand_crafted_create_file_patch` already build it: driving `CanonicalWriter` directly, bypassing
//! `CreateFile`'s own validating `encode_canonical`/`validate_repo_path` (no honest `commit` can
//! author this path at all). Those two helpers are each unreachable from here (one is `prikk-
//! store`-internal `pub(crate)`, the other lives in a different test binary and is not `pub`), so
//! this file builds its own copy, mirroring `step3f_forged_publication_signature.rs`'s (this
//! directory) own convention of building fixtures directly through `prikk-store`'s public API
//! rather than through the CLI (which can never author a non-ASCII path, or a forged signature,
//! honestly).
//!
//! Decode of a `Patch`'s own operations (`decode_patch_operations_for_admission`) runs
//! unconditionally for every carried `Patch`, during the equivalent early stage for `archive
//! import` (`archive import`'s own pass 1) -- before any publication-trust check ever runs. So
//! this fixture's own `Block`/`RefState` signatures need not be adopted by the destination at
//! all: the non-ASCII path is refused on its own, before trust is ever consulted.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::io::Write as _;
use std::path::Path;
use std::process::Output;

use crate::support;

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, CanonicalWriter, MerkleRoot, NodeId, ObjectEnvelope,
    ObjectId, ObjectType, RefKind, RefStatePayload, RefUpdatePayload, WireType,
};
use prikk_store::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, author_signature, export_archive, maintainer_signature,
};

/// Full hand-written recursive-descent JSON syntax checker -- this crate has no `serde_json`; the
/// same copy `step3f_forged_publication_signature.rs` (this directory) and several siblings each
/// already carry, duplicated here rather than factored into `support`, the same convention those
/// already established.
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

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn cleanup(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// A single `CreateFile` patch naming `path` exactly as given, built by driving
/// `prikk_object::CanonicalWriter` directly -- bypassing `CreateFile`'s own validating
/// `encode_canonical`/`validate_repo_path`, the identical bypass `archive/tests.rs::seal_one_
/// block_with_an_outside_version_rule_patch` and `extract_and_read/step3f_claim_chain_report.rs::
/// write_hand_crafted_create_file_patch` each already use. Signed for real, with a genuine
/// `Ed25519AuthorSigner` -- no honest CLI `commit` can author this path at all, but nothing else
/// about this patch's own envelope is dishonest.
fn write_hand_crafted_create_file_patch(
    store: &mut FileObjectStore,
    author: &Ed25519AuthorSigner,
    path: &str,
    node_byte: u8,
) -> ObjectId {
    let mut create_file = CanonicalWriter::new();
    create_file.field_repo_path(1, path).expect("encode path");
    create_file
        .field_bytes(2, NodeId::from_bytes([node_byte; 32]).as_bytes())
        .expect("encode node id");
    create_file
        .field_object_id(3, &ObjectId::from_bytes([node_byte.wrapping_add(1); 32]))
        .expect("encode blob id");
    create_file.field_u32(4, 0o100_644).expect("encode mode");
    let create_file_bytes = create_file.finish();

    let mut operation = CanonicalWriter::new();
    operation.field_u32(1, 1).expect("encode op_seq");
    operation
        .field_raw(10, WireType::Record, &create_file_bytes)
        .expect("encode operation");
    let operation_bytes = operation.finish();

    let mut payload = CanonicalWriter::new();
    payload
        .field_raw(1, WireType::RecordListItem, &operation_bytes)
        .expect("encode patch payload");
    let canonical_payload = payload.finish();

    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, canonical_payload);
    let patch_id = patch.object_id();
    let patch_sig = author_signature(author, patch_id).expect("sign the patch for real");
    patch.add_signature(patch_sig).expect("attach signature");
    store.write_object(&patch).expect("write patch")
}

/// Build a fresh, standalone repository (never touched by the CLI) holding two generations: a
/// root `Block` with no patches, and a child `Block` carrying one patch whose own `CreateFile`
/// names a non-ASCII path -- the exact shape a newer `prikk` (once it supports Unicode paths)
/// could carry, and that this version's own authoring side refuses to create at all. Both blocks
/// and both `RefState`s are signed by a genuine, but never-adopted-anywhere, maintainer key: the
/// non-ASCII path is refused on its own, during decode, before any publication-trust check would
/// ever run, so this fixture does not need the destination to trust anything about it.
fn build_source_with_outside_version_rule_patch(tag: &str) -> RepositoryLayout {
    let dir = support::unique_repo(&format!("step3f2-{tag}-src"));
    let layout = RepositoryLayout::init(dir.clone()).expect("init layout");
    let mut store = FileObjectStore::new(layout.clone());

    let maintainer_seed = [0x5a_u8; 32];
    let maintainer = Ed25519MaintainerSigner::from_seed("step3f2-maintainer", &maintainer_seed)
        .expect("construct a maintainer signer");

    let root_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: prikk_store::compute_state_root(&[]).unwrap_or(MerkleRoot([0u8; 32])),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let root_bytes = root_payload
        .to_canonical_bytes()
        .expect("encode root block");
    let mut root_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, root_bytes);
    let root_id = root_envelope.object_id();
    root_envelope
        .add_signature(
            maintainer_signature(&maintainer, ObjectType::Block, root_id).expect("sign root block"),
        )
        .expect("attach signature");
    store
        .write_object(&root_envelope)
        .expect("write root block");

    let ref_state1_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: root_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state1_bytes = ref_state1_payload
        .to_canonical_bytes()
        .expect("encode ref state 1");
    let mut ref_state1_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state1_bytes);
    let ref_state1_id = ref_state1_envelope.object_id();
    ref_state1_envelope
        .add_signature(
            maintainer_signature(&maintainer, ObjectType::RefState, ref_state1_id)
                .expect("sign ref state 1"),
        )
        .expect("attach signature");

    let ref_update1_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state1_id,
        new_target_object_id: root_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: "step3f2-maintainer".to_string(),
    };
    let ref_update1_bytes = ref_update1_payload
        .to_canonical_bytes()
        .expect("encode ref update 1");
    let mut ref_update1_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update1_bytes);
    let ref_update1_id = ref_update1_envelope.object_id();
    ref_update1_envelope
        .add_signature(
            maintainer_signature(&maintainer, ObjectType::RefUpdate, ref_update1_id)
                .expect("sign ref update 1"),
        )
        .expect("attach signature");

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: ref_state1_envelope,
            ref_update: ref_update1_envelope,
        })
        .expect("publish heads/main at the root");

    let author = Ed25519AuthorSigner::from_seed("step3f2-author", &[0x5b_u8; 32])
        .expect("construct an author signer");
    let non_ascii_patch =
        write_hand_crafted_create_file_patch(&mut store, &author, "pages/日本語.md", 0x5c);

    let child_payload = BlockPayload {
        parent_block_ids: vec![root_id],
        kind: BlockKind::Normal,
        patch_ids: vec![non_ascii_patch],
        state_merkle_root: prikk_store::compute_state_root(&[]).unwrap_or(MerkleRoot([0u8; 32])),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let child_bytes = child_payload
        .to_canonical_bytes()
        .expect("encode child block");
    let mut child_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, child_bytes);
    let child_id = child_envelope.object_id();
    child_envelope
        .add_signature(
            maintainer_signature(&maintainer, ObjectType::Block, child_id)
                .expect("sign child block"),
        )
        .expect("attach signature");
    store
        .write_object(&child_envelope)
        .expect("write child block");

    let ref_state2_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: child_id,
        update_seq: 2,
        previous_ref_state_id: Some(ref_state1_id),
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state2_bytes = ref_state2_payload
        .to_canonical_bytes()
        .expect("encode ref state 2");
    let mut ref_state2_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state2_bytes);
    let ref_state2_id = ref_state2_envelope.object_id();
    ref_state2_envelope
        .add_signature(
            maintainer_signature(&maintainer, ObjectType::RefState, ref_state2_id)
                .expect("sign ref state 2"),
        )
        .expect("attach signature");

    let ref_update2_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: Some(ref_state1_id),
        new_ref_state_id: ref_state2_id,
        new_target_object_id: child_id,
        update_seq: 2,
        created_at: 0,
        author_key_id: "step3f2-maintainer".to_string(),
    };
    let ref_update2_bytes = ref_update2_payload
        .to_canonical_bytes()
        .expect("encode ref update 2");
    let mut ref_update2_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update2_bytes);
    let ref_update2_id = ref_update2_envelope.object_id();
    ref_update2_envelope
        .add_signature(
            maintainer_signature(&maintainer, ObjectType::RefUpdate, ref_update2_id)
                .expect("sign ref update 2"),
        )
        .expect("attach signature");

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: Some(ref_state1_id),
            ref_state: ref_state2_envelope,
            ref_update: ref_update2_envelope,
        })
        .expect("publish heads/main at the child");

    layout
}

#[test]
fn archive_import_refuses_outside_version_rule_and_writes_nothing() {
    let layout = build_source_with_outside_version_rule_patch("archive");
    let archive_path = std::env::temp_dir().join(format!(
        "prikk-step3f2-archive-outside-version-rule-{}.prepo",
        support::unique_suffix()
    ));
    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export archive");
    let mut file = std::fs::File::create(&archive_path).expect("create archive file");
    file.write_all(&bytes).expect("write archive file");
    let source_dir = layout.root().to_path_buf();

    let dest = support::unique_repo("step3f2-archive-outside-version-rule-dest");
    support::init(&dest);
    let before = support::store_bytes(&dest);

    let json = support::prikk(&dest)
        .args([
            "archive",
            "import",
            archive_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let value = assert_valid_json(&stdout_of(&json));
    assert_eq!(value.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(value.get("command").as_str(), "archive-import");
    assert_eq!(value.get("refusal").as_str(), "outside-version-rule");
    assert!(value.get("reason").is_null());

    assert_eq!(
        support::store_bytes(&dest),
        before,
        "a refused archive import (fresh run) must write nothing: no object, no journal"
    );

    cleanup(&[&source_dir, &dest]);
    let _ = std::fs::remove_file(&archive_path);
}
