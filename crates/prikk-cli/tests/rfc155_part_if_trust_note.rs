//! RFC 155 Part I-F item 1: the publication-trust note names every *planned* entry, landed or
//! not -- Part I-E's own rule (review `rfc155-impl-IE-review-v1`) looked only at landed refs, but
//! a received pointer that loses Part I-C's own landing-collision rule (the exporter's own ref
//! wins the name) is never written as a ref, yet its own object is still imported by stage 1,
//! and the destination's own `prikk verify` checks it regardless of whether any ref names it. A
//! user who did nothing unusual could get a failing `verify` with no warning at all about why.
//!
//! The fixture: a source repository "a" whose own `heads/main` is signed by the fixed maintainer
//! key, which also `bundle import`s a bundle for `heads/main` signed by a *second*, distinct
//! maintainer key -- landing as a received pointer at `remotes/heads/main` in "a", the exact name
//! "a"'s own `heads/main` will also want to land at by default at any destination. The exported
//! archive therefore carries a genuine landing collision.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use support::ok;

const SECOND_MAINTAINER_KEY_ID: &str = "rfc155-if-second-maintainer";
const SECOND_MAINTAINER_SEED: [u8; 32] = [
    0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f, 0x90,
    0x01, 0x12, 0x23, 0x34, 0x45, 0x56, 0x67, 0x78, 0x89, 0x9a, 0xab, 0xbc, 0xcd, 0xde, 0xef, 0xf0,
];

fn second_maintainer_public_key_hex() -> String {
    use prikk_store::MaintainerSigner;
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed(
        SECOND_MAINTAINER_KEY_ID,
        &SECOND_MAINTAINER_SEED,
    )
    .expect("fixed second maintainer seed derives a valid signer");
    support::hex(&signer.public_key_bytes())
}

fn trust_second_maintainer(repo: &Path) {
    let _ = support::prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            SECOND_MAINTAINER_KEY_ID,
            "--public-key",
            &second_maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
}

fn seal_with_second_maintainer(repo: &Path, ref_name: &str) -> std::process::Output {
    trust_second_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", SECOND_MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&SECOND_MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit", "--ref", ref_name])
        .output()
        .unwrap()
}

/// A "sender" repository with one sealed commit on `heads/main`, signed by the *second*
/// maintainer key -- and its bundle already exported.
fn export_sender_bundle(tag: &str) -> PathBuf {
    let repo = support::unique_repo(&format!("rfc155-if-sender-{tag}"));
    support::init(&repo);
    std::fs::write(repo.join("s.txt"), b"the sender's own one file\n").unwrap();
    ok(
        &support::commit(&repo, "heads/main", "sender generation"),
        "sender commit",
    );
    ok(
        &seal_with_second_maintainer(&repo, "heads/main"),
        "sender seal (second maintainer)",
    );
    let mut bundle_path = std::env::temp_dir();
    bundle_path.push(format!(
        "prikk-cli-rfc155-if-sender-{tag}-{}.bundle",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&repo)
            .args(["bundle", "export", "--ref", "heads/main", "--output"])
            .arg(&bundle_path)
            .output()
            .unwrap(),
        "bundle export",
    );
    bundle_path
}

/// Source repository "a": its own `heads/main` (fixed maintainer key), plus the sender's bundle
/// `bundle import`ed for the same ref name, landing as a received pointer at
/// `remotes/heads/main` -- the exact collision shape external review 023 found. Exports the
/// archive.
fn export_colliding_archive(tag: &str) -> PathBuf {
    let repo = support::unique_repo(&format!("rfc155-if-source-{tag}"));
    support::init(&repo);
    support::generation(
        &repo,
        "heads/main",
        "a.txt",
        b"the exporter's own one file\n",
        "exporter generation",
    );
    let sender_bundle = export_sender_bundle(tag);
    ok(
        &support::prikk(&repo)
            .args(["bundle", "import", "--input"])
            .arg(&sender_bundle)
            .output()
            .unwrap(),
        "bundle import (second maintainer, untrusted at the source)",
    );
    let _ = std::fs::remove_file(&sender_bundle);

    let mut archive_path = std::env::temp_dir();
    archive_path.push(format!(
        "prikk-cli-rfc155-if-trust-note-{tag}-{}.prepo",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&repo)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );
    archive_path
}

struct ImportResult {
    text: String,
    json: String,
}

fn import_into_fresh_destination(
    archive_path: &Path,
    tag: &str,
    trust_first: bool,
    trust_second: bool,
    adopt: bool,
) -> ImportResult {
    let dest = support::unique_repo(&format!("rfc155-if-dest-{tag}"));
    support::init(&dest);
    if trust_first {
        support::trust_maintainer(&dest);
    }
    if trust_second {
        trust_second_maintainer(&dest);
    }
    let mut cmd = support::prikk(&dest);
    cmd.args(["archive", "import"]);
    if adopt {
        cmd.arg("--adopt");
    }
    let output = cmd.arg(archive_path).output().unwrap();
    assert!(
        output.status.success(),
        "import itself must succeed regardless of trust: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).to_string();

    let dest_json = support::unique_repo(&format!("rfc155-if-dest-json-{tag}"));
    support::init(&dest_json);
    if trust_first {
        support::trust_maintainer(&dest_json);
    }
    if trust_second {
        trust_second_maintainer(&dest_json);
    }
    let mut cmd_json = support::prikk(&dest_json);
    cmd_json.args(["archive", "import", "--format", "json"]);
    if adopt {
        cmd_json.arg("--adopt");
    }
    let output_json = cmd_json.arg(archive_path).output().unwrap();
    assert!(output_json.status.success(), "json import must succeed too");
    let json = String::from_utf8_lossy(&output_json.stdout).to_string();

    ImportResult { text, json }
}

/// Row 1: nothing trusted -- the note must name both the exporter's own ref and the collided
/// received pointer.
#[test]
fn row1_nothing_trusted_names_both() {
    let archive_path = export_colliding_archive("row1");
    let result = import_into_fresh_destination(&archive_path, "row1", false, false, false);
    assert!(
        result.text.contains("note:") && result.text.contains("heads/main"),
        "the note must name the exporter's own untrusted ref: {}",
        result.text
    );
    assert!(
        result.text.contains("remotes/heads/main"),
        "the note must also name the collided, not-landed received pointer: {}",
        result.text
    );
    assert!(
        result.text.contains("imported without a ref"),
        "the collided entry must be described as not landed: {}",
        result.text
    );
    assert!(
        !result.json.contains("\"note\": null"),
        "the json note must not be null: {}",
        result.json
    );
    assert!(
        result
            .text
            .contains("adopts the signing keys (or re-run with --adopt once they are)"),
        "two distinct keys are named, so the note must use the plural: {}",
        result.text
    );
    let _ = std::fs::remove_file(&archive_path);
}

/// Row 2 (the review's own case): only the exporter's own key is trusted, no `--adopt`. The
/// exporter's own ref lands fine (trusted, even though not adopted) and must NOT appear in the
/// note; the collided received pointer is still untrusted and must appear.
#[test]
fn row2_trusts_only_the_exporter_names_the_collided_pointer() {
    let archive_path = export_colliding_archive("row2");
    let result = import_into_fresh_destination(&archive_path, "row2", true, false, false);
    assert!(
        result.text.contains("note:"),
        "a note must still print, for the collided pointer: {}",
        result.text
    );
    assert!(
        result.text.contains("remotes/heads/main"),
        "the note must name the collided, untrusted received pointer: {}",
        result.text
    );
    // The exporter's own ref (trusted) must not be named in the note.
    let note_line = result
        .text
        .lines()
        .find(|line| line.starts_with("note:"))
        .expect("a note line must be present");
    assert!(
        !note_line.contains("heads/main landed"),
        "the trusted exporter's own ref must not appear in the note: {note_line}"
    );
    assert!(
        !result.json.contains("\"note\": null"),
        "the json note must not be null: {}",
        result.json
    );
    assert!(
        result.text.contains("adopts the signing key") && !result.text.contains("signing keys"),
        "only one distinct key is named, so the note must stay singular: {}",
        result.text
    );
    let _ = std::fs::remove_file(&archive_path);
}

/// Row 3: only the exporter's own key is trusted, with `--adopt` -- the exporter's own ref now
/// lands as a real local branch (never under `remotes/`); the collided received pointer is still
/// untrusted and must still be named.
#[test]
fn row3_trusts_only_the_exporter_with_adopt_names_the_received_pointer() {
    let archive_path = export_colliding_archive("row3");
    let result = import_into_fresh_destination(&archive_path, "row3", true, false, true);
    assert!(
        result.text.contains("adopted as a local branch"),
        "the exporter's own ref must adopt: {}",
        result.text
    );
    assert!(
        result.text.contains("note:") && result.text.contains("remotes/heads/main"),
        "the collided received pointer must still be named: {}",
        result.text
    );
    assert!(
        !result.json.contains("\"note\": null"),
        "the json note must not be null: {}",
        result.json
    );
    let _ = std::fs::remove_file(&archive_path);
}

/// Row 4: both keys trusted, `--adopt` used -- everything trusted, no note at all.
#[test]
fn row4_both_trusted_with_adopt_prints_no_note() {
    let archive_path = export_colliding_archive("row4");
    let result = import_into_fresh_destination(&archive_path, "row4", true, true, true);
    assert!(
        !result.text.contains("note:"),
        "no note must print once every signer is trusted: {}",
        result.text
    );
    assert!(
        result.json.contains("\"note\": null"),
        "the json note must be null: {}",
        result.json
    );
    let _ = std::fs::remove_file(&archive_path);
}

/// Row 5: both keys trusted, no `--adopt` -- still everything trusted, no note at all, even
/// though nothing was adopted.
#[test]
fn row5_both_trusted_without_adopt_prints_no_note() {
    let archive_path = export_colliding_archive("row5");
    let result = import_into_fresh_destination(&archive_path, "row5", true, true, false);
    assert!(
        !result.text.contains("note:"),
        "no note must print once every signer is trusted, even without --adopt: {}",
        result.text
    );
    assert!(
        result.json.contains("\"note\": null"),
        "the json note must be null: {}",
        result.json
    );
    let _ = std::fs::remove_file(&archive_path);
}
