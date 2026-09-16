//! DC-78 v2, through the compiled binary: a history that deletes a text file it edited earlier can be
//! exported, synced, received and rolled back.
//!
//! A text file whose content only ever arrived as `EditText` spans has no stored content Blob (DC-65).
//! When it is deleted, the deletion's preimage names a Blob id nothing holds, and three things used to
//! fail: `bundle export`, `sync build`, and sealing the inverse of that deletion. The exporters now
//! derive the content by replay and carry it; the seal derives and stores it.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

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

fn commit_and_seal(repo: &Path, message: &str) {
    support::ok(&support::commit(repo, "heads/main", message), message);
    support::ok(&support::seal(repo, "heads/main"), "seal");
}

/// `notes.txt` created, then edited (so its current content is an `EditText` result nothing stored),
/// then deleted. Three blocks; the deletion's preimage names a Blob the store lacks.
fn edited_then_deleted_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("notes.txt"), b"line one\nline two\n").unwrap();
    commit_and_seal(&repo, "create notes.txt");
    std::fs::write(repo.join("notes.txt"), b"line one\nline two, edited\n").unwrap();
    commit_and_seal(&repo, "edit notes.txt");
    std::fs::remove_file(repo.join("notes.txt")).unwrap();
    commit_and_seal(&repo, "delete notes.txt");
    repo
}

/// Controls 1 and 3: the export carries the derived preimage, a fresh repository imports and verifies
/// it, and `show` of the deletion renders the content the export derived.
#[test]
fn an_edited_then_deleted_file_exports_imports_and_verifies() {
    let repo = edited_then_deleted_repo("dc78v2-export");
    let bundle = repo.join("history.bundle");
    let export = run(
        &repo,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );
    support::ok(&export, "bundle export after deleting an edited file");

    let received = support::unique_repo("dc78v2-export-received");
    support::init(&received);
    support::trust_maintainer(&received);
    support::ok(
        &run(
            &received,
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        ),
        "bundle import",
    );
    support::ok(&run(&received, &["verify"]), "verify after import");

    // The deletion's preimage content travelled: `show` renders it instead of `<unavailable blob …>`.
    let tip = stdout(&run(&received, &["log", "--ref", "remotes/heads/main"]))
        .lines()
        .find_map(|line| line.strip_prefix("block ").map(str::to_owned))
        .expect("log names the tip block");
    let shown = stdout(&run(
        &received,
        &["show", tip.split_whitespace().next().unwrap()],
    ));
    assert!(
        shown.contains("line two, edited") && !shown.contains("unavailable blob"),
        "the received deletion must render its content: {shown}"
    );
    for dir in [repo, received] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Controls 2 and 3: `sync build` across a have-list from before the edit carries it too, and the
/// receiver accepts the artifact.
#[test]
fn sync_build_carries_a_derived_preimage_and_accept_takes_it() {
    let sender = edited_then_deleted_repo("dc78v2-sync-sender");
    let receiver = support::unique_repo("dc78v2-sync-receiver");
    support::init(&receiver);
    support::trust_maintainer(&receiver);

    let have = sender.join("receiver.have");
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
    let artifact = sender.join("delta.artifact");
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
        "sync build after deleting an edited file",
    );
    support::ok(
        &run(&receiver, &["sync", "accept", artifact.to_str().unwrap()]),
        "sync accept",
    );
    support::ok(&run(&receiver, &["verify"]), "verify after accept");
    for dir in [sender, receiver] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Control 5: a preimage the store does hold still travels unchanged — here a binary file, whose
/// content Blob every write stores.
#[test]
fn a_stored_preimage_still_travels() {
    let repo = support::unique_repo("dc78v2-stored-preimage");
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("image.bin"), [0_u8, 159, 146, 150, 0, 255]).unwrap();
    commit_and_seal(&repo, "create image.bin");
    std::fs::remove_file(repo.join("image.bin")).unwrap();
    commit_and_seal(&repo, "delete image.bin");

    let bundle = repo.join("binary.bundle");
    support::ok(
        &run(
            &repo,
            &[
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle.to_str().unwrap(),
            ],
        ),
        "bundle export of a stored binary preimage",
    );
    let received = support::unique_repo("dc78v2-stored-preimage-received");
    support::init(&received);
    support::trust_maintainer(&received);
    support::ok(
        &run(
            &received,
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        ),
        "bundle import",
    );
    support::ok(&run(&received, &["verify"]), "verify after import");
    for dir in [repo, received] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Control 6: the inverse of such a deletion seals, where it used to fail with
/// `blob … required for a state effect is missing`, and the sealed inverse carries the derived content.
///
/// `rollback-draft --append-inverse` inverts the **ref's history**, not only its last block: here that is
/// create-file, edit-text and delete-node, so the worktree correctly ends empty. What this control is
/// about is that the seal succeeds at all and that the content it needed is in the store afterwards.
#[test]
fn the_inverse_of_such_a_deletion_seals() {
    let repo = edited_then_deleted_repo("dc78v2-rollback");
    support::ok(
        &run(
            &repo,
            &[
                "rollback-draft",
                "--append-inverse",
                "-m",
                "undo the delete",
            ],
        ),
        "rollback-draft --append-inverse",
    );
    support::ok(
        &support::seal(&repo, "heads/main"),
        "seal the appended inverse",
    );
    support::ok(&run(&repo, &["verify"]), "verify after the rollback");

    // The inverse's create-file names the content the deletion removed; the seal derived and stored it,
    // so `show` renders the bytes instead of `<unavailable blob …>`.
    let tip = stdout(&run(&repo, &["log", "--ref", "heads/main", "--limit", "1"]))
        .lines()
        .find_map(|line| line.strip_prefix("block ").map(str::to_owned))
        .expect("log names the tip block");
    let shown = stdout(&run(
        &repo,
        &["show", tip.split_whitespace().next().unwrap()],
    ));
    assert!(
        shown.contains("operation 1: create-file") && shown.contains("line two, edited"),
        "the sealed inverse must carry the derived content: {shown}"
    );
    assert!(
        !shown.contains("unavailable blob"),
        "nothing in the sealed inverse may be unavailable: {shown}"
    );

    support::ok(
        &run(
            &repo,
            &["checkout", "--patch-materialize", "--ref", "heads/main"],
        ),
        "materialize after the rollback",
    );
    assert!(
        !repo.join("notes.txt").exists(),
        "the whole-history inverse ends with the file deleted again"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
