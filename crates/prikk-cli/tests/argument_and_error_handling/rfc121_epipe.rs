//! RFC 121 §2.1: a closed stdout must exit `0` silently, not panic. Drives the compiled `prikk`
//! binary through a real OS pipe whose read end is closed before the child has written anything --
//! the same failure the handoff's own `prikk verify | head -3` reproduces via a shell, made
//! deterministic here instead of depending on how much a real reader happens to consume first.
//!
//! **Why closing immediately, rather than reading a prefix first (`head -3`'s own shape):** the
//! child process still has to be scheduled, exec, and run before it writes anything, while the
//! parent closing its own read-end handle is a single, essentially instantaneous syscall in an
//! already-running process. Reading a few bytes and then dropping would still work in practice
//! (it's what a real `| head -3` does), but would make the test's reliability depend on that race
//! rather than the much wider one this version relies on -- see `dc59_commit_benchmark.rs` and
//! others in this same `tests/` directory for the project's existing tolerance for process-timing
//! based integration tests; this one needs no `sleep` or retry because the margin is large by
//! construction, not by luck.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[path = "../support/mod.rs"]
mod support;

fn fixture_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("f.txt"), "hello").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "genesis commit",
    );
    repo
}

/// Spawn `prikk <args>` in `repo` with stdout piped, then close the read end **without reading
/// anything** -- see the module doc for why this is the deterministic half of the shell's
/// `| head -3`/`| grep -q .` shape rather than a simulation of it.
fn run_against_a_closed_pipe(repo: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .current_dir(repo)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn prikk");

    drop(child.stdout.take().expect("stdout was piped"));

    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("stderr was piped")
        .read_to_end(&mut stderr)
        .expect("read stderr");
    let status = child.wait().expect("wait for prikk");

    Output {
        status,
        stdout: Vec::new(),
        stderr,
    }
}

fn assert_closed_pipe_is_silent_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what}: expected exit 0 on a closed stdout, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{what}: expected empty stderr on a closed stdout (RFC 121 §2.1: not an error), got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Control 1/2: the handoff's own reproduction (`prikk verify | head -3`), through a real pipe,
/// with the exit code checked explicitly rather than inferred from `Output::status.success()`
/// alone reading as "not a panic" -- it is checked as `== 0` too, at the assertion above.
#[test]
fn verify_through_a_closed_pipe_exits_zero_with_empty_stderr() {
    let repo = fixture_repo("rfc121-epipe-verify");
    let output = run_against_a_closed_pipe(&repo, &["verify"]);
    assert_eq!(output.status.code(), Some(0));
    assert_closed_pipe_is_silent_success(&output, "prikk verify");
}

/// Control 3: a second command and, implicitly, a second output path (`print_history` via
/// `output.rs`, a different call site than `verify`'s `print_verify_report`) -- the fix is not
/// shown only where the handoff's own example was written.
#[test]
fn log_through_a_closed_pipe_exits_zero_with_empty_stderr() {
    let repo = fixture_repo("rfc121-epipe-log");
    let output = run_against_a_closed_pipe(&repo, &["log"]);
    assert_eq!(output.status.code(), Some(0));
    assert_closed_pipe_is_silent_success(&output, "prikk log");
}

/// RFC 121 §6a amendment control 8: a non-`BrokenPipe` stdout write failure exits `1` with a
/// message on stderr, not a panic banner outside the ruled 0/1/2 vocabulary (it used to exit
/// `101`). `/dev/full` is a genuine full device, not a simulation -- the same control the original
/// EPIPE increment used to prove this fix is narrow, re-run here to prove the *other* arm changed
/// only the way it was supposed to. Linux-only (`/dev/full` does not exist on macOS or Windows);
/// skips rather than fails where it is absent, since the underlying `classify`/`WriteOutcome` logic
/// this exercises is platform-independent and already covered by `stdout::tests`'s own unit tests
/// on every platform.
#[test]
fn a_write_failure_exits_one_with_a_message_not_a_panic() {
    if !std::path::Path::new("/dev/full").exists() {
        eprintln!("skipping: /dev/full is not available on this platform");
        return;
    }
    let repo = fixture_repo("rfc121-write-failure");
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let output = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .current_dir(&repo)
        .arg("verify")
        .stdout(full)
        .output()
        .expect("spawn prikk");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed printing to stdout"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "must not panic, got: {stderr}"
    );
}

/// AUD-09: when *both* stdout and stderr fail on the same write attempt (sharing a cause, not
/// independent), the previous arm's `eprintln!` panicked on its own stderr write, exiting `101` --
/// outside the ruled 0/1/2 vocabulary. Asserts only the exit code: the message is precisely what
/// cannot be delivered in this case, so asserting on stderr content would pass for the wrong reason.
/// Linux-only, same as the test above.
#[test]
fn a_write_failure_on_both_streams_exits_one_not_a_panic() {
    if !std::path::Path::new("/dev/full").exists() {
        eprintln!("skipping: /dev/full is not available on this platform");
        return;
    }
    let repo = fixture_repo("rfc121-write-failure-both-streams");
    let stdout_full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full for stdout");
    let stderr_full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full for stderr");
    let output = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .current_dir(&repo)
        .arg("verify")
        .stdout(stdout_full)
        .stderr(stderr_full)
        .output()
        .expect("spawn prikk");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}
