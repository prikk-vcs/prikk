//! 0.58.0 P3b: `sync accept` streams -- through the binary. planeter's promise (the write order: patches and blobs, then AUTHOR key
//! material, then tags and claims, each after what it names, and exit 1 for a write error) and the state a failed accept leaves.
//!
//! The two `test-support` hooks (in no shipped build): `PRIKK_TEST_ACCEPT_TRACE=<file>` appends one line per durable write in order, and
//! `PRIKK_TEST_ACCEPT_FAIL_AT=<n>` makes the n-th durable write (objects and key entries, from 0) fail before it is made.
//!
//! **Red control (planeter's order):** write the claims and tags before the patches and blobs and `the_writes_come_in_planeters_order` goes red.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};
use std::process::Output;

fn file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("p3b-{tag}-{}.bin", support::unique_suffix()))
}

/// A sender with one sealed patch and a tag, and the artifact `sync build` makes for an empty receiver: patch, blob, author key, claim, tag.
fn artifact() -> PathBuf {
    let sender = support::unique_repo("p3b-sender");
    support::init(&sender);
    support::generation(
        &sender,
        "heads/main",
        "a.txt",
        b"p3b streaming accept\n",
        "first",
    );
    support::ok(
        &support::tag_create(&sender, "tags/v1", "heads/main"),
        "tag create",
    );
    let empty = support::unique_repo("p3b-empty");
    support::init(&empty);
    let have = file("have");
    support::ok(
        &support::prikk(&empty)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact = file("artifact");
    support::ok(
        &support::prikk(&sender)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have.to_str().unwrap(),
                "--output",
                artifact.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );
    artifact
}

fn receiver(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    repo
}

fn accept(repo: &Path, artifact: &Path, env: &[(&str, &str)]) -> Output {
    let mut command = support::prikk(repo);
    for (name, value) in env {
        command.env(name, value);
    }
    command
        .args(["sync", "accept", artifact.to_str().unwrap()])
        .output()
        .unwrap()
}

fn trace_lines(trace: &Path) -> Vec<String> {
    std::fs::read_to_string(trace)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn kinds(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line.split(' ').next().unwrap())
        .collect()
}

fn pending(repo: &Path) -> String {
    String::from_utf8_lossy(
        &support::prikk(repo)
            .args(["sync", "pending"])
            .output()
            .unwrap()
            .stdout,
    )
    .to_string()
}

#[test]
fn the_writes_come_in_planeters_order() {
    let artifact = artifact();
    let repo = receiver("p3b-order");
    let trace = file("trace");
    let output = accept(
        &repo,
        &artifact,
        &[("PRIKK_TEST_ACCEPT_TRACE", trace.to_str().unwrap())],
    );
    support::ok(&output, "sync accept");
    let lines = trace_lines(&trace);
    // Patches and blobs, then AUTHOR key material, then tags, then claims (a tag before a claim: the dependency rank), and a claim only
    // after the patch it names.
    assert_eq!(
        kinds(&lines),
        ["patch", "blob", "key", "tag", "claim"],
        "the order of durable writes: {lines:?}"
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("patches: 1")
            && out.contains("blobs: 1")
            && out.contains("claims: 1")
            && out.contains("tags: 1"),
        "{out}"
    );
}

/// A write error mid-accept exits 1 and leaves the state the case table names, at every point it can happen: before any write, between
/// patches and blobs, before the key material, before the tag, before the claim. Each time: no claim is listed as pending until the claim
/// is written; and the same accept run again finishes it.
#[test]
fn a_write_error_exits_1_and_a_rerun_finishes_from_the_state_it_left() {
    let artifact = artifact();
    for fail_at in 0..5_usize {
        let repo = receiver(&format!("p3b-fail-{fail_at}"));
        let trace = file(&format!("trace-{fail_at}"));
        let failing = accept(
            &repo,
            &artifact,
            &[
                ("PRIKK_TEST_ACCEPT_TRACE", trace.to_str().unwrap()),
                ("PRIKK_TEST_ACCEPT_FAIL_AT", &fail_at.to_string()),
            ],
        );
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&failing.stdout),
            String::from_utf8_lossy(&failing.stderr)
        );
        assert_eq!(failing.status.code(), Some(1), "step {fail_at}: {said}");
        assert!(said.contains("simulated write error"), "{said}");
        let written = kinds(&trace_lines(&trace)).len();
        assert_eq!(
            written, fail_at,
            "exactly the writes before step {fail_at} were made"
        );
        // Claims are what `sync pending` lists: nothing is accepted until the claim (the last write) is made.
        assert!(
            pending(&repo).contains("patches: 0"),
            "step {fail_at}: nothing is pending before the claim: {}",
            pending(&repo)
        );

        // The same accept, again: it finishes, writing only what is missing.
        let rerun = accept(&repo, &artifact, &[]);
        support::ok(&rerun, "the rerun");
        let out = String::from_utf8_lossy(&rerun.stdout);
        assert!(out.contains("claims: 1"), "{out}");
        assert!(
            pending(&repo).contains("patch"),
            "after the rerun the patch is pending"
        );
        let verify = support::prikk(&repo).arg("verify").output().unwrap();
        assert_eq!(verify.status.code(), Some(0), "verify after the rerun");
    }
}

/// A non-regular input (a FIFO) still works: it cannot be read twice, so it is buffered once under the same bound. Unix only.
#[cfg(unix)]
#[test]
fn a_pipe_is_accepted_buffered_once() {
    use std::io::Write as _;
    use std::process::Stdio;
    let artifact = artifact();
    let bytes = std::fs::read(&artifact).unwrap();
    let repo = receiver("p3b-pipe");
    let fifo = file("fifo");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let child = support::prikk(&repo)
        .args(["sync", "accept", fifo.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut writer = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    support::ok(&output, "sync accept from a FIFO");
    assert!(String::from_utf8_lossy(&output.stdout).contains("patches: 1"));
    let _ = std::fs::remove_file(&fifo);
}
