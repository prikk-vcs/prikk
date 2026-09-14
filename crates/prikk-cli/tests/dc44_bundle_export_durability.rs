//! DC-44 increment 2 (`bundle-export-durability-handoff-v1.md`) — `bundle export`'s destination
//! write, through the compiled binary. Store-level, filesystem-level coverage of the atomic-write
//! primitive itself (induced failures, temp-file cleanup) lives in
//! `crates/prikk-cli/src/durable_output/tests.rs`; this file covers what only the CLI surface can
//! prove: the `--force` collision policy end to end, and that a real export survives an induced
//! failure with the previous backup still passing `bundle verify` afterward.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

mod support;

fn bundle_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "dc44-durability-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

fn export(repo: &std::path::Path, output: &std::path::Path, force: bool) -> std::process::Output {
    let mut args = vec![
        "bundle".to_string(),
        "export".to_string(),
        "--ref".to_string(),
        "heads/main".to_string(),
        "--output".to_string(),
        output.to_str().unwrap().to_string(),
    ];
    if force {
        args.push("--force".to_string());
    }
    support::prikk(repo).args(args).output().unwrap()
}

/// Control 3, protected case: exporting to an existing destination without `--force` refuses, and
/// the original file is untouched -- still passing `bundle verify`, not merely byte-present.
#[test]
fn export_without_force_refuses_an_existing_destination_and_leaves_it_verifiable() {
    let repo = support::unique_repo("dc44-durability-protected");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");

    let output = bundle_path("protected");
    let first = export(&repo, &output, false);
    support::ok(&first, "first export (no prior file, --force not needed)");
    let original_bytes = std::fs::read(&output).unwrap();

    // Change history, then try to export over the same path without --force.
    support::generation(&repo, "heads/main", "b.txt", b"second\n", "second");
    let second = export(&repo, &output, false);
    assert!(
        !second.status.success(),
        "exporting over an existing file without --force must refuse"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("refusing to overwrite existing file"),
        "stderr: {stderr}"
    );

    // The original file must be completely untouched -- same bytes, and still a real,
    // independently verifiable bundle, not merely present.
    assert_eq!(
        std::fs::read(&output).unwrap(),
        original_bytes,
        "the refused export must not alter the existing file at all"
    );
    let verify = support::prikk(&repo)
        .args(["bundle", "verify", "--input", output.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&verify, "the untouched original must still verify");
    let verify_stdout = String::from_utf8_lossy(&verify.stdout);
    assert!(
        verify_stdout.contains("objects: 5"),
        "must still report the first export's own object count (1 RefState + 1 Block + 1 Patch \
         + 1 Blob + the root checkpoint's snapshot Blob), not the second history's: {verify_stdout}"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_file(output);
}

/// Control 4: the new write path must not alter content. Nothing in a CLI test can see
/// `export_bundle`'s own in-memory return value directly to compare against, so the closest real
/// proxy is determinism: exporting the identical, unchanged repository state twice, to two
/// different destinations, must produce byte-identical files -- if the write path silently
/// mutated, truncated, or otherwise touched the bytes on their way to disk, two independent writes
/// of the same content would not agree.
#[test]
fn a_successful_export_is_deterministic_and_the_result_verifies() {
    let repo = support::unique_repo("dc44-durability-deterministic");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");

    let first_output = bundle_path("deterministic-a");
    let second_output = bundle_path("deterministic-b");
    support::ok(&export(&repo, &first_output, false), "first export");
    support::ok(&export(&repo, &second_output, false), "second export");

    assert_eq!(
        std::fs::read(&first_output).unwrap(),
        std::fs::read(&second_output).unwrap(),
        "two exports of identical, unchanged repository state must write byte-identical files"
    );

    let verify = support::prikk(&repo)
        .args([
            "bundle",
            "verify",
            "--input",
            first_output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&verify, "the written file must verify");

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_file(first_output);
    let _ = std::fs::remove_file(second_output);
}

/// Control 3, permitted case: `--force` overwrites, and the new content is exactly what the
/// second export actually produced -- not a stale copy, not a merge of the two.
#[test]
fn export_with_force_overwrites_and_the_result_is_the_new_export() {
    let repo = support::unique_repo("dc44-durability-forced");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");

    let output = bundle_path("forced");
    support::ok(&export(&repo, &output, false), "first export");

    support::generation(&repo, "heads/main", "b.txt", b"second\n", "second");
    let forced = export(&repo, &output, true);
    support::ok(&forced, "second export with --force must succeed");
    let forced_stdout = String::from_utf8_lossy(&forced.stdout);
    assert!(
        forced_stdout.contains("objects: 9"),
        "the forced export must report the full two-generation closure (2 RefStates via the \
         previous_ref_state_id chain + 2 Blocks + 2 Patches + 2 Blobs + the root checkpoint's snapshot Blob -- the same count \
         dc78_bundle_exchange.rs's own two-generation fixture establishes): {forced_stdout}"
    );

    let verify = support::prikk(&repo)
        .args(["bundle", "verify", "--input", output.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&verify, "the forced export's own result must verify");
    let verify_stdout = String::from_utf8_lossy(&verify.stdout);
    assert!(
        verify_stdout.contains("objects: 9"),
        "verify must see the forced export's own content, not the first export's: {verify_stdout}"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_file(output);
}

/// Control 1, through the real binary: an export that cannot write to its destination directory
/// leaves the previous backup at that path completely intact -- content unchanged and still
/// independently verifiable, demonstrated on the assertion the handoff itself named ("the
/// assertion is on the destination's contents, not on the error message").
#[cfg(unix)]
#[test]
fn a_failed_export_leaves_the_previous_backup_verifiable() {
    use std::os::unix::fs::PermissionsExt;

    let repo = support::unique_repo("dc44-durability-failed-write");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");

    let output_dir = support::unique_repo("dc44-durability-failed-write-dir");
    let output = output_dir.join("backup.bundle");
    support::ok(
        &export(&repo, &output, false),
        "first export, before the directory is locked down",
    );
    let original_bytes = std::fs::read(&output).unwrap();

    support::generation(&repo, "heads/main", "b.txt", b"second\n", "second");
    let original_mode = std::fs::metadata(&output_dir).unwrap().permissions().mode();
    std::fs::set_permissions(&output_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = export(&repo, &output, true); // --force: the refusal under test is the write itself, not the collision policy
    std::fs::set_permissions(&output_dir, std::fs::Permissions::from_mode(original_mode)).unwrap();
    assert!(
        !failed.status.success(),
        "the export must fail when its destination directory refuses new files"
    );

    assert_eq!(
        std::fs::read(&output).unwrap(),
        original_bytes,
        "the previous backup must be byte-for-byte untouched by the failed export"
    );
    let verify = support::prikk(&repo)
        .args(["bundle", "verify", "--input", output.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&verify, "the untouched previous backup must still verify");

    let entries: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        entries,
        vec![std::ffi::OsString::from("backup.bundle")],
        "no abandoned temp file may remain: {entries:?}"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_dir_all(output_dir);
}
