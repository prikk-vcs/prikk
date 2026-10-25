//! DC-73 node-model operation apply: `ReplaceBinary` and `ChangePerm` materialize on checkout and
//! invert for rollback. Criterion 3's own words: rebuild the worktree from sealed history and assert
//! byte-exact content, including the mode bit — not just that a command exits zero.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support::*;

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn replace_binary_materializes_byte_exact_across_sealed_generations() {
    let repo = unique_repo("dc73-replace-binary-generations");
    init(&repo);
    generation(
        &repo,
        "heads/main",
        "bin.dat",
        &[0xff, 0x00, 0xfe, 0x01],
        "genesis",
    );
    generation(
        &repo,
        "heads/main",
        "bin.dat",
        &[0xaa, 0xbb, 0xcc, 0xdd],
        "generation 2",
    );
    generation(
        &repo,
        "heads/main",
        "bin.dat",
        &[0xff],
        "generation 3 — shrink to one byte",
    );

    let materialize_root = rebuild_from_sealed_history(&repo, "dc73-replace-binary");
    let rebuilt = std::fs::read(materialize_root.join("bin.dat")).unwrap();
    assert_eq!(rebuilt, vec![0xff]);
}

#[test]
#[cfg(unix)]
fn change_perm_materializes_mode_exact_across_sealed_generations() {
    let repo = unique_repo("dc73-change-perm-generations");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"content").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");

    set_mode(&repo.join("a.txt"), 0o755);
    ok(
        &commit(&repo, "heads/main", "make executable"),
        "commit change-perm",
    );
    ok(&seal(&repo, "heads/main"), "seal change-perm");

    set_mode(&repo.join("a.txt"), 0o644);
    ok(
        &commit(&repo, "heads/main", "make non-executable again"),
        "commit second change-perm",
    );
    ok(&seal(&repo, "heads/main"), "seal second change-perm");

    let materialize_root = rebuild_from_sealed_history(&repo, "dc73-change-perm");
    let rebuilt_path = materialize_root.join("a.txt");
    assert_eq!(std::fs::read(&rebuilt_path).unwrap(), b"content");
    assert_eq!(
        mode_of(&rebuilt_path),
        0o644,
        "expected the final mode to survive materialization"
    );
}

#[test]
#[cfg(unix)]
fn create_file_mode_survives_materialization_without_any_change_perm() {
    // The case DC-72's ruling called out specifically: a CreateFile with a non-default mode and no
    // ChangePerm anywhere in history. Materializing ChangePerm correctly is not sufficient on its
    // own — this is the case that stayed broken if the fix only touched ChangePerm's own arm.
    let repo = unique_repo("dc73-createfile-mode-only");
    init(&repo);
    std::fs::write(repo.join("script.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    set_mode(&repo.join("script.sh"), 0o755);
    ok(
        &commit(&repo, "heads/main", "genesis with executable file"),
        "genesis commit",
    );
    ok(&seal(&repo, "heads/main"), "genesis seal");

    let materialize_root = rebuild_from_sealed_history(&repo, "dc73-createfile-mode");
    let rebuilt_path = materialize_root.join("script.sh");
    assert_eq!(mode_of(&rebuilt_path), 0o755);
}

#[test]
fn inverse_plan_succeeds_for_replace_binary_and_change_perm_history() {
    let repo = unique_repo("dc73-inverse-plan");
    init(&repo);
    std::fs::write(repo.join("bin.dat"), [0xff, 0x00, 0xfe]).unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");

    std::fs::write(repo.join("bin.dat"), [0xff, 0xee, 0xdd]).unwrap();
    ok(
        &commit(&repo, "heads/main", "replace binary"),
        "commit replace-binary",
    );
    ok(&seal(&repo, "heads/main"), "seal replace-binary");

    #[cfg(unix)]
    {
        set_mode(&repo.join("bin.dat"), 0o755);
        ok(
            &commit(&repo, "heads/main", "change perm"),
            "commit change-perm",
        );
        ok(&seal(&repo, "heads/main"), "seal change-perm");
    }

    let out = prikk(&repo).arg("inverse-plan").output().unwrap();
    ok(&out, "inverse-plan");
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("inverse-plan stdout: {stdout}");
    assert!(
        stdout.contains("replace-binary"),
        "expected a replace-binary inverse: {stdout}"
    );
    #[cfg(unix)]
    assert!(
        stdout.contains("change-perm"),
        "expected a change-perm inverse: {stdout}"
    );
}
