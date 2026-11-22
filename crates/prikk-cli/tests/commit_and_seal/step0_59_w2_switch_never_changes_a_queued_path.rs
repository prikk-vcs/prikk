//! 0.59.0 step W2, through the compiled binary (`rfcs/handoffs/147-refusal-visibility/0.59.0-step-0-handoff-v1.md`, part W; the
//! design is `.git-exclude/review-request/0.59.0-step-0-W-part-1-report.md` §3-§4, ruled in its review).
//!
//! **The rule:** a branch switch never changes, on disk, a path the target's queue holds. 0.56.0 .. 0.58.0 judged "the branch being
//! left's files" against the worktree's record, which `commit --ref <target>` writes with the queue folded on top; so a queued
//! new file was deleted, a queued edit reverted, a queued deletion written back, a queued rename undone, exit 0. Now the switch
//! refuses before any write and names `seal --ref` (publishes the work, keeps the files) and `uncommit <id>` (takes it back).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::{Path, PathBuf};
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn prikk(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn switch(repo: &Path, target: &str) -> Output {
    prikk(repo, &["branch", "switch", target])
}

/// `heads/main` sealed with `a.txt` (`base`) and `k.txt` (`keep`), `heads/feature` created from it, `heads/main` current.
fn fixture(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"base\n", "base a");
    support::generation(&repo, "heads/main", "k.txt", b"keep\n", "base k");
    ok(
        &support::branch_create(&repo, "heads/feature", "heads/main"),
        "branch create heads/feature",
    );
    repo
}

fn read(repo: &Path, path: &str) -> Option<Vec<u8>> {
    std::fs::read(repo.join(path)).ok()
}

fn pointer(repo: &Path) -> String {
    std::fs::read_to_string(repo.join(".prikk/current-branch")).unwrap()
}

fn queue_count(repo: &Path) -> usize {
    let status = stdout(&prikk(repo, &["status", "--format", "json"]));
    let after = status.split("\"count\":").nth(1).expect("queue count");
    after
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// The id `uncommit` takes, read from the refusal: the sentence must name something typeable.
fn named_uncommit_id(refusal: &str) -> String {
    let after = refusal
        .split("`prikk uncommit ")
        .nth(1)
        .unwrap_or_else(|| panic!("no `prikk uncommit <id>` in {refusal}"));
    let id: String = after.chars().take_while(char::is_ascii_hexdigit).collect();
    assert_eq!(id.len(), 64, "a full patch id: {refusal}");
    id
}

/// The four defect cases, as the work the user queues for `heads/feature` from the `heads/main` worktree.
#[derive(Clone, Copy, Debug)]
enum Case {
    NewFile,
    Edit,
    Deletion,
    Rename,
}

impl Case {
    fn queue(self, repo: &Path) {
        match self {
            Case::NewFile => std::fs::write(repo.join("queued.txt"), b"q\n").unwrap(),
            Case::Edit => std::fs::write(repo.join("a.txt"), b"base\nedit\n").unwrap(),
            Case::Deletion => std::fs::remove_file(repo.join("k.txt")).unwrap(),
            Case::Rename => ok(&prikk(repo, &["mv", "a.txt", "r.txt"]), "mv a.txt r.txt"),
        }
        ok(
            &support::commit(repo, "heads/feature", "queued for feature"),
            "commit --ref heads/feature",
        );
    }

    /// The paths the refusal must name.
    fn paths(self) -> &'static [&'static str] {
        match self {
            Case::NewFile => &["queued.txt"],
            Case::Edit => &["a.txt"],
            Case::Deletion => &["k.txt"],
            Case::Rename => &["a.txt", "r.txt"],
        }
    }

    /// The worktree exactly as the user left it: `(path, bytes or absent)`.
    fn disk(self) -> Vec<(&'static str, Option<&'static [u8]>)> {
        match self {
            Case::NewFile => vec![
                ("a.txt", Some(&b"base\n"[..])),
                ("queued.txt", Some(&b"q\n"[..])),
            ],
            Case::Edit => vec![("a.txt", Some(&b"base\nedit\n"[..]))],
            Case::Deletion => vec![("a.txt", Some(&b"base\n"[..])), ("k.txt", None)],
            Case::Rename => vec![("a.txt", None), ("r.txt", Some(&b"base\n"[..]))],
        }
    }
}

const CASES: [Case; 4] = [Case::NewFile, Case::Edit, Case::Deletion, Case::Rename];

fn assert_disk(repo: &Path, case: Case) {
    for (path, bytes) in case.disk() {
        assert_eq!(
            read(repo, path).as_deref(),
            bytes,
            "{case:?}: {path} on disk"
        );
    }
}

/// The switch refuses before any write: exit 1, the sentence, the files as they were, the pointer and the record untouched.
#[test]
fn a_switch_to_the_branch_whose_queue_holds_a_path_refuses_before_any_write() {
    for case in CASES {
        let repo = fixture(&format!("w2-refuse-{case:?}"));
        case.queue(&repo);
        let record = std::fs::read(repo.join(".prikk/worktree.baseline")).unwrap();

        let out = switch(&repo, "heads/feature");
        assert_eq!(out.status.code(), Some(1), "{case:?}: {}", stderr(&out));
        let message = stderr(&out);
        assert!(
            message.contains(
                "refusing to switch to heads/feature: its queue holds 1 commit(s) that change "
            ),
            "{case:?}: {message}"
        );
        for path in case.paths() {
            assert!(message.contains(path), "{case:?}: {path} not in {message}");
        }
        assert!(
            message.contains("which the switch would overwrite or delete on disk (nothing was written)")
                && message.contains(
                    "`prikk seal --allow-no-audit --ref heads/feature` publishes them and keeps your files"
                )
                && message.contains("takes the newest commit back and leaves your files"),
            "{case:?}: {message}"
        );
        assert!(
            stdout(&out).is_empty(),
            "{case:?}: a refusal prints nothing on stdout"
        );
        assert_disk(&repo, case);
        assert_eq!(pointer(&repo), "heads/main\n", "{case:?}");
        assert_eq!(
            std::fs::read(repo.join(".prikk/worktree.baseline")).unwrap(),
            record,
            "{case:?}: the record is untouched"
        );
        assert_eq!(queue_count(&repo), 1, "{case:?}: the queue is untouched");
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// The first way out the sentence names: `seal --ref heads/feature`, then the switch: exit 0, every file kept as it was.
#[test]
fn seal_then_switch_keeps_every_file() {
    for case in CASES {
        let repo = fixture(&format!("w2-seal-{case:?}"));
        case.queue(&repo);
        assert_eq!(switch(&repo, "heads/feature").status.code(), Some(1));

        ok(
            &support::seal(&repo, "heads/feature"),
            "seal --ref heads/feature",
        );
        assert_disk(&repo, case);
        let out = switch(&repo, "heads/feature");
        ok(&out, &format!("{case:?}: the switch after the seal"));
        assert!(
            stdout(&out).contains("files written: 0") && stdout(&out).contains("files deleted: 0"),
            "{case:?}: {}",
            stdout(&out)
        );
        assert_disk(&repo, case);
        assert_eq!(pointer(&repo), "heads/feature\n");
        ok(
            &prikk(&repo, &["worktree-status"]),
            "worktree-status is clean",
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// The second way out: the id the sentence names is the newest commit and `uncommit` takes it back, leaving the files.
#[test]
fn the_named_uncommit_takes_the_work_back_and_leaves_the_files() {
    for case in CASES {
        let repo = fixture(&format!("w2-uncommit-{case:?}"));
        case.queue(&repo);
        let refusal = stderr(&switch(&repo, "heads/feature"));
        let id = named_uncommit_id(&refusal);

        ok(&prikk(&repo, &["uncommit", &id]), "the named uncommit");
        assert_disk(&repo, case);
        assert_eq!(queue_count(&repo), 0, "{case:?}: the queue is empty");
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// The clean row: a queue that creates a file and deletes it again holds no path the switch changes; the switch is as before.
#[test]
fn a_queue_the_switch_does_not_touch_switches_as_before() {
    let repo = fixture("w2-clean");
    std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "add"),
        "commit add",
    );
    std::fs::remove_file(repo.join("queued.txt")).unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "remove"),
        "commit remove",
    );
    let out = switch(&repo, "heads/feature");
    ok(&out, "the switch");
    assert!(
        stdout(&out).contains("files written: 0") && stdout(&out).contains("files deleted: 0"),
        "{}",
        stdout(&out)
    );
    assert_eq!(pointer(&repo), "heads/feature\n");
    // A queue that adds no file the worktree lacks keeps `worktree-status`'s sentence as it was: no "adding 0 file(s)".
    let status = stdout(&prikk(&repo, &["worktree-status"]));
    assert!(
        !status.contains("its queue holds"),
        "the queue adds nothing: {status}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The third-branch row: a queue for another ref is refusal 2's sentence, unchanged.
#[test]
fn a_queue_for_a_third_branch_is_refused_as_before() {
    let repo = fixture("w2-third");
    ok(
        &support::branch_create(&repo, "heads/other", "heads/main"),
        "branch create heads/other",
    );
    std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
    ok(
        &support::commit(&repo, "heads/other", "q"),
        "commit --ref heads/other",
    );
    let out = switch(&repo, "heads/feature");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "the active WAL holds unsealed work for heads/other; seal it (`prikk seal --allow-no-audit --ref heads/other`) before switching to heads/feature"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(read(&repo, "queued.txt").as_deref(), Some(&b"q\n"[..]));
    let _ = std::fs::remove_dir_all(&repo);
}

/// A mode the switch would reset on a file it leaves byte-identical is a change to a path the queue holds.
#[cfg(unix)]
#[test]
fn a_queued_mode_change_is_never_reset_by_the_switch() {
    use std::os::unix::fs::PermissionsExt;
    let repo = fixture("w2-mode");
    let path = repo.join("a.txt");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "chmod"),
        "commit --ref heads/feature",
    );
    let out = switch(&repo, "heads/feature");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("that change a.txt"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755,
        "the mode is as the user left it"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The state 0.58.0's bad switch left behind, reached without a switch: the record names the branch's tip with no queue, and the
/// branch's queue holds a file this worktree lacks. `commit`'s sentence no longer contradicts itself, `worktree-status` blames the
/// queue and not a sealed block, both name `seal --ref` first, and the route they name works.
#[test]
fn a_worktree_behind_the_queue_says_so_and_the_route_it_names_works() {
    let repo = fixture("w2-behind-queue");
    std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "q"),
        "commit --ref heads/feature",
    );
    // The record `commit --ref` wrote names the tip with the queue folded; make it name the tip alone, as a switch of 0.58.0 left it.
    let record = std::fs::read_to_string(repo.join(".prikk/worktree.baseline")).unwrap();
    let fields: Vec<&str> = record.split_whitespace().collect();
    assert_eq!(fields[0], "PRIKK-WORKTREE-BASELINE-v2");
    let empty_digest = {
        // The empty-fold digest is the one every record without a queue carries: a fresh repository's.
        let probe = fixture("w2-behind-queue-probe");
        let text = std::fs::read_to_string(probe.join(".prikk/worktree.baseline")).unwrap();
        let _ = std::fs::remove_dir_all(&probe);
        text.split_whitespace().nth(4).unwrap().to_string()
    };
    std::fs::write(
        repo.join(".prikk/worktree.baseline"),
        format!(
            "{} {} {} 0 {empty_digest}\n",
            fields[0], fields[1], fields[2]
        ),
    )
    .unwrap();
    std::fs::remove_file(repo.join("queued.txt")).unwrap();

    let refused = support::commit(&repo, "heads/feature", "again");
    assert_eq!(refused.status.code(), Some(1));
    let message = stderr(&refused);
    assert!(
        message.contains("heads/feature's queue holds 1 commit(s) this worktree was not written with: it is now at Block "),
        "{message}"
    );
    assert!(
        message.contains(
            " with 1 queued patch(es) folded on top, and this worktree was last written from Block "
        ),
        "the live side counts its queue: {message}"
    );
    let seal_at = message
        .find("`prikk seal --allow-no-audit --ref heads/feature`")
        .expect("names seal");
    let switch_at = message
        .find("`prikk branch switch heads/feature`")
        .expect("names the switch");
    assert!(
        seal_at < switch_at,
        "seal first, then the switch: {message}"
    );

    let status = prikk(&repo, &["worktree-status", "--ref", "heads/feature"]);
    let text = stdout(&status);
    assert!(
        text.contains(
            "worktree: behind heads/feature (its queue holds 1 commit(s) this worktree was not written with, adding 1 file(s) not here yet; not a deletion); run `prikk seal --allow-no-audit --ref heads/feature`, then `prikk branch switch heads/feature`, to catch up"
        ),
        "{text}"
    );
    let json = stdout(&prikk(
        &repo,
        &[
            "worktree-status",
            "--ref",
            "heads/feature",
            "--format",
            "json",
        ],
    ));
    assert!(json.contains("\"behind_ref\": \"heads/feature\""), "{json}");

    // The route: seal, then the switch the text names.
    ok(
        &support::seal(&repo, "heads/feature"),
        "seal --ref heads/feature",
    );
    let out = switch(&repo, "heads/feature");
    ok(&out, "the switch after the seal");
    assert_eq!(read(&repo, "queued.txt").as_deref(), Some(&b"q\n"[..]));
    ok(
        &prikk(&repo, &["worktree-status"]),
        "worktree-status after the route",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// With a switch marked interrupted, `uncommit` itself refuses (and names the switch), so the switch's refusal must not name it:
/// a way out that refuses is the failure this part removes. `seal --ref` then the switch works and clears the marker.
#[test]
fn an_interrupted_switch_does_not_name_the_uncommit_that_refuses() {
    let repo = fixture("w2-marked");
    Case::NewFile.queue(&repo);
    std::fs::write(
        repo.join(".prikk/worktree.marker"),
        b"PRIKK-WORKTREE-DIRTY\ntarget switch heads/feature\n",
    )
    .unwrap();
    let out = switch(&repo, "heads/feature");
    assert_eq!(out.status.code(), Some(1));
    let message = stderr(&out);
    assert!(
        message.contains("publishes them and keeps your files; run the switch again after it")
            && !message.contains("uncommit"),
        "{message}"
    );
    assert_eq!(read(&repo, "queued.txt").as_deref(), Some(&b"q\n"[..]));
    ok(
        &support::seal(&repo, "heads/feature"),
        "seal --ref heads/feature",
    );
    ok(&switch(&repo, "heads/feature"), "the switch after the seal");
    assert_eq!(
        std::fs::read(repo.join(".prikk/worktree.marker"))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(read(&repo, "queued.txt").as_deref(), Some(&b"q\n"[..]));
    assert_eq!(pointer(&repo), "heads/feature\n");
    let _ = std::fs::remove_dir_all(&repo);
}
