//! 0.57.0 U2 (U4 review, Q2): **the content half** of "a format-6 or 7 repository refuses non-ASCII". U4's rule refuses an
//! artifact that *declares* format 8. This one refuses an artifact that *carries* a non-ASCII path whatever it declares: the
//! corner is an artifact a format-7 repository exported after the 0.55.0 silent-admit hole let a non-ASCII path in, which
//! declares 7 and carries it. Same refusal shape (a precondition naming the path and `prikk format upgrade`, nothing
//! written, `refusal-v1` id `repository-format-too-old`), asked by every importer that has a receiving repository.
//!
//! | artifact carries a non-ASCII path, declares | receiver 6 or 7 | receiver 8 | `verify` (no receiver) |
//! |---|---|---|---|
//! | 8 (a real format-8 export) | refused (U4's rule: the declaration) | imports | passes |
//! | 7 (the hole's export, built by editing the declaration) | **refused (the content rule)** | imports | passes |
//!
//! Driven through `bundle import`, `bundle preview` and `archive import`; `sync accept` is covered in `prikk-store`'s own
//! test (`patch_exchange`), because a `sync` artifact needs its own fixture.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::u4_artifact_format::{
    archive_declaring, bundle_declaring, import_archive, import_bundle, preview_bundle, receiver,
    store_bytes, temp_file, text,
};
use crate::support;
use std::path::PathBuf;

const NON_ASCII: &str = "pages/\u{65e5}\u{672c}\u{8a9e}.md";

/// A format-8 repository with one sealed generation that adds [`NON_ASCII`], and its bundle and archive.
fn unicode_artifacts(tag: &str) -> (PathBuf, PathBuf) {
    let source = support::unique_repo(tag);
    support::init(&source);
    std::fs::create_dir_all(source.join("pages")).unwrap();
    support::generation(&source, "heads/main", NON_ASCII, b"hello\n", "genesis");
    let bundle = temp_file(tag, "pbndl");
    support::ok(
        &support::prikk(&source)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    let archive = temp_file(tag, "parc");
    support::ok(
        &support::prikk(&source)
            .args(["archive", "export"])
            .arg(&archive)
            .output()
            .unwrap(),
        "archive export",
    );
    let _ = std::fs::remove_dir_all(&source);
    (bundle, archive)
}

fn assert_content_refusal(output: &std::process::Output, noun: &str, receiver_format: u8) {
    let said = text(output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains(&format!(
            "this {noun} carries the non-ASCII path {NON_ASCII}"
        )),
        "{said}"
    );
    assert!(
        said.contains(&format!("this repository is format {receiver_format}")),
        "{said}"
    );
    assert!(said.contains("prikk format upgrade"), "{said}");
    assert!(said.contains("nothing was written"), "{said}");
}

#[test]
fn a_bundle_carrying_a_non_ascii_path_is_refused_by_a_format_6_or_7_repository_whatever_it_declares()
 {
    let (bundle8, _archive) = unicode_artifacts("u2-bundle-content");
    let declares_7 = bundle_declaring(&bundle8, 7);
    for receiver_format in [6u8, 7] {
        let repo = receiver("u2-bundle-content-receiver", receiver_format);
        let before = store_bytes(&repo);
        assert_content_refusal(
            &import_bundle(&repo, &declares_7),
            "bundle",
            receiver_format,
        );
        assert_content_refusal(
            &preview_bundle(&repo, &declares_7),
            "bundle",
            receiver_format,
        );
        assert_eq!(
            store_bytes(&repo),
            before,
            "a refused import writes nothing"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
    // Receiver 8: it imports, and the path reads back.
    let repo = receiver("u2-bundle-content-into-8", 8);
    support::ok(
        &import_bundle(&repo, &declares_7),
        "bundle import into format 8",
    );
    // Verify has no receiver: it passes for either declaration.
    let nowhere = support::unique_repo("u2-bundle-content-nowhere");
    for bundle in [&bundle8, &declares_7] {
        support::ok(
            &support::prikk(&nowhere)
                .args(["bundle", "verify", "--input"])
                .arg(bundle)
                .output()
                .unwrap(),
            "bundle verify",
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&nowhere);
}

#[test]
fn an_archive_carrying_a_non_ascii_path_is_refused_by_a_format_6_or_7_repository_whatever_it_declares()
 {
    let (_bundle, archive8) = unicode_artifacts("u2-archive-content");
    let declares_7 = archive_declaring(&archive8, 7);
    for receiver_format in [6u8, 7] {
        let repo = receiver("u2-archive-content-receiver", receiver_format);
        let before = store_bytes(&repo);
        assert_content_refusal(
            &import_archive(&repo, &declares_7),
            "archive",
            receiver_format,
        );
        assert_eq!(
            store_bytes(&repo),
            before,
            "a refused import writes nothing, journal included"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
    let repo = receiver("u2-archive-content-into-8", 8);
    support::ok(
        &import_archive(&repo, &declares_7),
        "archive import into format 8",
    );
    let nowhere = support::unique_repo("u2-archive-content-nowhere");
    for archive in [&archive8, &declares_7] {
        support::ok(
            &support::prikk(&nowhere)
                .args(["archive", "verify"])
                .arg(archive)
                .output()
                .unwrap(),
            "archive verify",
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&nowhere);
}

#[test]
fn the_content_refusal_prints_repository_format_too_old_under_json() {
    let (bundle8, archive8) = unicode_artifacts("u2-content-json");
    let bundle7 = bundle_declaring(&bundle8, 7);
    let archive7 = archive_declaring(&archive8, 7);
    let repo = receiver("u2-content-json-receiver", 7);
    for (command, output) in [
        (
            "bundle-import",
            support::prikk(&repo)
                .args(["bundle", "import", "--format", "json", "--input"])
                .arg(&bundle7)
                .output()
                .unwrap(),
        ),
        (
            "bundle-preview",
            support::prikk(&repo)
                .args(["bundle", "preview", "--format", "json", "--input"])
                .arg(&bundle7)
                .output()
                .unwrap(),
        ),
        (
            "archive-import",
            support::prikk(&repo)
                .args(["archive", "import", "--format", "json"])
                .arg(&archive7)
                .output()
                .unwrap(),
        ),
    ] {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            stdout.contains(&format!(
                "\"command\": \"{command}\", \"refusal\": \"repository-format-too-old\""
            )),
            "{command}: {stdout}"
        );
        assert_eq!(output.status.code(), Some(1), "{command}");
    }
    let _ = std::fs::remove_dir_all(&repo);
}
