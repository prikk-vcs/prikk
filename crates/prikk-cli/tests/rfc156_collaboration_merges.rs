//! RFC 156 Stage 2b control 7, through the compiled binary: the collaboration case merges in both orders.
//!
//! A sender seals a patch and sends it twice — as a `sync build` artifact and as a `bundle export` of
//! `heads/main`. The receiver, under its own maintainer key and having adopted the sender's, accepts and
//! seals the artifact's claim, which gives the sender's block id (RFC 156 §2; `docs/src/guide/sync.md`).
//! Before format 7 importing the sender's bundle then refused (`differs from candidate`). Now the stored
//! block carries both maintainers' signatures, whichever of the two arrives first.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use prikk_object::{ObjectId, ObjectType, RefStatePayload, SignerRole};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

const RECEIVER_MAINTAINER_KEY_ID: &str = "rfc156-receiver-maintainer";
const RECEIVER_MAINTAINER_SEED: [u8; 32] = [0x7e; 32];

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn as_sender(repo: &Path) -> Command {
    let mut command = support::prikk(repo);
    command
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        );
    command
}

fn as_receiver(repo: &Path) -> Command {
    let mut command = support::prikk(repo);
    command
        .env("PRIKK_MAINTAINER_KEY_ID", RECEIVER_MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&RECEIVER_MAINTAINER_SEED)),
        );
    command
}

fn run(mut command: Command, args: &[&str], what: &str) -> Output {
    let output = command.args(args).output().unwrap();
    support::ok(&output, what);
    output
}

fn receiver_public_key_hex() -> String {
    use prikk_store::MaintainerSigner;
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed(
        RECEIVER_MAINTAINER_KEY_ID,
        &RECEIVER_MAINTAINER_SEED,
    )
    .unwrap();
    support::hex(&signer.public_key_bytes())
}

/// The sender's two files: a `sync build` artifact for an empty receiver and a bundle of `heads/main`.
fn sender_files(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let sender = support::unique_repo(&format!("{tag}-sender"));
    support::init(&sender);
    support::generation(&sender, "heads/main", "a.txt", b"shared\n", "first");

    let empty = support::unique_repo(&format!("{tag}-empty"));
    support::init(&empty);
    let have = empty.join("have.bin");
    run(
        support::prikk(&empty),
        &[
            "sync",
            "have",
            "heads/main",
            "--output",
            have.to_str().unwrap(),
        ],
        "sync have",
    );
    let artifact = sender.join("artifact.bin");
    run(
        as_sender(&sender),
        &[
            "sync",
            "build",
            "heads/main",
            "--have",
            have.to_str().unwrap(),
            "--output",
            artifact.to_str().unwrap(),
        ],
        "sync build",
    );
    let bundle = sender.join("main.bundle");
    run(
        as_sender(&sender),
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
        "bundle export",
    );
    let _ = std::fs::remove_dir_all(&empty);
    (sender, artifact, bundle)
}

fn receiver(tag: &str) -> PathBuf {
    let repo = support::unique_repo(&format!("{tag}-receiver"));
    support::init(&repo);
    support::trust_maintainer(&repo);
    run(
        support::prikk(&repo),
        &[
            "trust",
            "maintainer",
            "add",
            "--key-id",
            RECEIVER_MAINTAINER_KEY_ID,
            "--public-key",
            &receiver_public_key_hex(),
        ],
        "adopt the receiver's own key",
    );
    repo
}

fn accept_and_seal(repo: &Path, artifact: &Path) -> Output {
    let accept = run(
        support::prikk(repo),
        &["sync", "accept", artifact.to_str().unwrap()],
        "sync accept",
    );
    let accept_text = text(&accept);
    let claim_id = accept_text
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("claim "))
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("sync accept prints a claim: {accept_text}"))
        .trim()
        .to_string();
    as_receiver(repo)
        .args(["sync", "seal", "heads/main", "--claim", &claim_id])
        .output()
        .unwrap()
}

fn import(repo: &Path, bundle: &Path) -> Output {
    support::prikk(repo)
        .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
        .output()
        .unwrap()
}

/// The sender's tip block id, read from its own `heads/main`.
fn tip_block(sender: &Path) -> ObjectId {
    let layout = RepositoryLayout::open(sender.to_path_buf()).unwrap();
    let pointer = RefStore::new(layout.clone())
        .list_ref_pointers()
        .unwrap()
        .into_iter()
        .find(|pointer| pointer.ref_name == "heads/main")
        .expect("the sender holds heads/main");
    let envelope = FileObjectStore::new(layout)
        .read_object(pointer.ref_state_id)
        .unwrap()
        .expect("the RefState is stored");
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}

fn maintainer_signers(repo: &Path, block: ObjectId) -> Vec<String> {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let envelope = FileObjectStore::new(layout)
        .read_object(block)
        .unwrap()
        .expect("the block is stored");
    assert_eq!(envelope.object_type, ObjectType::Block);
    envelope
        .signatures
        .iter()
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
        .map(|signature| signature.key_id.clone())
        .collect()
}

fn assert_merged(repo: &Path, block: ObjectId) {
    let verify = support::verify(repo);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    assert_eq!(
        maintainer_signers(repo, block),
        vec![
            support::MAINTAINER_KEY_ID.to_string(),
            RECEIVER_MAINTAINER_KEY_ID.to_string()
        ],
        "both maintainers' signatures are stored on the one block"
    );
}

#[test]
fn seal_then_import_merges_the_senders_block() {
    let (sender, artifact, bundle) = sender_files("rfc156-collab-seal-first");
    let block = tip_block(&sender);
    let repo = receiver("rfc156-collab-seal-first");

    let sealed = accept_and_seal(&repo, &artifact);
    support::ok(&sealed, "sync seal");
    let imported = import(&repo, &bundle);
    support::ok(&imported, "bundle import after sealing");
    assert!(
        text(&imported).contains("objects gaining signatures: "),
        "{}",
        text(&imported)
    );
    assert_merged(&repo, block);

    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn import_then_seal_merges_the_receivers_block() {
    let (sender, artifact, bundle) = sender_files("rfc156-collab-import-first");
    let block = tip_block(&sender);
    let repo = receiver("rfc156-collab-import-first");

    let imported = import(&repo, &bundle);
    support::ok(&imported, "bundle import first");
    let sealed = accept_and_seal(&repo, &artifact);
    support::ok(&sealed, "sync seal after importing");
    assert_merged(&repo, block);

    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_dir_all(&repo);
}
