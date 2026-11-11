//! 0.57.0 U4b (review `0.57.0-U4-review-v1`, ruling Q3): under `--format json`, `bundle import`, `bundle preview` and
//! `archive import` print a `refusal-v1` document for the two refusals U4 added, as they do for their other classified
//! refusals (0.56.0 A4/3c2). An empty stdout for an actionable refusal leaves a tool nothing to act on.
//!
//! | the refusal | `refusal` id | on |
//! |---|---|---|
//! | a format-8 artifact into a format-6 or 7 repository | `repository-format-too-old` | `bundle import`, `bundle preview`, `archive import` |
//! | an artifact declaring a format number this build does not know | `outside-version-rule` | the same three |
//!
//! The second is **not damage**: the id says so (`outside-version-rule` is "a different version of prikk wrote it"), and the
//! sentence must say so too -- the archive's old sentence began `malformed persisted data:`, which read as damage while the
//! id could not (the 0.56.0 lesson: measure the JSON id, not only the sentence). Every document's `message` is exactly
//! stderr's text without its `error: ` prefix; stdout holds that one document and nothing else; the exit code is `1`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::u4_artifact_format::{
    archive_declaring, artifacts_from, bundle_declaring, receiver, store_bytes, text,
};
use crate::support;
use std::path::Path;
use std::process::Output;

fn json_command(repo: &Path, args: &[&str], artifact: &Path, flag_input: bool) -> Output {
    let mut command = support::prikk(repo);
    command.args(args);
    if flag_input {
        command.arg("--input");
    }
    command
        .arg(artifact)
        .args(["--format", "json"])
        .output()
        .unwrap()
}

fn bundle_import(repo: &Path, artifact: &Path) -> Output {
    json_command(repo, &["bundle", "import"], artifact, true)
}

fn bundle_preview(repo: &Path, artifact: &Path) -> Output {
    json_command(repo, &["bundle", "preview"], artifact, true)
}

fn archive_import(repo: &Path, artifact: &Path) -> Output {
    json_command(repo, &["archive", "import"], artifact, false)
}

fn stderr_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("error: "))
        .unwrap_or_else(|| panic!("an `error: ` line on stderr:\n{stderr}"))
        .to_string()
}

/// Assert `output` is a refusal that prints exactly one `refusal-v1` document: `command`, `id`, `"reason": null`, and a
/// message equal to stderr's. Returns the message.
fn assert_refusal_document(output: &Output, command: &str, id: &str) -> String {
    let said = text(output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    let message = stderr_message(output);
    assert!(
        !message.contains('"') && !message.contains('\\') && !message.contains('\n'),
        "the message needs no JSON escaping, so the comparison below is exact: {message}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let expected = format!(
        "{{\"schema_version\": \"refusal-v1\", \"command\": \"{command}\", \"refusal\": \"{id}\", \"reason\": null, \"message\": \"{message}\"}}"
    );
    assert_eq!(
        stdout.trim_end(),
        expected,
        "stdout is exactly the one refusal-v1 document ({command}, {id})"
    );
    message
}

#[test]
fn a_receiver_older_than_the_artifact_gets_repository_format_too_old_from_all_three_commands() {
    let (bundle, archive) = artifacts_from("u4b-too-old", 8);
    for receiver_format in [6u8, 7] {
        let repo = receiver("u4b-too-old-receiver", receiver_format);
        let before = store_bytes(&repo);
        for (command, artifact_name, output) in [
            ("bundle-import", "bundle", bundle_import(&repo, &bundle)),
            ("bundle-preview", "bundle", bundle_preview(&repo, &bundle)),
            ("archive-import", "archive", archive_import(&repo, &archive)),
        ] {
            let message = assert_refusal_document(&output, command, "repository-format-too-old");
            assert!(
                message.starts_with(&format!(
                    "precondition not met: this {artifact_name} declares repository format 8, and this repository is format {receiver_format}"
                )),
                "{command}: {message}"
            );
            assert!(
                message.contains("prikk format upgrade"),
                "{command}: {message}"
            );
        }
        assert_eq!(store_bytes(&repo), before, "nothing was written");
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn a_format_8_receiver_prints_no_refusal_and_the_import_report_is_unchanged() {
    let (bundle, archive) = artifacts_from("u4b-no-refusal", 8);
    let repo = receiver("u4b-no-refusal-bundle", 8);
    let output = bundle_import(&repo, &bundle);
    support::ok(&output, "bundle import --format json");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("bundle-import-report-v1"),
        "{}",
        text(&output)
    );
    let repo2 = receiver("u4b-no-refusal-archive", 8);
    let output = archive_import(&repo2, &archive);
    support::ok(&output, "archive import --format json");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("prepo-import-report-v1"),
        "{}",
        text(&output)
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&repo2);
}

#[test]
fn an_unknown_declared_format_gets_outside_version_rule_and_the_sentence_says_it_is_not_damage() {
    let (bundle, archive) = artifacts_from("u4b-unknown", 8);
    let unknown_bundle = bundle_declaring(&bundle, 9);
    let unknown_archive = archive_declaring(&archive, 9);
    for receiver_format in [6u8, 7, 8] {
        let repo = receiver("u4b-unknown-receiver", receiver_format);
        let before = store_bytes(&repo);
        for (command, artifact_name, output) in [
            (
                "bundle-import",
                "bundle",
                bundle_import(&repo, &unknown_bundle),
            ),
            (
                "bundle-preview",
                "bundle",
                bundle_preview(&repo, &unknown_bundle),
            ),
            (
                "archive-import",
                "archive",
                archive_import(&repo, &unknown_archive),
            ),
        ] {
            let message = assert_refusal_document(&output, command, "outside-version-rule");
            assert!(
                message.starts_with(&format!(
                    "{artifact_name} declares repository format 9, which this build does not know how to read;"
                )),
                "{command} into {receiver_format}: {message}"
            );
            assert!(
                message.contains("outside what this version of prikk accepts"),
                "{command}: {message}"
            );
            assert!(message.contains("not damage"), "{command}: {message}");
            assert!(
                !message.contains("malformed"),
                "the sentence must agree with the id that this is not damage: {message}"
            );
        }
        assert_eq!(store_bytes(&repo), before, "nothing was written");
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// `verify` has no `refusal-v1` document (it never had one), but its sentence is the same one, so it does not read as
/// damage either.
#[test]
fn verify_says_the_same_sentence_for_an_unknown_declared_format() {
    let (bundle, archive) = artifacts_from("u4b-verify-unknown", 8);
    let nowhere = support::unique_repo("u4b-verify-unknown-nowhere");
    let bundle_said = text(
        &support::prikk(&nowhere)
            .args(["bundle", "verify", "--input"])
            .arg(bundle_declaring(&bundle, 9))
            .output()
            .unwrap(),
    );
    let archive_said = text(
        &support::prikk(&nowhere)
            .args(["archive", "verify"])
            .arg(archive_declaring(&archive, 9))
            .output()
            .unwrap(),
    );
    for (noun, said) in [("bundle", bundle_said), ("archive", archive_said)] {
        assert!(
            said.contains(&format!(
                "{noun} declares repository format 9, which this build does not know how to read;"
            )),
            "{said}"
        );
        assert!(said.contains("not damage"), "{said}");
        assert!(!said.contains("malformed"), "{said}");
    }
    let _ = std::fs::remove_dir_all(&nowhere);
}
