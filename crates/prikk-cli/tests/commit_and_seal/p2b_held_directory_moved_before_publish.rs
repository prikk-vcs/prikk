//! 0.58.0 P2b, row 2 through the binary: a `seal` that has written its patches and block into `.prikk/containers/patch` and is about
//! to publish finds that directory renamed away, and refuses with nothing published.
//!
//! The binary is stopped between its writes and its publish by `prikk-store`'s `test-support` hook (the build these tests run
//! against; no shipped build has it): with `PRIKK_TEST_PAUSE_BEFORE_PUBLISH=<go>` it writes `<go>.waiting` when it reaches the check
//! before the publish and waits for `<go>`.
//!
//! Measured the same way for rows 3 to 6 (`.git-exclude/logs/0.58.0-P1/p2b-casetable.txt`): the store-level tests hold each row.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::support;

#[test]
fn a_seal_whose_patch_directory_was_renamed_away_refuses_and_publishes_nothing() {
    let repo = support::unique_repo("p2b-seal-row2");
    support::init(&repo);
    for index in 0..2 {
        std::fs::write(
            repo.join(format!("f{index}.txt")),
            format!("line {index}\n"),
        )
        .unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("c{index}")),
            "commit",
        );
    }
    support::trust_maintainer(&repo);

    let refs = repo.join(".prikk/refs/containers");
    let before = (
        std::fs::read(refs.join("pointer-index-a.container")).unwrap(),
        std::fs::read(refs.join("log-a.container")).unwrap(),
    );

    let go = std::env::temp_dir().join(format!("p2b-go-{}", support::unique_suffix()));
    let waiting = {
        let mut name = go.clone().into_os_string();
        name.push(".waiting");
        std::path::PathBuf::from(name)
    };
    let mut seal = support::prikk(&repo);
    seal.env("PRIKK_TEST_PAUSE_BEFORE_PUBLISH", &go)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit", "--ref", "heads/main"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = seal.spawn().unwrap();

    let started = Instant::now();
    while !waiting.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the seal never reached the check before its publish"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Between the writes and the publish: the held directory is renamed away.
    std::fs::rename(
        repo.join(".prikk/containers/patch"),
        repo.join(".prikk/containers/patch-moved"),
    )
    .unwrap();
    std::fs::write(&go, b"go").unwrap();
    let output = child.wait_with_output().unwrap();
    let _ = std::fs::remove_file(&go);
    let _ = std::fs::remove_file(&waiting);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("containers/patch"),
        "names the directory: {stderr}"
    );
    assert!(
        stderr.contains("was moved or replaced while this command ran"),
        "{stderr}"
    );
    assert!(stderr.contains("nothing was published"), "{stderr}");
    assert!(
        stderr.contains("run the command again") && stderr.contains("`prikk verify`"),
        "a typeable way out: {stderr}"
    );
    assert_eq!(
        before,
        (
            std::fs::read(refs.join("pointer-index-a.container")).unwrap(),
            std::fs::read(refs.join("log-a.container")).unwrap(),
        ),
        "the pointer index and the ref log are byte-identical"
    );
    let status = support::prikk(&repo).arg("status").output().unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("queued patches: 2"),
        "the queue is untouched: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}
