//! 0.58.0 F2: a publication **in flight** is not an interrupted one.
//!
//! A `tag create` is stopped by `prikk-store`'s `test-support` hook (`PRIKK_TEST_PAUSE_AFTER_POINTER=<go>`: it writes `<go>.waiting` after its
//! pointer is written and before its log record, and waits for `<go>`). While it waits, its pointer leads the log and it holds the ref's lock,
//! alive: the state the write-path precondition, `ref complete`, `verify` and `doctor` used to read as an interrupted publication.
//! Killed (`SIGKILL`: nothing is cleaned up), the same bytes are an interrupted one and keep today's answers. Linux and macOS only (the pid of a
//! lock's holder is judged by `/proc` or `ps`; the hook's wait is the same everywhere, but the liveness judge is what makes the difference).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

use crate::support;

struct InFlight {
    child: Child,
    go: PathBuf,
    waiting: PathBuf,
}

impl InFlight {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Let it finish and return its output.
    fn release(self) -> Output {
        std::fs::write(&self.go, b"go").unwrap();
        let output = self.child.wait_with_output().unwrap();
        let _ = std::fs::remove_file(&self.go);
        let _ = std::fs::remove_file(&self.waiting);
        output
    }

    /// `SIGKILL` it: its locks stay, its pointer leads the log.
    fn kill(mut self) {
        self.child.kill().unwrap();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.waiting);
    }
}

/// A repository with two sealed generations and `extra` queued commits, and a `tag create tags/t1 --target heads/main` stopped between its
/// pointer write and its log append.
fn in_flight_tag(tag: &str, queued: bool) -> (PathBuf, InFlight) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "c1");
    support::generation(&repo, "heads/main", "b.txt", b"b\n", "c2");
    if queued {
        std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
        support::ok(&support::commit(&repo, "heads/main", "queued"), "queued");
    }
    support::trust_maintainer(&repo);
    let go = std::env::temp_dir().join(format!("f2-go-{}", support::unique_suffix()));
    let mut waiting = go.clone().into_os_string();
    waiting.push(".waiting");
    let waiting = PathBuf::from(waiting);
    let mut command = support::prikk(&repo);
    command
        .env("PRIKK_TEST_PAUSE_AFTER_POINTER", &go)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["tag", "create", "tags/t1", "--target", "heads/main"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    let started = Instant::now();
    while !waiting.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the tag create never reached its pause"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    (repo, InFlight { child, go, waiting })
}

fn ref_lock(repo: &Path) -> PathBuf {
    let dir = repo.join(".prikk/refs/locks");
    let mut locks: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "lock"))
        .collect();
    assert_eq!(locks.len(), 1, "the publisher's ref lock: {locks:?}");
    locks.remove(0)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The in-flight refusal's words, and that it is not the interrupted-publication refusal.
fn assert_in_flight_refusal(output: &Output, pid: u32, what: &str) {
    let text = stderr(output);
    assert_eq!(output.status.code(), Some(1), "{what}: {text}");
    assert!(
        text.contains(&format!(
            "another command is publishing tags/t1 (pid {pid}, from its lock)"
        )),
        "{what}: names the ref and the other command's pid: {text}"
    );
    assert!(
        text.contains("run this command again when it finishes"),
        "{what}: {text}"
    );
    assert!(!text.contains("interrupted"), "{what}: {text}");
    assert!(!text.contains("ref complete"), "{what}: {text}");
    assert!(!text.contains("signer-backed"), "{what}: {text}");
}

/// Every writer kind the precondition gates is told the truth, before any write.
#[test]
fn every_writer_is_told_a_publication_is_in_flight_and_writes_nothing() {
    let (repo, publisher) = in_flight_tag("f2-writers", true);
    let pid = publisher.pid();
    let before = support::store_bytes(&repo);

    // `commit` (a non-publishing writer, gated by the same precondition).
    std::fs::write(repo.join("more.txt"), b"m\n").unwrap();
    let commit = support::commit(&repo, "heads/main", "more");
    assert_in_flight_refusal(&commit, pid, "commit");
    // `seal` of the queued commit (another ref).
    let seal = support::seal(&repo, "heads/main");
    assert_in_flight_refusal(&seal, pid, "seal");
    // `branch create` and `tag create` of another ref.
    let branch = support::branch_create(&repo, "heads/x", "heads/main");
    assert_in_flight_refusal(&branch, pid, "branch create");
    let tag = support::tag_create(&repo, "tags/t2", "heads/main");
    assert_in_flight_refusal(&tag, pid, "tag create (another ref)");

    assert_eq!(support::store_bytes(&repo), before, "nothing was written");
    let output = publisher.release();
    support::ok(&output, "the publisher finishes");
    // Once it has, the same writers go through.
    support::ok(
        &support::tag_create(&repo, "tags/t2", "heads/main"),
        "tag create after",
    );
}

/// The same ref: a second `tag create tags/t1` while the first is still publishing it is neither "already exists" nor "interrupted".
#[test]
fn the_same_ref_being_published_is_said_to_be_in_flight() {
    let (repo, publisher) = in_flight_tag("f2-same-ref", false);
    let pid = publisher.pid();
    let again = support::tag_create(&repo, "tags/t1", "heads/main");
    let text = stderr(&again);
    assert_eq!(again.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!(
            "tags/t1 is being published by another command right now (pid {pid}, from its lock)"
        )),
        "{text}"
    );
    assert!(!text.contains("already exists"), "{text}");
    assert!(!text.contains("interrupted"), "{text}");
    publisher.release();
}

/// `ref complete` of a ref being published says so, instead of printing a plan and meeting the lock.
#[test]
fn ref_complete_of_a_publication_in_flight_says_so() {
    let (repo, publisher) = in_flight_tag("f2-ref-complete", false);
    let pid = publisher.pid();
    let output = support::prikk(&repo)
        .args(["ref", "complete", "tags/t1"])
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .output()
        .unwrap();
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!(
            "tags/t1 is being published by another command right now (pid {pid}, from its lock)"
        )),
        "{text}"
    );
    assert!(
        text.contains("run this command again when it finishes"),
        "{text}"
    );
    assert!(!text.contains("lock conflict"), "{text}");
    assert!(!text.contains("unlock"), "{text}");
    assert!(
        !stdout(&output).contains("leading RefState"),
        "no plan is printed for a publication that is still running: {}",
        stdout(&output)
    );
    publisher.release();
}

/// `verify` and `doctor` while it is in flight: information, exit 0. After the publisher dies: today's finding, exit 1, and the lock it left.
#[test]
fn verify_and_doctor_see_the_difference_between_in_flight_and_interrupted() {
    let (repo, publisher) = in_flight_tag("f2-readers", false);
    let pid = publisher.pid();

    let verify = support::verify(&repo);
    let text = stdout(&verify);
    assert_eq!(verify.status.code(), Some(0), "{text}{}", stderr(&verify));
    assert!(
        text.contains("PRIKK-VERIFY-REF-PUBLICATION-IN-FLIGHT"),
        "{text}"
    );
    assert!(
        text.contains(&format!("a publication of tags/t1 is in flight (pid {pid}")),
        "{text}"
    );
    assert!(!text.contains("POINTER-LEADS-LOG"), "{text}");
    let doctor = support::prikk(&repo).arg("doctor").output().unwrap();
    let doctor_text = stdout(&doctor);
    assert_eq!(doctor.status.code(), Some(0), "{doctor_text}");
    assert!(
        doctor_text.contains("PRIKK-VERIFY-REF-PUBLICATION-IN-FLIGHT"),
        "{doctor_text}"
    );
    assert!(!doctor_text.contains("POINTER-LEADS-LOG"), "{doctor_text}");

    // Killed: the same bytes, an interrupted publication, with its lock still on disk and its holder judged.
    publisher.kill();
    let verify = support::verify(&repo);
    let text = stdout(&verify);
    assert_eq!(verify.status.code(), Some(1), "{text}");
    assert!(
        text.contains("PRIKK-VERIFY-REF-POINTER-LEADS-LOG"),
        "{text}"
    );
    assert!(!text.contains("PUBLICATION-IN-FLIGHT"), "{text}");
    assert!(
        text.contains("the lock it took is still on disk")
            && text.contains("does not appear to be running"),
        "{text}"
    );
    assert!(
        text.contains("run `prikk ref complete tags/t1`"),
        "a way out: {text}"
    );
    let commit_refusal = {
        std::fs::write(repo.join("more.txt"), b"m\n").unwrap();
        support::commit(&repo, "heads/main", "more")
    };
    let text = stderr(&commit_refusal);
    assert_eq!(commit_refusal.status.code(), Some(1), "{text}");
    assert!(
        text.contains("interrupted publication for tags/t1")
            && text.contains("run `prikk ref complete tags/t1`")
            && text.contains("does not appear to be running"),
        "{text}"
    );
    assert!(!text.contains("another command is publishing"), "{text}");
    // And the way out works: clear the dead holder's lock (the judge refuses to clear a live one), then complete.
    let lock = ref_lock(&repo);
    support::ok(
        &support::prikk(&repo)
            .args(["unlock", "--lock"])
            .arg(&lock)
            .arg("--yes")
            .output()
            .unwrap(),
        "unlock --yes (a dead holder)",
    );
    // The publisher also held the container locks, left behind by the kill.
    for container in ["pointer-index.lock", "log.lock"] {
        let path = repo.join(".prikk/refs/containers").join(container);
        if path.exists() {
            support::ok(
                &support::prikk(&repo)
                    .args(["unlock", "--lock"])
                    .arg(&path)
                    .arg("--yes")
                    .output()
                    .unwrap(),
                "unlock --yes (a container lock)",
            );
        }
    }
    support::ok(
        &support::prikk(&repo)
            .args(["ref", "complete", "tags/t1"])
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .output()
            .unwrap(),
        "ref complete",
    );
    support::ok(&support::verify(&repo), "verify after the completion");
}

/// A holder whose liveness cannot be judged (another pid namespace, as step 0a's tests write it) is not "in flight": only *appears running*
/// is. The answer is today's, with what the judge said.
#[cfg(target_os = "linux")]
#[test]
fn an_unknown_holder_keeps_todays_interrupted_answer_and_names_what_the_judge_said() {
    let (repo, publisher) = in_flight_tag("f2-unknown", false);
    publisher.kill();
    let body = prikk_store::lock_body_naming_pid_for_test("ref", std::process::id());
    let body: String = body
        .lines()
        .map(|line| {
            if line.starts_with("pidns=") {
                "pidns=pid:[1]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert!(body.contains("pidns=pid:[1]"), "{body}");
    std::fs::write(ref_lock(&repo), body).unwrap();

    std::fs::write(repo.join("more.txt"), b"m\n").unwrap();
    let commit = support::commit(&repo, "heads/main", "more");
    let text = stderr(&commit);
    assert_eq!(commit.status.code(), Some(1), "{text}");
    assert!(
        text.contains("interrupted publication for tags/t1"),
        "unknown is not in flight: {text}"
    );
    assert!(
        text.contains("unknown") && text.contains("another pid or time namespace"),
        "it adds what the judge said: {text}"
    );
    assert!(!text.contains("another command is publishing"), "{text}");
    let verify = support::verify(&repo);
    assert_eq!(verify.status.code(), Some(1), "{}", stdout(&verify));
    assert!(
        stdout(&verify).contains("PRIKK-VERIFY-REF-POINTER-LEADS-LOG"),
        "{}",
        stdout(&verify)
    );
}
