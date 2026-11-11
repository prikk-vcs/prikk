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
    build_two_generations_with_tip_root(
        tag,
        key_id,
        ancestor_seed,
        tip_seed,
        prikk_store::compute_state_root(&[]).unwrap_or(MerkleRoot([0u8; 32])),
    )
}

/// [`build_source_with_two_generations`], with block 2's state root chosen by the caller: a different
/// root makes block 2 a different object from the honest chain's block 2 over the same block 1, so a
/// delivery of it is a `diverged` one (0.57.0 step 1b2).
fn build_two_generations_with_tip_root(
    tag: &str,
    key_id: &str,
    ancestor_seed: &[u8; 32],
    tip_seed: &[u8; 32],
    tip_root: MerkleRoot,
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
        state_merkle_root: tip_root,
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

fn temp_bundle(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "prikk-step1b2-{tag}-{}.pbndl",
        support::unique_suffix()
    ))
}

/// `prikk bundle preview --format json` of `bundle` in `dest`, as the document it prints.
fn preview_json(dest: &Path, bundle: &Path) -> serde_json_like::Value {
    let output = support::prikk(dest)
        .args([
            "bundle",
            "preview",
            "--input",
            bundle.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_valid_json(&stdout_of(&output))
}

fn import_attempt(dest: &Path, bundle: &Path, allow_non_fast_forward: bool) -> Output {
    let mut command = support::prikk(dest);
    command.args(["bundle", "import", "--input", bundle.to_str().unwrap()]);
    if allow_non_fast_forward {
        command.arg("--allow-non-fast-forward");
    }
    command.output().unwrap()
}

/// **0.57.0 step 1b2: `bundle preview` answers what `bundle import` will do, whole.** The preview's
/// `import_refusals` lists every refusal the import raises without flags, in the import's own order
/// -- the delivery-class refusal first (`rollback`, `diverged`, `does-not-connect`: lifted by
/// `--allow-non-fast-forward`), then the publication-signature refusal (lifted by nothing) -- each
/// from the import's own function. One refusal can hide another: a user who passes the flag to get past
/// the class refusal meets the signature one, which a single-valued field would not have told them.
/// Four deliveries, through the binary:
/// 1. an honest one: `[]`, and the text form prints no `would refuse` line;
/// 2. a sound tip over a forged ancestor (the tip's `sealed_by_signatures` read `sound`): one entry,
///    `forged-publication-signature`, naming a Block or RefState, the import's own sentence;
/// 3. a rollback alone: one entry, `rollback`, with `null` object fields;
/// 4. a diverged delivery whose new tip is forged: two entries in that order; the real import refuses
///    with the first's sentence, and with `--allow-non-fast-forward` refuses with the **second's**.
///
/// **Perturb (red control, the order):** push the signature entry before the class entry in
/// `preview_bundle_reporting_anchor` -- case 4's order assertion goes red; drop the signature entry
/// -- cases 2 and 4 go red; drop the class entry -- cases 3 and 4.
#[test]
fn preview_lists_every_refusal_the_import_would_raise_in_the_imports_order() {
    // The honest two-generation chain, signed throughout by the real key.
    let honest = build_source_with_two_generations(
        "step1b2-honest",
        support::MAINTAINER_KEY_ID,
        &support::MAINTAINER_SEED,
        &support::MAINTAINER_SEED,
    );
    let honest_bundle = temp_bundle("honest");
    export_bundle_to(&honest, &honest_bundle);

    // 1. an honest delivery to a repository that holds nothing of it.
    let fresh = support::unique_repo("step1b2-fresh");
    support::init(&fresh);
    support::trust_maintainer(&fresh);
    let value = preview_json(&fresh, &honest_bundle);
    assert!(
        value.get("import_refusals").as_array().is_empty(),
        "{value:?}"
    );
    let text = support::prikk(&fresh)
        .args([
            "bundle",
            "preview",
            "--input",
            honest_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !stdout_of(&text).contains("would refuse"),
        "nothing is printed when the import would admit it: {}",
        stdout_of(&text)
    );

    // 2. a forged ancestor under a sound tip.
    let ancestor = build_source_with_two_generations(
        "step1b2-ancestor",
        support::MAINTAINER_KEY_ID,
        &ANCESTOR_FORGER_SEED,
        &support::MAINTAINER_SEED,
    );
    let ancestor_bundle = temp_bundle("ancestor");
    export_bundle_to(&ancestor, &ancestor_bundle);
    let value = preview_json(&fresh, &ancestor_bundle);
    assert!(
        value
            .get("sealed_by_signatures")
            .as_array()
            .iter()
            .all(|signature| signature.get("outcome").as_str() == "sound"),
        "the gap this closes: the tip's sealers read sound: {value:?}"
    );
    let refusals = value.get("import_refusals").as_array();
    assert_eq!(refusals.len(), 1, "{value:?}");
    assert_eq!(
        refusals[0].get("id").as_str(),
        "forged-publication-signature"
    );
    assert!(!refusals[0].get("object_type").is_null());
    assert!(!refusals[0].get("object_id").is_null());
    let text = support::prikk(&fresh)
        .args([
            "bundle",
            "preview",
            "--input",
            ancestor_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        stdout_of(&text)
            .contains("bundle import of this delivery would refuse: forged-publication-signature"),
        "{}",
        stdout_of(&text)
    );
    let import = import_attempt(&fresh, &ancestor_bundle, false);
    assert_eq!(import.status.code(), Some(1), "{import:?}");
    assert!(
        stderr_of(&import).contains(refusals[0].get("message").as_str()),
        "the entry's sentence is the import's"
    );

    // The destination for cases 3 and 4 holds the honest chain's received pointer (at block 2).
    let dest = support::unique_repo("step1b2-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
    let first = import_attempt(&dest, &honest_bundle, false);
    assert!(first.status.success(), "{first:?}");

    // 3. a rollback alone: block 1 only, signed honestly -- an ancestor of the pointer.
    let rollback = build_source_signed_by(
        "step1b2-rollback",
        &support::MAINTAINER_SEED,
        support::MAINTAINER_KEY_ID,
    );
    let rollback_bundle = temp_bundle("rollback");
    export_bundle_to(&rollback, &rollback_bundle);
    let value = preview_json(&dest, &rollback_bundle);
    let refusals = value.get("import_refusals").as_array();
    assert_eq!(refusals.len(), 1, "{value:?}");
    assert_eq!(refusals[0].get("id").as_str(), "rollback");
    assert!(refusals[0].get("object_type").is_null());
    assert!(refusals[0].get("object_id").is_null());
    assert!(
        refusals[0]
            .get("message")
            .as_str()
            .contains("--allow-non-fast-forward"),
        "the class sentence already names the flag that lifts it"
    );

    // 4. a diverged delivery whose new tip carries a forged signature: the honest chain's block 1,
    //    then a *different* block 2, signed by the forger under the adopted key id. Two entries, class
    //    first. (A rollback cannot carry a forged object this destination does not already hold; one
    //    that re-sends a held object with a forged extra signature is refused earlier still, by the
    //    import's own signature admission -- see the report.)
    let forged_diverged = build_two_generations_with_tip_root(
        "step1b2-forged-diverged",
        support::MAINTAINER_KEY_ID,
        &support::MAINTAINER_SEED,
        &FORGER_SEED,
        MerkleRoot([0x42; 32]),
    );
    let forged_diverged_bundle = temp_bundle("forged-diverged");
    export_bundle_to(&forged_diverged, &forged_diverged_bundle);
    let value = preview_json(&dest, &forged_diverged_bundle);
    let refusals = value.get("import_refusals").as_array();
    assert_eq!(refusals.len(), 2, "{value:?}");
    assert_eq!(
        refusals[0].get("id").as_str(),
        "diverged",
        "the import's order"
    );
    assert_eq!(
        refusals[1].get("id").as_str(),
        "forged-publication-signature"
    );
    let text = support::prikk(&dest)
        .args([
            "bundle",
            "preview",
            "--input",
            forged_diverged_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let text = stdout_of(&text);
    let class_at = text.find("would refuse: diverged").expect("class line");
    let signature_at = text
        .find("would refuse: forged-publication-signature")
        .expect("signature line");
    assert!(class_at < signature_at, "{text}");
    // The real import: the class refusal first; past it with the flag, the signature refusal.
    let without_flag = import_attempt(&dest, &forged_diverged_bundle, false);
    assert_eq!(without_flag.status.code(), Some(1), "{without_flag:?}");
    assert!(
        stderr_of(&without_flag).contains(refusals[0].get("message").as_str()),
        "{}",
        stderr_of(&without_flag)
    );
    let with_flag = import_attempt(&dest, &forged_diverged_bundle, true);
    assert_eq!(with_flag.status.code(), Some(1), "{with_flag:?}");
    assert!(
        stderr_of(&with_flag).contains(refusals[1].get("message").as_str()),
        "past the class refusal the user meets the second entry's sentence: {}",
        stderr_of(&with_flag)
    );

    cleanup(&[
        honest.root(),
        ancestor.root(),
        rollback.root(),
        forged_diverged.root(),
        &fresh,
        &dest,
    ]);
    for path in [
        honest_bundle,
        ancestor_bundle,
        rollback_bundle,
        forged_diverged_bundle,
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// **0.57.0 step 1b3: one condition, one id -- a forged signature on an object this repository already
/// holds.** A rollback delivery is made only of objects the destination holds, so a forged extra
/// signature on one of them is met by the import's pass-1 signature admission, *before* the class
/// check. Before this part that refusal was `invalid signature: … refusing the whole import` (no route,
/// and `--format json` printed nothing); the preview did not run that step and read `[rollback, …]`.
/// Now, through the binary:
/// - the preview lists `[forged-publication-signature, rollback]`, the import's order, and at most one
///   signature entry;
/// - the real import, **with and without** `--allow-non-fast-forward`, refuses with the first entry's
///   sentence (which carries the route), and `--format json` prints its `refusal-v1` document with the
///   id the new-object case already had;
/// - the destination is unchanged by the refused imports.
///
/// **Perturb (red controls):** make the preview skip the held-object admission -- the first entry
/// disappears and the order assertion goes red; move the held-object entry after the class entry -- the
/// order assertion goes red; make admission raise `InvalidSignature` again -- the sentence, route and JSON
/// assertions go red.
#[test]
fn a_forged_signature_on_a_held_object_is_one_refusal_in_the_imports_order() {
    // The destination holds the honest two-generation chain (received pointer at block 2).
    let honest = build_source_with_two_generations(
        "step1b3-honest",
        support::MAINTAINER_KEY_ID,
        &support::MAINTAINER_SEED,
        &support::MAINTAINER_SEED,
    );
    let honest_bundle = temp_bundle("step1b3-honest");
    export_bundle_to(&honest, &honest_bundle);
    let dest = support::unique_repo("step1b3-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
    assert!(
        import_attempt(&dest, &honest_bundle, false)
            .status
            .success()
    );

    // The delivery: block 1 again (held), with a forged signature under the adopted key id -- a rollback.
    let forged = build_source_signed_by("step1b3-forged", &FORGER_SEED, support::MAINTAINER_KEY_ID);
    let forged_bundle = temp_bundle("step1b3-forged");
    export_bundle_to(&forged, &forged_bundle);

    let value = preview_json(&dest, &forged_bundle);
    let refusals = value.get("import_refusals").as_array();
    assert_eq!(refusals.len(), 2, "{value:?}");
    assert_eq!(
        refusals[0].get("id").as_str(),
        "forged-publication-signature",
        "the held-object admission is met before the class check"
    );
    assert!(!refusals[0].get("object_id").is_null());
    assert_eq!(refusals[1].get("id").as_str(), "rollback");
    let text = support::prikk(&dest)
        .args([
            "bundle",
            "preview",
            "--input",
            forged_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let text = stdout_of(&text);
    let signature_at = text
        .find("would refuse: forged-publication-signature")
        .expect("signature line");
    let class_at = text.find("would refuse: rollback").expect("class line");
    assert!(signature_at < class_at, "{text}");

    let before = support::store_bytes(&dest);
    for allow in [false, true] {
        let import = import_attempt(&dest, &forged_bundle, allow);
        assert_eq!(import.status.code(), Some(1), "allow={allow}: {import:?}");
        let stderr = stderr_of(&import);
        assert!(
            stderr.contains(refusals[0].get("message").as_str()),
            "allow={allow}: the import refuses with the first entry's sentence: {stderr}"
        );
        assert!(
            stderr.contains("prikk trust maintainer add"),
            "allow={allow}: the sentence carries the route: {stderr}"
        );
        assert!(
            !stderr.contains("refusing the whole import"),
            "allow={allow}: the old routeless sentence is gone: {stderr}"
        );
    }
    let json = support::prikk(&dest)
        .args([
            "bundle",
            "import",
            "--input",
            forged_bundle.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let document = assert_valid_json(&stdout_of(&json));
    assert_eq!(document.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(document.get("command").as_str(), "bundle-import");
    assert_eq!(
        document.get("refusal").as_str(),
        "forged-publication-signature",
        "was an empty stdout before this part"
    );
    assert_eq!(
        support::store_bytes(&dest),
        before,
        "a refused import writes nothing"
    );

    cleanup(&[honest.root(), forged.root(), &dest]);
    let _ = std::fs::remove_file(honest_bundle);
    let _ = std::fs::remove_file(forged_bundle);
}

/// 0.57.0 S1b: `bundle import --adopt` refuses a forged MAINTAINER signature exactly as `bundle import` does, with the same
/// `refusal-v1` id, and writes nothing -- adoption is decided after, and only over, a delivery that passed the forgery check.
#[test]
fn bundle_import_adopt_refuses_a_forged_publication_signature_exactly_as_without_adopt() {
    let layout = build_source_signed_by(
        "bundle-forged-adopt",
        &FORGER_SEED,
        support::MAINTAINER_KEY_ID,
    );
    let bundle_path = std::env::temp_dir().join(format!(
        "prikk-s1b-bundle-forged-adopt-{}.pbndl",
        support::unique_suffix()
    ));
    export_bundle_to(&layout, &bundle_path);
    let source_dir = layout.root().to_path_buf();

    let dest = support::unique_repo("s1b-bundle-forged-adopt-dest");
    support::init(&dest);
    support::trust_maintainer(&dest); // adopts the *real* key under MAINTAINER_KEY_ID
    let before = support::store_bytes(&dest);

    let json = support::prikk(&dest)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "bundle",
            "import",
            "--input",
            bundle_path.to_str().unwrap(),
            "--adopt",
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
    assert_eq!(
        support::store_bytes(&dest),
        before,
        "a refused adopting import must write nothing"
    );
    cleanup(&[&source_dir, &dest]);
    let _ = std::fs::remove_file(&bundle_path);
}
