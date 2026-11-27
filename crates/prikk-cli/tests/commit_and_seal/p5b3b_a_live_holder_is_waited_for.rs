//! 0.59.0 P5b3b: a writer waits, for a bounded time, for a lock or a publication whose holder **appears to be running**, and for nothing else.
//!
//! The case table of P5b2b §4, on the real binary: **the holder finishes, dies, never ends, a second waiter**, and the holders that are never
//! waited on (a previous boot, an empty body, an older prikk's body). The holder of a *lock* is a real `sleep` process whose lock body is
//! written by `prikk-store` exactly as a running command writes it, so the real judge says "appears running"; the holder of a *publication*
//! is F2's own paused `tag create` (a live process holding a ref's lock between its pointer write and its log record).
//!
//! The harness sets `PRIKK_LOCK_WAIT_SECONDS=0` for every `prikk` it starts (so a test that holds a lock to see a refusal still sees it at once);
//! each test here sets the variable itself. Linux and macOS only (the liveness judge reads `/proc` or `ps`).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::f2_publication_in_flight::in_flight_tag;
use crate::support;

fn active_lock(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/active.lock")
}

/// A live process that is not this one, and a lock body that names it as a running command's body does.
fn live_holder(kind: &str) -> (Child, String) {
    let child = Command::new("sleep").arg("60").spawn().unwrap();
    let body = prikk_store::lock_body_naming_running_process_for_test(kind, child.id());
    (child, body)
}

fn repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "c1");
    repo
}

/// `prikk commit` of one new file, with the wait set to `seconds`.
fn commit_waiting(repo: &Path, seconds: &str, label: &str) -> std::process::Child {
    std::fs::write(repo.join(format!("{label}.txt")), format!("{label}\n")).unwrap();
    support::prikk(repo)
        .env("PRIKK_LOCK_WAIT_SECONDS", seconds)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--ref", "heads/main", "-m", label])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn finish(child: Child) -> (Output, Duration) {
    let started = Instant::now();
    let output = child.wait_with_output().unwrap();
    (output, started.elapsed())
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// **The holder finishes:** the command goes through after about the holder's time, says on stderr after a second what it waits for, and its
/// stdout is a success's. Perturb: no wait in `acquire_lock_file`: red (it is refused at once).
#[test]
fn a_command_waits_for_a_lock_whose_holder_finishes() {
    let repo = repository("p5b3b-lock-finishes");
    let lock = active_lock(&repo);
    let (holder, body) = live_holder("active");
    let pid = holder.id();
    std::fs::write(&lock, body).unwrap();
    let started = Instant::now();
    let waiter = commit_waiting(&repo, "5", "d");
    std::thread::sleep(Duration::from_millis(1_400));
    std::fs::remove_file(&lock).unwrap();
    let (output, _) = finish(waiter);
    let waited = started.elapsed();
    stop(holder);
    support::ok(&output, "the commit that waited");
    assert!(waited >= Duration::from_millis(1_300), "{waited:?}");
    assert!(waited < Duration::from_secs(4), "{waited:?}");
    assert!(
        stderr(&output).contains(&format!(
            "waiting for the active lock held by pid {pid} (it appears to be running); giving up after 5 s (PRIKK_LOCK_WAIT_SECONDS sets this)"
        )),
        "the one line after a second: {}",
        stderr(&output)
    );
    assert_eq!(
        stderr(&output).matches("waiting for").count(),
        1,
        "said once"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("recorded worktree patch in active WAL"),
        "stdout is a success's"
    );
    assert_eq!(support::verify(&repo).status.code(), Some(0));
}

/// **The holder dies, its lock stays:** refused when it does, with today's sentence naming `prikk unlock` and the words for the wait. Perturb:
/// every taken name waited for, whatever the judge says: it runs to the bound and this is red.
#[test]
fn a_command_waiting_for_a_lock_is_refused_when_its_holder_dies() {
    let repo = repository("p5b3b-lock-dies");
    let (holder, body) = live_holder("active");
    let pid = holder.id();
    std::fs::write(active_lock(&repo), body).unwrap();
    let waiter = commit_waiting(&repo, "30", "d");
    std::thread::sleep(Duration::from_millis(600));
    stop(holder);
    let killed = Instant::now();
    let (output, _) = finish(waiter);
    let after_the_kill = killed.elapsed();
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        after_the_kill < Duration::from_secs(2),
        "refused when the holder died, not at the bound: {after_the_kill:?}"
    );
    assert!(text.contains("active lock already exists: "), "{text}");
    assert!(
        text.contains("run `prikk unlock` to list held locks"),
        "{text}"
    );
    assert!(
        text.contains(&format!("pid {pid}: no process has the recorded pid)")),
        "{text}"
    );
    assert!(text.contains("(waited "), "{text}");
    assert!(
        active_lock(&repo).is_file(),
        "the dead holder's lock stays, for `unlock`"
    );
}

/// **The holder never ends:** refused at the bound (`PRIKK_LOCK_WAIT_SECONDS=1`) with today's sentence and the words for the wait. Exit code 1,
/// as today. Perturb: the bound not read from the variable: red.
#[test]
fn a_command_waiting_for_a_lock_that_is_never_released_is_refused_at_the_bound() {
    let repo = repository("p5b3b-lock-never");
    let (holder, body) = live_holder("active");
    let pid = holder.id();
    std::fs::write(active_lock(&repo), body).unwrap();
    let started = Instant::now();
    let waiter = commit_waiting(&repo, "1", "d");
    let (output, _) = finish(waiter);
    // From before the spawn: the clock `finish` keeps starts after the process did, and read 997 ms against a 1 s bound (0.59.0 H2c's gate run).
    let waited = started.elapsed();
    stop(holder);
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        waited >= Duration::from_millis(1_000) && waited < Duration::from_millis(3_000),
        "{waited:?}"
    );
    assert!(
        text.contains(&format!(
            "(waited 1 s: pid {pid} still appears to be running)"
        )),
        "{text}"
    );
    assert!(
        text.contains("run `prikk unlock` to list held locks"),
        "{text}"
    );
    assert!(
        !text.contains("waiting for"),
        "the line is for a wait longer than a second: {text}"
    );
}

/// **A wait of `0` is today's behaviour exactly:** the refusal at once, today's sentence, no words added, the same exit code. Perturb: the zero
/// read as the default: red.
#[test]
fn a_wait_of_zero_refuses_at_once_with_todays_sentence() {
    let repo = repository("p5b3b-lock-zero");
    let (holder, body) = live_holder("active");
    std::fs::write(active_lock(&repo), body).unwrap();
    let waiter = commit_waiting(&repo, "0", "d");
    let (output, waited) = finish(waiter);
    stop(holder);
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(waited < Duration::from_secs(1), "{waited:?}");
    assert!(!text.contains("(waited"), "{text}");
    assert!(
        text.trim_end().ends_with("confirmed gone"),
        "today's sentence, last: {text}"
    );
}

/// **Holders that are never waited on:** a lock from a previous boot, an empty body (a command killed between its create and its body), and an
/// older prikk's body (it records no start time, so the judge cannot say its pid is the process that took the lock, even though the pid names a
/// running process here). Each is refused at once with `PRIKK_LOCK_WAIT_SECONDS=30`, with today's sentence and nothing added.
/// Perturb: "unknown" or "does not appear to be running" waited for: red.
#[test]
fn a_stale_unknown_or_incomplete_holder_is_never_waited_on() {
    let repo = repository("p5b3b-lock-never-waited");
    let (live, _) = live_holder("active");
    let cases: [(&str, String); 3] = [
        (
            "a lock from a previous boot",
            "prikk-lock-v2\npid=1\nkind=active\nos=linux\nstart=1\nboot=00000000-0000-0000-0000-000000000000\npidns=pid:[1]\nend-of-lock-v2\n".to_string(),
        ),
        ("an empty body", String::new()),
        (
            "an older prikk's body",
            format!(
                "pid={}\nkind=active\nnote=PR-007 lock has no stale-lock stealing yet\n",
                live.id()
            ),
        ),
    ];
    for (what, body) in cases {
        std::fs::write(active_lock(&repo), body).unwrap();
        let waiter = commit_waiting(&repo, "30", "d");
        let (output, waited) = finish(waiter);
        let text = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{what}: {text}");
        assert!(
            waited < Duration::from_secs(2),
            "{what}: refused at once, took {waited:?}"
        );
        assert!(!text.contains("(waited"), "{what}: {text}");
        assert!(!text.contains("waiting for"), "{what}: {text}");
        assert!(
            text.contains("active lock already exists: "),
            "{what}: {text}"
        );
    }
    stop(live);
}

/// **A second waiter:** two commands wait for one holder that finishes (two `branch create`s, each a writer that takes the active lock); both go
/// through, one after the other (the second waits for the first's lock, which is a running command's). Both branches exist and `verify` is clean.
/// Perturb: as the first test.
#[test]
fn two_waiters_for_one_holder_both_go_through() {
    let repo = repository("p5b3b-lock-two-waiters");
    support::trust_maintainer(&repo);
    let lock = active_lock(&repo);
    let (holder, body) = live_holder("active");
    std::fs::write(&lock, body).unwrap();
    let branch = |name: &str| {
        support::prikk(&repo)
            .env("PRIKK_LOCK_WAIT_SECONDS", "10")
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args(["branch", "create", name, "--from", "heads/main"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = branch("heads/b1");
    let second = branch("heads/b2");
    std::thread::sleep(Duration::from_millis(800));
    std::fs::remove_file(&lock).unwrap();
    let (a, _) = finish(first);
    let (b, _) = finish(second);
    stop(holder);
    support::ok(&a, "the first waiter");
    support::ok(&b, "the second waiter");
    let list = support::prikk(&repo)
        .args(["branch", "list"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&list.stdout).into_owned();
    assert!(
        listing.contains("heads/b1") && listing.contains("heads/b2"),
        "{listing}"
    );
    assert_eq!(support::verify(&repo).status.code(), Some(0));
}

/// **A publication in flight is waited for and the writer goes on when it finishes.** F2's paused `tag create tags/t1` holds the ref's lock with its
/// pointer ahead of its log; a second `tag create tags/t2` meets that lead, waits, and succeeds when the first is let go. It says on stderr after a
/// second what it waits for. Both tags are listed afterwards and `verify` is clean. Perturb: the in-flight loop removed: red (refused at once).
#[test]
fn a_writer_waits_for_a_publication_in_flight_and_goes_on_when_it_finishes() {
    let (repo, publisher) = in_flight_tag("p5b3b-in-flight-finishes", false);
    let pid = publisher.pid();
    let second = support::prikk(&repo)
        .env("PRIKK_LOCK_WAIT_SECONDS", "10")
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["tag", "create", "tags/t2", "--target", "heads/main"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1_500));
    let first = publisher.release();
    support::ok(&first, "the publisher");
    let (output, _) = finish(second);
    support::ok(&output, "the writer that waited");
    assert!(
        stderr(&output).contains(&format!(
            "waiting for another command to finish publishing tags/t1 (pid {pid}, from its lock); giving up after 10 s (PRIKK_LOCK_WAIT_SECONDS sets this)"
        )),
        "{}",
        stderr(&output)
    );
    let list = support::prikk(&repo)
        .args(["tag", "list"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&list.stdout).into_owned();
    assert!(
        listing.contains("tags/t1") && listing.contains("tags/t2"),
        "{listing}"
    );
    assert_eq!(support::verify(&repo).status.code(), Some(0));
}

/// **A publication that never finishes is waited for to the bound** (`PRIKK_LOCK_WAIT_SECONDS=1`), then refused with today's sentence and the
/// words for the wait; **killed, it is not waited for** and the answer is today's (interrupted; `ref complete`) at once. Perturb: the judge ignored
/// in the loop: the killed case runs to the bound, red.
#[test]
fn a_publication_in_flight_is_waited_for_to_the_bound_and_a_killed_one_is_not() {
    let (repo, publisher) = in_flight_tag("p5b3b-in-flight-bound", false);
    let pid = publisher.pid();
    let tag = |seconds: &str| {
        let started = Instant::now();
        let output = support::prikk(&repo)
            .env("PRIKK_LOCK_WAIT_SECONDS", seconds)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args(["tag", "create", "tags/t2", "--target", "heads/main"])
            .output()
            .unwrap();
        (output, started.elapsed())
    };
    let (refused, waited) = tag("1");
    let text = stderr(&refused);
    assert_eq!(refused.status.code(), Some(1), "{text}");
    assert!(
        waited >= Duration::from_millis(1_000) && waited < Duration::from_millis(3_000),
        "{waited:?}"
    );
    assert!(
        text.contains(&format!(
            "another command is publishing tags/t1 (pid {pid}, from its lock); run this command again when it finishes (waited 1 s: the other command is still publishing)"
        )),
        "{text}"
    );
    let (today, at_once) = tag("0");
    assert!(at_once < Duration::from_secs(1), "{at_once:?}");
    assert!(!stderr(&today).contains("(waited"), "{}", stderr(&today));
    publisher.kill();
    let (killed, killed_in) = tag("30");
    assert!(
        killed_in < Duration::from_secs(2),
        "not waited for: {killed_in:?}"
    );
    let text = stderr(&killed);
    assert_eq!(killed.status.code(), Some(1), "{text}");
    assert!(!text.contains("(waited"), "{text}");
    assert!(!text.contains("another command is publishing"), "{text}");
}

/// **The commands that report or repair never wait:** with a publication in flight and `PRIKK_LOCK_WAIT_SECONDS=30`, `verify`, `status`, `doctor`
/// and `unlock` answer at once, as they did (the exit codes and the words are F2's, and `f2_publication_in_flight` holds them). Perturb: the CLI's
/// `refuse_without_waiting` call removed: `doctor` (which takes locks to repair) is the one that stalls, red.
#[test]
fn verify_status_doctor_and_unlock_never_wait() {
    let (repo, publisher) = in_flight_tag("p5b3b-never-wait-readers", false);
    // A lock held by a running process that the repair verbs would otherwise wait for.
    let (holder, body) = live_holder("active");
    std::fs::write(active_lock(&repo), body).unwrap();
    for args in [
        vec!["verify"],
        vec!["status"],
        vec!["doctor", "--repair-tails"],
        vec!["unlock"],
    ] {
        let started = Instant::now();
        let output = support::prikk(&repo)
            .env("PRIKK_LOCK_WAIT_SECONDS", "30")
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(5),
            "{args:?} did not wait: {took:?}"
        );
        assert!(
            !stderr(&output).contains("waiting for"),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(
            !stderr(&output).contains("(waited"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
    stop(holder);
    publisher.kill();
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// 0.59.0 P2 item 0: a mis-set `PRIKK_LOCK_WAIT_SECONDS` never makes prikk wait.

const MISSET: [&str; 5] = ["off", "0.5", "-1", "", "301"];

fn suffix_for(value: &str) -> String {
    format!(
        "(not waited: PRIKK_LOCK_WAIT_SECONDS must be a whole number of seconds from 0 to 300, got {value:?})"
    )
}

/// **A value that is not a whole number from 0 to 300 refuses at once, with today's sentence and a suffix that names the variable and the value;**
/// `off`, `0.5`, `-1`, an empty value and `301`. Perturb: the old fallback (the value read as 5): each waits five seconds, and the elapsed time and
/// the missing suffix are red.
#[test]
fn a_mis_set_wait_variable_refuses_at_once_and_names_the_variable() {
    for value in MISSET {
        let repo = repository("p2-misset-lock");
        let (holder, body) = live_holder("active");
        std::fs::write(active_lock(&repo), body).unwrap();
        let started = Instant::now();
        let waiter = commit_waiting(&repo, value, "d");
        let (output, _) = finish(waiter);
        let took = started.elapsed();
        stop(holder);
        let text = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{value:?}: {text}");
        assert!(
            took < Duration::from_millis(2_500),
            "{value:?}: refused at once, took {took:?}"
        );
        assert!(
            text.contains("lock conflict: active lock already exists: "),
            "{value:?}: today's sentence: {text}"
        );
        assert!(
            text.trim_end().ends_with(&suffix_for(value)),
            "{value:?}: the suffix: {text}"
        );
        assert!(
            !text.contains("waiting for"),
            "{value:?}: nothing was waited for: {text}"
        );
    }
}

/// **A mis-set value in front of a publication in flight** is refused at once the same way (the in-flight refusal, then the suffix).
#[test]
fn a_mis_set_wait_variable_refuses_a_publication_in_flight_at_once() {
    let (repo, publisher) = in_flight_tag("p2-misset-in-flight", false);
    let started = Instant::now();
    let second = support::prikk(&repo)
        .env("PRIKK_LOCK_WAIT_SECONDS", "off")
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["tag", "create", "tags/t2", "--target", "heads/main"])
        .output()
        .unwrap();
    let took = started.elapsed();
    let text = stderr(&second);
    let first = publisher.release();
    support::ok(&first, "the publisher");
    assert_eq!(second.status.code(), Some(1), "{text}");
    assert!(took < Duration::from_millis(2_500), "{took:?}");
    assert!(
        text.contains("another command is publishing tags/t1"),
        "{text}"
    );
    assert!(
        text.trim_end().ends_with(&suffix_for("off")),
        "the suffix: {text}"
    );
}

/// **`0` refuses at once with no suffix (today), and `7` waits.** Perturb: `0` given the suffix; `7` refused at once: red.
#[test]
fn zero_refuses_with_no_suffix_and_seven_waits() {
    let repo = repository("p2-zero-seven");
    let (holder, body) = live_holder("active");
    let lock = active_lock(&repo);
    std::fs::write(&lock, body).unwrap();
    let (output, _) = finish(commit_waiting(&repo, "0", "d"));
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(!text.contains("not waited"), "no suffix: {text}");
    assert!(!text.contains("waited"), "no words at all: {text}");
    let started = Instant::now();
    let waiter = commit_waiting(&repo, "7", "e");
    std::thread::sleep(Duration::from_millis(1_400));
    std::fs::remove_file(&lock).unwrap();
    let (output, _) = finish(waiter);
    let waited = started.elapsed();
    stop(holder);
    support::ok(&output, "the commit that waited with 7");
    assert!(waited >= Duration::from_millis(1_300), "{waited:?}");
}

/// **A command that meets no holder is unaffected by a mis-set value:** it succeeds and prints nothing about the variable.
#[test]
fn a_mis_set_wait_variable_costs_a_command_that_meets_no_holder_nothing() {
    for value in MISSET {
        let repo = repository("p2-misset-free");
        let (output, _) = finish(commit_waiting(&repo, value, "d"));
        support::ok(&output, &format!("a commit with the variable {value:?}"));
        assert!(
            !stderr(&output).contains("PRIKK_LOCK_WAIT_SECONDS"),
            "{value:?}: {}",
            stderr(&output)
        );
    }
}
