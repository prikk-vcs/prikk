//! 0.58.0 step 0, part 0b: the readings around an interrupted seal, through the binary, in the state the architect's probes make
//! (no failpoint): a sound `seal` followed by putting the pre-seal `queue.wal`, `ref-name` and `witness` back, which is what a
//! `seal` killed after it published leaves ("cleanup-pending"). In that state the queue is exactly the patches of the Block the
//! ref points at.
//!
//! The case table (measured on the published 0.57.0, then on the build; `.git-exclude/logs/0.58.0-step0-0b/casetable-*.txt`):
//!
//! | # | command, state | 0.57.0 | now |
//! |---|---|---|---|
//! | 1 | `status --format json`, cleanup-pending | exit 1, `integrity error: lifecycle replay … node_id is already live` | exit 0; `queue.already_sealed` names the Block and `prikk seal --allow-no-audit`; the patches are listed |
//! | 2 | `doctor`, cleanup-pending | `way out [a ref's pointer and log disagree]` over a finding that says they agree | `way out [a seal stopped after it published, before it cleaned the queue]` |
//! | 3 | `doctor` / `verify`, empty WAL, stale `ref-name` | `warning`, "no repair is required" | `info` / `note`, naming the `prikk commit` that replaces it; same for malformed metadata |
//! | 4 | `uncommit`, cleanup-pending | `… use signer-backed seal retry` | names `prikk seal --allow-no-audit` |
//! | 5 | `seal --allow-no-audit`, nothing queued, stale `ref-name` | removed the `ref-name`, then exit 1 | exit 1 with `.prikk` byte-identical |

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support;
use crate::support::ok;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn active(repo: &Path, name: &str) -> PathBuf {
    repo.join(".prikk/active/default").join(name)
}

/// Every file under `.prikk` except lock files, as path -> bytes: what "nothing was written" is compared against.
fn digest(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("lock"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn patch_id_of(commit: &Output) -> String {
    String::from_utf8_lossy(&commit.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints its patch id")
        .to_string()
}

/// A repository in the cleanup-pending state: one sealed base generation, two queued commits (one creates `b.md` and edits
/// `a.md`, the next edits `b.md`), a sound `seal`, then the pre-seal active state put back. Returns the repository, the two patch
/// ids, and the Block the seal published.
fn cleanup_pending(tag: &str) -> (PathBuf, [String; 2], String) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.md", b"one\n", "base");
    std::fs::write(repo.join("a.md"), b"one\ntwo\n").unwrap();
    std::fs::write(repo.join("b.md"), b"three\n").unwrap();
    let first = support::commit(&repo, "heads/main", "c1");
    ok(&first, "commit c1");
    std::fs::write(repo.join("b.md"), b"three\nfour\n").unwrap();
    let second = support::commit(&repo, "heads/main", "c2");
    ok(&second, "commit c2");
    let saved: Vec<(String, Vec<u8>)> = ["queue.wal", "ref-name", "witness"]
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                std::fs::read(active(&repo, name)).unwrap(),
            )
        })
        .collect();
    ok(&support::seal(&repo, "heads/main"), "seal");
    for (name, bytes) in &saved {
        std::fs::write(active(&repo, name), bytes).unwrap();
    }
    let log = support::prikk(&repo)
        .args([
            "log",
            "--ref",
            "heads/main",
            "--limit",
            "1",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let json = String::from_utf8_lossy(&log.stdout).to_string();
    let marker = "\"block_id\": \"";
    let at = json.find(marker).expect("a block in the log") + marker.len();
    let block = json[at..at + 64].to_string();
    (repo, [patch_id_of(&first), patch_id_of(&second)], block)
}

// ---- row 1 -------------------------------------------------------------------------------------------------------------

#[test]
fn row1_status_json_in_the_cleanup_pending_state_lists_the_queue_and_names_the_state() {
    let (repo, patches, block) = cleanup_pending("s0b-row1");
    let output = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let json = support::json::parse(&String::from_utf8_lossy(&output.stdout));
    let queue = json.get("queue");
    assert_eq!(queue.get("patches").as_array().len(), 2);
    let listed: Vec<String> = queue
        .get("patches")
        .as_array()
        .iter()
        .map(|patch| patch.get("patch_id").as_str().to_string())
        .collect();
    assert_eq!(listed, patches, "the queued patch ids, in queue order");
    let sealed = queue.get("already_sealed");
    assert_eq!(
        sealed.get("block_id").as_str(),
        block,
        "the Block the seal published"
    );
    assert_eq!(sealed.get("route").as_str(), "prikk seal --allow-no-audit");
    // The operations resolve against what the queue was authored against (the Block's parent): `a.md` and `b.md`.
    let paths: Vec<String> = queue
        .get("patches")
        .as_array()
        .iter()
        .flat_map(|patch| patch.get("operations").as_array().to_vec())
        .flat_map(|operation| operation.get("paths").as_array().to_vec())
        .map(|path| path.get("path").as_str().to_string())
        .collect();
    assert_eq!(paths, ["b.md", "a.md", "b.md"], "{paths:?}");
    // The named route works: the retry cleans the queue, and the field is then null.
    ok(&support::seal(&repo, "heads/main"), "seal retry");
    let after = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    let after_json = support::json::parse(&String::from_utf8_lossy(&after.stdout));
    assert!(after_json.get("queue").get("patches").as_array().is_empty());
    assert!(after_json.get("queue").get("already_sealed").is_null());
    let _ = std::fs::remove_dir_all(&repo);
}

/// The field is `null` (present, never omitted) for an ordinary queue and for an empty one.
#[test]
fn row1_already_sealed_is_null_for_an_ordinary_queue() {
    let repo = support::unique_repo("s0b-row1-ordinary");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.md", b"one\n", "base");
    std::fs::write(repo.join("c.md"), b"new\n").unwrap();
    ok(&support::commit(&repo, "heads/main", "queued"), "commit");
    let output = support::prikk(&repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    ok(&output, "status --format json");
    let json = support::json::parse(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(json.get("queue").get("patches").as_array().len(), 1);
    assert!(json.get("queue").get("already_sealed").is_null());
    let _ = std::fs::remove_dir_all(&repo);
}

// ---- row 2 -------------------------------------------------------------------------------------------------------------

#[test]
fn row2_doctor_and_verify_name_the_state_not_a_disagreement() {
    let (repo, _, _) = cleanup_pending("s0b-row2");
    let doctor = support::prikk(&repo).arg("doctor").output().unwrap();
    let said = text(&doctor);
    assert!(
        said.contains("way out [a seal stopped after it published, before it cleaned the queue]: "),
        "{said}"
    );
    assert!(
        !said.contains("way out [a ref's pointer and log disagree]"),
        "this state is not a disagreement: {said}"
    );
    assert!(said.contains("prikk seal --allow-no-audit"), "{said}");
    let verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert!(
        text(&verify)
            .contains("way out [a seal stopped after it published, before it cleaned the queue]"),
        "{}",
        text(&verify)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

// ---- row 3 -------------------------------------------------------------------------------------------------------------

/// An empty WAL with `ref-name` left over: `doctor` gives information, `verify` a note, both exit 0 and both name the `prikk
/// commit` that replaces it; the next `commit` does.
#[test]
fn row3_stale_ref_name_is_information_and_the_next_commit_replaces_it() {
    for (label, stale) in [
        ("valid name", b"heads/main".to_vec()),
        ("malformed", b"not a ref \xff\n".to_vec()),
    ] {
        let repo = support::unique_repo("s0b-row3");
        support::init(&repo);
        support::generation(&repo, "heads/main", "a.md", b"one\n", "base");
        std::fs::write(active(&repo, "ref-name"), &stale).unwrap();

        let doctor = support::prikk(&repo).arg("doctor").output().unwrap();
        let said = text(&doctor);
        assert_eq!(doctor.status.code(), Some(0), "{label}: {said}");
        let line = said
            .lines()
            .find(|line| line.contains("REF-METADATA-") && line.contains("DEBRIS"))
            .unwrap_or_else(|| panic!("{label}: a debris finding: {said}"));
        assert!(
            line.starts_with("info ["),
            "{label}: information, not a warning: {line}"
        );
        assert!(
            said.contains("nothing to do: the next `prikk commit` replaces it"),
            "{label}: {said}"
        );
        let verify = support::prikk(&repo).arg("verify").output().unwrap();
        let verify_said = text(&verify);
        assert_eq!(verify.status.code(), Some(0), "{label}: {verify_said}");
        assert!(
            verify_said.contains(
                "note: active WAL ref metadata exists but the active WAL is empty; the next `prikk commit` replaces it"
            ),
            "{label}: {verify_said}"
        );
        assert!(
            !verify_said.contains("warning: active WAL ref metadata"),
            "{label}: {verify_said}"
        );

        std::fs::write(repo.join("n.md"), b"next\n").unwrap();
        ok(
            &support::commit(&repo, "heads/main", "next"),
            "the next commit",
        );
        let after = text(&support::prikk(&repo).arg("doctor").output().unwrap());
        assert!(
            !after.contains("DEBRIS"),
            "{label}: the commit replaced it: {after}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

// ---- row 4 -------------------------------------------------------------------------------------------------------------

#[test]
fn row4_uncommit_in_the_cleanup_pending_state_names_the_seal_that_finishes_it() {
    let (repo, patches, block) = cleanup_pending("s0b-row4");
    let before = digest(&repo);
    for plan_only in [false, true] {
        let mut args = vec!["uncommit", patches[1].as_str()];
        if plan_only {
            args.push("--plan-only");
        }
        let output = support::prikk(&repo).args(&args).output().unwrap();
        let said = text(&output);
        assert_eq!(output.status.code(), Some(1), "{said}");
        assert!(said.contains("prikk seal --allow-no-audit"), "{said}");
        assert!(
            said.contains(&block),
            "names the Block the queue is already sealed in: {said}"
        );
        assert!(!said.contains("signer-backed seal retry"), "{said}");
    }
    assert_eq!(digest(&repo), before, "a refusal writes nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

// ---- row 5 -------------------------------------------------------------------------------------------------------------

#[test]
fn row5_seal_with_nothing_queued_refuses_before_any_write() {
    let repo = support::unique_repo("s0b-row5");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.md", b"one\n", "base");
    std::fs::write(active(&repo, "ref-name"), b"heads/main").unwrap();
    let before = digest(&repo);
    let output = support::seal(&repo, "heads/main");
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains("active WAL has no patch records to seal"),
        "{said}"
    );
    assert_eq!(
        digest(&repo),
        before,
        "the refusal wrote nothing: the stale ref-name is still there (row 3's information, replaced by the next commit)"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
