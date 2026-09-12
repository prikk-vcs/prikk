//! RFC 147 §2e: `worktree-status` reports, per entry, whether `commit` would author the path or
//! refuse it — and with what reason.
//!
//! The field is **orthogonal to `kind`**: a path can be `modified` *and* refused, `untracked` *and*
//! refused. `unsupported-path` keeps its old meaning (an unrepresentable *name*) and is not where
//! refusals go; every test below asserts `unsupported paths: 0` alongside a non-zero refusal count
//! precisely so a later change that folds one into the other fails here.
//!
//! The agreement between the two commands is not asserted twice with two hand-written
//! expectations. `status_and_commit_agree_on_the_same_tree` runs **both commands against one
//! worktree** and compares `commit`'s own stderr against the string `worktree-status` printed —
//! whatever that string is. If the shared classifier's wording changes, this test keeps passing; if
//! the two sides ever render it differently, it fails, which is the only thing worth pinning.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
// Every refusal the classifier can currently raise needs a symlink or another non-regular worktree
// entry, and a test cannot create one portably -- so the whole file is unix-gated, on
// `foundation/fsutil/tests/directory.rs`'s own precedent. The consequence is worth stating rather
// than hiding behind the attribute: **on Windows this field has no test coverage at all**, because
// on Windows there is no way for an ordinary worktree to reach a refusal in the first place. If a
// later round adds a refusal reachable without a symlink, its test does not belong in this file.
#![cfg(target_family = "unix")]

mod support;

use std::path::{Path, PathBuf};

use support::json;

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// One agreement case: a fixture tag, the path it refuses, and the builder that produces the tree.
type Case = (&'static str, &'static str, fn(&str) -> PathBuf);

/// `worktree-status` exits **1** for any worktree that differs from the baseline — refused or not.
/// That is the pre-existing contract (it is the "is this tree clean" question), and this round does
/// not change it: see `a_merely_modified_tree_exits_the_same_way`, which pins the other half.
fn worktree_status(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut command = support::prikk(repo);
    command.arg("worktree-status");
    command.args(args);
    command.output().unwrap()
}

fn commit_attempt(repo: &Path, message: &str) -> std::process::Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env("PRIKK_AUTHOR_SEED", support::AUTHOR_SEED_HEX)
        .args(["commit", "-m", message])
        .output()
        .unwrap()
}

/// One committed regular file, `a.txt`, and nothing else.
fn repo_with_one_tracked_file(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&commit_attempt(&repo, "add a.txt"), "commit a.txt");
    repo
}

/// A tracked regular file replaced by a symlink: the entry is `modified` *and* refused.
fn repo_with_a_tracked_file_now_a_symlink(tag: &str) -> PathBuf {
    let repo = repo_with_one_tracked_file(tag);
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    std::os::unix::fs::symlink("/etc/hostname", repo.join("a.txt")).unwrap();
    repo
}

/// A tracked regular file replaced by a **dangling** symlink — one whose target does not exist.
///
/// RFC 147 §2e G3: this is the case that used to read as `Missing`, because the tracked-path
/// presence check resolved the link. `commit`'s own walk sees the directory entry and refuses, so
/// the two commands disagreed. The fixture deliberately points at a name that will never exist.
fn repo_with_a_tracked_file_now_a_dangling_symlink(tag: &str) -> PathBuf {
    let repo = repo_with_one_tracked_file(tag);
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    std::os::unix::fs::symlink("nope-does-not-exist.txt", repo.join("a.txt")).unwrap();
    assert!(
        !repo.join("a.txt").exists(),
        "the fixture is only meaningful if the link really dangles"
    );
    repo
}

/// An untracked symlink beside a clean tracked file: the entry is `untracked` *and* refused.
fn repo_with_an_untracked_symlink(tag: &str) -> PathBuf {
    let repo = repo_with_one_tracked_file(tag);
    std::os::unix::fs::symlink("/etc/hostname", repo.join("link.txt")).unwrap();
    repo
}

fn change_for<'a>(report: &'a json::Value, path: &str) -> &'a json::Value {
    report
        .get("changes")
        .as_array()
        .iter()
        .find(|change| change.get("path").as_str() == path)
        .unwrap_or_else(|| panic!("no change entry for {path} in {report:?}"))
}

fn prose_line_for(prose: &str, path: &str) -> String {
    prose
        .lines()
        .find(|line| line.starts_with("  ") && line.contains(path))
        .unwrap_or_else(|| panic!("no entry line for {path} in:\n{prose}"))
        .to_string()
}

fn counter(prose: &str, label: &str) -> String {
    prose
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{label}: ")))
        .unwrap_or_else(|| panic!("no `{label}:` line in:\n{prose}"))
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// Control 1: both truths in one entry, in both forms
// ---------------------------------------------------------------------------------------------

/// The entry is `modified` **and** refused, in prose and in JSON, at once. `kind` did not become a
/// refusal marker and the refusal did not become a kind.
#[test]
fn a_refused_entry_keeps_its_change_kind_in_both_forms() {
    let repo = repo_with_a_tracked_file_now_a_symlink("rfc147a-both-truths");

    let prose = stdout_of(&worktree_status(&repo, &[]));
    assert_eq!(counter(&prose, "modified files"), "1", "{prose}");
    assert_eq!(counter(&prose, "unsupported paths"), "0", "{prose}");
    assert_eq!(counter(&prose, "refused paths"), "1", "{prose}");

    let line = prose_line_for(&prose, "a.txt");
    // The marker is a *bracketed suffix*. A consumer that splits the entry line on whitespace and
    // reads the first word still reads the change kind, exactly as it did before this field
    // existed -- which is why the marker cannot be mistaken for one.
    assert_eq!(
        line.split_whitespace().next(),
        Some("modified"),
        "the first word of an entry line must still be the change kind: {line}"
    );
    assert!(
        line.contains(" [refused: "),
        "the refusal is a bracketed suffix: {line}"
    );

    let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));
    assert_eq!(
        report.get("refused_count").clone(),
        json::Value::Number("1".to_string())
    );
    let change = change_for(&report, "a.txt");
    assert_eq!(change.get("kind").as_str(), "modified");
    assert_eq!(change.get("authoring").as_str(), "refused");
    assert!(
        !change.get("refusal").is_null(),
        "a refused entry carries its reason: {change:?}"
    );
    // The prose suffix and the JSON field are the same string, not two renderings of one decision.
    assert!(
        line.contains(change.get("refusal").as_str()),
        "prose suffix and JSON `refusal` must be the same string:\n{line}\n{change:?}"
    );
}

/// The same for an *untracked* symlink: a different `kind`, the same refusal machinery, and
/// `unsupported paths` still `0` — control 4's case.
#[test]
fn an_untracked_symlink_is_refused_without_becoming_an_unsupported_path() {
    let repo = repo_with_an_untracked_symlink("rfc147a-untracked-symlink");

    let prose = stdout_of(&worktree_status(&repo, &[]));
    assert_eq!(counter(&prose, "untracked files"), "1", "{prose}");
    assert_eq!(counter(&prose, "unsupported paths"), "0", "{prose}");
    assert_eq!(counter(&prose, "refused paths"), "1", "{prose}");

    let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));
    assert_eq!(
        report.get("refused_count").clone(),
        json::Value::Number("1".to_string())
    );
    let change = change_for(&report, "link.txt");
    assert_eq!(change.get("kind").as_str(), "untracked");
    assert_eq!(change.get("authoring").as_str(), "refused");
}

/// RFC 147 §2e G3: a **dangling** symlink at a tracked path is refused, not reported as missing.
///
/// The presence check used to resolve the link, so this exact tree read as `missing a.txt —
/// tracked file is absent from the worktree` with `refused paths: 0`, while `commit` refused it.
/// The rule never changed; the path simply never reached the rule. Both halves are asserted here:
/// the entry is refused **and** `missing files: 0`, because a report that called this both missing
/// and refused would be describing two different worktrees.
#[test]
fn a_dangling_symlink_at_a_tracked_path_is_refused_not_missing() {
    let repo = repo_with_a_tracked_file_now_a_dangling_symlink("rfc147a-dangling");

    let prose = stdout_of(&worktree_status(&repo, &[]));
    assert_eq!(counter(&prose, "missing files"), "0", "{prose}");
    assert_eq!(counter(&prose, "modified files"), "1", "{prose}");
    assert_eq!(counter(&prose, "unsupported paths"), "0", "{prose}");
    assert_eq!(counter(&prose, "refused paths"), "1", "{prose}");

    let line = prose_line_for(&prose, "a.txt");
    assert_eq!(line.split_whitespace().next(), Some("modified"), "{line}");
    assert!(line.contains(" [refused: "), "{line}");

    let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));
    let change = change_for(&report, "a.txt");
    assert_eq!(change.get("kind").as_str(), "modified");
    assert_eq!(change.get("authoring").as_str(), "refused");

    let _ = std::fs::remove_dir_all(&repo);
}

/// The other half of the presence change: a path that is **genuinely absent** is still `Missing`,
/// and carries no refusal. Making presence non-following must not turn deletions into refusals.
#[test]
fn a_genuinely_absent_tracked_path_is_still_missing() {
    let repo = repo_with_one_tracked_file("rfc147a-absent");
    std::fs::remove_file(repo.join("a.txt")).unwrap();

    let prose = stdout_of(&worktree_status(&repo, &[]));
    assert_eq!(counter(&prose, "missing files"), "1", "{prose}");
    assert_eq!(counter(&prose, "refused paths"), "0", "{prose}");

    let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));
    let change = change_for(&report, "a.txt");
    assert_eq!(change.get("kind").as_str(), "missing");
    assert_eq!(change.get("authoring").as_str(), "authored");
    assert!(change.get("refusal").is_null(), "{change:?}");

    let _ = std::fs::remove_dir_all(&repo);
}

// ---------------------------------------------------------------------------------------------
// Control 2: status and commit agree, on one tree, by comparison rather than by two expectations
// ---------------------------------------------------------------------------------------------

/// **One tree, both commands.** Whatever reason `worktree-status` reports for a refused entry,
/// `commit`'s own stderr is `error: ` followed by exactly that string. Nothing here hard-codes the
/// wording, so the test cannot pass by two expectations drifting together.
#[test]
fn status_and_commit_agree_on_the_same_tree() {
    let cases: [Case; 3] = [
        (
            "rfc147a-agree-modified",
            "a.txt",
            repo_with_a_tracked_file_now_a_symlink,
        ),
        (
            "rfc147a-agree-untracked",
            "link.txt",
            repo_with_an_untracked_symlink,
        ),
        // RFC 147 §2e G3: one more fixture through this same test, not a second test with its own
        // expectations -- a dangling symlink is the case where the two commands actually diverged.
        (
            "rfc147a-agree-dangling",
            "a.txt",
            repo_with_a_tracked_file_now_a_dangling_symlink,
        ),
    ];
    for (tag, path, build) in cases {
        let repo = build(tag);

        let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));
        let reported = change_for(&report, path)
            .get("refusal")
            .as_str()
            .to_string();

        let attempt = commit_attempt(&repo, "would this author?");
        assert_eq!(attempt.status.code(), Some(1), "{path}: commit must refuse");
        assert_eq!(
            stderr_of(&attempt),
            format!("error: {reported}\n"),
            "{path}: commit's message must be the reason status reported"
        );
    }
}

/// The converse, which is the half a one-sided test would miss: for a tree whose only changes are
/// ordinary edits and additions, no entry is refused and `commit` **proceeds**. A field that said
/// "refused" for everything would pass the test above and fail here.
///
/// Read the scope exactly: this is *not* "`refused_count == 0` implies `commit` succeeds" in
/// general. `a_contradicted_rename_declaration_is_not_reported_as_refused` below is the
/// counterexample, and it is a real, measured limitation of this round's classifier.
#[test]
fn a_tree_with_no_refused_entries_commits() {
    let repo = repo_with_one_tracked_file("rfc147a-no-refusals");
    std::fs::write(repo.join("a.txt"), "hello, edited\n").unwrap();
    std::fs::write(repo.join("b.txt"), "new\n").unwrap();

    let prose = stdout_of(&worktree_status(&repo, &[]));
    assert_eq!(counter(&prose, "refused paths"), "0", "{prose}");
    assert!(
        !prose.contains("[refused:"),
        "no entry carries a marker: {prose}"
    );

    let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));
    assert_eq!(
        report.get("refused_count").clone(),
        json::Value::Number("0".to_string())
    );
    for change in report.get("changes").as_array() {
        assert_eq!(change.get("authoring").as_str(), "authored", "{change:?}");
        assert!(change.get("refusal").is_null(), "{change:?}");
    }

    support::ok(
        &commit_attempt(&repo, "authors both paths"),
        "commit proceeds when status reports no refusal",
    );
}

/// **A known gap, pinned so it cannot be mistaken for coverage.** `commit` also refuses a worktree
/// that contradicts a live rename declaration (`AuthorError::DeclarationContradicted`, already
/// `Precondition` since RFC 132). That refusal is *not* a property of a single path's shape, so this
/// round's classifier -- which sees a path's baseline kind and its `symlink_metadata`, and nothing
/// else -- cannot produce it, and `worktree-status` reports `refused_count: 0` for a tree `commit`
/// will refuse.
///
/// `worktree-status` already prints the declaration and already sees the source back on disk, so the
/// information is there; joining it is new scope this round deliberately did not take. The assertion
/// below is therefore a statement of the current, incomplete truth, not an endorsement of it: when a
/// later round closes the gap, this test fails, which is exactly when someone should be reading it.
#[test]
fn a_contradicted_rename_declaration_is_not_reported_as_refused() {
    let repo = repo_with_one_tracked_file("rfc147a-declaration-gap");
    support::ok(
        &support::prikk(&repo)
            .args(["mv", "a.txt", "b.txt"])
            .output()
            .unwrap(),
        "declare the move",
    );
    // Contradict it: the declared source is back on disk, so the move was never completed.
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();

    let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));
    assert_eq!(
        report.get("refused_count").clone(),
        json::Value::Number("0".to_string()),
        "the classifier sees no refusal here -- the gap, measured"
    );

    let attempt = commit_attempt(&repo, "would this author?");
    assert_eq!(attempt.status.code(), Some(1), "but commit refuses anyway");
    assert!(
        stderr_of(&attempt).starts_with("error: precondition not met: "),
        "and refuses as a precondition, not as damage: {}",
        stderr_of(&attempt)
    );
}

// ---------------------------------------------------------------------------------------------
// Control 3: the schema stayed additive
// ---------------------------------------------------------------------------------------------

/// A consumer written against the **old** field set reads the new document unchanged: every field
/// it knew is present, with the value it would have had. This is the evidence for staying inside
/// `worktree-status-report-v1` rather than minting a v2 — the `declarations` precedent, applied to
/// its own test rather than asserted in a comment.
#[test]
fn an_old_shape_consumer_reads_the_new_document_unchanged() {
    let repo = repo_with_a_tracked_file_now_a_symlink("rfc147a-old-consumer");
    let report = json::parse(&stdout_of(&worktree_status(&repo, &["--format", "json"])));

    assert_eq!(
        report.get("schema_version").as_str(),
        "worktree-status-report-v1",
        "additive, so no bump"
    );
    // Every top-level field the old shape had, read the old way.
    assert!(!report.get("repository").as_str().is_empty());
    assert_eq!(report.get("ref").as_str(), "heads/main");
    assert_eq!(
        report.get("tracked_files").clone(),
        json::Value::Number("1".to_string())
    );
    assert_eq!(
        report.get("unchanged_files").clone(),
        json::Value::Number("0".to_string())
    );
    assert!(!report.get("clean").as_bool());
    assert!(report.get("queued_elsewhere").is_null());
    assert!(report.get("declarations").as_array().is_empty());

    // And every per-change field, with its old meaning: `kind` is still the change kind and
    // `detail` is still the change's own detail -- neither was repurposed to carry the refusal.
    let change = change_for(&report, "a.txt");
    assert_eq!(change.get("path").as_str(), "a.txt");
    assert_eq!(change.get("kind").as_str(), "modified");
    assert_eq!(
        change.get("detail").as_str(),
        "tracked path is not a regular file"
    );
}

// ---------------------------------------------------------------------------------------------
// Control 5: exit codes
// ---------------------------------------------------------------------------------------------

/// `worktree-status` exits 1 for a changed tree whether or not anything is refused, and `commit`
/// exits 1 on a refusal — never 2, which RFC 121 reserves for usage. The refusal field is a
/// *report*, so it does not get its own exit code; the pair below is what says so.
#[test]
fn a_merely_modified_tree_exits_the_same_way() {
    let refused = repo_with_a_tracked_file_now_a_symlink("rfc147a-exit-refused");
    let merely_modified = repo_with_one_tracked_file("rfc147a-exit-modified");
    std::fs::write(merely_modified.join("a.txt"), "hello, edited\n").unwrap();

    assert_eq!(worktree_status(&refused, &[]).status.code(), Some(1));
    assert_eq!(
        worktree_status(&merely_modified, &[]).status.code(),
        Some(1)
    );
    assert_eq!(
        worktree_status(&refused, &["--format", "json"])
            .status
            .code(),
        Some(1)
    );

    assert_eq!(commit_attempt(&refused, "refused").status.code(), Some(1));
    support::ok(&commit_attempt(&merely_modified, "authored"), "commit");
}
