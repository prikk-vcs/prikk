//! RFC 142 §7a / RFC 140, on stikk's letter 011: a queued patch carries its message in
//! `status --format json`, and `show` addresses a queued patch -- rendering it as it will render once
//! sealed. Every command runs through `support::prikk`, the isolation seam.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::json;

const MESSAGE: &str = "add a.txt, queued";

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A repository with one committed, unsealed patch creating `a.txt`; returns its patch id.
fn queued_repo(tag: &str) -> (PathBuf, String) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    let commit = support::commit(&repo, "heads/main", MESSAGE);
    support::ok(&commit, "commit");
    let id = stdout(&commit)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints the patch id")
        .trim()
        .to_string();
    (repo, id)
}

fn show(repo: &Path, id: &str, json: bool) -> Output {
    let mut command = support::prikk(repo);
    command.args(["show", id]);
    if json {
        command.args(["--format", "json"]);
    }
    command.output().unwrap()
}

#[test]
fn a_queued_patch_entry_carries_its_message() {
    let (repo, id) = queued_repo("rfc142q-message");
    let out = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "status --format json");
    let report = json::parse(&stdout(&out));
    let patch = &report.get("queue").get("patches").as_array()[0];
    assert_eq!(patch.get("patch_id").as_str(), id);
    assert_eq!(patch.get("message").as_str(), MESSAGE);

    // RFC 140 §5: the prose `status` still enumerates nothing, so it gains no message line.
    let prose = support::prikk(&repo).arg("status").output().unwrap();
    support::ok(&prose, "status");
    assert!(!stdout(&prose).contains(MESSAGE), "{}", stdout(&prose));
}

/// `show` on the queued id renders; after `seal`, the same id renders the same document with only the
/// `queued` fact changed -- the queued rendering is the sealed one, not a lookalike.
#[test]
fn show_addresses_a_queued_patch_as_it_will_render_once_sealed() {
    let (repo, id) = queued_repo("rfc142q-show");

    let queued_json = show(&repo, &id, true);
    support::ok(&queued_json, "show <queued id> --format json");
    let queued_json = stdout(&queued_json);
    assert!(queued_json.contains("\"queued\": true"), "{queued_json}");
    let document = json::parse(&queued_json);
    let operation = &document.get("patches").as_array()[0]
        .get("operations")
        .as_array()[0];
    assert_eq!(operation.get("kind").as_str(), "create-file");
    let status = json::parse(&stdout(
        &support::prikk(&repo)
            .args(["status", "--format", "json"])
            .output()
            .unwrap(),
    ));
    let queue_operation = &status.get("queue").get("patches").as_array()[0]
        .get("operations")
        .as_array()[0];
    assert_eq!(
        operation.get("paths").as_array()[0].get("path").as_str(),
        queue_operation.get("paths").as_array()[0]
            .get("path")
            .as_str(),
        "show and the queue name the same path"
    );

    let queued_prose = show(&repo, &id, false);
    support::ok(&queued_prose, "show <queued id>");
    let queued_prose = stdout(&queued_prose);
    assert!(queued_prose.contains("queued: yes\n"), "{queued_prose}");

    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let sealed_json = show(&repo, &id, true);
    support::ok(&sealed_json, "show <sealed id> --format json");
    let sealed_json = stdout(&sealed_json);
    assert!(sealed_json.contains("\"queued\": false"), "{sealed_json}");
    assert_eq!(
        queued_json.replace("\"queued\": true", "\"queued\": false"),
        sealed_json,
        "the queued rendering must equal the sealed one apart from `queued`"
    );

    let sealed_prose = stdout(&show(&repo, &id, false));
    assert!(!sealed_prose.contains("queued:"), "{sealed_prose}");
    assert_eq!(queued_prose.replace("queued: yes\n", ""), sealed_prose);
}

/// An id in neither the object store nor the active WAL is caller-fixable: a precondition naming both
/// places and both routes, and never reported as damage.
#[test]
fn an_id_found_nowhere_is_a_precondition_naming_both_places() {
    let (repo, _id) = queued_repo("rfc142q-nowhere");
    let out = show(&repo, &"0".repeat(64), false);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let message = stderr(&out);
    for needle in [
        "precondition not met: no object",
        "in the object store or the active WAL",
        "prikk status --format json",
        "prikk log",
    ] {
        assert!(message.contains(needle), "{needle:?} not in {message}");
    }
    assert!(!message.contains("integrity error"), "{message}");
}
