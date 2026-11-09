//! 0.57.0 step 3, part S3: `verify --format json` (`verify-report-v1`) gains the item-level facts its text has
//! always printed (stikk 023 §1.2, answered in 029 §2), additively: `block_seals`, `object_items`,
//! `failed_objects`, `unverifiable_author_signatures` and a typed `publication_trust_issues`.
//!
//! stikk's use case is the test: three blocks, two sealed by one key and one by `colleague`; the document says
//! `colleague` sealed 1 of 3, before the key is removed and after.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;
use std::process::Output;

use crate::support;
use crate::support::json::Value;
use prikk_store::{Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout};

const COLLEAGUE: &str = "s3-colleague";
const COLLEAGUE_SEED: [u8; 32] = [0x4c; 32];

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn verify_json(repo: &Path) -> (Option<i32>, Value) {
    let output = support::prikk(repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    (
        output.status.code(),
        support::json::parse(&String::from_utf8_lossy(&output.stdout)),
    )
}

fn log_blocks(repo: &Path) -> Vec<Value> {
    let output = support::prikk(repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&output, "log --format json");
    support::json::parse(&String::from_utf8_lossy(&output.stdout))
        .get("blocks")
        .as_array()
        .to_vec()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .iter()
        .map(|item| item.as_str().to_string())
        .collect()
}

fn number(value: &Value) -> usize {
    match value {
        Value::Number(text) => text.parse().unwrap(),
        other => panic!("not a number: {other:?}"),
    }
}

/// Seal `heads/main` as `colleague` (adopted first): the third block of three.
fn seal_as_colleague(repo: &Path) {
    let colleague = Ed25519MaintainerSigner::from_seed(COLLEAGUE, &COLLEAGUE_SEED).unwrap();
    support::ok(
        &support::prikk(repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                COLLEAGUE,
                "--public-key",
                &support::hex(&colleague.public_key_bytes()),
            ])
            .output()
            .unwrap(),
        "adopt colleague",
    );
    support::ok(
        &support::prikk(repo)
            .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
            .env(
                "PRIKK_AUTHOR_SEED_FILE",
                support::seed_file(support::AUTHOR_SEED_HEX),
            )
            .args(["commit", "--ref", "heads/main", "-m", "colleague's"])
            .output()
            .unwrap(),
        "commit",
    );
    support::ok(
        &support::prikk(repo)
            .env("PRIKK_MAINTAINER_KEY_ID", COLLEAGUE)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&COLLEAGUE_SEED)),
            )
            .args(["seal", "--allow-no-audit", "--ref", "heads/main"])
            .output()
            .unwrap(),
        "seal as colleague",
    );
}

fn three_blocks_one_by_colleague(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "f.txt", b"one\n", "one");
    support::generation(&repo, "heads/main", "f.txt", b"two\n", "two");
    std::fs::write(repo.join("f.txt"), b"three\n").unwrap();
    seal_as_colleague(&repo);
    repo
}

fn sealed_by_count(seals: &[Value], key: &str) -> usize {
    seals
        .iter()
        .filter(|seal| strings(seal.get("sealed_by")).contains(&key.to_string()))
        .count()
}

/// stikk's case: `colleague` sealed 1 of 3 -- before it is removed, and after. `sealed_by` is as recorded, so
/// the count survives the removal; what the removal changes is `trusted_by`, the per-signature outcome, the
/// verdict and the typed trust issue, each beside it.
#[test]
fn colleague_sealed_one_of_three_before_and_after_removal() {
    let repo = three_blocks_one_by_colleague("s3-stikk");

    let (code, before) = verify_json(&repo);
    assert_eq!(code, Some(0), "{before:?}");
    let seals = before.get("block_seals").as_array();
    assert_eq!(seals.len(), 3, "every reached block");
    assert_eq!(sealed_by_count(seals, COLLEAGUE), 1);
    assert_eq!(sealed_by_count(seals, support::MAINTAINER_KEY_ID), 2);
    assert!(
        seals.iter().all(|seal| !seal.get("trusted_by").is_null()),
        "all three verify against an adopted key"
    );
    assert_eq!(before.get("publication_trust_issues").as_array().len(), 0);

    // Remove colleague (the maintainer key stays adopted).
    support::ok(
        &support::prikk(&repo)
            .args(["trust", "maintainer", "remove", "--key-id", COLLEAGUE])
            .output()
            .unwrap(),
        "trust maintainer remove",
    );
    let (code, after) = verify_json(&repo);
    assert_eq!(code, Some(1), "removing a sealer fails its blocks");
    let seals = after.get("block_seals").as_array();
    assert_eq!(seals.len(), 3);
    assert_eq!(
        sealed_by_count(seals, COLLEAGUE),
        1,
        "as recorded: the count does not change when trust does"
    );
    assert_eq!(sealed_by_count(seals, support::MAINTAINER_KEY_ID), 2);

    // What changed is beside `sealed_by`, never inside it.
    let colleagues_block = seals
        .iter()
        .find(|seal| strings(seal.get("sealed_by")).contains(&COLLEAGUE.to_string()))
        .unwrap();
    assert!(
        colleagues_block.get("trusted_by").is_null(),
        "no adopted key's signature verifies it any more"
    );
    let outcomes = colleagues_block.get("sealed_by_signatures").as_array();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].get("key_id").as_str(), COLLEAGUE);
    assert_eq!(outcomes[0].get("outcome").as_str(), "unverifiable");
    let block_id = colleagues_block.get("block_id").as_str();
    let issues = after.get("publication_trust_issues").as_array();
    assert!(
        issues.iter().any(|issue| {
            issue.get("object_type").as_str() == "block"
                && issue.get("object_id").as_str() == block_id
                && issue.get("key_id").as_str() == COLLEAGUE
                && issue.get("outcome").as_str() == "unverifiable"
                && issue.get("code").as_str() == "PRIKK-TRUST-PUBLICATION-UNTRUSTED"
        }),
        "a typed issue names the block, the key and the outcome: {issues:?}"
    );

    // The text's `sealed blocks:` is the entries with a trusted sealer.
    let verified = support::prikk(&repo).arg("verify").output().unwrap();
    let trusted = seals
        .iter()
        .filter(|seal| !seal.get("trusted_by").is_null())
        .count();
    assert!(
        text(&verified).contains(&format!("sealed blocks: {trusted}")),
        "{}",
        text(&verified)
    );
}

/// `log`'s `sealed_by` and `verify`'s are one field from one function: block for block, equal -- before and
/// after a removal.
#[test]
fn verify_and_log_cannot_differ_on_sealed_by() {
    let repo = three_blocks_one_by_colleague("s3-same-field");
    for stage in ["before", "after removal"] {
        let log = log_blocks(&repo);
        let (_, verify) = verify_json(&repo);
        for block in &log {
            let id = block.get("block_id").as_str();
            let seal = verify
                .get("block_seals")
                .as_array()
                .iter()
                .find(|seal| seal.get("block_id").as_str() == id)
                .unwrap_or_else(|| panic!("{stage}: verify lists block {id}"));
            assert_eq!(
                strings(seal.get("sealed_by")),
                strings(block.get("sealed_by")),
                "{stage}: block {id}"
            );
        }
        if stage == "before" {
            support::ok(
                &support::prikk(&repo)
                    .args(["trust", "maintainer", "remove", "--key-id", COLLEAGUE])
                    .output()
                    .unwrap(),
                "remove",
            );
        }
    }
}

/// A byte flipped in a patch container: `object_items` counts it, `failed_objects` types it, and the text's
/// numbers are the same numbers.
#[test]
fn a_damaged_object_is_counted_and_typed() {
    let repo = support::unique_repo("s3-failed");
    support::init(&repo);
    support::generation(&repo, "heads/main", "f.txt", b"one\n", "one");
    let healthy = verify_json(&repo).1;
    assert_eq!(number(healthy.get("object_items").get("failed")), 0);
    assert_eq!(healthy.get("failed_objects").as_array().len(), 0);

    let container = repo
        .join(".prikk")
        .join("containers")
        .join("patch")
        .join("a.container");
    let mut bytes = std::fs::read(&container).unwrap();
    let last = bytes.len() - 8;
    bytes[last] ^= 0xff;
    std::fs::write(&container, bytes).unwrap();

    let (code, document) = verify_json(&repo);
    assert_eq!(code, Some(1));
    let items = document.get("object_items");
    let failed = number(items.get("failed"));
    assert!(failed >= 1, "{document:?}");
    let listed = document.get("failed_objects").as_array();
    assert_eq!(listed.len(), failed);
    let entry = listed
        .iter()
        .find(|entry| entry.get("object_type").as_str() == "patch")
        .expect("the damaged patch");
    assert_eq!(entry.get("class").as_str(), "corrupt");
    assert!(entry.get("locator").as_str().contains("#"));
    assert!(!entry.get("message").as_str().is_empty());
    // The frame is unreadable: it has no id to give.
    assert!(entry.get("object_id").is_null());

    let verified = support::prikk(&repo).arg("verify").output().unwrap();
    assert!(
        text(&verified).contains(&format!(
            "object items: {} scanned, {failed} failed",
            number(items.get("scanned"))
        )),
        "the text says the same numbers: {}",
        text(&verified)
    );
}

/// An author key id with no recorded material is counted, as the text counts it.
#[test]
fn unverifiable_author_signatures_are_counted() {
    let repo = support::unique_repo("s3-author");
    support::init(&repo);
    support::generation(&repo, "heads/main", "f.txt", b"one\n", "one");
    support::generation(&repo, "heads/main", "f.txt", b"two\n", "two");
    assert_eq!(
        number(verify_json(&repo).1.get("unverifiable_author_signatures")),
        0
    );

    // Forget the recorded author key material: every patch's AUTHOR key id now has none.
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    std::fs::write(layout.author_key_container_path(), b"").unwrap();
    let (code, document) = verify_json(&repo);
    let count = number(document.get("unverifiable_author_signatures"));
    assert_eq!(count, 2, "{document:?} (exit {code:?})");
    let verified = support::prikk(&repo).arg("verify").output().unwrap();
    assert!(
        text(&verified).contains("unverifiable author signatures: 2"),
        "{}",
        text(&verified)
    );
}

/// Additive: the schema name and every key the document had are unchanged.
#[test]
fn the_old_keys_and_the_schema_name_stay() {
    let repo = three_blocks_one_by_colleague("s3-additive");
    let (_, document) = verify_json(&repo);
    assert_eq!(document.get("schema_version").as_str(), "verify-report-v1");
    for key in ["verdict", "active_sessions", "stages", "unreached_objects"] {
        document.get(key);
    }
    assert_eq!(document.get("stages").as_array().len(), 17);
}

/// `log`'s and `verify`'s `sealed_by` list the **Block's** signatures; `bundle preview`'s lists the **delivered
/// tip RefState's**. They are different objects and can differ: when another signer's copy of a stored Block
/// arrives, it is merged as one superseding record carrying the union of both signature sets, and the local
/// RefState gains nothing. Built here with that record on the Block only (the same record
/// `repository-layout.md` describes), the tip then reads two sealers in `log` and `verify` and one in a
/// bundle's preview.
#[test]
fn log_and_verify_list_the_blocks_sealers_and_a_previews_list_the_refstates() {
    use prikk_object::{ObjectId, ObjectType};
    use prikk_store::{
        FileObjectStore, ObjectReader, append_superseding_record_for_test_support,
        maintainer_signature,
    };

    let repo = support::unique_repo("s3-block-vs-refstate");
    support::init(&repo);
    support::generation(&repo, "heads/main", "f.txt", b"one\n", "one");
    let tip: ObjectId = log_blocks(&repo)[0]
        .get("block_id")
        .as_str()
        .parse()
        .unwrap();
    let second = Ed25519MaintainerSigner::from_seed(COLLEAGUE, &COLLEAGUE_SEED).unwrap();
    support::ok(
        &support::prikk(&repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                COLLEAGUE,
                "--public-key",
                &support::hex(&second.public_key_bytes()),
            ])
            .output()
            .unwrap(),
        "adopt the second sealer",
    );
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    let mut envelope = FileObjectStore::new(layout.clone())
        .read_object(tip)
        .unwrap()
        .unwrap();
    envelope
        .add_signature(maintainer_signature(&second, ObjectType::Block, tip).unwrap())
        .unwrap();
    append_superseding_record_for_test_support(&layout, &envelope).unwrap();

    let log_sealers = strings(log_blocks(&repo)[0].get("sealed_by"));
    assert_eq!(log_sealers.len(), 2, "{log_sealers:?}");
    let (code, verify) = verify_json(&repo);
    assert_eq!(code, Some(0));
    assert_eq!(
        strings(verify.get("block_seals").as_array()[0].get("sealed_by")),
        log_sealers,
        "verify lists the Block's sealers, as log does"
    );

    let bundle =
        std::env::temp_dir().join(format!("prikk-cli-s3-{}.pbndl", support::unique_suffix()));
    support::ok(
        &support::prikk(&repo)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    let preview = support::prikk(&repo)
        .args(["bundle", "preview", "--format", "json", "--input"])
        .arg(&bundle)
        .output()
        .unwrap();
    support::ok(&preview, "bundle preview");
    let preview_sealers =
        strings(support::json::parse(&String::from_utf8_lossy(&preview.stdout)).get("sealed_by"));
    assert_eq!(
        preview_sealers,
        vec![support::MAINTAINER_KEY_ID.to_string()],
        "a preview lists the delivered RefState's sealers: the Block's second signer is not among them"
    );
}
