//! Anchors `docs/src/guide/backup-restore.md` to the real compiled binary (DC-44 increment 4,
//! `backup-restore-page-handoff-v1.md` §4/§7): a recovery procedure that has drifted from the CLI
//! is worse than no procedure at all. This test runs the page's own command sequence, in order,
//! with the same commands, example key material (`dev-author`/`dev-maintainer`, the same fixed
//! public example values `beginners_tutorial.rs` and `tutorial.md` already use — not new ones),
//! file names, and messages the page shows, and asserts on the same output substrings the page
//! quotes — including the `--force` refusal (control 1) and the unsealed-commit demonstration,
//! the page's own most important and most easily-misread claim (control 2).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

mod support;

const MAINTAINER_PUBLIC_KEY_HEX: &str =
    "a00899dfd3357aee69729405913f9324dfc033cec04a2215239eda64ae6d9d91";
const MAINTAINER_SEED_HEX: &str =
    "111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000";
const AUTHOR_SEED_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn bundle_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "dc44-backup-restore-page-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

#[test]
fn backup_restore_page_sequence_runs_exactly_as_the_page_shows_it() {
    let source_repo = support::unique_repo("dc44-backup-restore-page-source");

    // "Set up a repository to back up"
    support::init(&source_repo);
    let trust = support::prikk(&source_repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "dev-maintainer",
            "--public-key",
            MAINTAINER_PUBLIC_KEY_HEX,
        ])
        .output()
        .unwrap();
    support::ok(&trust, "trust maintainer add");
    std::fs::write(source_repo.join("readme.txt"), b"hello prikk\n").unwrap();
    let commit = support::prikk(&source_repo)
        .env("PRIKK_AUTHOR_KEY_ID", "dev-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", "genesis"])
        .output()
        .unwrap();
    support::ok(&commit, "commit genesis");
    let seal = support::prikk(&source_repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "dev-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(MAINTAINER_SEED_HEX),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    support::ok(&seal, "seal genesis");
    assert!(
        String::from_utf8_lossy(&seal.stdout).contains("sealed active WAL into block"),
        "page quotes this line: {}",
        String::from_utf8_lossy(&seal.stdout)
    );

    // "Make a backup"
    let backup = bundle_path("backup");
    let export = support::prikk(&source_repo)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export, "first bundle export");
    let export_stdout = String::from_utf8_lossy(&export.stdout);
    assert!(
        export_stdout.contains("objects: 4"),
        "page quotes objects: 4 for the first export: {export_stdout}"
    );
    assert!(
        export_stdout.contains("this bundle contains one ref's closure only"),
        "page quotes the manifest's own single-ref note: {export_stdout}"
    );
    // §7.4: the page elides this line (`tool version: ...`) precisely because it goes stale every
    // release -- assert only that it is present, never its value, so this test does not itself go
    // wrong at the next version bump.
    assert!(
        export_stdout.contains("tool version: "),
        "page shows a tool version line (elided): {export_stdout}"
    );
    let first_tip_block_line = export_stdout
        .lines()
        .find(|line| line.starts_with("tip block:"))
        .unwrap_or_else(|| panic!("no tip block line: {export_stdout}"))
        .to_string();

    // "Running this again refuses" -- control 1.
    let refused = support::prikk(&source_repo)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "page shows this refusing without --force"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refusing to overwrite existing file"),
        "page quotes this exact refusal: {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    // "Only sealed history is backed up" -- control 2, the page's most important claim: commit
    // without sealing, export again, and the bundle must be unchanged.
    std::fs::write(source_repo.join("notes.txt"), b"not yet sealed\n").unwrap();
    let unsealed_commit = support::prikk(&source_repo)
        .env("PRIKK_AUTHOR_KEY_ID", "dev-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", "notes, not yet sealed"])
        .output()
        .unwrap();
    support::ok(&unsealed_commit, "commit without sealing");
    let after_unsealed_commit = bundle_path("after-unsealed-commit");
    let export_after_unsealed = support::prikk(&source_repo)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            after_unsealed_commit.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export_after_unsealed, "export after an unsealed commit");
    let export_after_unsealed_stdout = String::from_utf8_lossy(&export_after_unsealed.stdout);
    assert!(
        export_after_unsealed_stdout.contains("objects: 4"),
        "page's central claim: the unsealed commit must not appear -- object count stays 4: \
         {export_after_unsealed_stdout}"
    );
    assert!(
        export_after_unsealed_stdout.contains(&first_tip_block_line),
        "page's central claim: the tip block must be unchanged by the unsealed commit: \
         {export_after_unsealed_stdout}"
    );
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        std::fs::read(&after_unsealed_commit).unwrap(),
        "page's claim is byte-for-byte: exporting again before sealing produces the identical \
         bundle, not merely a matching summary"
    );

    // `prikk status` shows the queued patch -- the tell the page names.
    let status = support::prikk(&source_repo).arg("status").output().unwrap();
    support::ok(&status, "status");
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("queued patches: 1 targeting heads/main"),
        "page quotes this status line: {}",
        String::from_utf8_lossy(&status.stdout)
    );

    // Seal, then export with --force -- now both generations are included.
    let seal_second = support::prikk(&source_repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "dev-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(MAINTAINER_SEED_HEX),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    support::ok(&seal_second, "seal the second commit");
    let export_forced = support::prikk(&source_repo)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            backup.to_str().unwrap(),
            "--force",
        ])
        .output()
        .unwrap();
    support::ok(&export_forced, "export --force after sealing everything");
    let export_forced_stdout = String::from_utf8_lossy(&export_forced.stdout);
    assert!(
        export_forced_stdout.contains("objects: 8"),
        "page quotes objects: 8 once both generations are sealed: {export_forced_stdout}"
    );
    assert!(
        export_forced_stdout.contains("tool version: "),
        "page shows a tool version line (elided): {export_forced_stdout}"
    );

    // "Check a backup later, without restoring it" -- run from a directory with no repository.
    let no_repo_dir = support::unique_repo("dc44-backup-restore-page-no-repo");
    let verify = support::prikk(&no_repo_dir)
        .args(["bundle", "verify", "--input", backup.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&verify, "bundle verify with no repository present");
    let verify_stdout = String::from_utf8_lossy(&verify.stdout);
    assert!(
        verify_stdout.contains("objects: 8"),
        "page quotes objects: 8 from verify too: {verify_stdout}"
    );
    assert!(
        verify_stdout.contains("tool version: "),
        "page shows a tool version line (elided): {verify_stdout}"
    );
    assert!(
        verify_stdout.contains("A verified bundle is not yet a trusted one"),
        "page quotes this exact caveat: {verify_stdout}"
    );

    // "Restore"
    let restored_repo = support::unique_repo("dc44-backup-restore-page-restored");
    support::init(&restored_repo);
    let import = support::prikk(&restored_repo)
        .args(["bundle", "import", "--input", backup.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&import, "bundle import");
    let import_stdout = String::from_utf8_lossy(&import.stdout);
    assert!(
        import_stdout.contains("received remotes/heads/main"),
        "page quotes this import line: {import_stdout}"
    );
    assert!(
        import_stdout
            .contains("no local ref was created or advanced, and no MAINTAINER key was trusted"),
        "page quotes this exact note: {import_stdout}"
    );

    let trust_restored = support::prikk(&restored_repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "dev-maintainer",
            "--public-key",
            MAINTAINER_PUBLIC_KEY_HEX,
        ])
        .output()
        .unwrap();
    support::ok(
        &trust_restored,
        "trust maintainer add on the restored repository",
    );

    // "Now confirm the restore" -- the assertion that matters most: ordinary `prikk verify`
    // passes against what was actually restored.
    let restore_verify = support::prikk(&restored_repo)
        .arg("verify")
        .output()
        .unwrap();
    support::ok(&restore_verify, "verify the restored repository");
    let restore_verify_stdout = String::from_utf8_lossy(&restore_verify.stdout);
    assert!(
        restore_verify_stdout.contains("object items: 8 scanned, 0 failed"),
        "page quotes this verify line: {restore_verify_stdout}"
    );
    assert!(
        restore_verify_stdout.contains("publication trust issues: 0"),
        "page quotes this verify line: {restore_verify_stdout}"
    );
    assert!(
        restore_verify_stdout.contains("sealed blocks: 2"),
        "page quotes this verify line: {restore_verify_stdout}"
    );
    assert!(
        restore_verify_stdout.matches("dev-maintainer").count() >= 2,
        "both sealed blocks must show the trusted maintainer: {restore_verify_stdout}"
    );
    assert!(
        restore_verify_stdout.contains("received refs: 1"),
        "page quotes this verify line: {restore_verify_stdout}"
    );

    let _ = std::fs::remove_dir_all(source_repo);
    let _ = std::fs::remove_dir_all(no_repo_dir);
    let _ = std::fs::remove_dir_all(restored_repo);
    let _ = std::fs::remove_file(backup);
    let _ = std::fs::remove_file(after_unsealed_commit);
}
