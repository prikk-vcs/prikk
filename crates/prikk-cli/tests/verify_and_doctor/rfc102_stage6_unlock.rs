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
        "pid=999999\nkind=active\nnote=simulated crash\n",
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
