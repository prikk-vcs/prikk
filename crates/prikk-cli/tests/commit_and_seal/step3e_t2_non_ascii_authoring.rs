//! 0.56.0 step 3e, T2 (first item, per the step-3d2 review): the authoring sentence a user
//! actually meets when `commit`/`status` scans a non-ASCII-named worktree file has no test of its
//! own before this file -- `prikk-object`'s own
//! `create_file_validate_gives_the_authoring_sentence_for_a_non_ascii_path` only proves
//! `CreateFile::validate` gives the right wording, and the step-3d2 report's own finding is that
//! `CreateFile::validate` is never actually the *first* refusal for a brand-new worktree file:
//! `prikk-store`'s own five worktree-scan call sites (`worktree_files.rs`, `node_authoring.rs`,
//! `worktree_read.rs`, `worktree_status.rs`) see it first, while the worktree is still being
//! walked, before any `Patch` operation is ever constructed. These two tests drive the compiled
//! binary exactly as a user would, through `commit` and through `status`, and assert the
//! *authoring* sentence reaches them -- never the *reading* sentence (which would say "a newer
//! version of prikk may have written it", wrong for a file the test itself just wrote by hand) and
//! never the retired "deferred until ... implemented" wording.

use crate::support::{commit, init, unique_repo};

/// `commit` of a brand-new worktree file whose name is non-ASCII: the authoring sentence, naming
/// the file, never the reading sentence.
#[test]
fn commit_of_a_non_ascii_worktree_file_gives_the_authoring_sentence() {
    let repo = unique_repo("step3e-t2-commit-non-ascii");
    init(&repo);
    std::fs::create_dir_all(repo.join("pages")).unwrap();
    std::fs::write(repo.join("pages/日本語.md"), b"hello\n").unwrap();

    let output = commit(&repo, "heads/main", "add a non-ascii file");
    assert!(
        !output.status.success(),
        "a non-ASCII worktree file must still refuse authoring"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("this version of prikk does not accept non-ASCII repository paths")
            && stderr.contains("日本語"),
        "expected the authoring sentence naming the file, got: {stderr}"
    );
    assert!(
        !stderr.contains("newer version of prikk may have") && !stderr.contains("deferred"),
        "authoring must never give the reading sentence or the retired wording: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `worktree-status` (the same `scan_untracked`/worktree-scan path `diff` also shares) over a
/// non-ASCII-named untracked file: the same authoring sentence, never the reading one. (Plain
/// `status` reports only repository/WAL state, not per-path worktree scan results, so it never
/// reaches this refusal at all -- confirmed by reading its own output with the identical fixture
/// before choosing `worktree-status` here instead.)
#[test]
fn worktree_status_of_a_non_ascii_worktree_file_gives_the_authoring_sentence() {
    let repo = unique_repo("step3e-t2-worktree-status-non-ascii");
    init(&repo);
    std::fs::write(repo.join("日本.txt"), b"hello\n").unwrap();

    let output = prikk_test_support::prikk(&repo)
        .arg("worktree-status")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "worktree-status over a non-ASCII untracked file must still refuse"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("this version of prikk does not accept non-ASCII repository paths")
            && stdout.contains("日本.txt"),
        "expected the authoring sentence naming the file, got: {stdout}"
    );
    assert!(
        !stdout.contains("newer version of prikk may have") && !stdout.contains("deferred"),
        "worktree-status must never give the reading sentence or the retired wording: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
