//! 0.56.0 step W3b, `rollback-draft --append-inverse`'s own one new output line
//! (`.git-exclude/reviewed/0.56.0-step-3w3-review-v1.md`'s ruling: no materialization happens here
//! at all, and the CLI output says so plainly): the worktree is unchanged by the draft; `seal`,
//! then `branch switch <the current branch>`, applies the inverse.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::Path;

fn write_and_commit(repo: &Path, path: &str, content: &str, message: &str) {
    std::fs::write(repo.join(path), content).unwrap();
    support::ok(&support::commit(repo, "heads/main", message), message);
}

/// Test 4 (the ruling's own list): the exact new sentence, and that the worktree genuinely is
/// unchanged by the draft itself (already true before this round -- new coverage of the message,
/// not new behavior).
#[test]
fn append_inverse_states_the_worktree_is_unchanged_and_names_the_route() {
    let repo = support::unique_repo("w3b-rollback-draft-output");
    support::init(&repo);
    write_and_commit(&repo, "genesis.txt", "genesis", "genesis");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    let before = std::fs::read(repo.join("genesis.txt")).unwrap();

    let draft = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["rollback-draft", "--append-inverse", "-m", "undo"])
        .output()
        .unwrap();
    support::ok(&draft, "rollback-draft onto an empty active WAL");

    let stdout = String::from_utf8_lossy(&draft.stdout).into_owned();
    assert!(
        stdout.contains(
            "note: the worktree is unchanged by this draft; run `prikk seal`, then `prikk branch \
             switch heads/main`, to apply the inverse"
        ),
        "the CLI output must state plainly that the worktree is unchanged and name the route: \
         {stdout}"
    );

    // The worktree genuinely is unchanged -- `genesis.txt` is still physically present, byte for
    // byte, even though the active WAL now queues its own deletion as the inverse's first (and
    // only) operation.
    assert_eq!(
        std::fs::read(repo.join("genesis.txt")).unwrap(),
        before,
        "rollback-draft must never touch the worktree"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
