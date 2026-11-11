//! 0.57.0 U4 (Unicode repository paths), through the compiled binary: what a bundle or archive's declared repository
//! format means to the repository that receives it.
//!
//! An artifact declares its **sending repository's** format (U0 ruling Q1). The rules (`prikk-store`'s
//! `artifact_format`), as a table -- what `import` does:
//!
//! | declared | receiver 6 | receiver 7 | receiver 8 |
//! |---|---|---|---|
//! | 6 or 7 | imports | imports | imports |
//! | 8 | **refused**, names `format upgrade`, nothing written | **refused**, the same | imports |
//! | a number this build does not know (a bundle's is new in U4) | refused, naming the number | refused | refused |
//!
//! `verify` has no receiver, so it applies only the last row. `bundle preview` has one and agrees with `import`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};
use std::process::Output;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn set_marker(repo: &Path, number: u8) {
    std::fs::write(repo.join(".prikk").join("FORMAT"), [b'0' + number, b'\n']).unwrap();
}

fn temp_file(tag: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!("u4-{tag}-{}.{extension}", support::unique_suffix()))
}

/// Every file under `.prikk` except lock files: what "nothing was written" is compared against.
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

/// A source repository at `format` with one sealed generation on `heads/main`, and its bundle and archive.
fn artifacts_from(tag: &str, format: u8) -> (PathBuf, PathBuf) {
    let source = support::unique_repo(tag);
    support::init(&source);
    support::generation(&source, "heads/main", "a.txt", b"hello\n", "genesis");
    set_marker(&source, format);
    let bundle = temp_file(tag, "pbndl");
    let export = support::prikk(&source)
        .args(["bundle", "export", "--ref", "heads/main", "--output"])
        .arg(&bundle)
        .output()
        .unwrap();
    support::ok(&export, "bundle export");
    assert!(
        text(&export).contains(&format!("repository format: {format}")),
        "the bundle declares its repository's format\n{}",
        text(&export)
    );
    let archive = temp_file(tag, "parc");
    let export = support::prikk(&source)
        .args(["archive", "export"])
        .arg(&archive)
        .output()
        .unwrap();
    support::ok(&export, "archive export");
    let _ = std::fs::remove_dir_all(&source);
    (bundle, archive)
}

fn receiver(tag: &str, format: u8) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    set_marker(&repo, format);
    repo
}

fn import_bundle(repo: &Path, bundle: &Path) -> Output {
    support::prikk(repo)
        .args(["bundle", "import", "--input"])
        .arg(bundle)
        .output()
        .unwrap()
}

fn preview_bundle(repo: &Path, bundle: &Path) -> Output {
    support::prikk(repo)
        .args(["bundle", "preview", "--input"])
        .arg(bundle)
        .output()
        .unwrap()
}

fn import_archive(repo: &Path, archive: &Path) -> Output {
    support::prikk(repo)
        .args(["archive", "import"])
        .arg(archive)
        .output()
        .unwrap()
}

fn assert_refused_naming_the_upgrade(output: &Output, noun: &str, receiver_format: u8) {
    let said = text(output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains(&format!("this {noun} declares repository format 8")),
        "{said}"
    );
    assert!(
        said.contains(&format!("this repository is format {receiver_format}")),
        "{said}"
    );
    assert!(
        said.contains("run `prikk format upgrade`") || said.contains("Run `prikk format upgrade`"),
        "{said}"
    );
    assert!(
        said.contains("a prikk older than this one cannot open a format-8 repository"),
        "the cost is named: {said}"
    );
    assert!(said.contains("nothing was written"), "{said}");
}

#[test]
fn a_format_8_bundle_is_refused_by_a_format_6_or_7_repository_before_any_write() {
    let (bundle, _archive) = artifacts_from("u4-bundle-8", 8);
    for receiver_format in [6u8, 7] {
        let repo = receiver("u4-bundle-8-into-older", receiver_format);
        let before = store_bytes(&repo);
        let output = import_bundle(&repo, &bundle);
        assert_refused_naming_the_upgrade(&output, "bundle", receiver_format);
        assert_eq!(
            store_bytes(&repo),
            before,
            "a refused import writes nothing"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn bundle_preview_agrees_with_import() {
    let (bundle, _archive) = artifacts_from("u4-preview-8", 8);
    for receiver_format in [6u8, 7] {
        let repo = receiver("u4-preview-8-into-older", receiver_format);
        let before = store_bytes(&repo);
        assert_refused_naming_the_upgrade(
            &preview_bundle(&repo, &bundle),
            "bundle",
            receiver_format,
        );
        assert_eq!(store_bytes(&repo), before);
        let _ = std::fs::remove_dir_all(&repo);
    }
    let repo = receiver("u4-preview-8-into-8", 8);
    support::ok(
        &preview_bundle(&repo, &bundle),
        "bundle preview into format 8",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_format_8_archive_is_refused_by_a_format_6_or_7_repository_before_any_write() {
    let (_bundle, archive) = artifacts_from("u4-archive-8", 8);
    for receiver_format in [6u8, 7] {
        let repo = receiver("u4-archive-8-into-older", receiver_format);
        let before = store_bytes(&repo);
        let output = import_archive(&repo, &archive);
        assert_refused_naming_the_upgrade(&output, "archive", receiver_format);
        assert_eq!(
            store_bytes(&repo),
            before,
            "a refused import writes nothing, journal included"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn the_suggestion_works_upgrade_then_import() {
    let (bundle, archive) = artifacts_from("u4-follow", 8);
    for receiver_format in [6u8, 7] {
        let repo = receiver("u4-follow-bundle", receiver_format);
        assert_eq!(import_bundle(&repo, &bundle).status.code(), Some(1));
        support::ok(
            &support::prikk(&repo)
                .args(["format", "upgrade"])
                .output()
                .unwrap(),
            "format upgrade",
        );
        support::ok(
            &import_bundle(&repo, &bundle),
            "bundle import after the upgrade",
        );
        let _ = std::fs::remove_dir_all(&repo);

        let repo = receiver("u4-follow-archive", receiver_format);
        assert_eq!(import_archive(&repo, &archive).status.code(), Some(1));
        support::ok(
            &support::prikk(&repo)
                .args(["format", "upgrade"])
                .output()
                .unwrap(),
            "format upgrade",
        );
        support::ok(
            &import_archive(&repo, &archive),
            "archive import after the upgrade",
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn a_format_8_repository_receives_a_format_8_artifact() {
    let (bundle, archive) = artifacts_from("u4-8-into-8", 8);
    let repo = receiver("u4-8-into-8-bundle", 8);
    support::ok(
        &import_bundle(&repo, &bundle),
        "bundle import into format 8",
    );
    let _ = std::fs::remove_dir_all(&repo);
    let repo = receiver("u4-8-into-8-archive", 8);
    support::ok(
        &import_archive(&repo, &archive),
        "archive import into format 8",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_format_6_or_7_artifact_is_received_by_every_format() {
    for declared in [6u8, 7] {
        let (bundle, archive) = artifacts_from(&format!("u4-{declared}-into-all"), declared);
        for receiver_format in [6u8, 7, 8] {
            let repo = receiver("u4-older-artifact-bundle", receiver_format);
            support::ok(
                &import_bundle(&repo, &bundle),
                &format!("bundle declared {declared} into format {receiver_format}"),
            );
            let _ = std::fs::remove_dir_all(&repo);
            let repo = receiver("u4-older-artifact-archive", receiver_format);
            support::ok(
                &import_archive(&repo, &archive),
                &format!("archive declared {declared} into format {receiver_format}"),
            );
            let _ = std::fs::remove_dir_all(&repo);
        }
    }
}

/// `verify` has no receiving repository, so a format-8 artifact verifies anywhere.
#[test]
fn verify_needs_no_receiver_and_passes() {
    let (bundle, archive) = artifacts_from("u4-verify-8", 8);
    let nowhere = support::unique_repo("u4-verify-nowhere");
    support::ok(
        &support::prikk(&nowhere)
            .args(["bundle", "verify", "--input"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle verify",
    );
    support::ok(
        &support::prikk(&nowhere)
            .args(["archive", "verify"])
            .arg(&archive)
            .output()
            .unwrap(),
        "archive verify",
    );
    let _ = std::fs::remove_dir_all(&nowhere);
}

/// The bundle's declared number was informational: any value passed. Now one this build does not know is refused,
/// by `verify`, `preview` and `import` alike, whatever the receiver.
#[test]
fn a_bundle_declaring_an_unknown_format_is_refused_by_verify_preview_and_import() {
    let (bundle, _archive) = artifacts_from("u4-bundle-9", 8);
    let version = {
        let nowhere = support::unique_repo("u4-bundle-9-version");
        let verify = support::prikk(&nowhere)
            .args(["bundle", "verify", "--input"])
            .arg(&bundle)
            .output()
            .unwrap();
        let said = text(&verify);
        let at = said.find("tool version: ").expect("tool version line") + "tool version: ".len();
        said[at..].lines().next().unwrap().trim().to_string()
    };
    let mut bytes = std::fs::read(&bundle).unwrap();
    let mut pattern = 8u64.to_be_bytes().to_vec();
    pattern.extend_from_slice(&(version.len() as u64).to_be_bytes());
    pattern.extend_from_slice(version.as_bytes());
    let at = bytes
        .windows(pattern.len())
        .rposition(|window| window == pattern.as_slice())
        .expect("the manifest's format field");
    bytes[at..at + 8].copy_from_slice(&9u64.to_be_bytes());
    let tampered = temp_file("u4-bundle-9-tampered", "pbndl");
    std::fs::write(&tampered, &bytes).unwrap();

    let nowhere = support::unique_repo("u4-bundle-9-nowhere");
    let verify = support::prikk(&nowhere)
        .args(["bundle", "verify", "--input"])
        .arg(&tampered)
        .output()
        .unwrap();
    let said = text(&verify);
    assert_eq!(verify.status.code(), Some(1), "verify: {said}");
    assert!(
        said.contains(
            "bundle declares repository format 9, which this build does not know how to read"
        ),
        "verify: {said}"
    );
    for receiver_format in [6u8, 7, 8] {
        let repo = receiver("u4-bundle-9-receiver", receiver_format);
        let before = store_bytes(&repo);
        for (what, output) in [
            ("preview", preview_bundle(&repo, &tampered)),
            ("import", import_bundle(&repo, &tampered)),
        ] {
            let said = text(&output);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{what} into {receiver_format}: {said}"
            );
            assert!(
                said.contains("bundle declares repository format 9, which this build does not know how to read"),
                "{what} into {receiver_format}: {said}"
            );
        }
        assert_eq!(store_bytes(&repo), before);
        let _ = std::fs::remove_dir_all(&repo);
    }
}
