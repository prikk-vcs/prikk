//! 0.57.0 step 3, part S4 (stikk 029 §4.5): `cat --output` applies the recorded mode, as `extract` does, and
//! `cat`'s refusals get `refusal-v1` under `--format json`.
//!
//! The mode comes through `extract`'s own function (`durable_output::set_file_mode`): only the two modes prikk
//! records (0644 and 0755) are ever applied; a setuid, setgid or sticky bit is never taken from content. On
//! Windows there is no observable POSIX mode and nothing is applied, exactly as `extract` behaves.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support;
use crate::support::json::Value;

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn text_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A repository with `plain.txt` (0644) and `tool.sh` (0755), both sealed.
fn repo_with_both_modes(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("plain.txt"), b"plain\n").unwrap();
    std::fs::write(repo.join("tool.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            repo.join("plain.txt"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::set_permissions(repo.join("tool.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    support::ok(
        &support::commit(&repo, "heads/main", "both modes"),
        "commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

fn out_path(tag: &str) -> PathBuf {
    let dir = support::unique_repo(&format!("s4-out-{tag}"));
    dir.join("out.bin")
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// 0755 stays 0755 and 0644 stays 0644, written to a new file.
#[cfg(unix)]
#[test]
fn the_recorded_mode_is_applied_to_a_new_output_file() {
    let repo = repo_with_both_modes("s4-mode-new");
    for (path, expected) in [("tool.sh", 0o755), ("plain.txt", 0o644)] {
        let out = out_path(&format!("new-{expected:o}"));
        support::ok(
            &run(
                &repo,
                &["cat", "--path", path, "--output", out.to_str().unwrap()],
            ),
            "cat --output",
        );
        assert_eq!(mode_of(&out), expected, "{path}");
        assert_eq!(
            std::fs::read(&out).unwrap(),
            std::fs::read(repo.join(path)).unwrap()
        );
    }
}

/// With `--force` over an existing file the mode is the recorded one too: not the old file's mode, in either
/// direction (0600 becomes 0755; 0755 becomes 0644).
#[cfg(unix)]
#[test]
fn force_over_an_existing_file_applies_the_recorded_mode() {
    use std::os::unix::fs::PermissionsExt;
    let repo = repo_with_both_modes("s4-mode-force");
    for (path, old_mode, expected) in [("tool.sh", 0o600, 0o755), ("plain.txt", 0o755, 0o644)] {
        let out = out_path(&format!("force-{expected:o}"));
        std::fs::write(&out, b"old content that must be replaced").unwrap();
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(old_mode)).unwrap();
        // Without --force it refuses and changes nothing.
        let refused = run(
            &repo,
            &["cat", "--path", path, "--output", out.to_str().unwrap()],
        );
        assert_eq!(refused.status.code(), Some(1));
        assert_eq!(mode_of(&out), old_mode, "a refusal leaves the old mode");
        support::ok(
            &run(
                &repo,
                &[
                    "cat",
                    "--path",
                    path,
                    "--output",
                    out.to_str().unwrap(),
                    "--force",
                ],
            ),
            "cat --output --force",
        );
        assert_eq!(mode_of(&out), expected, "{path} over a {old_mode:o} file");
        assert_eq!(
            std::fs::read(&out).unwrap(),
            std::fs::read(repo.join(path)).unwrap()
        );
    }
}

/// Where there is no POSIX mode (Windows), `cat --output` writes the content and applies nothing: the same
/// behaviour `extract` has there.
#[cfg(not(unix))]
#[test]
fn without_a_posix_mode_the_content_is_written_and_no_mode_is_applied() {
    let repo = repo_with_both_modes("s4-mode-nonunix");
    let out = out_path("nonunix");
    support::ok(
        &run(
            &repo,
            &[
                "cat",
                "--path",
                "tool.sh",
                "--output",
                out.to_str().unwrap(),
            ],
        ),
        "cat --output",
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        std::fs::read(repo.join("tool.sh")).unwrap()
    );
}

// ---- refusal-v1

/// One `refusal-v1` document on stdout and nothing else there; the exit code is 1; and stderr carries the
/// text that the document's `message` repeats.
fn assert_refusal(output: &Output, refusal: &str) -> Value {
    assert_eq!(output.status.code(), Some(1), "{}", text_of(output));
    let document = support::json::parse(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(document.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(document.get("command").as_str(), "cat");
    assert_eq!(document.get("refusal").as_str(), refusal, "{document:?}");
    assert!(document.get("reason").is_null());
    assert!(
        text_of(output).contains(document.get("message").as_str()),
        "the document carries the text stderr prints: {} / {}",
        text_of(output),
        document.get("message").as_str()
    );
    document
}

fn cat_json(repo: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["cat", "--format", "json"];
    args.extend_from_slice(extra);
    run(repo, &args)
}

#[test]
fn each_refusal_prints_its_own_id_and_the_text_is_unchanged() {
    let repo = repo_with_both_modes("s4-refusals");

    // A path that is not a file at the point, and a directory prefix (not a file either).
    assert_refusal(&cat_json(&repo, &["--path", "nope.txt"]), "path-not-found");
    std::fs::create_dir_all(repo.join("dir")).unwrap();
    std::fs::write(repo.join("dir/inner.txt"), b"x\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "dir"), "commit dir");
    support::ok(&support::seal(&repo, "heads/main"), "seal dir");
    assert_refusal(&cat_json(&repo, &["--path", "dir"]), "path-not-found");

    // A point that does not resolve: an absent ref, an unknown block id, an object that is not a block.
    assert_refusal(
        &cat_json(&repo, &["--path", "plain.txt", "--ref", "heads/nosuch"]),
        "point-not-found",
    );
    let absent = "a".repeat(64);
    assert_refusal(
        &cat_json(&repo, &["--path", "plain.txt", "--ref", &absent]),
        "point-not-found",
    );
    let log = run(&repo, &["log", "--format", "json"]);
    let log = support::json::parse(&String::from_utf8_lossy(&log.stdout));
    let patch_id = log.get("blocks").as_array()[0]
        .get("patch_messages")
        .as_array()[0]
        .get("patch_id")
        .as_str()
        .to_string();
    let not_a_block = assert_refusal(
        &cat_json(&repo, &["--path", "plain.txt", "--ref", &patch_id]),
        "point-not-found",
    );
    assert!(not_a_block.get("message").as_str().contains("not a block"));

    // --max-bytes.
    assert_refusal(
        &cat_json(&repo, &["--path", "plain.txt", "--max-bytes", "1"]),
        "over-max-bytes",
    );

    // --output: an existing file, and inside .prikk.
    let existing = out_path("existing");
    std::fs::write(&existing, b"already here").unwrap();
    assert_refusal(
        &cat_json(
            &repo,
            &[
                "--path",
                "plain.txt",
                "--output",
                existing.to_str().unwrap(),
            ],
        ),
        "destination-exists",
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"already here");
    let inside = repo.join(".prikk").join("cat-out");
    assert_refusal(
        &cat_json(
            &repo,
            &["--path", "plain.txt", "--output", inside.to_str().unwrap()],
        ),
        "destination-inside-repository",
    );
    assert!(!inside.exists());
}

#[test]
fn no_repository_and_an_unpublished_current_branch_are_refusals_too() {
    let nowhere = support::unique_repo("s4-no-repo");
    assert_refusal(
        &cat_json(&nowhere, &["--path", "f.txt"]),
        "not-a-repository",
    );
    let fresh = support::unique_repo("s4-fresh");
    support::init(&fresh);
    let document = assert_refusal(&cat_json(&fresh, &["--path", "f.txt"]), "path-not-found");
    assert!(
        document
            .get("message")
            .as_str()
            .contains("no published history yet")
    );
}

/// A damaged object is `damaged-repository`: a byte flipped in a patch container makes the replay fail.
#[test]
fn a_damaged_object_is_damaged_repository() {
    let repo = repo_with_both_modes("s4-damaged");
    let container = repo
        .join(".prikk")
        .join("containers")
        .join("patch")
        .join("a.container");
    let mut bytes = std::fs::read(&container).unwrap();
    let last = bytes.len() - 8;
    bytes[last] ^= 0xff;
    std::fs::write(&container, bytes).unwrap();
    let output = cat_json(&repo, &["--path", "plain.txt"]);
    assert_eq!(output.status.code(), Some(1), "{}", text_of(&output));
    let document = support::json::parse(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(document.get("refusal").as_str(), "damaged-repository");
}

/// A usage error is exit 2 and prints no document; a success prints `path-content-v1`, not a refusal; and a
/// text-mode refusal prints nothing on stdout (the document is `--format json`'s only).
#[test]
fn only_json_refusals_print_documents_and_usage_errors_print_none() {
    let repo = repo_with_both_modes("s4-boundaries");
    let usage = run(&repo, &["cat", "--format", "json", "--path", "."]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(usage.stdout.is_empty());

    let success = cat_json(&repo, &["--path", "plain.txt"]);
    support::ok(&success, "cat --format json");
    let document = support::json::parse(&String::from_utf8_lossy(&success.stdout));
    assert_eq!(document.get("schema_version").as_str(), "path-content-v1");

    let text_refusal = run(&repo, &["cat", "--path", "nope.txt"]);
    assert_eq!(text_refusal.status.code(), Some(1));
    assert!(
        text_refusal.stdout.is_empty(),
        "no document without --format json"
    );
}
