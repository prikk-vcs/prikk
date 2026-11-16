//! 0.58.0 R2 (i): `archive import --adopt` and `bundle import --adopt` record, in `verify`'s record, the Blocks they confirmed by
//! replay -- exactly the adopted branch's lineage -- so a receiver can anchor from its first command. A landing under `remotes/`
//! (nothing adopted) records nothing. The walk's failure path (a Block that does not replay: one stderr line, exit 0, the Blocks before
//! it recorded) is held by `prikk-store`'s `confirmed_history` tests, which can write such a Block; no command can.
//!
//! The record is `<repo>/.prikk/cache/replay-verified-blocks.v1`: its tail is the 32-byte ids, preceded by their count.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use prikk_store::MaintainerSigner as _;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Output;

const RECEIVER_KEY_ID: &str = "r2-receiver";
const RECEIVER_SEED: [u8; 32] = [0x52; 32];

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn as_receiver(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", RECEIVER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&RECEIVER_SEED)),
        )
        .args(args)
        .output()
        .unwrap()
}

fn trust_receiver_key(repo: &Path) {
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(RECEIVER_KEY_ID, &RECEIVER_SEED).unwrap();
    support::ok(
        &support::prikk(repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                RECEIVER_KEY_ID,
                "--public-key",
                &support::hex(&signer.public_key_bytes()),
            ])
            .output()
            .unwrap(),
        "trust the receiver's key",
    );
}

/// The sender: `heads/main` with three sealed generations, the third editing text. Returns the repository and its block ids (newest first).
fn sender(tag: &str) -> (PathBuf, Vec<String>) {
    let sender = support::unique_repo(tag);
    support::init(&sender);
    support::generation(&sender, "heads/main", "a.txt", b"one\ntwo\nthree\n", "one");
    support::generation(&sender, "heads/main", "b.txt", b"two\n", "two");
    support::generation(
        &sender,
        "heads/main",
        "a.txt",
        b"one\ntwo edited\nthree\n",
        "three",
    );
    let log = support::prikk(&sender)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&log.stdout).to_string();
    let marker = "\"block_id\": \"";
    let ids: Vec<String> = said
        .match_indices(marker)
        .map(|(at, _)| said[at + marker.len()..at + marker.len() + 64].to_string())
        .collect();
    assert_eq!(ids.len(), 3, "three blocks in the log:\n{said}");
    (sender, ids)
}

/// The ids in the receiver's record, or `None` when it has none.
fn record(repo: &Path) -> Option<BTreeSet<String>> {
    let bytes = std::fs::read(repo.join(".prikk/cache/replay-verified-blocks.v1")).ok()?;
    // The tail is `count (u32 BE)` then `count` ids; find the only count that fits.
    (0..=bytes.len() / 32).find_map(|count| {
        let ids_at = bytes.len().checked_sub(count * 32)?;
        let count_at = ids_at.checked_sub(4)?;
        (u32::from_be_bytes(bytes[count_at..ids_at].try_into().ok()?) as usize == count).then(
            || {
                bytes[ids_at..]
                    .chunks(32)
                    .map(|id| id.iter().map(|byte| format!("{byte:02x}")).collect())
                    .collect()
            },
        )
    })
}

fn ids(blocks: &[String]) -> BTreeSet<String> {
    blocks.iter().cloned().collect()
}

#[test]
fn bundle_import_adopt_records_the_adopted_lineage() {
    let (sender, blocks) = sender("r2-bundle");
    let bundle = std::env::temp_dir().join(format!("r2-{}.pbndl", support::unique_suffix()));
    support::ok(
        &support::prikk(&sender)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    let receiver = support::unique_repo("r2-bundle-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    assert_eq!(record(&receiver), None, "a receiver starts with no record");

    let imported = as_receiver(
        &receiver,
        &[
            "bundle",
            "import",
            "--input",
            bundle.to_str().unwrap(),
            "--adopt",
        ],
    );
    assert_eq!(imported.status.code(), Some(0), "{}", text(&imported));
    assert!(
        !text(&imported).contains("did not replay cleanly"),
        "a clean history prints no line: {}",
        text(&imported)
    );

    assert_eq!(
        record(&receiver),
        Some(ids(&blocks)),
        "exactly the adopted lineage"
    );
    // The first command after the import anchors; a text edit commits and the repository verifies.
    std::fs::write(receiver.join("a.txt"), b"one\ntwo edited again\nthree\n").unwrap();
    let committed = support::commit(&receiver, "heads/main", "edit");
    assert_eq!(committed.status.code(), Some(0), "{}", text(&committed));
    let verify = as_receiver(&receiver, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn archive_import_adopt_records_the_adopted_lineage() {
    let (sender, blocks) = sender("r2-archive");
    let archive = std::env::temp_dir().join(format!("r2-{}.parchive", support::unique_suffix()));
    support::ok(
        &support::prikk(&sender)
            .args(["archive", "export"])
            .arg(&archive)
            .output()
            .unwrap(),
        "archive export",
    );
    let receiver = support::unique_repo("r2-archive-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);

    let imported = as_receiver(
        &receiver,
        &["archive", "import", archive.to_str().unwrap(), "--adopt"],
    );
    assert_eq!(imported.status.code(), Some(0), "{}", text(&imported));
    assert!(
        text(&imported).contains("adopted as a local branch"),
        "{}",
        text(&imported)
    );
    assert!(
        !text(&imported).contains("did not replay cleanly"),
        "{}",
        text(&imported)
    );

    assert_eq!(
        record(&receiver),
        Some(ids(&blocks)),
        "exactly the adopted lineage"
    );
    let _ = std::fs::remove_dir_all(&receiver);
}

/// Nothing adopted, nothing recorded: the sender's key is not adopted, so the history lands under `remotes/` and the import never
/// claims it confirmed anything. **Red control:** record whatever an import carries and this goes red.
#[test]
fn a_remotes_landing_records_nothing() {
    let (sender, _blocks) = sender("r2-remotes");
    let bundle = std::env::temp_dir().join(format!("r2-{}.pbndl", support::unique_suffix()));
    support::ok(
        &support::prikk(&sender)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    let receiver = support::unique_repo("r2-remotes-receiver");
    support::init(&receiver);
    trust_receiver_key(&receiver);

    let imported = as_receiver(
        &receiver,
        &[
            "bundle",
            "import",
            "--input",
            bundle.to_str().unwrap(),
            "--adopt",
        ],
    );
    assert_eq!(imported.status.code(), Some(0), "{}", text(&imported));
    assert!(
        text(&imported).contains("received remotes/heads/main"),
        "{}",
        text(&imported)
    );
    assert_eq!(
        record(&receiver),
        None,
        "a landing under remotes/ records nothing"
    );
    let _ = std::fs::remove_dir_all(&receiver);
}
