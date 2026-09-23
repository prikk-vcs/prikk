//! A refused `bundle import` or `sync accept` leaves the receiving repository exactly as it was.
//!
//! Driven through the compiled binary with two real installations, each made by `prikk setup` in its
//! own key directory. The collision controls sign both installations' patches under one shared
//! `PRIKK_AUTHOR_KEY_ID` with different keys. Before 0.45.0 two default installations collided on
//! `author` without trying; new keys now get distinct ids, so the collision is made deliberately.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

/// One author key id both collision installations sign under, with different keys.
const SHARED_AUTHOR_ID: &str = "shared-author";

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// `prikk setup`, one file committed and sealed on `heads/main`, the commit signed under `author_id`
/// when given.
fn installation(tag: &str, file: &str, author_id: Option<&str>) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::ok(&run(&repo, &["setup", "."]), "setup");
    std::fs::write(repo.join(file), format!("{file}\n")).unwrap();
    let mut commit = support::prikk(&repo);
    if let Some(author_id) = author_id {
        commit.env("PRIKK_AUTHOR_KEY_ID", author_id);
    }
    support::ok(
        &commit.args(["commit", "-m", file]).output().unwrap(),
        "commit",
    );
    support::ok(&run(&repo, &["seal", "--allow-no-audit"]), "seal");
    repo
}

/// What a user can see of the store: `verify`'s exit code and its object line.
fn verify_view(repo: &Path) -> (Option<i32>, String) {
    let output = run(repo, &["verify"]);
    let line = text(&output)
        .lines()
        .find(|line| line.starts_with("object items:"))
        .unwrap_or_default()
        .to_string();
    (output.status.code(), line)
}

/// Control 1: the colliding author id. Before the fix `verify` went from exit 0 to exit 1 and the
/// object count doubled; now the refusal changes nothing, byte for byte.
#[test]
fn a_bundle_import_refused_for_a_colliding_author_id_changes_nothing() {
    let receiver = installation(
        "import-collision-receiver",
        "mine.txt",
        Some(SHARED_AUTHOR_ID),
    );
    let sender = installation(
        "import-collision-sender",
        "theirs.txt",
        Some(SHARED_AUTHOR_ID),
    );
    let bundle = sender.join("theirs.bundle");
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

    let view_before = verify_view(&receiver);
    assert_eq!(view_before.0, Some(0), "the receiver starts healthy");
    let bytes_before = support::store_bytes(&receiver);

    let imported = run(
        &receiver,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    assert_eq!(imported.status.code(), Some(1), "{}", text(&imported));
    assert!(
        text(&imported).contains("already has a different recorded public key"),
        "{}",
        text(&imported)
    );

    assert_eq!(verify_view(&receiver), view_before, "verify reads the same");
    assert!(
        support::store_bytes(&receiver) == bytes_before,
        "every file under .prikk is unchanged"
    );
    let _ = std::fs::remove_dir_all(&receiver);
    let _ = std::fs::remove_dir_all(&sender);
}

/// Control 3: `sync accept` under the same collision writes nothing either.
#[test]
fn a_sync_accept_refused_for_a_colliding_author_id_changes_nothing() {
    let receiver = installation(
        "accept-collision-receiver",
        "mine.txt",
        Some(SHARED_AUTHOR_ID),
    );
    let sender = installation(
        "accept-collision-sender",
        "theirs.txt",
        Some(SHARED_AUTHOR_ID),
    );
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

    let view_before = verify_view(&receiver);
    let bytes_before = support::store_bytes(&receiver);
    let accepted = run(&receiver, &["sync", "accept", artifact.to_str().unwrap()]);
    assert_eq!(accepted.status.code(), Some(1), "{}", text(&accepted));
    assert!(
        text(&accepted).contains("already has a different recorded public key"),
        "{}",
        text(&accepted)
    );
    assert_eq!(verify_view(&receiver), view_before);
    assert!(support::store_bytes(&receiver) == bytes_before);
    let _ = std::fs::remove_dir_all(&receiver);
    let _ = std::fs::remove_dir_all(&sender);
}

/// Control 4: an honest import still works. The receiver first imports the sender's history, which
/// records the sender's author key; the sender then commits again, and the second import carries the
/// same key id with the same key — no conflict — and succeeds and verifies.
#[test]
fn an_import_carrying_an_already_recorded_key_still_imports_and_verifies() {
    let sender = installation("import-honest-sender", "one.txt", None);
    let receiver = support::unique_repo("import-honest-receiver");
    support::ok(&run(&receiver, &["init"]), "init the receiver");
    let export = |name: &str| -> PathBuf {
        let bundle = sender.join(name);
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
        bundle
    };

    let first = export("first.bundle");
    support::ok(
        &run(
            &receiver,
            &["bundle", "import", "--input", first.to_str().unwrap()],
        ),
        "first import",
    );

    std::fs::write(sender.join("two.txt"), "two\n").unwrap();
    support::ok(&run(&sender, &["commit", "-m", "two"]), "commit two");
    support::ok(&run(&sender, &["seal", "--allow-no-audit"]), "seal two");
    let second = export("second.bundle");
    let imported = run(
        &receiver,
        &["bundle", "import", "--input", second.to_str().unwrap()],
    );
    support::ok(&imported, "second import, same author key");

    // Trust the sender's sealing key so `verify` can check the received history, then verify.
    let status = text(&run(&sender, &["key", "status", "--role", "maintainer"]));
    let maintainer_id = status
        .lines()
        .find_map(|line| line.strip_prefix("key id: "))
        .and_then(|rest| rest.split(' ').next())
        .expect("key status prints the key id")
        .to_string();
    let public = text(&run(&sender, &["key", "public", "--role", "maintainer"]));
    let hex = public
        .lines()
        .find_map(|line| line.strip_prefix("public key: "))
        .expect("key public prints the key")
        .trim()
        .to_string();
    support::ok(
        &run(
            &receiver,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                &maintainer_id,
                "--public-key",
                &hex,
            ],
        ),
        "trust the sender's maintainer key",
    );
    let verified = run(&receiver, &["verify"]);
    support::ok(&verified, "verify the imported history");
    let _ = std::fs::remove_dir_all(&receiver);
    let _ = std::fs::remove_dir_all(&sender);
}
