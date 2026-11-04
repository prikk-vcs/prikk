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
