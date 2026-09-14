//! DC-44 increment 1 (`bundle-offline-verify-handoff-v1.md`) — `prikk bundle verify --input <file>`
//! through the compiled binary. Store-level coverage (framing failures, the decisive corrupted-id
//! case, verify/import agreement across every DC-44 failure mode) lives in
//! `crates/prikk-store/src/bundle/tests.rs`; this file covers what only the CLI surface can prove:
//! the command needs no repository at all (control 1/4), and a corrupted file on disk is refused
//! the same way through this binary as it is through the library.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

mod support;

fn bundle_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "dc44-verify-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

/// Control 1 + 4: a real export verifies, reporting the same facts export itself reported, and
/// needs no repository — run from a directory with none present, and confirm the directory is
/// unchanged afterward (nothing written).
#[test]
fn verify_of_a_real_export_succeeds_with_no_repository_present_and_writes_nothing() {
    let source = support::unique_repo("dc44-verify-source");
    support::init(&source);
    support::generation(&source, "heads/main", "a.txt", b"hello\n", "genesis");

    let bundle = bundle_path("good");
    let export = support::prikk(&source)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export, "bundle export");
    let export_stdout = String::from_utf8_lossy(&export.stdout).into_owned();

    // No repository at all in the directory `bundle verify` runs from.
    let no_repo_dir = support::unique_repo("dc44-verify-no-repo");
    let before: Vec<_> = std::fs::read_dir(&no_repo_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(before.is_empty(), "fixture sanity: directory starts empty");

    let verify = support::prikk(&no_repo_dir)
        .args(["bundle", "verify", "--input", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&verify, "bundle verify");
    let verify_stdout = String::from_utf8_lossy(&verify.stdout).into_owned();

    assert!(
        verify_stdout.contains("bundle verifies: heads/main"),
        "verify stdout: {verify_stdout}"
    );
    assert!(
        verify_stdout.contains("objects: 5"),
        "verify must report the same object count export did (1 RefState + 1 Block + 1 Patch + \
         1 Blob + the root checkpoint's snapshot Blob): {verify_stdout}\nexport stdout was: {export_stdout}"
    );
    assert!(
        verify_stdout.contains("no signature is cryptographically verified"),
        "verify must state its own limit (§3.2), not stay silent about it: {verify_stdout}"
    );

    // Nothing was written -- no `.prikk`, no new file of any kind, in the directory verify ran
    // from.
    let after: Vec<_> = std::fs::read_dir(&no_repo_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        before, after,
        "bundle verify must write nothing, even into the directory it runs from"
    );

    let _ = std::fs::remove_dir_all(source);
    let _ = std::fs::remove_dir_all(no_repo_dir);
    let _ = std::fs::remove_file(bundle);
}

/// Control 2 + 3: a bundle file corrupted on disk (wrong magic, and a truncated file) is refused
/// by `bundle verify`, and the identical bytes are refused by `bundle import` too — demonstrated
/// through the real binary, not only the library the previous test and `bundle/tests.rs` already
/// cover.
#[test]
fn verify_and_import_agree_through_the_cli_on_a_corrupted_file() {
    let source = support::unique_repo("dc44-verify-corrupt-source");
    support::init(&source);
    support::generation(&source, "heads/main", "a.txt", b"hello\n", "genesis");

    let bundle = bundle_path("corrupt-source");
    let export = support::prikk(&source)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export, "bundle export");

    let mut bytes = std::fs::read(&bundle).unwrap();
    bytes[0..8].copy_from_slice(b"XXXXXXXX"); // wrong magic
    let corrupt_bundle = bundle_path("corrupt-file");
    std::fs::write(&corrupt_bundle, &bytes).unwrap();

    let no_repo_dir = support::unique_repo("dc44-verify-corrupt-no-repo");
    let verify = support::prikk(&no_repo_dir)
        .args([
            "bundle",
            "verify",
            "--input",
            corrupt_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !verify.status.success(),
        "a bundle with the wrong magic must fail verification"
    );
    let verify_stderr = String::from_utf8_lossy(&verify.stderr).into_owned();
    assert!(
        verify_stderr.contains("invalid bundle magic"),
        "verify stderr: {verify_stderr}"
    );

    let target = support::unique_repo("dc44-verify-corrupt-target");
    support::init(&target);
    let import = support::prikk(&target)
        .args([
            "bundle",
            "import",
            "--input",
            corrupt_bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !import.status.success(),
        "the identical bytes must fail import too"
    );
    let import_stderr = String::from_utf8_lossy(&import.stderr).into_owned();
    assert_eq!(
        verify_stderr, import_stderr,
        "verify and import must refuse the same corrupted file for the identical reason"
    );

    let _ = std::fs::remove_dir_all(source);
    let _ = std::fs::remove_dir_all(no_repo_dir);
    let _ = std::fs::remove_dir_all(target);
    let _ = std::fs::remove_file(bundle);
    let _ = std::fs::remove_file(corrupt_bundle);
}
