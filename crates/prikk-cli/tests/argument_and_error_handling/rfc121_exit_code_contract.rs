//! RFC 121 §6a's ruled exit-code contract, end-to-end through the compiled binary:
//! `0` success, `1` operational failure, `2` usage error. `commands::exit_code_tests` unit-tests
//! `CliError`'s own mapping directly; this drives real commands to confirm the mapping is actually
//! wired all the way to the process exit code, not just correct in isolation.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
#[test]
fn a_successful_command_exits_zero() {
    let repo = support::unique_repo("rfc121-exit-zero");
    support::init(&repo);
    let output = support::verify(&repo);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

/// `worktree-status` reporting a dirty worktree is an operational failure -- the operation ran and
/// answered honestly, but did not do what a clean-worktree caller wanted -- not a usage error.
#[test]
fn an_operational_failure_exits_one() {
    let repo = support::unique_repo("rfc121-exit-one");
    support::init(&repo);
    std::fs::write(repo.join("f.txt"), "untracked").unwrap();
    let output = support::prikk(&repo)
        .arg("worktree-status")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

/// An unrecognized command name is detected before any repository work begins -- the exact shape
/// RFC 121 §6a's `2` is for. Before this commit this exited `1`; this is the control that shows
/// which.
#[test]
fn an_unrecognized_command_exits_two() {
    let repo = support::unique_repo("rfc121-exit-two");
    support::init(&repo);
    let output = support::prikk(&repo)
        .arg("nonsense-command")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown command: nonsense-command"),
        "{stderr}"
    );
}
