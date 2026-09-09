//! RFC 144 increment 3, piece 2: the commit-time move hint
//! (`rfcs/handoffs/144-two-point-comparison/increment-3-piece-2-commit-time-hint-handoff-v1.md`).
//!
//! `commit` may print a hint for an unambiguous delete+create pair with byte-identical content --
//! authors nothing, changes nothing already planned. The five required controls, end to end
//! through the compiled binary, plus a dedicated check for the empty-file edge case §4o.4's own
//! case 2 asks to be verified rather than assumed.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use support::{commit, init, ok, unique_repo};

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn operation_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("  ") && !line.trim_start().starts_with("declaration"))
        .collect()
}

fn extract_line<'a>(stdout: &'a str, label: &str) -> &'a str {
    stdout
        .lines()
        .find(|line| line.starts_with(label))
        .unwrap_or_else(|| panic!("expected a {label} line: {stdout}"))
}

/// Control 1: a shell `mv` (not `prikk mv`) then commit prints the hint, naming both paths, and
/// authors delete+create exactly as an ordinary unrelated delete+create would -- asserted directly,
/// not merely that a hint appeared.
#[test]
fn control1_shell_move_prints_hint_and_authors_delete_create_unchanged() {
    let repo = unique_repo("rfc144-inc3-p2-control1");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"identical content\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");

    // A shell move, deliberately not `prikk mv` -- this is exactly the person the hint exists for.
    std::fs::rename(repo.join("a.txt"), repo.join("b.txt")).unwrap();

    let commit_out = commit(&repo, "heads/main", "shell-moved a.txt to b.txt");
    ok(&commit_out, "commit the shell move");
    let stdout = stdout_of(&commit_out);
    assert!(
        stdout.contains("delete-file a.txt"),
        "a shell move must still author a plain delete: {stdout}"
    );
    assert!(
        stdout.contains("create-file b.txt"),
        "a shell move must still author a plain create: {stdout}"
    );
    assert!(
        !stdout.contains("rename-path"),
        "a shell move was never declared and must not author a rename: {stdout}"
    );
    assert!(
        stdout.contains(
            "hint: looks like a.txt moved to b.txt; `prikk mv` would have preserved its identity"
        ),
        "expected the exact hint wording, naming both paths: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2: a declared move (`prikk mv`) authors a `RenamePath`, not a delete+create pair, so it
/// never reaches the heuristic at all -- no hint.
#[test]
fn control2_declared_move_prints_no_hint() {
    let repo = unique_repo("rfc144-inc3-p2-control2");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"declared\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");

    let mv_out = support::prikk(&repo)
        .args(["mv", "a.txt", "b.txt"])
        .output()
        .unwrap();
    ok(&mv_out, "mv a.txt b.txt");

    let commit_out = commit(&repo, "heads/main", "declared rename a.txt to b.txt");
    ok(&commit_out, "commit the declared rename");
    let stdout = stdout_of(&commit_out);
    assert!(stdout.contains("rename-path a.txt -> b.txt"), "{stdout}");
    assert!(
        !stdout.contains("hint:"),
        "a declared move authored a RenamePath and must never also print a hint: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: an ambiguous set -- two deleted files and two created files all sharing identical
/// content -- prints no hint for that content at all, rather than guessing a pairing.
#[test]
fn control3_ambiguous_set_prints_no_hint() {
    let repo = unique_repo("rfc144-inc3-p2-control3");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"shared\n").unwrap();
    std::fs::write(repo.join("b.txt"), b"shared\n").unwrap();
    ok(
        &commit(&repo, "heads/main", "add a.txt and b.txt"),
        "commit a.txt and b.txt",
    );

    std::fs::remove_file(repo.join("a.txt")).unwrap();
    std::fs::remove_file(repo.join("b.txt")).unwrap();
    std::fs::write(repo.join("c.txt"), b"shared\n").unwrap();
    std::fs::write(repo.join("d.txt"), b"shared\n").unwrap();

    let commit_out = commit(&repo, "heads/main", "ambiguous reshuffle");
    ok(&commit_out, "commit the ambiguous set");
    let stdout = stdout_of(&commit_out);
    assert!(stdout.contains("delete-file a.txt"), "{stdout}");
    assert!(stdout.contains("delete-file b.txt"), "{stdout}");
    assert!(stdout.contains("create-file c.txt"), "{stdout}");
    assert!(stdout.contains("create-file d.txt"), "{stdout}");
    assert!(
        !stdout.contains("hint:"),
        "which deleted file moved to which created file is unknowable here -- no hint, not a \
         guess: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: a mass reorganisation (more than `MOVE_HINT_SUMMARY_THRESHOLD` unambiguous pairs)
/// prints one summary line, not one hint per pair.
#[test]
fn control4_mass_reorganisation_prints_the_summary_not_n_lines() {
    let repo = unique_repo("rfc144-inc3-p2-control4");
    init(&repo);
    const PAIR_COUNT: usize = 6; // > prikk_store::MOVE_HINT_SUMMARY_THRESHOLD (5)
    for index in 0..PAIR_COUNT {
        std::fs::write(
            repo.join(format!("old-{index}.txt")),
            format!("unique content {index}\n"),
        )
        .unwrap();
    }
    ok(
        &commit(&repo, "heads/main", "add six files"),
        "commit six files",
    );

    for index in 0..PAIR_COUNT {
        std::fs::rename(
            repo.join(format!("old-{index}.txt")),
            repo.join(format!("new-{index}.txt")),
        )
        .unwrap();
    }

    let commit_out = commit(&repo, "heads/main", "mass shell reorganisation");
    ok(&commit_out, "commit the mass reorganisation");
    let stdout = stdout_of(&commit_out);
    assert_eq!(
        stdout.matches("hint:").count(),
        1,
        "expected exactly one summary line, not one per pair: {stdout}"
    );
    assert!(
        stdout.contains("hint: 6 deleted/created pairs in this commit look like moves"),
        "expected the summary to name the count: {stdout}"
    );
    assert!(
        !stdout.contains("looks like old-0.txt moved to new-0.txt"),
        "the summary must not also list individual pairs: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5 (the one that matters, per the handoff): whether the hint fires or not must never
/// change what a commit authors. Two independent repositories, same shape (one file deleted, one
/// file created), differing only in whether the created file's content matches the deleted file's
/// (hint-eligible) or not (hint-ineligible) -- the authored operation kinds, paths, and counts must
/// be identical either way; only `report.move_hints` (surfaced as the extra `hint:` line) differs.
#[test]
fn control5_the_hint_never_changes_what_is_authored() {
    let hint_fires = unique_repo("rfc144-inc3-p2-control5-fires");
    init(&hint_fires);
    std::fs::write(hint_fires.join("a.txt"), b"same bytes\n").unwrap();
    ok(
        &commit(&hint_fires, "heads/main", "add a.txt"),
        "commit a.txt (fires)",
    );
    std::fs::remove_file(hint_fires.join("a.txt")).unwrap();
    std::fs::write(hint_fires.join("b.txt"), b"same bytes\n").unwrap();
    let fires_out = commit(
        &hint_fires,
        "heads/main",
        "delete a, create b (matching content)",
    );
    ok(&fires_out, "commit delete+create (fires)");
    let fires_stdout = stdout_of(&fires_out);
    assert!(
        fires_stdout.contains("hint:"),
        "expected the hint to fire for matching content: {fires_stdout}"
    );

    let hint_silent = unique_repo("rfc144-inc3-p2-control5-silent");
    init(&hint_silent);
    std::fs::write(hint_silent.join("a.txt"), b"same bytes\n").unwrap();
    ok(
        &commit(&hint_silent, "heads/main", "add a.txt"),
        "commit a.txt (silent)",
    );
    std::fs::remove_file(hint_silent.join("a.txt")).unwrap();
    std::fs::write(hint_silent.join("b.txt"), b"different bytes\n").unwrap();
    let silent_out = commit(
        &hint_silent,
        "heads/main",
        "delete a, create b (different content)",
    );
    ok(&silent_out, "commit delete+create (silent)");
    let silent_stdout = stdout_of(&silent_out);
    assert!(
        !silent_stdout.contains("hint:"),
        "expected no hint for non-matching content: {silent_stdout}"
    );

    // The only difference between these two runs is a single byte's worth of content in the
    // created file -- which is irrelevant to operation identity. Compare everything the hint could
    // plausibly have influenced: operation kinds/paths (order-independent), and the counts.
    let mut fires_ops = operation_lines(&fires_stdout);
    let mut silent_ops = operation_lines(&silent_stdout);
    fires_ops.sort_unstable();
    silent_ops.sort_unstable();
    assert_eq!(
        fires_ops, silent_ops,
        "the hint firing must not change which operations are authored: fires={fires_stdout} \
         silent={silent_stdout}"
    );

    assert_eq!(
        extract_line(&fires_stdout, "operations:"),
        extract_line(&silent_stdout, "operations:")
    );
    assert_eq!(
        extract_line(&fires_stdout, "referenced blobs:"),
        extract_line(&silent_stdout, "referenced blobs:")
    );

    let _ = std::fs::remove_dir_all(&hint_fires);
    let _ = std::fs::remove_dir_all(&hint_silent);
}

/// §4o.4 case 2, checked directly rather than assumed: a lone 1:1 pair of unrelated empty files is
/// *not* caught by the ambiguity check (that check only fires on more than one candidate per side),
/// so zero-byte content must be excluded explicitly. Two unrelated empty files, one deleted and one
/// created in the same commit, must print no hint.
#[test]
fn empty_file_content_is_excluded_explicitly_not_caught_by_ambiguity_alone() {
    let repo = unique_repo("rfc144-inc3-p2-empty-file");
    init(&repo);
    std::fs::write(repo.join("placeholder.txt"), b"").unwrap();
    ok(
        &commit(&repo, "heads/main", "add an empty file"),
        "commit placeholder.txt",
    );

    std::fs::remove_file(repo.join("placeholder.txt")).unwrap();
    std::fs::write(repo.join("unrelated-empty.txt"), b"").unwrap();

    let commit_out = commit(&repo, "heads/main", "delete one empty file, create another");
    ok(&commit_out, "commit the empty-file pair");
    let stdout = stdout_of(&commit_out);
    assert!(stdout.contains("delete-file placeholder.txt"), "{stdout}");
    assert!(
        stdout.contains("create-file unrelated-empty.txt"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("hint:"),
        "a lone 1:1 pair of empty files is exactly one candidate per side, so the ambiguity check \
         alone would not catch it -- zero-byte content must be excluded explicitly: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
