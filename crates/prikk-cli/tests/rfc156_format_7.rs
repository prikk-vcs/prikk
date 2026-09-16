//! RFC 156 §5b, Stage 2a, through the compiled binary: format 7, `prikk format upgrade`, and the version a
//! format refusal names.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::{ObjectId, ObjectType};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader, RepositoryLayout,
    append_superseding_record_for_test_support, maintainer_signature,
};

const SECOND_MAINTAINER_KEY_ID: &str = "stage2a-second-maintainer";
const SECOND_MAINTAINER_SEED: [u8; 32] = [0x6d; 32];

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

fn marker(repo: &Path) -> Vec<u8> {
    std::fs::read(repo.join(".prikk").join("FORMAT")).unwrap()
}

/// Every file under `.prikk` except lock files, which a refused command creates and removes itself.
fn store_bytes(repo: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
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

/// `a.txt` committed and sealed on `heads/main`. Returns the tip block.
fn sealed_repository(tag: &str) -> (PathBuf, ObjectId) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "a"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    let log = text(&run(
        &repo,
        &["log", "--ref", "heads/main", "--format", "json"],
    ));
    let start = log.find("\"block_id\": \"").expect("a block") + "\"block_id\": \"".len();
    (repo, log[start..start + 64].parse().unwrap())
}

#[test]
fn a_new_repository_is_created_at_format_7() {
    let repo = support::unique_repo("rfc156-2a-new");
    support::init(&repo);
    assert_eq!(marker(&repo), b"7\n");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 1: a format-7 repository holding a superseding Block record — two adopted MAINTAINER
/// signatures — verifies clean. Stage 0 measured this exact state failing `verify` in 0.44.0.
#[test]
fn a_format_7_repository_with_a_superseding_block_record_verifies_clean() {
    let (repo, block_id) = sealed_repository("rfc156-2a-superseding-block");
    assert_eq!(marker(&repo), b"7\n");
    let second =
        Ed25519MaintainerSigner::from_seed(SECOND_MAINTAINER_KEY_ID, &SECOND_MAINTAINER_SEED)
            .unwrap();
    support::ok(
        &run(
            &repo,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                SECOND_MAINTAINER_KEY_ID,
                "--public-key",
                &support::hex(&second.public_key_bytes()),
            ],
        ),
        "adopt the second maintainer",
    );
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    let mut envelope = FileObjectStore::new(layout.clone())
        .read_object(block_id)
        .unwrap()
        .unwrap();
    envelope
        .add_signature(maintainer_signature(&second, ObjectType::Block, block_id).unwrap())
        .unwrap();
    append_superseding_record_for_test_support(&layout, &envelope).unwrap();

    let verified = run(&repo, &["verify"]);
    assert_eq!(verified.status.code(), Some(0), "{}", text(&verified));
    let doctor = run(&repo, &["doctor"]);
    assert_eq!(doctor.status.code(), Some(0), "{}", text(&doctor));
    let _ = std::fs::remove_dir_all(&repo);
}

/// Controls 3 and 4 (CLI half): a verified format-6 repository upgrades in place, a second upgrade
/// changes nothing, and the repository still verifies.
#[test]
fn format_upgrade_moves_a_verified_format_6_repository_to_7_once() {
    let (repo, _) = sealed_repository("rfc156-2a-upgrade");
    // A repository this binary created holds nothing format 6 could not: marking it 6 makes a genuine
    // format-6 repository, as a 0.44.0 binary would have created it.
    std::fs::write(repo.join(".prikk").join("FORMAT"), b"6\n").unwrap();
    support::ok(&run(&repo, &["verify"]), "verify at format 6");

    let upgraded = run(&repo, &["format", "upgrade"]);
    support::ok(&upgraded, "format upgrade");
    assert!(
        text(&upgraded).contains("repository format: upgraded from 6 to 7"),
        "{}",
        text(&upgraded)
    );
    assert_eq!(marker(&repo), b"7\n");
    support::ok(&run(&repo, &["verify"]), "verify at format 7");

    let before = store_bytes(&repo);
    let again = run(&repo, &["format", "upgrade"]);
    support::ok(&again, "second format upgrade");
    assert!(
        text(&again).contains("repository format: already 7; nothing changed"),
        "{}",
        text(&again)
    );
    assert!(
        store_bytes(&repo) == before,
        "an idempotent upgrade writes nothing"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: an upgrade refuses on a repository that fails verification, and writes nothing.
#[test]
fn format_upgrade_refuses_a_repository_that_fails_verification() {
    let (repo, _) = sealed_repository("rfc156-2a-upgrade-damaged");
    std::fs::write(repo.join(".prikk").join("FORMAT"), b"6\n").unwrap();
    let container = repo
        .join(".prikk")
        .join("containers")
        .join("patch")
        .join("a.container");
    let mut bytes = std::fs::read(&container).unwrap();
    let last = bytes.len() - 8;
    bytes[last] ^= 0xff;
    std::fs::write(&container, bytes).unwrap();
    assert_eq!(run(&repo, &["verify"]).status.code(), Some(1));

    let before = store_bytes(&repo);
    let refused = run(&repo, &["format", "upgrade"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains("refusing to upgrade the repository format")
            && text(&refused).contains("nothing was changed"),
        "{}",
        text(&refused)
    );
    assert!(
        store_bytes(&repo) == before,
        "a refused upgrade writes nothing"
    );
    assert_eq!(marker(&repo), b"6\n");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6: a format refusal names the version it read — 0.44.0 said `unsupported format version: 0`.
#[test]
fn a_format_refusal_names_the_version_found() {
    let repo = support::unique_repo("rfc156-2a-format-8");
    support::init(&repo);
    std::fs::write(repo.join(".prikk").join("FORMAT"), b"8\n").unwrap();
    for args in [vec!["status"], vec!["verify"], vec!["log"]] {
        let refused = run(&repo, &args);
        assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
        assert!(
            text(&refused).contains("unsupported format version: 8"),
            "prikk {}: {}",
            args.join(" "),
            text(&refused)
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}
