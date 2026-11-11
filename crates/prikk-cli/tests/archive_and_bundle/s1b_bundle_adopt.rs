//! 0.57.0 S1b (the owner's ruling A, 2026-11-11): `bundle import --adopt` lands a delivered branch as a real local branch.
//!
//! The case table (what `bundle import --adopt` does with a delivery of `heads/main`):
//!
//! | the receiving repository | result |
//! |---|---|
//! | the sender's key adopted, the receiver has its own adopted maintainer key, `heads/main` absent | **adopted**: a local branch, published by the receiver's key over the delivered RefStates; no `remotes/` pointer; `verify` passes; the current branch is materialized |
//! | the sender's key **not** adopted | lands under `remotes/` with the reason, naming `trust maintainer add`; no local branch |
//! | a local `heads/main` already exists | lands under `remotes/` with the reason, naming `merge --from remotes/heads/main` and the new-repository route; the local branch is untouched |
//! | the receiver has **no** maintainer key | refused before any write, naming `prikk key status` and `prikk setup` |
//! | the worktree is dirty and `heads/main` is the current branch | refused before any write |
//! | `--format json` | `bundle-import-report-v1` carries `adopted`, `landed_as`, `remote_reason`, `materialized_*` |
//!
//! The bundle-only receiver of the S1 review: `bundle import`, then `trust maintainer add`, then `bundle import --adopt`, then
//! `branch create --from <its older block>`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use prikk_store::MaintainerSigner as _;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

const RECEIVER_KEY_ID: &str = "s1b-receiver";
const RECEIVER_SEED: [u8; 32] = [0x42; 32];

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Run `prikk` in `repo` as the receiver: its own maintainer key id and seed.
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

fn receiver_public_key() -> String {
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(RECEIVER_KEY_ID, &RECEIVER_SEED).unwrap();
    support::hex(&signer.public_key_bytes())
}

/// Adopt the receiver's own key (the sender's is adopted by `support::trust_maintainer`).
fn trust_receiver_key(repo: &Path) {
    support::ok(
        &support::prikk(repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                RECEIVER_KEY_ID,
                "--public-key",
                &receiver_public_key(),
            ])
            .output()
            .unwrap(),
        "trust the receiver's key",
    );
}

/// The sender: `heads/main` with two sealed generations; returns (repository, bundle path, the older block id, the newer one).
fn sender_with_bundle(tag: &str) -> (PathBuf, PathBuf, String, String) {
    let sender = support::unique_repo(tag);
    support::init(&sender);
    support::generation(&sender, "heads/main", "a.txt", b"one\n", "one");
    support::generation(&sender, "heads/main", "b.txt", b"two\n", "two");
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
    assert!(ids.len() >= 2, "two blocks in the log:\n{said}");
    let bundle = std::env::temp_dir().join(format!("s1b-{tag}-{}.pbndl", support::unique_suffix()));
    support::ok(
        &support::prikk(&sender)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    (sender, bundle, ids[1].clone(), ids[0].clone())
}

fn bundle_import(repo: &Path, bundle: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["bundle", "import", "--input", bundle.to_str().unwrap()];
    args.extend_from_slice(extra);
    as_receiver(repo, &args)
}

/// Every file under `.prikk` except lock files: what "nothing was written" is compared against.
fn store_bytes(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("lock"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn list_refs(repo: &Path) -> String {
    text(&as_receiver(repo, &["branch", "list"]))
}

#[test]
fn the_bundle_only_receiver_adopts_then_branches_from_an_older_block() {
    let (_sender, bundle, older, newer) = sender_with_bundle("s1b-flow");
    let receiver = support::unique_repo("s1b-flow-receiver");
    support::init(&receiver);

    // 1. Without --adopt: a received pointer only. Branching from its block is refused (S1), and the refusal is true.
    support::ok(&bundle_import(&receiver, &bundle, &[]), "bundle import");
    let before = as_receiver(
        &receiver,
        &["branch", "create", "heads/feat", "--from", &older],
    );
    assert_eq!(before.status.code(), Some(1), "{}", text(&before));

    // 2. The sender's key and the receiver's own are adopted; then --adopt lands a real local branch.
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    let adopted = bundle_import(&receiver, &bundle, &["--adopt"]);
    let said = text(&adopted);
    assert_eq!(adopted.status.code(), Some(0), "{said}");
    assert!(
        said.contains("heads/main: adopted as a local branch"),
        "{said}"
    );
    assert!(
        said.contains("no MAINTAINER key was trusted by this import")
            && said.contains("heads/main is now a local branch"),
        "the note names the adopted ref and stays true about trust: {said}"
    );
    assert!(
        said.contains("materialized into the worktree: files written: 2"),
        "heads/main is the current branch, so it is written to the worktree: {said}"
    );
    assert!(receiver.join("a.txt").exists() && receiver.join("b.txt").exists());
    let verify = as_receiver(&receiver, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));

    // 3. The block is now reached by a local branch: branching from its older block works.
    let after = as_receiver(
        &receiver,
        &["branch", "create", "heads/feat", "--from", &older],
    );
    assert_eq!(after.status.code(), Some(0), "{}", text(&after));
    assert!(
        text(&after).contains("created branch heads/feat"),
        "{}",
        text(&after)
    );
    let _ = newer;
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn json_carries_the_adoption_in_the_same_names_as_the_archive() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-json");
    let receiver = support::unique_repo("s1b-json-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    let output = bundle_import(&receiver, &bundle, &["--adopt", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let json = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        json.contains("\"schema_version\": \"bundle-import-report-v1\""),
        "{json}"
    );
    assert!(json.contains("\"landed_as\": \"heads/main\""), "{json}");
    assert!(json.contains("\"adopted\": true"), "{json}");
    assert!(json.contains("\"remote_reason\": null"), "{json}");
    assert!(
        json.contains("\"materialized_into_worktree\": true"),
        "{json}"
    );
    assert!(json.contains("\"materialized_written_files\": 2"), "{json}");
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn an_unadopted_sender_key_lands_under_remotes_with_a_typeable_reason() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-unadopted");
    let receiver = support::unique_repo("s1b-unadopted-receiver");
    support::init(&receiver);
    // Only the receiver's own key is adopted: the sender's is not.
    trust_receiver_key(&receiver);
    let output = bundle_import(&receiver, &bundle, &["--adopt"]);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(said.contains("received remotes/heads/main"), "{said}");
    assert!(said.contains("not adopted: "), "{said}");
    assert!(
        said.contains("has not adopted that key")
            && said.contains("prikk trust maintainer add --key-id <id> --public-key <hex>"),
        "{said}"
    );
    assert!(
        list_refs(&receiver).contains("current branch heads/main has no published history yet"),
        "no local branch was created: {}",
        list_refs(&receiver)
    );
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn an_existing_local_branch_is_not_overwritten_and_the_reason_names_the_two_routes() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-exists");
    let receiver = support::unique_repo("s1b-exists-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    support::generation(&receiver, "heads/main", "own.txt", b"mine\n", "mine");
    let own_tip = text(&as_receiver(&receiver, &["log", "--format", "json"]));
    let output = bundle_import(&receiver, &bundle, &["--adopt"]);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(
        said.contains("not adopted: a local branch heads/main already exists"),
        "{said}"
    );
    assert!(
        said.contains("prikk merge --from remotes/heads/main"),
        "{said}"
    );
    assert!(said.contains("prikk setup <new-dir>"), "{said}");
    assert_eq!(
        text(&as_receiver(&receiver, &["log", "--format", "json"])),
        own_tip,
        "the local branch is untouched"
    );
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn a_receiver_with_no_maintainer_key_is_refused_before_any_write() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-nokey");
    let receiver = support::unique_repo("s1b-nokey-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    let before = store_bytes(&receiver);
    // No PRIKK_MAINTAINER_SEED_FILE and an empty key directory: the receiver has no usable key.
    let output = support::prikk(&receiver)
        .args([
            "bundle",
            "import",
            "--input",
            bundle.to_str().unwrap(),
            "--adopt",
        ])
        .output()
        .unwrap();
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains("prikk key status --role maintainer") && said.contains("prikk setup"),
        "{said}"
    );
    assert_eq!(store_bytes(&receiver), before, "nothing was written");
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn a_dirty_worktree_on_the_current_branch_refuses_before_any_write() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-dirty");
    let receiver = support::unique_repo("s1b-dirty-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    // A file the adopted branch would write, already there with other bytes: in the way.
    std::fs::write(receiver.join("a.txt"), b"not what the branch holds\n").unwrap();
    let before = store_bytes(&receiver);
    let output = bundle_import(&receiver, &bundle, &["--adopt"]);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains("in the way") || said.contains("changes"),
        "{said}"
    );
    assert_eq!(store_bytes(&receiver), before, "nothing was written");
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn work_queued_on_the_name_refuses_the_adoption_before_any_write() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-queue");
    let receiver = support::unique_repo("s1b-queue-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    // An unsealed commit on the (still unborn) heads/main: the queue owns the name.
    std::fs::write(receiver.join("queued.txt"), b"queued\n").unwrap();
    support::ok(
        &support::commit(&receiver, "heads/main", "queued"),
        "an unsealed commit",
    );
    let before = store_bytes(&receiver);
    let output = bundle_import(&receiver, &bundle, &["--adopt"]);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(said.contains("adopting heads/main"), "{said}");
    assert!(
        said.contains("seal"),
        "the way forward is to seal first: {said}"
    );
    assert_eq!(store_bytes(&receiver), before, "nothing was written");
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn tag_create_at_a_received_only_or_unreached_block_is_refused_naming_both_importers() {
    let (_sender, bundle, older, _newer) = sender_with_bundle("s1b-tag");
    let receiver = support::unique_repo("s1b-tag-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    support::ok(&bundle_import(&receiver, &bundle, &[]), "bundle import");

    // Held only through remotes/heads/main.
    let received_only = as_receiver(
        &receiver,
        &["tag", "create", "tags/old", "--target", &older],
    );
    let said = text(&received_only);
    assert_eq!(received_only.status.code(), Some(1), "{said}");
    assert!(said.contains("tag create --target"), "{said}");
    assert!(
        said.contains("held only through the received ref(s) remotes/heads/main"),
        "{said}"
    );
    assert!(
        said.contains("prikk archive import <the archive> --adopt")
            && said.contains("prikk bundle import --input <the bundle> --adopt")
            && said.contains("prikk sync seal <ref> --claim <id>"),
        "every arrival route's way forward is named: {said}"
    );

    // After `received remove` no ref reaches it at all.
    support::ok(
        &as_receiver(&receiver, &["received", "remove", "remotes/heads/main"]),
        "received remove",
    );
    let unreached = as_receiver(
        &receiver,
        &["tag", "create", "tags/old", "--target", &older],
    );
    let said = text(&unreached);
    assert_eq!(unreached.status.code(), Some(1), "{said}");
    assert!(said.contains("no ref reaches it"), "{said}");
    assert!(
        said.contains("prikk bundle import --input <the bundle> --adopt"),
        "{said}"
    );

    // The way forward works: the bundle's own --adopt, then the tag at the same block.
    support::ok(
        &bundle_import(&receiver, &bundle, &["--adopt"]),
        "bundle import --adopt",
    );
    support::ok(
        &as_receiver(
            &receiver,
            &["tag", "create", "tags/old", "--target", &older],
        ),
        "tag create at a block a local branch reaches",
    );
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn the_refusal_branch_create_gives_for_a_bundle_delivery_is_true() {
    // S1's refusal named `archive import <the archive that carried it> --adopt`, and no archive carried a bundle's block.
    let (_sender, bundle, older, _newer) = sender_with_bundle("s1b-s1-refusal");
    let receiver = support::unique_repo("s1b-s1-refusal-receiver");
    support::init(&receiver);
    support::ok(&bundle_import(&receiver, &bundle, &[]), "bundle import");
    let refused = as_receiver(
        &receiver,
        &["branch", "create", "heads/feat", "--from", &older],
    );
    let said = text(&refused);
    assert_eq!(refused.status.code(), Some(1), "{said}");
    assert!(
        said.contains("prikk bundle import --input <the bundle> --adopt"),
        "a bundle receiver is told about the bundle's --adopt: {said}"
    );
    assert!(
        !said.contains("archive that carried it"),
        "and not told an archive carried it: {said}"
    );
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn a_receiver_key_this_repository_has_not_adopted_is_refused_before_any_write() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-signer");
    let receiver = support::unique_repo("s1b-signer-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver); // the sender's key only; the receiver's own key is *not* adopted
    let before = store_bytes(&receiver);
    let output = bundle_import(&receiver, &bundle, &["--adopt"]);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains(RECEIVER_KEY_ID) || said.contains("trust maintainer add"),
        "{said}"
    );
    assert_eq!(store_bytes(&receiver), before, "nothing was written");
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn an_adopted_delivery_leaves_no_received_pointer_behind() {
    let (_sender, bundle, _older, _newer) = sender_with_bundle("s1b-nopointer");
    let receiver = support::unique_repo("s1b-nopointer-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    support::ok(
        &bundle_import(&receiver, &bundle, &["--adopt"]),
        "bundle import --adopt",
    );
    let listing = list_refs(&receiver);
    assert!(
        !listing.contains("(received)"),
        "an adopted branch is a local branch and no remotes/ pointer is written for it: {listing}"
    );
    assert!(listing.contains("heads/main"), "{listing}");
    let _ = std::fs::remove_dir_all(&receiver);
}

#[test]
fn adopting_a_rollback_needs_the_flag_the_plain_import_needs() {
    let sender = support::unique_repo("s1b-rollback-sender");
    support::init(&sender);
    support::generation(&sender, "heads/main", "a.txt", b"one\n", "one");
    let older = std::env::temp_dir().join(format!("s1b-older-{}.pbndl", support::unique_suffix()));
    let newer = std::env::temp_dir().join(format!("s1b-newer-{}.pbndl", support::unique_suffix()));
    let export = |to: &Path| {
        support::ok(
            &support::prikk(&sender)
                .args(["bundle", "export", "--ref", "heads/main", "--output"])
                .arg(to)
                .output()
                .unwrap(),
            "bundle export",
        );
    };
    export(&older);
    support::generation(&sender, "heads/main", "b.txt", b"two\n", "two");
    export(&newer);

    let receiver = support::unique_repo("s1b-rollback-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    trust_receiver_key(&receiver);
    support::ok(
        &bundle_import(&receiver, &newer, &[]),
        "import the newer tip",
    );

    // The older delivery is a rollback of the received pointer: refused with or without `--adopt`, until the flag is given.
    let before = store_bytes(&receiver);
    let refused = bundle_import(&receiver, &older, &["--adopt"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(text(&refused).contains("rollback"), "{}", text(&refused));
    assert_eq!(store_bytes(&receiver), before, "a refusal writes nothing");

    let allowed = bundle_import(&receiver, &older, &["--adopt", "--allow-non-fast-forward"]);
    support::ok(&allowed, "bundle import --adopt --allow-non-fast-forward");
    let said = text(&allowed);
    assert!(
        said.contains("is now a local branch"),
        "the flag lets the adoption through: {said}"
    );
    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_dir_all(&receiver);
    let _ = std::fs::remove_file(&older);
    let _ = std::fs::remove_file(&newer);
}
