//! RFC 155 Part I-B's own required smoke: the carry-forward story (§5/§9), through the real
//! **release** binary, not the library -- the handoff's own "Smoke: a new section that runs the
//! carry-forward story on the release build."
//!
//! `#[ignore]`d, the same convention `rfc133_node_count_memory.rs`'s own release-build instruments
//! already use: `scripts/gates.py`'s own `test`/`test-msrv` targets run the default, non-ignored
//! set under a debug build, where `support::release_binary_path` would panic by its own design
//! (correctly -- it exists to catch exactly a debug build measuring as if it were a release one).
//! Run explicitly with `cargo test --release -p prikk --test
//! rfc155_part_ib_carry_forward_smoke -- --ignored` (a plain `cargo test`, without `--release`,
//! panics immediately, naming why).
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::Path;
use std::process::Command;

use support::{MAINTAINER_KEY_ID, isolate_key_environment_for, maintainer_public_key_hex, ok};

fn release_prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(support::release_binary_path());
    cmd.current_dir(repo);
    isolate_key_environment_for(&mut cmd, Some(repo));
    cmd
}

/// Export from repository 1; `init` repository 2; verify lists the maintainer keys; `trust
/// maintainer add`; `import --adopt`; every branch is a branch; `prikk verify` exits 0 for real
/// (this is the one place in the whole RFC 155 test suite where that is a literal process exit
/// code, not a `prikk-store`-level approximation of it -- see the library-level carry-forward
/// test's own comment on why it stops short of this claim).
#[test]
#[ignore = "release-build smoke: run explicitly with `cargo test --release ... -- --ignored`"]
fn carry_forward_story_smoke() {
    let repo1 = support::unique_repo("rfc155-ib-smoke-repo1");
    support::init(&repo1);
    support::generation(
        &repo1,
        "heads/main",
        "a.txt",
        b"carry-forward smoke content\n",
        "one real generation",
    );

    let mut archive_path = std::env::temp_dir();
    archive_path.push(format!(
        "prikk-cli-dc67-rfc155-ib-smoke-archive-{}.prepo",
        support::unique_suffix()
    ));
    ok(
        &release_prikk(&repo1)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );

    let repo2 = support::unique_repo("rfc155-ib-smoke-repo2");
    support::init(&repo2);

    let verify_output = release_prikk(&repo2)
        .args(["archive", "verify"])
        .arg(&archive_path)
        .output()
        .unwrap();
    ok(&verify_output, "archive verify");
    let verify_text = String::from_utf8_lossy(&verify_output.stdout);
    assert!(
        verify_text.contains(MAINTAINER_KEY_ID),
        "verify must list the archive's own carried maintainer key: {verify_text}"
    );

    ok(
        &release_prikk(&repo2)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                MAINTAINER_KEY_ID,
                "--public-key",
                &maintainer_public_key_hex(),
            ])
            .output()
            .unwrap(),
        "trust maintainer add",
    );

    let import_output = release_prikk(&repo2)
        .args(["archive", "import", "--adopt"])
        .arg(&archive_path)
        .output()
        .unwrap();
    ok(&import_output, "archive import --adopt");
    let import_text = String::from_utf8_lossy(&import_output.stdout);
    assert!(
        import_text.contains("adopted as a local branch"),
        "heads/main must adopt: {import_text}"
    );

    let branch_output = release_prikk(&repo2).args(["branch"]).output().unwrap();
    ok(&branch_output, "branch list");
    let branch_text = String::from_utf8_lossy(&branch_output.stdout);
    assert!(
        branch_text.contains("heads/main"),
        "every branch is a branch: {branch_text}"
    );
    assert!(
        !branch_text.contains("remotes/"),
        "the adopted branch must not also sit under remotes/: {branch_text}"
    );

    let final_verify = release_prikk(&repo2).arg("verify").output().unwrap();
    assert!(
        final_verify.status.success(),
        "prikk verify must exit 0 after the carry-forward story: {final_verify:?}"
    );

    let _ = std::fs::remove_dir_all(&repo1);
    let _ = std::fs::remove_dir_all(&repo2);
    let _ = std::fs::remove_file(&archive_path);
}
