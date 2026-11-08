//! `prikk unlock` end-to-end (RFC 102 Stage 6 Step 2, design-v1.md §15.7 decision 3): the CLI surface
//! for stale-lock recovery. Exercises listing, the interactive confirmation gate, `--yes`, and the
//! "not found" refusal -- everything `prikk_store::unlock`'s own unit tests don't cover, since those
//! stop at the library boundary and never touch a terminal-facing prompt.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Output, Stdio};

use crate::support::{init, ok, prikk, unique_repo};

fn active_lock_path(repo: &Path) -> std::path::PathBuf {
    repo.join(".prikk/active/default/active.lock")
}

fn plant_stale_active_lock(repo: &Path) {
    std::fs::write(
        active_lock_path(repo),
        prikk_store::lock_body_naming_pid_for_test("active", 999_999),
    )
    .unwrap();
}

fn unlock_with_stdin(repo: &Path, args: &[&str], stdin_line: &str) -> Output {
    let mut child = prikk(repo)
        .arg("unlock")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_line.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_fresh_repository_reports_no_locks() {
    let repo = unique_repo("unlock-empty");
    init(&repo);
    let output = prikk(&repo).arg("unlock").output().unwrap();
    ok(&output, "unlock");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no locks currently held"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn listing_a_stale_lock_shows_its_recorded_pid_and_advisory_liveness() {
    let repo = unique_repo("unlock-list-stale");
    init(&repo);
    plant_stale_active_lock(&repo);

    let output = prikk(&repo).arg("unlock").output().unwrap();
    ok(&output, "unlock");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("active.lock"), "stdout: {stdout}");
    assert!(stdout.contains("recorded pid: 999999"), "stdout: {stdout}");
    assert!(
        stdout.contains("does not appear to be running"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("advisory only"),
        "the advisory caveat must always print alongside a listing: {stdout}"
    );
}

/// The whole safety property: declining the interactive prompt leaves the lock in place. RFC 121
/// §6a: declining is also an operational failure now, not a silent success -- a script chaining
/// `prikk unlock && proceed` must not continue past a lock that is still held.
#[test]
fn declining_the_interactive_prompt_does_not_clear_the_lock() {
    let repo = unique_repo("unlock-decline");
    init(&repo);
    plant_stale_active_lock(&repo);
    let path = active_lock_path(&repo);

    let output = unlock_with_stdin(&repo, &["--lock", path.to_str().unwrap()], "no\n");
    assert_eq!(
        output.status.code(),
        Some(1),
        "declining must exit 1, not 0: {output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("lock not cleared"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        path.is_file(),
        "the lock must still be present after a declined confirmation"
    );
}

/// RFC 121 §6a's actual named victim: EOF on stdin (a non-interactive context with nothing
/// attached) resolves to "no" exactly like an explicit decline, and must exit `1` for the same
/// reason -- this is the path a CI script actually hits, not the interactive one.
#[test]
fn eof_on_stdin_does_not_clear_the_lock_either() {
    let repo = unique_repo("unlock-eof");
    init(&repo);
    plant_stale_active_lock(&repo);
    let path = active_lock_path(&repo);

    let output = prikk(&repo)
        .args(["unlock", "--lock", path.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "EOF on stdin must exit 1, not 0: {output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("lock not cleared"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        path.is_file(),
        "the lock must still be present after EOF on stdin"
    );
}

/// Typing exactly `yes` at the prompt clears the lock.
#[test]
fn confirming_yes_at_the_interactive_prompt_clears_the_lock() {
    let repo = unique_repo("unlock-confirm");
    init(&repo);
    plant_stale_active_lock(&repo);
    let path = active_lock_path(&repo);

    let output = unlock_with_stdin(&repo, &["--lock", path.to_str().unwrap()], "yes\n");
    ok(&output, "unlock --lock (confirmed)");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("cleared:"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!path.is_file(), "the lock must be gone after confirmation");
}

/// `--yes` skips the interactive prompt entirely -- the scripting escape.
#[test]
fn the_yes_flag_clears_without_any_prompt() {
    let repo = unique_repo("unlock-yes-flag");
    init(&repo);
    plant_stale_active_lock(&repo);
    let path = active_lock_path(&repo);

    let output = prikk(&repo)
        .args(["unlock", "--lock", path.to_str().unwrap(), "--yes"])
        .output()
        .unwrap();
    ok(&output, "unlock --lock --yes");
    assert!(
        !path.is_file(),
        "the lock must be gone with --yes and no stdin at all"
    );
}

/// Naming a path that is not currently a held lock refuses cleanly rather than silently
/// no-op-succeeding or panicking.
#[test]
fn naming_a_lock_that_is_not_held_refuses() {
    let repo = unique_repo("unlock-not-held");
    init(&repo);
    let path = active_lock_path(&repo);

    let output = prikk(&repo)
        .args(["unlock", "--lock", path.to_str().unwrap(), "--yes"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "unlocking a lock that is not held must fail, not silently succeed"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no held lock"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// L1: the listing says why an answer is "unknown", for a lock written by 0.56.0 (a live pid, no start
/// time to check it against) and for a body cut while being written; `doctor` prints the same words.
#[test]
fn the_listing_and_doctor_say_why_a_lock_is_unknown() {
    let repo = unique_repo("unlock-unknown-reasons");
    init(&repo);
    let old = format!(
        "pid={}\nkind=active\nnote=PR-007 lock has no stale-lock stealing yet\n",
        std::process::id()
    );
    std::fs::write(active_lock_path(&repo), old).unwrap();
    let output = prikk(&repo).arg("unlock").output().unwrap();
    ok(&output, "unlock");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("liveness: unknown: written by an older prikk"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!("recorded pid: {}", std::process::id())),
        "stdout: {stdout}"
    );
    let doctor = prikk(&repo).arg("doctor").output().unwrap();
    let doctor_out = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        doctor_out.contains("liveness: unknown: written by an older prikk"),
        "doctor must print the listing's words: {doctor_out}"
    );

    std::fs::write(active_lock_path(&repo), "prikk-lock-v2\npid=1").unwrap();
    let output = prikk(&repo).arg("unlock").output().unwrap();
    ok(&output, "unlock");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "liveness: unknown: the lock body is incomplete: still being written, or damaged"
        ),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("recorded pid: 1"),
        "a cut body's pid must never be shown or used: {stdout}"
    );
    // `pid=1` of a body cut mid-write parses as a number, so "unparseable" would be false: say it
    // is not read.
    assert!(
        stdout.contains("recorded pid: (not read: the lock body is incomplete)"),
        "stdout: {stdout}"
    );
    // One definition of the words: the listing's closing line names the doc, it does not restate them.
    assert!(
        stdout.contains("docs/src/reference/concurrency-locking.md"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("same pid and start time, and on Linux"),
        "the advisory line must not restate what `appears running` means: {stdout}"
    );
}

// 0.57.0 step 2, L2: `--lock` refuses a holder that appears running unless `--force`; `--yes` and
// `--force` are no longer synonyms.

/// Hold the repository's active lock the real way: this test process is the live holder, and the
/// lock body is written by the real writer (`ActiveLock::acquire`), naming this process.
fn hold_the_active_lock(repo: &Path) -> prikk_store::ActiveLock {
    let layout = prikk_store::RepositoryLayout::open(repo).unwrap();
    prikk_store::ActiveLock::acquire(&layout, prikk_store::DEFAULT_ACTIVE_NAME).unwrap()
}

#[test]
fn a_live_holder_is_refused_with_yes_and_with_a_typed_yes_and_the_lock_stays() {
    let repo = unique_repo("unlock-live-yes");
    init(&repo);
    let _held = hold_the_active_lock(&repo);
    let path = active_lock_path(&repo);

    let output = prikk(&repo)
        .args(["unlock", "--lock", path.to_str().unwrap(), "--yes"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("precondition not met"), "stderr: {stderr}");
    assert!(
        stderr.contains(&format!("pid {}", std::process::id())),
        "the refusal names the pid: {stderr}"
    );
    assert!(
        stderr.contains("start time"),
        "the refusal names the start time: {stderr}"
    );
    assert!(
        stderr.contains("--force") && stderr.contains("corrupting"),
        "the refusal says what --force does and its risk: {stderr}"
    );
    assert!(
        stderr.contains("docs/src/reference/concurrency-locking.md"),
        "the refusal points at the one definition: {stderr}"
    );
    assert!(
        path.is_file(),
        "a refused unlock must leave the lock in place"
    );

    // The interactive path: a typed `yes` does not clear a live holder either.
    let output = unlock_with_stdin(&repo, &["--lock", path.to_str().unwrap()], "yes\n");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("precondition not met"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(path.is_file());
}

#[test]
fn force_clears_a_live_holder_without_asking() {
    let repo = unique_repo("unlock-live-force");
    init(&repo);
    let held = hold_the_active_lock(&repo);
    let path = active_lock_path(&repo);

    let output = prikk(&repo)
        .args(["unlock", "--lock", path.to_str().unwrap(), "--force"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    ok(&output, "unlock --lock --force");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("cleared:"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!path.is_file(), "--force clears a live holder's lock");
    drop(held);
}

#[test]
fn a_dead_holder_is_cleared_by_yes_and_by_force() {
    for flag in ["--yes", "--force"] {
        let repo = unique_repo("unlock-dead-flags");
        init(&repo);
        plant_stale_active_lock(&repo);
        let path = active_lock_path(&repo);
        let output = prikk(&repo)
            .args(["unlock", "--lock", path.to_str().unwrap(), flag])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        ok(&output, &format!("unlock --lock {flag} (dead holder)"));
        assert!(
            !path.is_file(),
            "{flag} clears a holder that does not appear to be running"
        );
    }
}

/// An older prikk's body (no start time to check) and a cut one are `unknown`: `--yes` still clears
/// them, as it did. The old body names a pid that IS alive (this test process); only the missing
/// start time keeps it from `appears running`.
#[test]
fn an_unknown_holder_is_still_cleared_by_yes() {
    let bodies = [
        format!(
            "pid={}\nkind=active\nnote=PR-007 lock has no stale-lock stealing yet\n",
            std::process::id()
        ),
        "prikk-lock-v2\npid=1".to_string(),
    ];
    for (n, body) in bodies.iter().enumerate() {
        let repo = unique_repo("unlock-unknown-yes");
        init(&repo);
        std::fs::write(active_lock_path(&repo), body).unwrap();
        let path = active_lock_path(&repo);
        let output = prikk(&repo)
            .args(["unlock", "--lock", path.to_str().unwrap(), "--yes"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        ok(&output, &format!("unlock --lock --yes (unknown body {n})"));
        assert!(!path.is_file(), "--yes clears an unknown holder (body {n})");
    }
}

#[test]
fn yes_and_force_may_be_given_together_and_each_only_once() {
    let repo = unique_repo("unlock-yes-and-force");
    init(&repo);
    plant_stale_active_lock(&repo);
    let path = active_lock_path(&repo);
    let both = prikk(&repo)
        .args([
            "unlock",
            "--lock",
            path.to_str().unwrap(),
            "--yes",
            "--force",
        ])
        .output()
        .unwrap();
    ok(&both, "unlock --yes --force");
    let twice = prikk(&repo)
        .args([
            "unlock",
            "--lock",
            path.to_str().unwrap(),
            "--force",
            "--force",
        ])
        .output()
        .unwrap();
    assert_eq!(twice.status.code(), Some(2), "{twice:?}");
}
