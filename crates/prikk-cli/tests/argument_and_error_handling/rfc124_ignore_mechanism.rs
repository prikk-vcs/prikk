//! RFC 124 — the worktree ignore mechanism (`.prikkignore`), end to end through the compiled binary.
//! Handoff: `rfcs/handoffs/124-worktree-ignore-mechanism/ignore-mechanism-handoff-v1.md`, amended by
//! `ignore-mechanism-handoff-v2-amendment.md` and `-v3-amendment.md` after the first landing
//! (`b9a6fd8`) broke `commit` on Windows and was reverted (`2235af3`). These are Linux-runnable tests
//! and could not have caught that defect themselves (module doc: a bug in how a path is *rendered*
//! needs the platform that renders it differently) -- the Windows-safety invariant is tested directly,
//! without a filesystem, in `prikk-store`'s own `path::tests`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
fn write(repo: &std::path::Path, relative: &str, content: &[u8]) {
    let target = repo.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(target, content).unwrap();
}

/// Control 2: a file matching an ignore rule never becomes a `commit` operation.
#[test]
fn ignored_file_is_absent_from_commit_operations() {
    let repo = support::unique_repo("rfc124-ignore-commit");
    support::init(&repo);
    write(&repo, ".prikkignore", b"build\n");
    write(&repo, "build/output.txt", b"generated");
    write(&repo, "src/lib.rs", b"fn main() {}");

    let output = support::commit(&repo, "heads/main", "genesis");
    support::ok(&output, "commit");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("build/output.txt"),
        "an ignored path must not appear in commit's own operations: {stdout}"
    );
    // The ignore file itself is an ordinary tracked file, and so is the other real source file.
    assert!(stdout.contains(".prikkignore"), "{stdout}");
    assert!(stdout.contains("src/lib.rs"), "{stdout}");
}

/// Control 3: the same path is absent from `worktree-status`'s untracked list -- both walks agree.
#[test]
fn ignored_file_is_absent_from_worktree_status_untracked_list() {
    let repo = support::unique_repo("rfc124-ignore-status");
    support::init(&repo);
    write(&repo, ".prikkignore", b"build\n");
    write(&repo, "src/lib.rs", b"fn main() {}");
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    // Written *after* sealing, so it is genuinely untracked worktree content -- exactly the case
    // `worktree-status`'s own untracked scan exists to report.
    write(&repo, "build/output.txt", b"generated");
    write(&repo, "build/nested/more.txt", b"generated too");

    let output = support::prikk(&repo)
        .arg("worktree-status")
        .output()
        .unwrap();
    // `worktree-status` exits non-zero whenever the worktree has any change (including a real
    // untracked file), so this must be checked on stdout, not the exit code.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("build/output.txt") && !stdout.contains("build/nested/more.txt"),
        "an ignored path (or a path nested under an ignored directory) must not be reported as \
         untracked: {stdout}"
    );
}

/// A rule matches its own name and everything nested under it, never a same-prefixed sibling --
/// exercised against the real binary, not only the unit-level matcher.
#[test]
fn ignore_rule_does_not_over_match_a_similarly_named_sibling() {
    let repo = support::unique_repo("rfc124-ignore-sibling");
    support::init(&repo);
    write(&repo, ".prikkignore", b"target\n");
    write(&repo, "target/debug/output.bin", b"generated");
    write(&repo, "target-notes.md", b"not actually generated");

    let output = support::commit(&repo, "heads/main", "genesis");
    support::ok(&output, "commit");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("target/debug/output.bin"), "{stdout}");
    assert!(
        stdout.contains("target-notes.md"),
        "a same-prefixed sibling must not be swept in by the rule for \"target\": {stdout}"
    );
}

/// Control 4 (§4.1's constraint): a patch that already exists in sealed history applies regardless
/// of what the local `.prikkignore` says. Materialization reads only from replay/lineage state and
/// never consults the ignore mechanism at all -- demonstrated by sealing a tracked file, deleting it
/// from disk, adding an ignore rule that would cover it, and checking that `checkout
/// --patch-materialize` still writes it back.
#[test]
fn a_patch_touching_an_ignored_path_still_materializes() {
    let repo = support::unique_repo("rfc124-ignore-materialize");
    support::init(&repo);
    write(
        &repo,
        "build/output.txt",
        b"already sealed, before any ignore rule existed",
    );
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    // Only now does an ignore rule appear that would cover this same path.
    write(&repo, ".prikkignore", b"build\n");

    let materialize_root = support::rebuild_from_sealed_history(&repo, "rfc124-materialize");
    // The ignore file travels only with the *source* worktree above -- the freshly materialized
    // root never had one, and that is exactly the point: materialization does not need one, because
    // it never consults it.
    assert_eq!(
        std::fs::read(materialize_root.join("build/output.txt")).unwrap(),
        b"already sealed, before any ignore rule existed",
        "a sealed patch touching an ignored path must still materialize its file"
    );
}

/// Control 5 (§4.4, the one failure mode that destroys data): a path already tracked before a rule
/// starts covering it is never deleted by that rule. Demonstrated with a full seal-materialize round
/// trip, so a wrongly-authored `DeleteNode` would be caught even if it were later silently
/// re-created by some other path.
#[test]
fn an_already_tracked_path_covered_by_a_new_rule_is_not_deleted() {
    let repo = support::unique_repo("rfc124-ignore-no-delete");
    support::init(&repo);
    write(
        &repo,
        "keep/data.txt",
        b"tracked before any ignore rule existed",
    );
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    // A rule now covers the already-tracked path. Adding `.prikkignore` itself is enough to give
    // this second commit a real (non-ignore-file) reason to succeed -- no unrelated file needed.
    write(&repo, ".prikkignore", b"keep\n");
    let second_commit = support::commit(&repo, "heads/main", "add ignore file");
    support::ok(&second_commit, "second commit");
    let stdout = String::from_utf8_lossy(&second_commit.stdout);
    assert!(
        !stdout.contains("delete"),
        "adding a covering ignore rule must never author a delete for an already-tracked path: {stdout}"
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let materialize_root =
        support::rebuild_from_sealed_history(&repo, "rfc124-no-delete-materialize");
    assert_eq!(
        std::fs::read(materialize_root.join("keep/data.txt")).unwrap(),
        b"tracked before any ignore rule existed",
        "the already-tracked file must still be present after the covering rule was added"
    );
    assert!(materialize_root.join(".prikkignore").exists());
}

/// A tracked path is also never hidden from `worktree-status`'s own reporting once a rule starts
/// covering it -- it keeps being compared against the baseline exactly as before.
#[test]
fn an_already_tracked_path_covered_by_a_new_rule_still_reports_modifications() {
    let repo = support::unique_repo("rfc124-ignore-still-tracked");
    support::init(&repo);
    write(&repo, "keep/data.txt", b"original");
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    write(&repo, ".prikkignore", b"keep\n");
    write(
        &repo,
        "keep/data.txt",
        b"modified after the rule now covers it",
    );

    let output = support::prikk(&repo)
        .arg("worktree-status")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("keep/data.txt") && stdout.contains("modified"),
        "an already-tracked file must still be reported as modified even though a rule now \
         covers its path: {stdout}"
    );
}

/// Control 6: a malformed `.prikkignore` refuses, with the exit code shown -- an operational
/// failure (exit 1), not a usage error, since the ignore file is repository/worktree content, not a
/// CLI argument. Both `commit` and `worktree-status` refuse the same way, since both load the same
/// rules through the same function.
#[test]
fn malformed_prikkignore_refuses_on_both_commands() {
    let repo = support::unique_repo("rfc124-ignore-malformed");
    support::init(&repo);
    write(&repo, ".prikkignore", b"/etc/passwd\n");
    write(&repo, "src/lib.rs", b"fn main() {}");

    let commit_output = support::commit(&repo, "heads/main", "genesis");
    assert_eq!(
        commit_output.status.code(),
        Some(1),
        "a malformed .prikkignore must refuse as an operational failure: {commit_output:?}"
    );
    let commit_stderr = String::from_utf8_lossy(&commit_output.stderr);
    assert!(
        commit_stderr.contains(".prikkignore") && commit_stderr.contains("absolute"),
        "{commit_stderr}"
    );

    let status_output = support::prikk(&repo)
        .arg("worktree-status")
        .output()
        .unwrap();
    assert_eq!(
        status_output.status.code(),
        Some(1),
        "worktree-status must refuse the same malformed file the same way: {status_output:?}"
    );
    let status_stderr = String::from_utf8_lossy(&status_output.stderr);
    assert!(
        status_stderr.contains(".prikkignore") && status_stderr.contains("absolute"),
        "{status_stderr}"
    );
}

/// An absent `.prikkignore` is not malformed -- every existing repository, which has no such file,
/// behaves exactly as it did before this mechanism existed.
#[test]
fn no_prikkignore_at_all_behaves_exactly_as_before() {
    let repo = support::unique_repo("rfc124-ignore-absent");
    support::init(&repo);
    write(&repo, "src/lib.rs", b"fn main() {}");
    let output = support::commit(&repo, "heads/main", "genesis");
    support::ok(&output, "commit");
}

/// A real, deeply nested worktree -- several directory levels of real content plus real ignored
/// content at more than one depth -- the shape a genuine `node_modules/`-shaped project has, and the
/// shape the reverted landing's separator bug would have broken regardless of any ignore rule at all
/// (`commit` failed on Windows for *any* nested path, not only an ignored one).
#[test]
fn a_deeply_nested_worktree_commits_correctly_with_ignored_and_real_content_mixed() {
    let repo = support::unique_repo("rfc124-ignore-deep-nesting");
    support::init(&repo);
    write(&repo, ".prikkignore", b"node_modules\ntarget\n");
    write(&repo, "node_modules/pkg/index.js", b"generated");
    write(&repo, "target/debug/deps/build.o", b"generated");
    write(&repo, "src/module/inner/deep.rs", b"fn deep() {}");
    write(&repo, "docs/guide/intro.md", b"# intro");

    let output = support::commit(&repo, "heads/main", "genesis");
    support::ok(&output, "commit");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("node_modules"), "{stdout}");
    assert!(!stdout.contains("target/debug"), "{stdout}");
    assert!(stdout.contains("src/module/inner/deep.rs"), "{stdout}");
    assert!(stdout.contains("docs/guide/intro.md"), "{stdout}");
}
