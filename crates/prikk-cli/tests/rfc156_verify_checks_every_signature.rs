//! RFC 156 Stage 1, through the compiled binary: `verify` checks **every** AUTHOR signature on a patch.
//!
//! The patch's first signature (canonical order: key id bytes) is its committer's, `dc67-test-author`;
//! a superseding record adds a second by `stage1-second-author`, whose key this repository recorded
//! through an ordinary commit. A `verify` that reads only the first signature passes the repository
//! whether that second signature is valid or not.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::ObjectId;
use prikk_store::{
    Ed25519AuthorSigner, FileObjectStore, ObjectReader, RepositoryLayout,
    append_superseding_record_for_test_support, author_signature,
};

const SECOND_AUTHOR_KEY_ID: &str = "stage1-second-author";
const SECOND_AUTHOR_SEED: [u8; 32] = [0x5d; 32];

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(args)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A sealed patch by the harness author, the second author's key recorded by its own commit on another
/// ref, and a
/// superseding record for the first patch carrying a second AUTHOR signature — corrupted or not.
fn two_author_patch(tag: &str, corrupt_second: bool) -> (PathBuf, ObjectId) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    let first = support::commit(&repo, "heads/main", "a");
    support::ok(&first, "commit a");
    let patch_id: ObjectId = text(&first)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints a patch id")
        .trim()
        .parse()
        .unwrap();
    support::ok(&support::seal(&repo, "heads/main"), "seal a");

    std::fs::write(repo.join("b.txt"), b"beta\n").unwrap();
    support::ok(
        &support::prikk(&repo)
            .env("PRIKK_AUTHOR_KEY_ID", SECOND_AUTHOR_KEY_ID)
            .env(
                "PRIKK_AUTHOR_SEED_FILE",
                support::seed_file(&support::hex(&SECOND_AUTHOR_SEED)),
            )
            .args(["commit", "--ref", "heads/elsewhere", "-m", "b"])
            .output()
            .unwrap(),
        "commit b as the second author",
    );
    // On a ref `bundle export --ref heads/main` does not carry: the superseding signature is then the
    // only thing in that bundle naming the second author, so the export control cannot pass by
    // carrying the key for some other patch.
    support::ok(&support::seal(&repo, "heads/elsewhere"), "seal b");

    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    let mut envelope = FileObjectStore::new(layout.clone())
        .read_object(patch_id)
        .unwrap()
        .expect("the patch is stored");
    let second = Ed25519AuthorSigner::from_seed(SECOND_AUTHOR_KEY_ID, &SECOND_AUTHOR_SEED).unwrap();
    let mut signature = author_signature(&second, patch_id).unwrap();
    if corrupt_second {
        signature.signature_bytes[0] ^= 0x01;
    }
    envelope.add_signature(signature).unwrap();
    assert_eq!(
        envelope.signatures[0].key_id,
        support::AUTHOR_KEY_ID,
        "the committer's signature must stay first, so the checked one is genuinely the second"
    );
    append_superseding_record_for_test_support(&layout, &envelope).unwrap();
    (repo, patch_id)
}

/// Stage 1 control 5: `verify` fails on a patch whose second AUTHOR signature is invalid, naming that
/// signer — and its twin, with the second signature valid, verifies clean.
#[test]
fn verify_fails_a_patch_whose_second_author_signature_is_invalid() {
    let (valid, _) = two_author_patch("rfc156-stage1-valid-second", false);
    let clean = run(&valid, &["verify"]);
    assert_eq!(clean.status.code(), Some(0), "{}", text(&clean));

    let (invalid, patch_id) = two_author_patch("rfc156-stage1-invalid-second", true);
    let failed = run(&invalid, &["verify"]);
    assert_eq!(failed.status.code(), Some(1), "{}", text(&failed));
    let output = text(&failed);
    assert!(
        output.contains(&format!(
            "patch {patch_id} AUTHOR signature does not verify against recorded key material for {SECOND_AUTHOR_KEY_ID}"
        )),
        "{output}"
    );

    let _ = std::fs::remove_dir_all(&valid);
    let _ = std::fs::remove_dir_all(&invalid);
}

/// `bundle export` carries **every** AUTHOR signer's key, so a receiver can check every signature. Had
/// it carried only the first signer's, the receiver's `verify` would count the second signature as
/// unverifiable.
#[test]
fn a_bundle_carries_every_author_signers_key() {
    let (sender, _) = two_author_patch("rfc156-stage1-export-sender", false);
    let bundle = sender.join("two-authors.bundle");
    support::ok(
        &run(
            &sender,
            &[
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle.to_str().unwrap(),
            ],
        ),
        "bundle export",
    );

    let receiver = support::unique_repo("rfc156-stage1-export-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    support::ok(
        &run(
            &receiver,
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        ),
        "bundle import",
    );
    let verified = run(&receiver, &["verify"]);
    assert_eq!(verified.status.code(), Some(0), "{}", text(&verified));
    assert!(
        text(&verified).contains("unverifiable author signatures: 0"),
        "{}",
        text(&verified)
    );
    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_dir_all(&receiver);
}

/// `sync build` carries **every** AUTHOR signer's key too: the receiver records both.
#[test]
fn an_exchange_artifact_carries_every_author_signers_key() {
    let (sender, _) = two_author_patch("rfc156-stage1-sync-sender", false);
    let receiver = support::unique_repo("rfc156-stage1-sync-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);
    let have = receiver.join("have.bin");
    let artifact = sender.join("exchange.bin");
    support::ok(
        &run(
            &receiver,
            &[
                "sync",
                "have",
                "heads/main",
                "--output",
                have.to_str().unwrap(),
            ],
        ),
        "sync have",
    );
    support::ok(
        &run(
            &sender,
            &[
                "sync",
                "build",
                "heads/main",
                "--have",
                have.to_str().unwrap(),
                "--output",
                artifact.to_str().unwrap(),
            ],
        ),
        "sync build",
    );
    let accepted = run(&receiver, &["sync", "accept", artifact.to_str().unwrap()]);
    support::ok(&accepted, "sync accept");
    // Both signers' keys arrive: the committer's and the superseding signature's.
    let output = text(&accepted);
    assert!(
        output.contains("author key material: 2 recorded"),
        "{output}"
    );
    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_dir_all(&receiver);
}
