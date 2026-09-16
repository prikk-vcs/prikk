//! DC-44 increment 3 (`bundle-manifest-handoff-v1.md`) — the self-describing manifest, through the
//! compiled binary. Store-level coverage (the wire format, the manifest-vs-payload agreement
//! checks, `PBNDL001`/`PBNDL002` still importing with no manifest) lives in
//! `crates/prikk-store/src/bundle/tests.rs`; this file covers what only the CLI surface can prove:
//! `bundle export` and `bundle verify` both actually print the manifest (§4.3), and they agree with
//! each other on a real bundle.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

mod support;

fn bundle_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "dc44-manifest-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

/// §4.3: `bundle export` prints the manifest it just wrote -- repository format, tool version, and
/// the single-ref scope note that closes handoff §1's honesty gap -- and `bundle verify`, run
/// later with no repository at all, prints the identical facts read back from the file.
#[test]
fn export_and_verify_both_print_the_manifest_and_agree() {
    let source = support::unique_repo("dc44-manifest-source");
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

    assert!(
        export_stdout.contains("repository format: 7"),
        "export stdout: {export_stdout}"
    );
    assert!(
        export_stdout.contains("tool version: "),
        "export stdout: {export_stdout}"
    );
    assert!(
        export_stdout.contains("this bundle contains one ref's closure only"),
        "export must state the single-ref limitation where the operator creates the bundle, not \
         only where they later verify it: {export_stdout}"
    );

    let no_repo_dir = support::unique_repo("dc44-manifest-no-repo");
    let verify = support::prikk(&no_repo_dir)
        .args(["bundle", "verify", "--input", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&verify, "bundle verify");
    let verify_stdout = String::from_utf8_lossy(&verify.stdout).into_owned();

    assert!(
        verify_stdout.contains("repository format: 7"),
        "verify stdout: {verify_stdout}"
    );
    assert!(
        verify_stdout.contains("this bundle contains one ref's closure only"),
        "verify must state the same limitation, read back from the file with no repository at \
         all: {verify_stdout}"
    );

    // The exact tool-version line must agree between the two commands -- both ran the same
    // binary, so the manifest `export` wrote is exactly what `verify` reads back, not a
    // coincidentally similar value.
    let tool_version_line = |stdout: &str| -> String {
        stdout
            .lines()
            .find(|line| line.starts_with("tool version:"))
            .unwrap_or_else(|| panic!("no tool version line in: {stdout}"))
            .to_string()
    };
    assert_eq!(
        tool_version_line(&export_stdout),
        tool_version_line(&verify_stdout),
        "export's own manifest and verify's own reading of it must report the identical tool \
         version"
    );

    let _ = std::fs::remove_dir_all(source);
    let _ = std::fs::remove_dir_all(no_repo_dir);
    let _ = std::fs::remove_file(bundle);
}
