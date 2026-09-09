//! RFC 144 increment 3, piece 1: `prikk mv` and declaration-based rename authoring
//! (`rfcs/handoffs/144-two-point-comparison/increment-3-piece-1-declared-move-capture-handoff-v1.md`).
//!
//! The six required controls, end to end through the compiled binary. **This is the first sealed
//! prikk history to ever contain a `RenamePath`** -- until this round nothing authored one; increments
//! 1 and 2 only ever exercised replay/materialization against hand-built raw patches
//! (`rfc144_increment1_rename_replay.rs`).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use support::{commit, init, ok, prikk, rebuild_from_sealed_history, seal, unique_repo, verify};

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn mv(repo: &std::path::Path, old: &str, new: &str) -> std::process::Output {
    prikk(repo).args(["mv", old, new]).output().unwrap()
}

/// Control 1: a declared move authors one `RenamePath`, and replay accepts it -- `prikk mv`,
/// `commit`, `seal`, then verify, then rebuild from sealed history and check the bytes moved.
#[test]
fn control1_declared_move_authors_one_rename_path_and_replay_accepts_it() {
    let repo = unique_repo("rfc144-inc3-p1-control1");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");
    ok(&seal(&repo, "heads/main"), "seal a.txt");

    let mv_out = mv(&repo, "a.txt", "b.txt");
    ok(&mv_out, "mv a.txt b.txt");
    assert!(!repo.join("a.txt").exists(), "old path must be moved away");
    assert!(repo.join("b.txt").exists(), "new path must exist");

    let commit_out = commit(&repo, "heads/main", "rename a.txt to b.txt");
    ok(&commit_out, "commit the declared rename");
    let stdout = stdout_of(&commit_out);
    assert_eq!(
        stdout.matches("rename-path").count(),
        1,
        "expected exactly one rename-path operation: {stdout}"
    );
    assert!(
        stdout.contains("rename-path a.txt -> b.txt"),
        "expected the rename-path change line to name both paths: {stdout}"
    );

    ok(&seal(&repo, "heads/main"), "seal the rename");
    ok(&verify(&repo), "verify");

    let materialized = rebuild_from_sealed_history(&repo, "rfc144-inc3-p1-control1");
    assert!(
        !materialized.join("a.txt").exists(),
        "old path must not exist after materialization"
    );
    assert_eq!(
        std::fs::read(materialized.join("b.txt")).unwrap(),
        b"hello\n"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized);
}

/// Control 2: a chained declaration authors the net move -- `mv a b`, `mv b c`, commit; assert one
/// operation `a -> c` (never `a -> b` or `b -> c`), and that seal and verify both accept it.
#[test]
fn control2_chained_declaration_authors_the_net_move() {
    let repo = unique_repo("rfc144-inc3-p1-control2");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"chained\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");
    ok(&seal(&repo, "heads/main"), "seal a.txt");

    ok(&mv(&repo, "a.txt", "b.txt"), "mv a.txt b.txt");
    ok(&mv(&repo, "b.txt", "c.txt"), "mv b.txt c.txt");
    assert!(!repo.join("a.txt").exists());
    assert!(!repo.join("b.txt").exists());
    assert!(repo.join("c.txt").exists());

    let commit_out = commit(&repo, "heads/main", "chained rename a to c");
    ok(&commit_out, "commit the chained rename");
    let stdout = stdout_of(&commit_out);
    assert_eq!(
        stdout.matches("rename-path").count(),
        1,
        "a chain must collapse to exactly one operation: {stdout}"
    );
    assert!(
        stdout.contains("rename-path a.txt -> c.txt"),
        "expected the net move a.txt -> c.txt, not an intermediate hop: {stdout}"
    );
    assert!(!stdout.contains("a.txt -> b.txt"));
    assert!(!stdout.contains("b.txt -> c.txt"));

    ok(&seal(&repo, "heads/main"), "seal the chained rename");
    ok(&verify(&repo), "verify");

    let materialized = rebuild_from_sealed_history(&repo, "rfc144-inc3-p1-control2");
    assert!(!materialized.join("a.txt").exists());
    assert!(!materialized.join("b.txt").exists());
    assert_eq!(
        std::fs::read(materialized.join("c.txt")).unwrap(),
        b"chained\n"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized);
}

/// Control 3: a round-trip declaration authors nothing -- `mv a b`, `mv b a`, commit; assert no
/// `RenamePath` (or anything else) is authored, and the live-declaration store itself is empty.
#[test]
fn control3_round_trip_declaration_authors_nothing() {
    let repo = unique_repo("rfc144-inc3-p1-control3");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"round-trip\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");
    ok(&seal(&repo, "heads/main"), "seal a.txt");

    ok(&mv(&repo, "a.txt", "b.txt"), "mv a.txt b.txt");
    let second_mv = mv(&repo, "b.txt", "a.txt");
    ok(&second_mv, "mv b.txt a.txt");
    assert!(
        repo.join("a.txt").exists(),
        "worktree must be back to a.txt"
    );
    assert!(!repo.join("b.txt").exists());

    // RFC 144 §4p.2: the round trip is resolved (and the declaration dropped) at the second `mv`
    // itself, not at the next commit -- by commit time the declaration store already has nothing
    // left to disclose about it, so the disclosure belongs on `mv`'s own output.
    let second_mv_stdout = stdout_of(&second_mv);
    assert!(
        second_mv_stdout.contains("declaration a.txt -> b.txt -> a.txt: nets to no move"),
        "expected mv's own output to disclose the dropped round trip: {second_mv_stdout}"
    );

    let status_out = prikk(&repo).arg("worktree-status").output().unwrap();
    let status_stdout = stdout_of(&status_out);
    assert!(
        status_stdout.contains("live rename declarations: 0"),
        "a round-trip declaration must be dropped entirely, not stored as a.txt -> a.txt: \
         {status_stdout}"
    );

    // The worktree is byte-identical to the sealed baseline, so there is nothing to author at all --
    // proof that the round trip left no RenamePath queued (a broken collapse that instead recorded
    // two separate declarations would trip the contradiction refusal below, not this one, so this
    // assertion alone would not catch that failure mode; see this round's own report for the
    // perturbation that did).
    let commit_out = commit(&repo, "heads/main", "should be a no-op");
    assert!(
        !commit_out.status.success(),
        "expected commit to refuse: nothing changed"
    );
    let stderr = stderr_of(&commit_out);
    assert!(
        stderr.contains("no node-addressed changes"),
        "expected the ordinary empty-commit refusal, not a declaration-related one: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: a declared move whose destination is then deleted authors a plain `DeleteNode`, no
/// rename.
#[test]
fn control4_declared_move_whose_destination_is_deleted_authors_plain_delete() {
    let repo = unique_repo("rfc144-inc3-p1-control4");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"to be deleted\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");
    ok(&seal(&repo, "heads/main"), "seal a.txt");

    ok(&mv(&repo, "a.txt", "b.txt"), "mv a.txt b.txt");
    std::fs::remove_file(repo.join("b.txt")).unwrap();
    assert!(!repo.join("a.txt").exists());
    assert!(!repo.join("b.txt").exists());

    let commit_out = commit(
        &repo,
        "heads/main",
        "declared move whose destination vanished",
    );
    ok(&commit_out, "commit the deletion");
    let stdout = stdout_of(&commit_out);
    assert!(
        !stdout.contains("rename-path"),
        "a move whose destination was deleted must not author a rename: {stdout}"
    );
    assert!(
        stdout.contains("delete-file a.txt"),
        "expected a plain delete-file for a.txt: {stdout}"
    );
    assert!(
        stdout.contains("declaration a.txt -> b.txt: destination is gone"),
        "expected commit to disclose what the declaration became: {stdout}"
    );

    ok(&seal(&repo, "heads/main"), "seal the deletion");
    ok(&verify(&repo), "verify");

    let materialized = rebuild_from_sealed_history(&repo, "rfc144-inc3-p1-control4");
    assert!(!materialized.join("a.txt").exists());
    assert!(!materialized.join("b.txt").exists());

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized);
}

/// Control 5: a worktree contradicting a live declaration refuses the commit, with the declaration
/// named.
#[test]
fn control5_worktree_contradicting_declaration_refuses_commit() {
    let repo = unique_repo("rfc144-inc3-p1-control5");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"original\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");
    ok(&seal(&repo, "heads/main"), "seal a.txt");

    ok(&mv(&repo, "a.txt", "b.txt"), "mv a.txt b.txt");
    // Contradict the live declaration: the source is back, by whatever means (a shell `cp`, an
    // editor's autosave, or -- as here -- a plain write).
    std::fs::write(repo.join("a.txt"), b"resurrected\n").unwrap();

    let commit_out = commit(&repo, "heads/main", "should be refused");
    assert!(
        !commit_out.status.success(),
        "expected commit to refuse a contradicted declaration"
    );
    let stderr = stderr_of(&commit_out);
    assert!(
        stderr.contains("a.txt") && stderr.contains("b.txt"),
        "the declaration must be named in the refusal: {stderr}"
    );
    assert!(
        stderr.contains("present in the worktree again"),
        "expected the contradiction to be named as such: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6: the two-node swap, end to end -- the case increments 1 and 2 were built for, now
/// reachable for the first time. Author both declarations in one commit, seal, verify.
#[test]
fn control6_two_node_swap_end_to_end() {
    let repo = unique_repo("rfc144-inc3-p1-control6");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"content-a\n").unwrap();
    std::fs::write(repo.join("c.txt"), b"content-c\n").unwrap();
    ok(
        &commit(&repo, "heads/main", "add a.txt and c.txt"),
        "commit a.txt and c.txt",
    );
    ok(&seal(&repo, "heads/main"), "seal a.txt and c.txt");

    // a -> tmp, c -> a, tmp -> c: collapses to the swap, a <-> c.
    ok(&mv(&repo, "a.txt", "tmp.txt"), "mv a.txt tmp.txt");
    ok(&mv(&repo, "c.txt", "a.txt"), "mv c.txt a.txt");
    ok(&mv(&repo, "tmp.txt", "c.txt"), "mv tmp.txt c.txt");
    assert!(!repo.join("tmp.txt").exists());
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"content-c\n");
    assert_eq!(std::fs::read(repo.join("c.txt")).unwrap(), b"content-a\n");

    let status_out = prikk(&repo).arg("worktree-status").output().unwrap();
    let status_stdout = stdout_of(&status_out);
    assert!(
        status_stdout.contains("live rename declarations: 2"),
        "expected exactly two live declarations (the swap), not a collapsed or contradicted one: \
         {status_stdout}"
    );

    let commit_out = commit(&repo, "heads/main", "swap a.txt and c.txt");
    ok(&commit_out, "commit the swap");
    let stdout = stdout_of(&commit_out);
    assert_eq!(
        stdout.matches("rename-path").count(),
        2,
        "expected exactly two rename-path operations: {stdout}"
    );
    assert!(stdout.contains("rename-path a.txt -> c.txt"), "{stdout}");
    assert!(stdout.contains("rename-path c.txt -> a.txt"), "{stdout}");

    ok(&seal(&repo, "heads/main"), "seal the swap");
    ok(&verify(&repo), "verify");

    let materialized = rebuild_from_sealed_history(&repo, "rfc144-inc3-p1-control6");
    assert_eq!(
        std::fs::read(materialized.join("a.txt")).unwrap(),
        b"content-c\n",
        "a.txt must now hold the node that used to be c.txt"
    );
    assert_eq!(
        std::fs::read(materialized.join("c.txt")).unwrap(),
        b"content-a\n",
        "c.txt must now hold the node that used to be a.txt"
    );
    assert!(!materialized.join("tmp.txt").exists());

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialized);
}

/// RFC 144 §4p.2: a declared move whose destination is excluded by `.prikkignore` also nets to
/// deletion -- the same outcome as an outright deletion (control 4), but for a different reason,
/// and §4p.2 requires the disclosure to say which. Reproduces the review's own repro exactly.
#[test]
fn declared_move_onto_an_ignored_destination_nets_to_deletion_and_discloses_why() {
    let repo = unique_repo("rfc144-inc3-p1-ignore-disclosure");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"ignored destination\n").unwrap();
    std::fs::write(repo.join(".prikkignore"), b"build\n").unwrap();
    std::fs::create_dir_all(repo.join("build")).unwrap();
    ok(
        &commit(&repo, "heads/main", "add a.txt and .prikkignore"),
        "commit a.txt and .prikkignore",
    );
    ok(&seal(&repo, "heads/main"), "seal a.txt and .prikkignore");

    ok(&mv(&repo, "a.txt", "build/a.txt"), "mv a.txt build/a.txt");
    assert!(!repo.join("a.txt").exists());
    assert!(
        repo.join("build/a.txt").exists(),
        "the file really did move"
    );

    let commit_out = commit(&repo, "heads/main", "moved into an ignored directory");
    ok(&commit_out, "commit the deletion");
    let stdout = stdout_of(&commit_out);
    assert!(
        !stdout.contains("rename-path"),
        "an ignored destination must not author a rename: {stdout}"
    );
    assert!(
        stdout.contains("delete-file a.txt"),
        "expected a plain delete-file for a.txt: {stdout}"
    );
    assert!(
        stdout.contains("declaration a.txt -> build/a.txt: destination is ignored"),
        "expected commit to name the ignore-match as the reason, not a generic deletion: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 144 §4p.2: a declaration whose source was never a tracked baseline node is disclosed too --
/// reachable via `prikk mv`'s own row 1 (old exists, new doesn't) against an untracked file: the
/// physical move and the declaration both happen, but the source was never a sealed node.
#[test]
fn declared_move_of_a_never_tracked_source_discloses_it() {
    let repo = unique_repo("rfc144-inc3-p1-never-tracked-disclosure");
    init(&repo);
    // Nothing committed yet -- x.txt is a plain untracked file.
    std::fs::write(repo.join("x.txt"), b"never sealed\n").unwrap();

    ok(&mv(&repo, "x.txt", "y.txt"), "mv x.txt y.txt");
    assert!(!repo.join("x.txt").exists());
    assert!(repo.join("y.txt").exists());

    let commit_out = commit(&repo, "heads/main", "first commit, after an untracked mv");
    ok(&commit_out, "commit");
    let stdout = stdout_of(&commit_out);
    assert!(
        stdout.contains("create-file y.txt"),
        "y.txt is a real untracked file and must still be created: {stdout}"
    );
    assert!(
        stdout.contains("declaration x.txt -> y.txt: source was never a tracked node"),
        "expected commit to disclose the never-tracked declaration: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `prikk mv`'s own four worktree states (§4o.1), independent of the controls above: refuses when
/// both paths exist, refuses when neither exists, and row 2 ("old gone, new exists") records the
/// declaration alone, touching no bytes.
#[test]
fn mv_refuses_when_both_or_neither_path_exists_and_declares_only_for_an_already_moved_file() {
    let repo = unique_repo("rfc144-inc3-p1-mv-states");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"x\n").unwrap();
    std::fs::write(repo.join("b.txt"), b"y\n").unwrap();
    ok(
        &commit(&repo, "heads/main", "add a.txt and b.txt"),
        "commit a.txt and b.txt",
    );
    ok(&seal(&repo, "heads/main"), "seal a.txt and b.txt");

    // Both exist.
    let both = mv(&repo, "a.txt", "b.txt");
    assert!(
        !both.status.success(),
        "expected a refusal when both paths exist"
    );

    // Neither exists.
    let neither = mv(&repo, "nope.txt", "also-nope.txt");
    assert!(
        !neither.status.success(),
        "expected a refusal when neither path exists"
    );

    // Row 2: old gone (moved by hand), new exists -- declaration only, no bytes touched.
    std::fs::rename(repo.join("a.txt"), repo.join("c.txt")).unwrap();
    let before = std::fs::read(repo.join("c.txt")).unwrap();
    ok(
        &mv(&repo, "a.txt", "c.txt"),
        "mv a.txt c.txt (already moved)",
    );
    let after = std::fs::read(repo.join("c.txt")).unwrap();
    assert_eq!(before, after, "row 2 must not touch bytes");

    let status_out = prikk(&repo).arg("worktree-status").output().unwrap();
    let status_stdout = stdout_of(&status_out);
    assert!(status_stdout.contains("a.txt -> c.txt"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// §4o.3: `worktree-status --format json` carries live declarations as a machine-branchable field,
/// not only in prose.
#[test]
fn worktree_status_json_carries_live_declarations() {
    let repo = unique_repo("rfc144-inc3-p1-status-json");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"json\n").unwrap();
    ok(&commit(&repo, "heads/main", "add a.txt"), "commit a.txt");
    ok(&seal(&repo, "heads/main"), "seal a.txt");

    ok(&mv(&repo, "a.txt", "b.txt"), "mv a.txt b.txt");

    let status_out = prikk(&repo)
        .args(["worktree-status", "--format", "json"])
        .output()
        .unwrap();
    let stdout = stdout_of(&status_out);
    assert!(
        stdout.contains("\"schema_version\": \"worktree-status-report-v1\""),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"old_path\": \"a.txt\"") && stdout.contains("\"new_path\": \"b.txt\""),
        "expected the live declaration as a structured field: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
