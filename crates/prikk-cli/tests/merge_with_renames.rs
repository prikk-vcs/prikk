//! Merge with renames (`rfcs/handoffs/144-two-point-comparison/merge-with-renames-design-handoff-v1.md`,
//! Addendum 1), through the compiled binary.
//!
//! Every case starts from one sealed base holding `a.txt` and `c.txt`, with `heads/feature` renaming
//! `a.txt` to `b.txt` (`prikk mv`). Content is read back with `checkout --patch-plan --format json
//! --content-path`, which replays renames.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn ok(repo: &Path, args: &[&str]) -> String {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn commit_and_seal(repo: &Path, message: &str) {
    ok(repo, &["commit", "-m", message]);
    ok(repo, &["seal", "--allow-no-audit"]);
}

/// The base, `heads/feature` renaming `a.txt` to `b.txt`, and `heads/main` checked out again. Returns the
/// repository and the base block.
fn base_with_renaming_feature(tag: &str) -> (PathBuf, String) {
    let repo = support::unique_repo(tag);
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), "alpha\nline2\nline3\n").unwrap();
    std::fs::write(repo.join("c.txt"), "gamma\n").unwrap();
    commit_and_seal(&repo, "base");
    let base = ok(&repo, &["log", "--limit", "1"])
        .lines()
        .find_map(|line| line.strip_prefix("block "))
        .expect("log names the block")
        .trim()
        .to_string();
    ok(
        &repo,
        &["branch", "create", "heads/feature", "--from", "heads/main"],
    );
    ok(&repo, &["branch", "switch", "heads/feature"]);
    ok(&repo, &["mv", "a.txt", "b.txt"]);
    commit_and_seal(&repo, "rename a to b");
    ok(&repo, &["branch", "switch", "heads/main"]);
    (repo, base)
}

fn merge(repo: &Path, base: &str, into: &str, from: &str) -> Output {
    run(
        repo,
        &[
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            base,
            "--into",
            into,
            "--from",
            from,
        ],
    )
}

/// The `content` and `not_found` of `checkout --patch-plan --format json` for `paths` on `reference`.
fn tree(repo: &Path, reference: &str, paths: &[&str]) -> String {
    let mut args = vec![
        "checkout",
        "--patch-plan",
        "--format",
        "json",
        "--ref",
        reference,
    ];
    for path in paths {
        args.push("--content-path");
        args.push(path);
    }
    let json = ok(repo, &args);
    let start = json.find("\"content\"").expect("content");
    json[start..].to_string()
}

/// Merge `heads/feature` into `heads/main`, having first merged `heads/main` into a copy of
/// `heads/feature` (before main holds the adopted rename): both succeed, `verify` exits 0, and both
/// directions give the byte-identical tree. Returns the tree.
fn merge_both_ways(repo: &Path, base: &str, paths: &[&str]) -> String {
    ok(
        repo,
        &["branch", "create", "heads/copy", "--from", "heads/feature"],
    );
    let reverse = merge(repo, base, "heads/copy", "heads/main");
    support::ok(&reverse, "merge main into a copy of feature");
    let forward = merge(repo, base, "heads/main", "heads/feature");
    support::ok(&forward, "merge feature into main");
    let verify = run(repo, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    let main = tree(repo, "heads/main", paths);
    assert_eq!(
        main,
        tree(repo, "heads/copy", paths),
        "both directions agree"
    );
    main
}

/// S1: main edits `c.txt`.
#[test]
fn s1_a_rename_merges_beside_an_edit_of_another_file() {
    let (repo, base) = base_with_renaming_feature("renames-s1");
    std::fs::write(repo.join("c.txt"), "gamma2\n").unwrap();
    commit_and_seal(&repo, "edit c");
    let merged = merge_both_ways(&repo, &base, &["a.txt", "b.txt", "c.txt"]);
    assert!(
        merged.contains(r#""path": "b.txt""#)
            && merged.contains(r#""text": "alpha\nline2\nline3\n""#),
        "{merged}"
    );
    assert!(merged.contains(r#""text": "gamma2\n""#), "{merged}");
    assert!(merged.contains(r#""not_found": ["a.txt"]"#), "{merged}");
}

/// S2: main does nothing after the base.
#[test]
fn s2_a_rename_merges_when_the_other_side_has_nothing() {
    let (repo, base) = base_with_renaming_feature("renames-s2");
    let forward = merge(&repo, &base, "heads/main", "heads/feature");
    support::ok(&forward, "merge feature into main");
    let verify = run(&repo, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    let merged = tree(&repo, "heads/main", &["a.txt", "b.txt"]);
    assert!(merged.contains(r#""path": "b.txt""#), "{merged}");
    assert!(merged.contains(r#""not_found": ["a.txt"]"#), "{merged}");
}

/// S3: main edits the renamed file under its old name; the merged `b.txt` holds main's edit.
#[test]
fn s3_a_rename_merges_with_an_edit_of_the_same_file() {
    let (repo, base) = base_with_renaming_feature("renames-s3");
    std::fs::write(repo.join("a.txt"), "alpha-main\nline2\nline3\n").unwrap();
    commit_and_seal(&repo, "edit a");
    let merged = merge_both_ways(&repo, &base, &["a.txt", "b.txt", "c.txt"]);
    assert!(
        merged.contains(r#""path": "b.txt""#)
            && merged.contains(r#""text": "alpha-main\nline2\nline3\n""#),
        "{merged}"
    );
    assert!(merged.contains(r#""not_found": ["a.txt"]"#), "{merged}");
}

/// The S3 twin: main makes the same rename itself, then edits `b.txt`. Identical renames on both sides
/// stay a conflict (ruling 3).
#[test]
fn s3_twin_the_same_rename_on_both_sides_is_refused_as_a_conflict() {
    let (repo, base) = base_with_renaming_feature("renames-s3-twin");
    ok(&repo, &["mv", "a.txt", "b.txt"]);
    commit_and_seal(&repo, "the same rename");
    std::fs::write(repo.join("b.txt"), "alpha\nline2\nline3-main\n").unwrap();
    commit_and_seal(&repo, "edit b");
    let refused = merge(&repo, &base, "heads/main", "heads/feature");
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains("outcome: Conflict"),
        "{}",
        text(&refused)
    );
}

/// Twin-b: main renames `c.txt` to `b2.txt` and edits it; both renames land.
#[test]
fn twin_b_renames_of_different_files_on_both_sides_merge() {
    let (repo, base) = base_with_renaming_feature("renames-twin-b");
    ok(&repo, &["mv", "c.txt", "b2.txt"]);
    commit_and_seal(&repo, "rename c to b2");
    std::fs::write(repo.join("b2.txt"), "gamma-main\n").unwrap();
    commit_and_seal(&repo, "edit b2");
    let merged = merge_both_ways(&repo, &base, &["a.txt", "b.txt", "b2.txt", "c.txt"]);
    assert!(merged.contains(r#""path": "b.txt""#), "{merged}");
    assert!(
        merged.contains(r#""path": "b2.txt""#) && merged.contains(r#""text": "gamma-main\n""#),
        "{merged}"
    );
    assert!(
        merged.contains(r#""not_found": ["a.txt","c.txt"]"#),
        "{merged}"
    );
}

/// A merge block that lands on a checkpoint (the 65th block of `heads/main`) carries a snapshot whose
/// manifest names the renamed path, and a snapshot checkout plan agrees.
#[test]
fn a_merge_on_a_checkpoint_block_snapshots_the_renamed_path() {
    let (repo, base) = base_with_renaming_feature("renames-checkpoint");
    // Blocks 2..=64 on main, touching only c.txt; the merge is block 65, a checkpoint.
    for index in 2..=64 {
        std::fs::write(repo.join("c.txt"), format!("gamma {index}\n")).unwrap();
        commit_and_seal(&repo, &format!("edit c {index}"));
    }
    let forward = merge(&repo, &base, "heads/main", "heads/feature");
    support::ok(&forward, "merge feature into main");
    let verify = run(&repo, &["verify"]);
    assert_eq!(verify.status.code(), Some(0), "{}", text(&verify));
    let snapshot = ok(
        &repo,
        &["checkout", "--snapshot-plan", "--ref", "heads/main"],
    );
    assert!(snapshot.contains("b.txt"), "{snapshot}");
    assert!(!snapshot.contains("a.txt"), "{snapshot}");
    let merged = tree(&repo, "heads/main", &["a.txt", "b.txt"]);
    assert!(merged.contains(r#""path": "b.txt""#), "{merged}");
    assert!(merged.contains(r#""not_found": ["a.txt"]"#), "{merged}");
}
