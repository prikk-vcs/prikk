//! 0.56.0 step F (stikk 023 part 6): a bundle or archive whose delivered tip's own `Block` and
//! `RefState` both carry a MAINTAINER signature naming an adopted key id, but signed with a
//! different key -- a forgery, or another maintainer using the same key id. Before this round,
//! `bundle import` admitted it with no trust check at all, and `archive import` only reported it
//! (`publication_trusted: false`); neither refused, and only a later `prikk verify` caught it
//! (`PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE`), by which point the forged object was already
//! durably admitted. Both importers now refuse before the first write
//! (`PrikkError::ForgedPublicationSignature`, `refusal-v1`'s own new
//! `"forged-publication-signature"` id).
//!
//! The source repository for every fixture below is built directly through `prikk-store`'s own
//! public API (never through the CLI, which always signs for real with whatever key it is
//! configured with) -- the same low-level construction
//! `crates/prikk-store/src/archive/import/tests.rs`'s own `build_signed_source_repository` and
//! `crates/prikk-store/src/bundle/tests.rs`'s own `seal_two_block_history` already use, reused at
//! this layer since this round's own refusal is driven through the compiled binary, not a
//! store-level `Result`.
//!
//! The "forger" is a second, genuinely different Ed25519 keypair constructed with the exact same
//! `key_id` string `support::MAINTAINER_KEY_ID` names -- so its signature's own `key_id` field
//! needs no separate relabeling step, and the destination's own `support::trust_maintainer` call
//! (which adopts the *real* key under that same id) is exactly the condition that turns the
//! forger's signature into the refused case: a signature naming an adopted key id that does not
//! verify against it.
//!
//! **0.56.0 F2 item 1 addendum**: both importers' own fail-fast gate now checks *every* `Block`
//! and `RefState` the delivery carries, not only the delivered tip -- `build_source_with_two_
//! generations` builds the fixture for that widened scope (a sound tip over a forged ancestor,
//! one generation back), reused by `bundle_import_refuses_a_forged_ancestor_not_only_the_tip` and
//! `archive_import_refuses_a_forged_ancestor_not_only_the_tip`.
//!
//! **0.56.0 F2 item 4 addendum**: `archive_import_with_the_fail_fast_gate_disabled_writes_before_
//! the_backstop_refuses` is the red control the review's own "F2 item 4" asks for -- disables
//! only `archive import`'s own fail-fast gate
//! (`PRIKK_ARCHIVE_IMPORT_DISABLE_FORGED_SIGNATURE_FAIL_FAST_GATE_FOR_TEST`, a `test-support`-
//! gated seam) and confirms `land_one_ref`'s own backstop alone still refuses the whole-chain-
//! forged fixture, but only *after* stage 1 has already written every object and the journal
//! already exists -- proving the byte-identical result in `archive_import_refuses_a_forged_
//! publication_signature_and_writes_nothing` genuinely depends on the fail-fast gate running.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::io::Write as _;
use std::path::Path;
use std::process::Output;

use crate::support;

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope, ObjectType, RefKind,
    RefStatePayload, RefUpdatePayload,
};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, export_archive, export_bundle, maintainer_signature,
};

/// Full hand-written recursive-descent JSON syntax checker -- this crate has no `serde_json`
/// (RFC 118 §10 prerequisite 4); the same copy `step3b_bundle_import_json.rs`,
/// `step3c2_archive_import_json.rs`, and `rfc144_bundle_preview.rs` each already carry,
/// duplicated here rather than factored into `support`, the same convention those already
/// established.
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

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn verify_text(repo: &Path) -> String {
    let output = support::prikk(repo).arg("verify").output().unwrap();
    format!("{}{}", stdout_of(&output), stderr_of(&output))
}

fn cleanup(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Build a fresh, standalone repository (never touched by the CLI) holding one root `Block` and
/// one `RefState` publishing `heads/main` at it, both signed by a MAINTAINER whose own `key_id`
/// string is `support::MAINTAINER_KEY_ID` -- the same id `support::trust_maintainer` adopts at a
/// destination, under the *real* key -- but whose own keypair is `signer_seed`, which may or may
/// not be the real one. Returns the layout (its own directory is the caller's to remove).
fn build_source_signed_by(tag: &str, signer_seed: &[u8; 32], key_id: &str) -> RepositoryLayout {
    let dir = support::unique_repo(&format!("step3f-{tag}-src"));
    let layout = RepositoryLayout::init(dir.clone()).expect("init layout");
    let signer = Ed25519MaintainerSigner::from_seed(key_id.to_string(), signer_seed)
        .expect("construct a maintainer signer");

    let mut store = FileObjectStore::new(layout.clone());

    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: prikk_store::compute_state_root(&[]).unwrap_or(MerkleRoot([0u8; 32])),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_bytes = block_payload.to_canonical_bytes().expect("encode block");
    let mut block_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block_bytes);
    let block_id = block_envelope.object_id();
    block_envelope
        .add_signature(
            maintainer_signature(&signer, ObjectType::Block, block_id)
                .expect("sign the block for real, with whichever key `signer` holds"),
        )
        .expect("attach the maintainer signature");
    store.write_object(&block_envelope).expect("write block");

    let ref_state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_bytes = ref_state_payload
        .to_canonical_bytes()
        .expect("encode ref state");
    let mut ref_state_envelope = ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state_bytes);
    let ref_state_id = ref_state_envelope.object_id();
    ref_state_envelope
        .add_signature(
            maintainer_signature(&signer, ObjectType::RefState, ref_state_id)
                .expect("sign the ref state for real, with whichever key `signer` holds"),
        )
        .expect("attach the maintainer signature");

    let ref_update_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: key_id.to_string(),
    };
    let ref_update_bytes = ref_update_payload
        .to_canonical_bytes()
        .expect("encode ref update");
    let mut ref_update_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update_bytes);
    let ref_update_id = ref_update_envelope.object_id();
    ref_update_envelope
        .add_signature(
            maintainer_signature(&signer, ObjectType::RefUpdate, ref_update_id)
                .expect("sign the ref update for real"),
        )
        .expect("attach the maintainer signature");

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        })
        .expect("publish heads/main");

    layout
}

/// `build_source_signed_by`, exported to a bundle file at `bundle_path`.
fn export_bundle_to(layout: &RepositoryLayout, bundle_path: &Path) {
    let (_report, bytes) = export_bundle(layout, "heads/main").expect("export bundle");
    std::fs::write(bundle_path, bytes).expect("write bundle file");
}

/// `build_source_signed_by`, exported to an archive file at `archive_path`.
fn export_archive_to(layout: &RepositoryLayout, archive_path: &Path) {
    let mut bytes = Vec::new();
    export_archive(layout, &mut bytes).expect("export archive");
    let mut file = std::fs::File::create(archive_path).expect("create archive file");
    file.write_all(&bytes).expect("write archive file");
}

const FORGER_SEED: [u8; 32] = [0x99; 32];
const UNVERIFIABLE_KEY_ID: &str = "step3f-unrelated-maintainer";
const UNVERIFIABLE_SEED: [u8; 32] = [0x77; 32];

/// Build a fresh, standalone repository (never touched by the CLI) holding two generations: block
/// 1 (root), signed by `ancestor_seed` under `key_id`, with its own `RefState`, published for
/// real; then block 2 (its child), signed by `tip_seed` under the identical `key_id`, with a
/// second `RefState` publishing `heads/main` at block 2 -- whose own `previous_ref_state_id`
/// names block 1's own `RefState`, so an export of `heads/main` carries both generations in its
/// closure. 0.56.0 F2 item 1: this is the fixture for "a sound tip over a forged ancestor" --
/// `ancestor_seed` and `tip_seed` may be two genuinely different keypairs signing under the
/// identical `key_id` (the architect's own measured case, review `.git-exclude/reviewed/
/// 0.56.0-step-3-f-review-v1.md`: "B seals block 1 as mx/KB, then block 2 as ma/KA"). Two real,
/// sequential `RefStore::publish` calls (`validate_coherent_publication`'s own check requires
/// `RefState.previous_ref_state_id`, `RefUpdate.old_ref_state_id`, and `RefPublication::expected_
/// previous_ref_state_id` to all agree, so block 1's own generation must be a real CAS-sequenced
/// publish too, not a direct `write_object` the second publish's own preconditions could never
/// match).
fn build_source_with_two_generations(
    tag: &str,
    key_id: &str,
    ancestor_seed: &[u8; 32],
    tip_seed: &[u8; 32],
) -> RepositoryLayout {
    let dir = support::unique_repo(&format!("step3f-{tag}-src"));
    let layout = RepositoryLayout::init(dir.clone()).expect("init layout");
    let mut store = FileObjectStore::new(layout.clone());

    let ancestor_signer = Ed25519MaintainerSigner::from_seed(key_id.to_string(), ancestor_seed)
        .expect("construct a maintainer signer");

    let block1_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: prikk_store::compute_state_root(&[]).unwrap_or(MerkleRoot([0u8; 32])),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block1_bytes = block1_payload.to_canonical_bytes().expect("encode block 1");
    let mut block1_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block1_bytes);
    let block1_id = block1_envelope.object_id();
    block1_envelope
        .add_signature(
            maintainer_signature(&ancestor_signer, ObjectType::Block, block1_id)
                .expect("sign block 1, forged or not"),
        )
        .expect("attach the maintainer signature");
    store.write_object(&block1_envelope).expect("write block 1");

    let ref_state1_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block1_id,
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
            maintainer_signature(&ancestor_signer, ObjectType::RefState, ref_state1_id)
                .expect("sign ref state 1, forged or not"),
        )
        .expect("attach the maintainer signature");

    let ref_update1_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state1_id,
        new_target_object_id: block1_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: key_id.to_string(),
    };
    let ref_update1_bytes = ref_update1_payload
        .to_canonical_bytes()
        .expect("encode ref update 1");
    let mut ref_update1_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update1_bytes);
    let ref_update1_id = ref_update1_envelope.object_id();
    ref_update1_envelope
        .add_signature(
            maintainer_signature(&ancestor_signer, ObjectType::RefUpdate, ref_update1_id)
                .expect("sign the ref update, forged or not"),
        )
        .expect("attach the maintainer signature");

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: ref_state1_envelope,
            ref_update: ref_update1_envelope,
        })
        .expect("publish heads/main at block 1");

    let tip_signer = Ed25519MaintainerSigner::from_seed(key_id.to_string(), tip_seed)
        .expect("construct a maintainer signer");

    let block2_payload = BlockPayload {
        parent_block_ids: vec![block1_id],
        kind: BlockKind::Normal,
        patch_ids: Vec::new(),
        state_merkle_root: prikk_store::compute_state_root(&[]).unwrap_or(MerkleRoot([0u8; 32])),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block2_bytes = block2_payload.to_canonical_bytes().expect("encode block 2");
    let mut block2_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block2_bytes);
    let block2_id = block2_envelope.object_id();
    block2_envelope
        .add_signature(
            maintainer_signature(&tip_signer, ObjectType::Block, block2_id)
                .expect("sign block 2, soundly"),
        )
        .expect("attach the maintainer signature");
    store.write_object(&block2_envelope).expect("write block 2");

    let ref_state2_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block2_id,
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
            maintainer_signature(&tip_signer, ObjectType::RefState, ref_state2_id)
                .expect("sign ref state 2, soundly"),
        )
        .expect("attach the maintainer signature");

    let ref_update2_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: Some(ref_state1_id),
        new_ref_state_id: ref_state2_id,
        new_target_object_id: block2_id,
        update_seq: 2,
        created_at: 0,
        author_key_id: key_id.to_string(),
    };
    let ref_update2_bytes = ref_update2_payload
        .to_canonical_bytes()
        .expect("encode ref update 2");
    let mut ref_update2_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update2_bytes);
    let ref_update2_id = ref_update2_envelope.object_id();
    ref_update2_envelope
        .add_signature(
            maintainer_signature(&tip_signer, ObjectType::RefUpdate, ref_update2_id)
                .expect("sign the ref update, soundly"),
        )
        .expect("attach the maintainer signature");

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: Some(ref_state1_id),
            ref_state: ref_state2_envelope,
            ref_update: ref_update2_envelope,
        })
        .expect("publish heads/main at block 2");

    layout
}

#[test]
fn bundle_import_refuses_a_forged_publication_signature_and_writes_nothing() {
    let layout = build_source_signed_by("bundle-forged", &FORGER_SEED, support::MAINTAINER_KEY_ID);
    let bundle_path = std::env::temp_dir().join(format!(
        "prikk-step3f-bundle-forged-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&layout, &bundle_path);
    let source_dir = layout.root().to_path_buf();

    let dest = support::unique_repo("step3f-bundle-forged-dest");
    support::init(&dest);
    support::trust_maintainer(&dest); // adopts the *real* key under MAINTAINER_KEY_ID
    let before = support::store_bytes(&dest);

    let json = support::prikk(&dest)
        .args([
            "bundle",
            "import",
            "--input",
            bundle_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let value = assert_valid_json(&stdout_of(&json));
    assert_eq!(value.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(value.get("command").as_str(), "bundle-import");
    assert_eq!(
        value.get("refusal").as_str(),
        "forged-publication-signature"
    );
    assert!(value.get("reason").is_null());
    // The message names both possible causes, without implying which: stikk's own measured
    // forgery, or another maintainer who happens to use the same key id.
    assert!(
        value.get("message").as_str().contains("forgery"),
        "{value:?}"
    );

    assert_eq!(
        support::store_bytes(&dest),
        before,
        "a refused bundle import must write nothing"
    );

    // Plain text: same exit code, same cause named on stderr.
    let plain = support::prikk(&dest)
        .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(plain.status.code(), Some(1));
    assert!(stderr_of(&plain).contains("forgery"), "{plain:?}");
    assert_eq!(
        support::store_bytes(&dest),
        before,
        "the plain-text run must also write nothing"
    );

    // The after-state, not only the exit code (the live note's own reminder): `verify` still
    // reports a perfectly healthy, empty repository.
    let verify = verify_text(&dest);
    assert!(verify.contains("unreached objects: 0"), "{verify}");
    assert!(
        !verify.contains("publication trust issues: ")
            || verify.contains("publication trust issues: 0"),
        "{verify}"
    );
    let doctor = support::prikk(&dest).arg("doctor").output().unwrap();
    assert!(doctor.status.success(), "{doctor:?}");
    let status = support::prikk(&dest).arg("status").output().unwrap();
    assert!(
        status.status.success(),
        "the next ordinary command must proceed normally: {status:?}"
    );

    cleanup(&[&source_dir, &dest]);
    let _ = std::fs::remove_file(&bundle_path);
}

#[test]
fn archive_import_refuses_a_forged_publication_signature_and_writes_nothing() {
    let layout = build_source_signed_by("archive-forged", &FORGER_SEED, support::MAINTAINER_KEY_ID);
    let archive_path = std::env::temp_dir().join(format!(
        "prikk-step3f-archive-forged-{}.prepo",
        support::unique_suffix()
    ));
    export_archive_to(&layout, &archive_path);
    let source_dir = layout.root().to_path_buf();

    let dest = support::unique_repo("step3f-archive-forged-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
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
    assert_eq!(
        value.get("refusal").as_str(),
        "forged-publication-signature"
    );
    assert!(value.get("reason").is_null());
    assert!(
        value.get("message").as_str().contains("forgery"),
        "{value:?}"
    );

    assert_eq!(
        support::store_bytes(&dest),
        before,
        "a refused archive import (fresh run, fail-fast gate) must write nothing: no object, no \
         journal"
    );

    let verify = verify_text(&dest);
    assert!(verify.contains("unreached objects: 0"), "{verify}");
    let doctor = support::prikk(&dest).arg("doctor").output().unwrap();
    assert!(doctor.status.success(), "{doctor:?}");
    let status = support::prikk(&dest).arg("status").output().unwrap();
    assert!(
        status.status.success(),
        "the next ordinary command must proceed normally: {status:?}"
    );

    cleanup(&[&source_dir, &dest]);
    let _ = std::fs::remove_file(&archive_path);
}

/// The forger's own seed for block 1, when block 1 and block 2 are signed by two genuinely
/// different keypairs under the identical key id (0.56.0 F2 item 1's own "sound tip over a forged
/// ancestor" fixture) -- distinct from [`FORGER_SEED`] above (that one forges a whole, one-block
/// chain) so a test failure naming one can never be confused with the other.
const ANCESTOR_FORGER_SEED: [u8; 32] = [0xab; 32];

/// 0.56.0 F2 item 1 (the review's own measured gap, `.git-exclude/reviewed/
/// 0.56.0-step-3-f-review-v1.md`): before this round, the gate checked only each landing's own
/// delivered tip. A bundle whose tip is honestly signed but whose own *parent* block carries a
/// forged signature used to import with `bundle import` exiting `0`; only a later `prikk verify`
/// caught it (`PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE` on the forged block and its own
/// `RefState`) -- stikk's own part-6 finding, one block back. `bundle import` now refuses it the
/// same way it already refuses a forged tip, byte-identical.
#[test]
fn bundle_import_refuses_a_forged_ancestor_not_only_the_tip() {
    let layout = build_source_with_two_generations(
        "bundle-ancestor",
        support::MAINTAINER_KEY_ID,
        &ANCESTOR_FORGER_SEED,
        &support::MAINTAINER_SEED,
    );
    let bundle_path = std::env::temp_dir().join(format!(
        "prikk-step3f2-bundle-ancestor-forged-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&layout, &bundle_path);
    let source_dir = layout.root().to_path_buf();

    let dest = support::unique_repo("step3f2-bundle-ancestor-forged-dest");
    support::init(&dest);
    support::trust_maintainer(&dest); // adopts the *real* key under MAINTAINER_KEY_ID
    let before = support::store_bytes(&dest);

    let json = support::prikk(&dest)
        .args([
            "bundle",
            "import",
            "--input",
            bundle_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let value = assert_valid_json(&stdout_of(&json));
    assert_eq!(value.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(value.get("command").as_str(), "bundle-import");
    assert_eq!(
        value.get("refusal").as_str(),
        "forged-publication-signature"
    );
    assert!(value.get("reason").is_null());

    assert_eq!(
        support::store_bytes(&dest),
        before,
        "a sound tip over a forged ancestor must still write nothing"
    );

    let verify = verify_text(&dest);
    assert!(verify.contains("unreached objects: 0"), "{verify}");

    cleanup(&[&source_dir, &dest]);
    let _ = std::fs::remove_file(&bundle_path);
}

/// `archive import`'s own twin of `bundle_import_refuses_a_forged_ancestor_not_only_the_tip`: the
/// widened fail-fast gate (`refuse_forged_publication_signatures_before_admission`, now checking
/// every `Block`/`RefState` the archive carries, not only each planned landing's own tip) refuses
/// before stage 1 ever writes a single object or the journal is ever created -- byte-identical.
#[test]
fn archive_import_refuses_a_forged_ancestor_not_only_the_tip() {
    let layout = build_source_with_two_generations(
        "archive-ancestor",
        support::MAINTAINER_KEY_ID,
        &ANCESTOR_FORGER_SEED,
        &support::MAINTAINER_SEED,
    );
    let archive_path = std::env::temp_dir().join(format!(
        "prikk-step3f2-archive-ancestor-forged-{}.prepo",
        support::unique_suffix()
    ));
    export_archive_to(&layout, &archive_path);
    let source_dir = layout.root().to_path_buf();

    let dest = support::unique_repo("step3f2-archive-ancestor-forged-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
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
    assert_eq!(
        value.get("refusal").as_str(),
        "forged-publication-signature"
    );
    assert!(value.get("reason").is_null());

    assert_eq!(
        support::store_bytes(&dest),
        before,
        "a sound tip over a forged ancestor (fresh run, fail-fast gate) must write nothing: no \
         object, no journal"
    );

    let verify = verify_text(&dest);
    assert!(verify.contains("unreached objects: 0"), "{verify}");

    cleanup(&[&source_dir, &dest]);
    let _ = std::fs::remove_file(&archive_path);
}

/// 0.56.0 F2 item 4 (review `.git-exclude/reviewed/0.56.0-step-3-f-review-v1.md`, "F2 item 4"):
/// the 0.56.0 step F round's own red control removed `bundle import`'s calls only, leaving
/// `archive import`'s own fail-fast gate unexercised by any control. With
/// `PRIKK_ARCHIVE_IMPORT_DISABLE_FORGED_SIGNATURE_FAIL_FAST_GATE_FOR_TEST` set (a
/// `test-support`-gated seam, the same shape `PRIKK_ARCHIVE_IMPORT_DISABLE_EARLY_IN_THE_WAY_
/// CHECK_FOR_TEST` already uses), the whole-chain-forged fixture above
/// (`archive_import_refuses_a_forged_publication_signature_and_writes_nothing`'s own fixture) is
/// still refused -- `land_one_ref`'s own backstop still catches a forged *tip* -- but no longer
/// byte-identical: stage 1 has already admitted every carried object, and the journal already
/// exists, before the backstop ever runs, proving the byte-identical, "no object, no journal"
/// result in the test above genuinely depends on the fail-fast gate running, not on some other,
/// vacuous reason.
#[test]
fn archive_import_with_the_fail_fast_gate_disabled_writes_before_the_backstop_refuses() {
    let layout = build_source_signed_by(
        "archive-forged-red",
        &FORGER_SEED,
        support::MAINTAINER_KEY_ID,
    );
    let archive_path = std::env::temp_dir().join(format!(
        "prikk-step3f2-archive-forged-red-{}.prepo",
        support::unique_suffix()
    ));
    export_archive_to(&layout, &archive_path);
    let source_dir = layout.root().to_path_buf();

    let dest = support::unique_repo("step3f2-archive-forged-red-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
    let before = support::store_bytes(&dest);

    let refused = support::prikk(&dest)
        .env(
            "PRIKK_ARCHIVE_IMPORT_DISABLE_FORGED_SIGNATURE_FAIL_FAST_GATE_FOR_TEST",
            "1",
        )
        .args(["archive", "import", archive_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "the backstop must still refuse the forged tip even with the fail-fast gate disabled: \
         {refused:?}"
    );
    assert!(stderr_of(&refused).contains("forgery"), "{refused:?}");

    assert_ne!(
        support::store_bytes(&dest),
        before,
        "with the fail-fast gate disabled, stage 1 must have already admitted this archive's own \
         objects (and created the journal) before the backstop ever refuses -- proving the \
         byte-identical result above depends on the fail-fast gate, not on the backstop alone"
    );

    cleanup(&[&source_dir, &dest]);
    let _ = std::fs::remove_file(&archive_path);
}

/// Regression control (the live note's own reminder: don't widen the refusal past the one case
/// the ruling names): a sealer whose key id this repository has never adopted at all still
/// imports, exactly as before this round -- for both importers.
#[test]
fn an_unverifiable_sealer_still_imports_for_both_importers() {
    let layout = build_source_signed_by("unverifiable", &UNVERIFIABLE_SEED, UNVERIFIABLE_KEY_ID);
    let bundle_path = std::env::temp_dir().join(format!(
        "prikk-step3f-bundle-unverifiable-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&layout, &bundle_path);
    let archive_path = std::env::temp_dir().join(format!(
        "prikk-step3f-archive-unverifiable-{}.prepo",
        support::unique_suffix()
    ));
    export_archive_to(&layout, &archive_path);
    let source_dir = layout.root().to_path_buf();

    let bundle_dest = support::unique_repo("step3f-bundle-unverifiable-dest");
    support::init(&bundle_dest);
    // Adopts a key, but never `UNVERIFIABLE_KEY_ID` -- that is the point of this control. (An
    // empty trust policy -- no key adopted at all -- reads as a different, pre-existing
    // condition, `PRIKK-TRUST-POLICY-INVALID`, not `PRIKK-TRUST-PUBLICATION-UNTRUSTED`; adopting
    // an unrelated real key is what isolates the one condition this control is about.)
    support::trust_maintainer(&bundle_dest);
    let bundle_out = support::prikk(&bundle_dest)
        .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        bundle_out.status.success(),
        "an unadopted sealer must still import, unchanged: {bundle_out:?}"
    );
    let bundle_verify = verify_text(&bundle_dest);
    assert!(
        bundle_verify.contains("PRIKK-TRUST-PUBLICATION-UNTRUSTED"),
        "{bundle_verify}"
    );
    assert!(
        !bundle_verify.contains("PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE"),
        "{bundle_verify}"
    );

    let archive_dest = support::unique_repo("step3f-archive-unverifiable-dest");
    support::init(&archive_dest);
    support::trust_maintainer(&archive_dest);
    let archive_out = support::prikk(&archive_dest)
        .args(["archive", "import", archive_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        archive_out.status.success(),
        "an unadopted sealer must still import, unchanged: {archive_out:?}"
    );
    let archive_verify = verify_text(&archive_dest);
    assert!(
        archive_verify.contains("PRIKK-TRUST-PUBLICATION-UNTRUSTED"),
        "{archive_verify}"
    );

    cleanup(&[&source_dir, &bundle_dest, &archive_dest]);
    let _ = std::fs::remove_file(&bundle_path);
    let _ = std::fs::remove_file(&archive_path);
}

#[test]
fn bundle_preview_reports_invalid_and_unverifiable_sealers_by_outcome() {
    let forged_layout =
        build_source_signed_by("preview-forged", &FORGER_SEED, support::MAINTAINER_KEY_ID);
    let forged_bundle = std::env::temp_dir().join(format!(
        "prikk-step3f-preview-forged-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&forged_layout, &forged_bundle);
    let forged_source_dir = forged_layout.root().to_path_buf();

    let dest = support::unique_repo("step3f-preview-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);

    let preview = support::prikk(&dest)
        .args([
            "bundle",
            "preview",
            "--input",
            forged_bundle.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(preview.status.success(), "{preview:?}");
    let value = assert_valid_json(&stdout_of(&preview));
    let signatures = value.get("sealed_by_signatures").as_array();
    assert_eq!(signatures.len(), 1, "{value:?}");
    assert_eq!(
        signatures[0].get("key_id").as_str(),
        support::MAINTAINER_KEY_ID
    );
    assert_eq!(signatures[0].get("outcome").as_str(), "invalid");
    // `sealed_by` itself stays exactly as it already was -- the forged signature's own key id is
    // still named there, reported, not trusted.
    let sealed_by = value.get("sealed_by").as_array();
    assert_eq!(sealed_by.len(), 1);
    assert_eq!(sealed_by[0].as_str(), support::MAINTAINER_KEY_ID);

    let unverifiable_layout = build_source_signed_by(
        "preview-unverifiable",
        &UNVERIFIABLE_SEED,
        UNVERIFIABLE_KEY_ID,
    );
    let unverifiable_bundle = std::env::temp_dir().join(format!(
        "prikk-step3f-preview-unverifiable-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&unverifiable_layout, &unverifiable_bundle);
    let unverifiable_source_dir = unverifiable_layout.root().to_path_buf();

    let preview2 = support::prikk(&dest)
        .args([
            "bundle",
            "preview",
            "--input",
            unverifiable_bundle.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(preview2.status.success(), "{preview2:?}");
    let value2 = assert_valid_json(&stdout_of(&preview2));
    let signatures2 = value2.get("sealed_by_signatures").as_array();
    assert_eq!(signatures2.len(), 1, "{value2:?}");
    assert_eq!(signatures2[0].get("key_id").as_str(), UNVERIFIABLE_KEY_ID);
    assert_eq!(signatures2[0].get("outcome").as_str(), "unverifiable");

    cleanup(&[&forged_source_dir, &unverifiable_source_dir, &dest]);
    let _ = std::fs::remove_file(&forged_bundle);
    let _ = std::fs::remove_file(&unverifiable_bundle);
}

/// In-process `bundle preview` of a bundle file against `dest`, returning the store's report.
fn preview_in_process(dest: &Path, bundle: &Path) -> prikk_store::BundlePreviewReport {
    let layout = RepositoryLayout::open(dest.to_path_buf()).unwrap();
    let bytes = std::fs::read(bundle).unwrap();
    let mut source = prikk_store::BundleSource::Buffered(std::io::Cursor::new(bytes));
    prikk_store::preview_bundle(
        &layout,
        &mut source,
        &prikk_store::BundleImportOptions::default_limits(),
        "heads/main",
    )
    .unwrap()
}

/// **0.57.0 step 1b: `bundle preview` answers what `bundle import` will do about signatures.**
/// Before, a sound tip over a forged ancestor previewed as `sound` (`sealed_by_signatures` names the
/// delivered tip's sealers only) while `bundle import` refused it (0.56.0 P2, external 046/049). The
/// store's report now carries `import_refusal`, found by the importer's own function
/// (`find_forged_publication_signature`) over every Block and RefState:
/// - a forged tip, and a sound tip over a forged ancestor: `Some(forged-publication-signature)`,
///   naming the object, and the sentence is the one the real import prints;
/// - a sound delivery and a delivery by a sealer the destination never adopted: `None`, and the
///   import accepts both.
///
/// **Perturb (red control):** make `preview_bundle_reporting_anchor` report `import_refusal: None`
/// -- the two refusing cases go red. (The CLI's text and JSON output for the field is held for the
/// architect's ruling on its shape; this is the store's report.)
#[test]
fn preview_reports_the_refusal_the_import_would_raise_for_every_block_not_only_the_tip() {
    let dest = support::unique_repo("step1b-preview-dest");
    support::init(&dest);
    support::trust_maintainer(&dest); // adopts the real key under MAINTAINER_KEY_ID

    // A sound tip over a forged ancestor.
    let ancestor = build_source_with_two_generations(
        "step1b-ancestor",
        support::MAINTAINER_KEY_ID,
        &ANCESTOR_FORGER_SEED,
        &support::MAINTAINER_SEED,
    );
    let ancestor_bundle = std::env::temp_dir().join(format!(
        "prikk-step1b-ancestor-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&ancestor, &ancestor_bundle);
    let report = preview_in_process(&dest, &ancestor_bundle);
    assert!(
        report
            .sealed_by_signatures
            .iter()
            .all(|signature| signature.outcome == prikk_store::PublicationSignatureOutcome::Sound),
        "the gap this closes: the tip's sealers read sound ({:?})",
        report.sealed_by_signatures
    );
    let refusal = report
        .import_refusal
        .as_ref()
        .expect("the import would refuse the forged ancestor");
    assert_eq!(refusal.id, "forged-publication-signature");
    assert!(matches!(
        refusal.object_type,
        ObjectType::Block | ObjectType::RefState
    ));
    let import = support::prikk(&dest)
        .args([
            "bundle",
            "import",
            "--input",
            ancestor_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(import.status.code(), Some(1), "{import:?}");
    assert!(
        stderr_of(&import).contains(&refusal.message),
        "the preview's sentence is the import's: {:?} vs {}",
        refusal.message,
        stderr_of(&import)
    );

    // A forged tip.
    let forged = build_source_signed_by("step1b-forged", &FORGER_SEED, support::MAINTAINER_KEY_ID);
    let forged_bundle = std::env::temp_dir().join(format!(
        "prikk-step1b-forged-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&forged, &forged_bundle);
    let report = preview_in_process(&dest, &forged_bundle);
    assert_eq!(
        report.import_refusal.as_ref().map(|r| r.id),
        Some("forged-publication-signature")
    );

    // A sound delivery, and a sealer this repository never adopted: the import accepts both, so the
    // preview reports no refusal.
    let sound = build_source_signed_by(
        "step1b-sound",
        &support::MAINTAINER_SEED,
        support::MAINTAINER_KEY_ID,
    );
    let sound_bundle = std::env::temp_dir().join(format!(
        "prikk-step1b-sound-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&sound, &sound_bundle);
    assert_eq!(
        preview_in_process(&dest, &sound_bundle).import_refusal,
        None
    );
    let unadopted =
        build_source_signed_by("step1b-unadopted", &UNVERIFIABLE_SEED, UNVERIFIABLE_KEY_ID);
    let unadopted_bundle = std::env::temp_dir().join(format!(
        "prikk-step1b-unadopted-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&unadopted, &unadopted_bundle);
    assert_eq!(
        preview_in_process(&dest, &unadopted_bundle).import_refusal,
        None
    );
    let import = support::prikk(&dest)
        .args([
            "bundle",
            "import",
            "--input",
            unadopted_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(import.status.success(), "{import:?}");

    cleanup(&[
        ancestor.root(),
        forged.root(),
        sound.root(),
        unadopted.root(),
        &dest,
    ]);
    for path in [
        ancestor_bundle,
        forged_bundle,
        sound_bundle,
        unadopted_bundle,
    ] {
        let _ = std::fs::remove_file(path);
    }
}
