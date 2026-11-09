//! 0.57.0 step 3, part S2: `log --format json` gains `parent_block_ids` (stikk 029 §4.3) and
//! `sealed_by` (inklkunn 2026-10-31 §1), additively within `log-report-v1`.
//!
//! `parent_block_ids` is the Block's stored list as stored: sorted by object id, so the order carries no
//! "first parent". `sealed_by` is the MAINTAINER key ids the Block's seal records, **as recorded, never as
//! trusted**: the same function `bundle preview`'s `sealed_by` is built from, so it reads the same before
//! and after `trust maintainer remove`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;

use crate::support;
use crate::support::json::Value;
use prikk_object::{ObjectId, ObjectType};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader, RepositoryLayout,
    append_superseding_record_for_test_support, maintainer_signature,
};

const SECOND_SEALER_KEY_ID: &str = "s2-second-sealer";
const SECOND_SEALER_SEED: [u8; 32] = [0x5a; 32];

fn log_stdout(repo: &Path, extra: &[&str]) -> String {
    let mut args = vec!["log", "--format", "json"];
    args.extend_from_slice(extra);
    let output = support::prikk(repo).args(args).output().unwrap();
    support::ok(&output, "log --format json");
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn blocks_of(json_text: &str) -> Vec<Value> {
    support::json::parse(json_text)
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

fn two_generations(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "f.txt", b"one\n", "one");
    support::generation(&repo, "heads/main", "f.txt", b"two\n", "two");
    repo
}

/// Roots have `[]`; a child names its parent; `parent_count` is kept and always equals the list's
/// length; `sealed_by` is the one adopted sealer; and every field the schema had is still there.
#[test]
fn parents_as_stored_and_the_sealer_as_recorded_and_the_old_fields_stay() {
    let repo = two_generations("s2-basic");
    let blocks = blocks_of(&log_stdout(&repo, &[]));
    assert_eq!(blocks.len(), 2);
    let (newest, oldest) = (&blocks[0], &blocks[1]);

    assert_eq!(
        strings(oldest.get("parent_block_ids")),
        Vec::<String>::new()
    );
    assert_eq!(
        strings(newest.get("parent_block_ids")),
        vec![oldest.get("block_id").as_str().to_string()]
    );
    for block in &blocks {
        let parents = block.get("parent_block_ids").as_array().len();
        assert_eq!(
            format!("{parents}"),
            block_field_text(block, "parent_count"),
            "parent_count is kept and agrees with the list"
        );
        assert_eq!(
            strings(block.get("sealed_by")),
            vec![support::MAINTAINER_KEY_ID.to_string()],
            "the key id the seal recorded"
        );
    }

    // Additive: the schema name is unchanged and every field `log-report-v1` had is still present.
    let document = support::json::parse(&log_stdout(&repo, &[]));
    assert_eq!(document.get("schema_version").as_str(), "log-report-v1");
    for field in [
        "block_id",
        "ref_state_id",
        "update_seq",
        "kind",
        "rollback_block",
        "parent_count",
        "patch_count",
        "rollback_patch_count",
        "required_attestation_count",
        "patch_messages",
        "previous_ref_state_id",
    ] {
        newest.get(field);
    }
}

fn block_field_text(block: &Value, field: &str) -> String {
    match block.get(field) {
        Value::Number(text) => text.clone(),
        other => panic!("{field} is not a number: {other:?}"),
    }
}

/// A merge block lists both parents, sorted by object id (the order means nothing), with
/// `parent_count` 2.
#[test]
fn a_merge_blocks_two_parents_are_listed_sorted() {
    let repo = support::unique_repo("s2-merge");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"hello", "genesis");
    let root = blocks_of(&log_stdout(&repo, &[]))[0]
        .get("block_id")
        .as_str()
        .to_string();
    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch topic",
    );
    support::generation(&repo, "heads/topic", "topic.txt", b"from topic", "topic");
    let topic = blocks_of(&log_stdout(&repo, &["--ref", "heads/topic"]))[0]
        .get("block_id")
        .as_str()
        .to_string();
    support::ok(
        &support::prikk(&repo)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "merge",
                "--allow-no-audit",
                "--baseline-block",
                &root,
                "--into",
                "heads/main",
                "--from",
                "heads/topic",
            ])
            .output()
            .unwrap(),
        "merge",
    );
    let merge = &blocks_of(&log_stdout(&repo, &[]))[0];
    let mut expected = vec![root, topic];
    expected.sort();
    assert_eq!(strings(merge.get("parent_block_ids")), expected);
    assert_eq!(block_field_text(merge, "parent_count"), "2");
}

/// Inklkunn's case: a block with two sealers lists both, and the document is byte-identical before and
/// after `trust maintainer remove` of one sealer -- and of the key the seal was made with. The control:
/// `verify` does change with trust, so the identity is not because nothing changed.
#[test]
fn sealed_by_is_recorded_not_trusted_and_reads_the_same_after_trust_maintainer_remove() {
    let repo = two_generations("s2-remove");
    let tip: ObjectId = blocks_of(&log_stdout(&repo, &[]))[0]
        .get("block_id")
        .as_str()
        .parse()
        .unwrap();
    let second =
        Ed25519MaintainerSigner::from_seed(SECOND_SEALER_KEY_ID, &SECOND_SEALER_SEED).unwrap();
    support::ok(
        &support::prikk(&repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                SECOND_SEALER_KEY_ID,
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

    let before = log_stdout(&repo, &[]);
    let sealers = strings(blocks_of(&before)[0].get("sealed_by"));
    assert_eq!(sealers.len(), 2, "both sealers are listed: {sealers:?}");
    assert!(sealers.contains(&support::MAINTAINER_KEY_ID.to_string()));
    assert!(sealers.contains(&SECOND_SEALER_KEY_ID.to_string()));
    let verify_before = support::verify(&repo).status.code();
    assert_eq!(verify_before, Some(0));

    // Remove the second sealer: the list is unchanged.
    support::ok(
        &support::prikk(&repo)
            .args([
                "trust",
                "maintainer",
                "remove",
                "--key-id",
                SECOND_SEALER_KEY_ID,
            ])
            .output()
            .unwrap(),
        "trust maintainer remove (second sealer)",
    );
    assert_eq!(log_stdout(&repo, &[]), before);

    // Remove the key the seal was made with, too (a bystander key is adopted first: the last trusted
    // key cannot be removed): `verify` now changes, `log` does not.
    let bystander = Ed25519MaintainerSigner::from_seed("s2-bystander", &[0x33; 32]).unwrap();
    support::ok(
        &support::prikk(&repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                "s2-bystander",
                "--public-key",
                &support::hex(&bystander.public_key_bytes()),
            ])
            .output()
            .unwrap(),
        "adopt a bystander key",
    );
    support::ok(
        &support::prikk(&repo)
            .args([
                "trust",
                "maintainer",
                "remove",
                "--key-id",
                support::MAINTAINER_KEY_ID,
            ])
            .output()
            .unwrap(),
        "trust maintainer remove (the sealing key)",
    );
    assert_ne!(
        support::verify(&repo).status.code(),
        Some(0),
        "control: removing the sealing key changes what verify says"
    );
    assert_eq!(
        log_stdout(&repo, &[]),
        before,
        "sealed_by is as recorded, not as trusted"
    );
}

/// A rollback block is sealed like any other: it lists its sealer.
#[test]
fn a_rollback_block_lists_its_sealer() {
    let repo = two_generations("s2-rollback");
    support::ok(
        &support::prikk(&repo)
            .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
            .env(
                "PRIKK_AUTHOR_SEED_FILE",
                support::seed_file(support::AUTHOR_SEED_HEX),
            )
            .args(["rollback-draft", "--append-inverse", "-m", "undo"])
            .output()
            .unwrap(),
        "rollback-draft --append-inverse",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal the rollback");
    let blocks = blocks_of(&log_stdout(&repo, &[]));
    let newest = &blocks[0];
    assert!(
        newest.get("rollback_block").as_bool(),
        "the newest block is a rollback block"
    );
    assert_eq!(
        strings(newest.get("sealed_by")),
        vec![support::MAINTAINER_KEY_ID.to_string()]
    );
    assert_eq!(newest.get("parent_block_ids").as_array().len(), 1);
}

/// A received ref lists the sealer its blocks record even though this repository never adopted that
/// key (recorded, not trusted), and `bundle preview`'s `sealed_by` for the same tip is the same list --
/// one function behind both.
#[test]
fn a_received_blocks_sealer_is_listed_untrusted_and_matches_bundle_previews() {
    let source = two_generations("s2-received-source");
    let bundle =
        std::env::temp_dir().join(format!("prikk-cli-s2-{}.pbndl", support::unique_suffix()));
    support::ok(
        &support::prikk(&source)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    let source_tip_sealers = strings(blocks_of(&log_stdout(&source, &[]))[0].get("sealed_by"));

    let dest = support::unique_repo("s2-received-dest");
    support::init(&dest);
    // Nothing is adopted in `dest`: the sealing key is unknown there.
    let preview = support::prikk(&dest)
        .args(["bundle", "preview", "--format", "json", "--input"])
        .arg(&bundle)
        .output()
        .unwrap();
    support::ok(&preview, "bundle preview");
    let preview_sealed_by =
        strings(support::json::parse(&String::from_utf8_lossy(&preview.stdout)).get("sealed_by"));
    assert_eq!(preview_sealed_by, source_tip_sealers);

    support::ok(
        &support::prikk(&dest)
            .args(["bundle", "import", "--input"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle import",
    );
    let received = blocks_of(&log_stdout(&dest, &["--ref", "remotes/heads/main"]));
    assert_eq!(received.len(), 2);
    for block in &received {
        assert_eq!(
            strings(block.get("sealed_by")),
            vec![support::MAINTAINER_KEY_ID.to_string()],
            "an untrusted sealer is still listed: sealed_by is not a trust verdict"
        );
    }
    assert_eq!(strings(received[0].get("sealed_by")), source_tip_sealers);
    // The received half of the history lists parents too.
    assert_eq!(
        strings(received[0].get("parent_block_ids")),
        vec![received[1].get("block_id").as_str().to_string()]
    );
    assert_eq!(
        strings(received[1].get("parent_block_ids")),
        Vec::<String>::new()
    );
}
