//! RFC 155 Part I-E item 4: the default import's publication-trust note, re-scoped. Part I-C's
//! own note fired for every landed-but-not-adopted ref regardless of whether the signer was
//! actually trusted (review `rfc155-impl-IC-review-v1`'s own three-row finding). It now fires
//! only for a ref whose own tip carries no maintainer signature *this destination* already
//! trusts, and offers the `--adopt` hint only when one of those untrusted-tip refs is the
//! exporter's own ref (a received pointer can never be adopted). These three rows have no
//! landing collision; `rfc155_part_if_trust_note.rs` covers Part I-F's own broader rule (every
//! *planned* entry, landed or not) with a genuinely colliding fixture.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use support::ok;

fn export_one_generation(repo: &std::path::Path, tag: &str) -> std::path::PathBuf {
    support::init(repo);
    support::generation(
        repo,
        "heads/main",
        "a.txt",
        b"the exporter's own one file\n",
        "first generation",
    );
    let mut archive_path = std::env::temp_dir();
    archive_path.push(format!(
        "prikk-cli-rfc155-ie-trust-note-{tag}-{}.prepo",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(repo)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );
    archive_path
}

/// Row 1: the signer is not trusted at the destination -- the note prints, naming the ref.
#[test]
fn row_untrusted_signer_prints_the_note() {
    let repo = support::unique_repo("rfc155-ie-trust-note-row1-source");
    let archive_path = export_one_generation(&repo, "row1");

    let dest = support::unique_repo("rfc155-ie-trust-note-row1-dest");
    support::init(&dest);
    let import_text = support::prikk(&dest)
        .args(["archive", "import"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        import_text.status.success(),
        "import must still succeed: {}",
        String::from_utf8_lossy(&import_text.stderr)
    );
    let stdout = String::from_utf8_lossy(&import_text.stdout);
    assert!(
        stdout.contains("note:") && stdout.contains("heads/main"),
        "the note must print, naming the untrusted ref: {stdout}"
    );
    assert!(
        stdout.contains("--adopt"),
        "the exporter's own ref is untrusted, so the --adopt hint must appear: {stdout}"
    );

    let dest_json = support::unique_repo("rfc155-ie-trust-note-row1-dest-json");
    support::init(&dest_json);
    let import_json = support::prikk(&dest_json)
        .args(["archive", "import", "--format", "json"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(import_json.status.success(), "json import must succeed");
    let json_stdout = String::from_utf8_lossy(&import_json.stdout);
    assert!(
        !json_stdout.contains("\"note\": null"),
        "the json note field must not be null when the signer is untrusted: {json_stdout}"
    );

    let _ = std::fs::remove_file(&archive_path);
}

/// Row 2: the signer is trusted at the destination, but `--adopt` was not requested -- the ref
/// still lands under `remotes/`, but it is not untrusted, so no note prints.
#[test]
fn row_trusted_signer_without_adopt_prints_no_note() {
    let repo = support::unique_repo("rfc155-ie-trust-note-row2-source");
    let archive_path = export_one_generation(&repo, "row2");

    let dest = support::unique_repo("rfc155-ie-trust-note-row2-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
    let import = support::prikk(&dest)
        .args(["archive", "import"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "import must succeed: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    let stdout = String::from_utf8_lossy(&import.stdout);
    assert!(
        !stdout.contains("note:"),
        "no note must print once the signer is already trusted, even without --adopt: {stdout}"
    );

    let dest_json = support::unique_repo("rfc155-ie-trust-note-row2-dest-json");
    support::init(&dest_json);
    support::trust_maintainer(&dest_json);
    let import_json = support::prikk(&dest_json)
        .args(["archive", "import", "--format", "json"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(import_json.status.success(), "json import must succeed");
    let json_stdout = String::from_utf8_lossy(&import_json.stdout);
    assert!(
        json_stdout.contains("\"note\": null"),
        "the json note field must be null once the signer is already trusted: {json_stdout}"
    );

    let _ = std::fs::remove_file(&archive_path);
}

/// Row 3: the signer is trusted, and `--adopt` is used -- the exporter's own ref lands as a real
/// local branch, so no note prints either.
#[test]
fn row_trusted_signer_with_adopt_prints_no_note() {
    let repo = support::unique_repo("rfc155-ie-trust-note-row3-source");
    let archive_path = export_one_generation(&repo, "row3");

    let dest = support::unique_repo("rfc155-ie-trust-note-row3-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
    let import = support::prikk(&dest)
        .args(["archive", "import", "--adopt"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "import must succeed: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    let stdout = String::from_utf8_lossy(&import.stdout);
    assert!(
        stdout.contains("adopted as a local branch"),
        "the exporter's own ref must adopt as a real local branch: {stdout}"
    );
    assert!(
        !stdout.contains("note:"),
        "no note must print once the ref landed adopted: {stdout}"
    );

    let dest_json = support::unique_repo("rfc155-ie-trust-note-row3-dest-json");
    support::init(&dest_json);
    support::trust_maintainer(&dest_json);
    let import_json = support::prikk(&dest_json)
        .args(["archive", "import", "--adopt", "--format", "json"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(import_json.status.success(), "json import must succeed");
    let json_stdout = String::from_utf8_lossy(&import_json.stdout);
    assert!(
        json_stdout.contains("\"note\": null"),
        "the json note field must be null once the ref landed adopted: {json_stdout}"
    );

    let _ = std::fs::remove_file(&archive_path);
}
