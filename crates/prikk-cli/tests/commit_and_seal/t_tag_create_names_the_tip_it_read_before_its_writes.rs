//! 0.60.0 T: the rule `tag create --target <ref>` states, held by a test: **it names the ref's tip as the command reads it, before it writes.**
//!
//! A publication that completes while the command waits for a container lock to write its Tag and publish is not reflected (external review 058
//! §2.5, measured in 0.60.0 T: 24 of 31 waited tag creates ended at the old tip, every one waiting for the object-store or pointer-index lock).
//! `branch create --from <ref>` takes the active lock, which a `seal` holds for its whole run, so it waits for the seal and names the published tip.
//!
//! The seal here is stopped by `prikk-store`'s `test-support` hook `PRIKK_TEST_PAUSE_BEFORE_POINTER`: alive, holding the pointer-index and ref-log
//! locks, with `heads/main` still at the old tip. Linux and macOS only (the liveness judge reads `/proc` or `ps`).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

use crate::support;

fn tip(repo: &Path, ref_name: &str) -> String {
    let out = support::prikk(repo)
        .args(["log", "--ref", ref_name, "--limit", "1", "--format", "json"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let at = text.find("\"block_id\": \"").expect("a block id") + "\"block_id\": \"".len();
    text[at..at + 64].to_string()
}

fn maintainer_env(command: &mut std::process::Command) -> &mut std::process::Command {
    command
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
}

/// A repository with two sealed generations and a queued commit on `heads/main`; the `seal` of that commit stopped before its pointer write.
fn repository_with_a_seal_stopped_before_its_pointer(
    label: &str,
) -> (PathBuf, String, Child, PathBuf) {
    let repo = support::unique_repo(label);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "c1");
    support::generation(&repo, "heads/main", "b.txt", b"b\n", "c2");
    let old_tip = tip(&repo, "heads/main");
    std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued"), "queued");
    support::trust_maintainer(&repo);
    let go = std::env::temp_dir().join(format!("t2-go-{}", support::unique_suffix()));
    let mut waiting = go.clone().into_os_string();
    waiting.push(".waiting");
    let waiting = PathBuf::from(waiting);
    let mut command = support::prikk(&repo);
    maintainer_env(&mut command)
        .env("PRIKK_TEST_PAUSE_BEFORE_POINTER", &go)
        .args(["seal", "--allow-no-audit", "--ref", "heads/main"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    let started = Instant::now();
    while !waiting.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the seal never reached its pause"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(&waiting);
    (repo, old_tip, child, go)
}

fn release(seal: Child, go: &Path) -> Output {
    std::fs::write(go, b"go").unwrap();
    let output = seal.wait_with_output().unwrap();
    let _ = std::fs::remove_file(go);
    output
}

fn printed_target(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("target block: "))
        .expect("the command prints its target block")
        .to_string()
}

/// **The tag names the tip from before the seal, and what it printed is what it wrote.** The `tag create` waits for the pointer-index lock the stopped
/// seal holds (its stderr says so after a second), the seal is let go, the seal publishes the new tip, and the tag is written at the tip it read.
/// Perturb: the target resolved after the wait (here: `tag create` taking the active lock first, as `branch create` does): the tag names the new
/// tip, and this is red.
#[test]
fn a_tag_that_waits_for_a_container_lock_names_the_tip_it_read_before_its_writes() {
    let (repo, old_tip, seal, go) =
        repository_with_a_seal_stopped_before_its_pointer("t2-snapshot");
    let mut command = support::prikk(&repo);
    let tag = maintainer_env(&mut command)
        .env("PRIKK_LOCK_WAIT_SECONDS", "20")
        .args(["tag", "create", "tags/late", "--target", "heads/main"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Long enough for the one stderr line a wait of a second or more prints.
    std::thread::sleep(Duration::from_millis(2_500));
    let sealed = release(seal, &go);
    support::ok(&sealed, "the seal that was stopped");
    let output = tag.wait_with_output().unwrap();
    support::ok(&output, "the tag create that waited");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("waiting for the container:ref-pointer-index lock held by pid"),
        "it waited for the container lock the seal held: {stderr}"
    );
    let new_tip = tip(&repo, "heads/main");
    assert_ne!(
        new_tip, old_tip,
        "the seal published a new tip while the tag waited"
    );
    assert_eq!(
        printed_target(&output),
        old_tip,
        "the line it printed names the tip it read"
    );
    let tags = support::prikk(&repo)
        .args(["tag", "list", "--format", "json"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&tags.stdout).into_owned();
    let at = listing.find("tags/late").expect("the tag is listed");
    assert!(
        listing[at..].contains(&format!("\"target_block_id\": \"{old_tip}\"")),
        "the tag is at the tip it read, not the published one: {listing}"
    );
    assert_eq!(support::verify(&repo).status.code(), Some(0));
}
