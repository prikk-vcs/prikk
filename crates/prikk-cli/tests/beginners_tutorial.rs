//! Anchors `docs/src/guide/tutorial.md` to the real compiled binary (beginner's on-ramp handoff v1
//! §3): a tutorial is prose that can drift from the CLI, and a broken first command is worse than no
//! tutorial at all. This test runs the tutorial's own command sequence, in order, with the same
//! commands, environment variable names and values, file names, and messages the page shows, and
//! asserts on the same output substrings the page quotes -- both the two refusals it confronts
//! (author signing, then maintainer signing) and the eventual successes. A CLI change that alters any
//! quoted string here is a change the tutorial page would also need, and this test is what turns that
//! into a build failure instead of a silent mismatch.
//!
//! Uses the exact fixed example values the README Quick Start and
//! `docs/src/guide/security-setup.md` already use (`dev-author`/`dev-maintainer`), not new ones --
//! one set of public example values across every surface that shows one.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::process::Output;

mod support;

const MAINTAINER_PUBLIC_KEY_HEX: &str =
    "a00899dfd3357aee69729405913f9324dfc033cec04a2215239eda64ae6d9d91";
const MAINTAINER_SEED_HEX: &str =
    "111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000";
const AUTHOR_SEED_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn fail(output: &Output, what: &str) {
    assert!(
        !output.status.success(),
        "{what} unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn tutorial_sequence_runs_exactly_as_the_page_shows_it() {
    let repo = support::unique_repo("beginners-tutorial");

    // 1. `prikk init`
    support::init(&repo);

    // 2. write the first file the tutorial commits.
    std::fs::write(repo.join("readme.txt"), b"hello prikk\n").unwrap();

    // 3. the tutorial's first confrontation: commit with no author key configured.
    let out = support::prikk(&repo)
        .args(["commit", "-m", "genesis"])
        .output()
        .unwrap();
    fail(&out, "commit with no author key");
    // RFC 148: the refusal now names the *path* it looked for, which is machine-specific, so the
    // page quotes it with a placeholder path and this test pins the parts that do not vary — the
    // prefix a reader searches for, and each of the three routes out. Asserting the whole string
    // would mean the page had to quote one developer's home directory.
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("error: author signing is required: no seed at ")
            && String::from_utf8_lossy(&out.stderr).contains("prikk key generate --out ")
            && String::from_utf8_lossy(&out.stderr).contains("PRIKK_AUTHOR_SEED_FILE"),
        "tutorial quotes this refusal: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 4-5. set the author key, then the same commit succeeds.
    let out = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", "dev-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", "genesis"])
        .output()
        .unwrap();
    support::ok(&out, "commit with author key configured");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("create-file readme.txt"),
        "tutorial quotes this line of commit's own output: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // 6. the tutorial's second confrontation: seal with no maintainer key configured.
    let out = support::prikk(&repo)
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    fail(&out, "seal with no maintainer key");
    // Same shape as the AUTHOR refusal above: the path varies by machine, the prefix and the routes
    // do not.
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("error: maintainer signing is required: no seed at ")
            && String::from_utf8_lossy(&out.stderr).contains("PRIKK_MAINTAINER_SEED_FILE"),
        "tutorial quotes this refusal: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 7-8. the key is configured but not yet trusted -- a different, still-real refusal.
    let out = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "dev-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(MAINTAINER_SEED_HEX),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    fail(&out, "seal with an untrusted maintainer key");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(
            "error: precondition not met: no maintainer key is adopted in this repository yet"
        ),
        "tutorial quotes this exact refusal: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 9. trust the maintainer key.
    let out = support::prikk(&repo)
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
    support::ok(&out, "trust maintainer add");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("trusted maintainer key: dev-maintainer"),
        "tutorial quotes this line: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // 10. seal now succeeds.
    let out = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "dev-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(MAINTAINER_SEED_HEX),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    support::ok(&out, "seal");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("sealed active WAL into block"),
        "tutorial quotes this line: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // 11. `prikk log`
    let out = support::prikk(&repo).arg("log").output().unwrap();
    support::ok(&out, "log");
    let log_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        log_stdout.contains("Root"),
        "tutorial quotes a Root block: {log_stdout}"
    );
    assert!(
        log_stdout.contains("update-seq: 1"),
        "tutorial quotes update-seq 1: {log_stdout}"
    );

    // 12. `prikk verify`
    let out = support::prikk(&repo).arg("verify").output().unwrap();
    support::ok(&out, "verify");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("checked blocks: 1"),
        "tutorial quotes this verify line: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // 13. `prikk doctor`
    let out = support::prikk(&repo).arg("doctor").output().unwrap();
    support::ok(&out, "doctor");
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("issue summary: errors=0, warnings=0, info=1"),
        "tutorial quotes this doctor summary line: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
