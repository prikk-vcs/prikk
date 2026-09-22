//! RFC 153 §6.2, §6a and §7.2-§7.3, the diff handoff's Stage 2 and Addendum 1: `prikk diff` against the
//! worktree, the cost bound, and the two rename controls Stage 1 owed -- through the compiled binary.
//!
//! **The expected contents are the fixture's own constants**, never read back from prikk. The worktree side must
//! show exactly what `commit` would author, so several controls run `commit` itself and compare: what `diff`
//! showed before the commit is what the commit recorded, the report is empty again afterwards, and a refusal
//! `diff` names is the refusal `commit` prints.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use support::json::Value;

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn ok(repo: &Path, args: &[&str]) -> Output {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    output
}

fn stdout_of(repo: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&ok(repo, args).stdout).into_owned()
}

/// A refusal: the exit code, then the wording, then that **nothing** reached stdout.
fn refuses(repo: &Path, args: &[&str], code: i32, expected: &str) {
    let output = run(repo, args);
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}: {}",
        args.join(" "),
        text(&output)
    );
    assert!(
        text(&output).contains(expected),
        "{}: expected {expected:?} in {}",
        args.join(" "),
        text(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "{}: a refusal prints nothing on stdout: {:?}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout)
    );
}

fn tip(repo: &Path) -> String {
    stdout_of(repo, &["log", "--limit", "1"])
        .lines()
        .find_map(|line| line.strip_prefix("block "))
        .expect("log names a block")
        .trim()
        .to_string()
}

fn write(repo: &Path, path: &str, bytes: &[u8]) {
    let target = repo.join(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(target, bytes).unwrap();
}

fn commit_and_seal(repo: &Path, message: &str) -> String {
    ok(repo, &["commit", "-m", message]);
    ok(repo, &["seal", "--allow-no-audit"]);
    tip(repo)
}

/// Whether the platform under test can record an executable bit at all (see `rfc153_diff.rs`).
const EXECUTABLE_BIT_IS_AVAILABLE: bool = cfg!(unix);

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn report(repo: &Path, args: &[&str]) -> Value {
    let mut full = vec!["diff"];
    full.extend_from_slice(args);
    full.extend_from_slice(&["--format", "json"]);
    support::json::parse(&stdout_of(repo, &full))
}

fn entries(report: &Value) -> Vec<Value> {
    report.get("entries").as_array().to_vec()
}

fn statuses(report: &Value) -> Vec<(String, String)> {
    entries(report)
        .iter()
        .map(|entry| {
            (
                entry.get("path").as_str().to_string(),
                entry.get("status").as_str().to_string(),
            )
        })
        .collect()
}

fn has_key(value: &Value, key: &str) -> bool {
    matches!(value, Value::Object(map) if map.contains_key(key))
}

/// Every file under `.prikk/` with its bytes: "nothing written" is this map unchanged.
fn metadata_tree(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut tree = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                tree.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    tree
}

/// The sealed base the worktree fixture starts from: the fixture's own constants.
const BASE: &[(&str, &[u8])] = &[
    ("edit.txt", b"one\ntwo\nthree\n"),
    ("gone.txt", b"bye\n"),
    ("old.txt", b"stay the same\n"),
    ("keep.txt", b"keep\n"),
    ("run.sh", b"#!/bin/sh\n"),
    ("img.bin", b"\xff\x00AAAA"),
];
const EDIT_AFTER: &[u8] = b"one\nTWO\nthree\nfour\n";
const NEW_FILE: &[u8] = b"brand new\n";
const BINARY_AFTER: &[u8] = b"\xff\x00BBBBBB";

struct Wt {
    repo: PathBuf,
    base: String,
}

/// A repository with one sealed block, then a worktree edited in every way the statuses distinguish and **not
/// committed**: an edit, a delete, a create, a `chmod +x`, a `prikk mv`, and a changed binary.
fn worktree_fixture(tag: &str) -> Wt {
    let repo = support::unique_repo(tag);
    ok(&repo, &["setup", "."]);
    for (path, bytes) in BASE {
        write(&repo, path, bytes);
    }
    let base = commit_and_seal(&repo, "base");
    write(&repo, "edit.txt", EDIT_AFTER);
    std::fs::remove_file(repo.join("gone.txt")).unwrap();
    write(&repo, "new.txt", NEW_FILE);
    make_executable(&repo.join("run.sh"));
    ok(&repo, &["mv", "old.txt", "moved.txt"]);
    write(&repo, "img.bin", BINARY_AFTER);
    Wt { repo, base }
}

/// What the fixture's worktree must show, in canonical path order (the `mode` entry only where the platform can
/// record an executable bit).
fn expected_statuses() -> Vec<(String, String)> {
    let mut expected = vec![
        ("edit.txt", "modified"),
        ("gone.txt", "deleted"),
        ("img.bin", "binary"),
        ("moved.txt", "renamed"),
        ("new.txt", "added"),
    ];
    if EXECUTABLE_BIT_IS_AVAILABLE {
        expected.push(("run.sh", "mode"));
    }
    expected
        .into_iter()
        .map(|(path, status)| (path.to_string(), status.to_string()))
        .collect()
}

/// Control W1: a clean worktree gives an empty report, exit 0 -- and says what it compared.
#[test]
fn w1_a_clean_worktree_is_an_empty_report() {
    let repo = support::unique_repo("rfc153-wt-w1");
    ok(&repo, &["setup", "."]);
    for (path, bytes) in BASE {
        write(&repo, path, bytes);
    }
    let base = commit_and_seal(&repo, "base");

    let r = report(&repo, &[]);
    assert!(entries(&r).is_empty());
    assert!(r.get("unsupported_paths").as_array().is_empty());
    assert_eq!(r.get("schema_version").as_str(), "diff-report-v1");
    assert_eq!(r.get("from").get("point").as_str(), "heads/main");
    assert_eq!(r.get("from").get("target_block_id").as_str(), base);
    assert_eq!(
        r.get("from").get("queued_patches"),
        &Value::Number("0".to_string())
    );
    assert_eq!(r.get("to").get("point").as_str(), "worktree");
    assert_eq!(r.get("to").get("target_block_id").as_str(), "worktree");
    assert!(
        !has_key(&r, "untracked"),
        "RFC 153 §7.2: no untracked bucket"
    );
    let prose = stdout_of(&repo, &["diff"]);
    assert!(
        prose.contains(&format!("from: heads/main (block {base})\n")),
        "{prose}"
    );
    assert!(prose.contains("\nto: worktree\n"), "{prose}");
    assert!(prose.contains("\nno differences\n"), "{prose}");
    // `--from` alone names the same comparison from a point, and is empty too.
    assert!(entries(&report(&repo, &["--from", &base])).is_empty());
    assert!(entries(&report(&repo, &["--from", "heads/main"])).is_empty());
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control W2: after an edit, a create, a delete, `chmod +x` and `prikk mv`, each shows its status **once** --
/// and the renderer cannot lie here either: every text entry's hunks, applied to the fixture's own left content,
/// give the fixture's own right content.
#[test]
fn w2_each_status_shows_once_and_the_hunks_apply() {
    let w = worktree_fixture("rfc153-wt-w2");
    let r = report(&w.repo, &[]);
    assert_eq!(statuses(&r), expected_statuses());
    assert!(
        entries(&r)
            .iter()
            .all(|e| has_key(e, "minimal") && e.get("minimal") == &Value::Bool(true)),
        "every entry says whether its script is the shortest, and these are"
    );
    let by_path = |path: &str| {
        entries(&r)
            .into_iter()
            .find(|e| e.get("path").as_str() == path)
            .unwrap_or_else(|| panic!("no entry for {path}"))
    };
    let hunks_of = |entry: &Value| -> Vec<String> {
        entry
            .get("hunks")
            .as_array()
            .iter()
            .map(|h| h.as_str().to_string())
            .collect()
    };
    let left = |path: &str| -> &'static [u8] { BASE.iter().find(|(p, _)| *p == path).unwrap().1 };
    assert_eq!(
        support::apply_unified_hunks(
            std::str::from_utf8(left("edit.txt")).unwrap(),
            &hunks_of(&by_path("edit.txt"))
        )
        .unwrap()
        .as_bytes(),
        EDIT_AFTER
    );
    assert_eq!(
        support::apply_unified_hunks(
            std::str::from_utf8(left("gone.txt")).unwrap(),
            &hunks_of(&by_path("gone.txt"))
        )
        .unwrap(),
        ""
    );
    assert_eq!(
        support::apply_unified_hunks("", &hunks_of(&by_path("new.txt")))
            .unwrap()
            .as_bytes(),
        NEW_FILE
    );
    // The declared rename carries its old path and no hunks (the content is unchanged).
    let moved = by_path("moved.txt");
    assert_eq!(moved.get("from_path").as_str(), "old.txt");
    assert!(hunks_of(&moved).is_empty());
    // A binary file: ids and sizes, no bytes; the new side's id is what `commit` would give the blob.
    let image = by_path("img.bin");
    assert_eq!(
        image.get("from").get("size"),
        &Value::Number("6".to_string())
    );
    assert_eq!(image.get("to").get("size"), &Value::Number("8".to_string()));
    assert!(has_key(image.get("to"), "content_id") && has_key(image.get("from"), "content_id"));
    assert_ne!(
        image.get("to").get("content_id"),
        image.get("from").get("content_id")
    );
    let prose = stdout_of(&w.repo, &["diff"]);
    assert!(
        !prose.contains("AAAA") && !prose.contains("BBBB"),
        "binary bytes must never be printed: {prose}"
    );
    // Untouched files are not entries.
    assert!(
        !statuses(&r).iter().any(|(p, _)| p == "keep.txt"),
        "keep.txt is untouched"
    );
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W3 -- **the round trip, the RFC's central claim**: `commit` and then `prikk diff` is empty again,
/// what `diff` showed before the commit is what a point-against-worktree diff shows after it is sealed, and the
/// queue the bare diff folded is named.
#[test]
fn w3_commit_then_diff_is_empty_again() {
    let w = worktree_fixture("rfc153-wt-w3");
    let before = report(&w.repo, &[]);
    assert_eq!(statuses(&before), expected_statuses());

    ok(&w.repo, &["commit", "-m", "the change"]);
    // Committed, not sealed: the queue is part of the baseline `commit` authors against, so the diff is empty
    // -- and it says the left side carries one queued commit.
    let queued = report(&w.repo, &[]);
    assert!(entries(&queued).is_empty(), "{:?}", statuses(&queued));
    assert_eq!(
        queued.get("from").get("queued_patches"),
        &Value::Number("1".to_string())
    );
    assert_eq!(queued.get("from").get("target_block_id").as_str(), w.base);
    let prose = stdout_of(&w.repo, &["diff"]);
    assert!(
        prose.contains("plus 1 queued commit not yet sealed"),
        "{prose}"
    );
    // The sealed tip has not moved, so a point-against-worktree diff from it still shows the change.
    assert_eq!(
        statuses(&report(&w.repo, &["--from", "heads/main"])),
        expected_statuses()
    );

    ok(&w.repo, &["seal", "--allow-no-audit"]);
    let sealed = tip(&w.repo);
    assert_ne!(sealed, w.base);
    let after = report(&w.repo, &[]);
    assert!(entries(&after).is_empty());
    assert_eq!(after.get("from").get("target_block_id").as_str(), sealed);
    assert_eq!(
        after.get("from").get("queued_patches"),
        &Value::Number("0".to_string())
    );
    // Two roads to the same answer: the folded baseline (before) and the replayed base block (now).
    let from_base = report(&w.repo, &["--from", &w.base]);
    assert_eq!(
        from_base.get("entries"),
        before.get("entries"),
        "the entries a bare diff showed before the commit are the entries the sealed change now shows"
    );
    assert!(entries(&report(&w.repo, &["--from", &sealed])).is_empty());
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W4: a fresh repository, whose branch is unpublished, has an empty left side: every file `commit`
/// would author is `added`, exit 0. An explicit `--from` naming an absent ref refuses; a lone `--to` is a usage
/// error.
#[test]
fn w4_a_fresh_repository_lists_every_file_as_added() {
    let repo = support::unique_repo("rfc153-wt-w4");
    ok(&repo, &["setup", "."]);
    write(&repo, "a.txt", b"alpha\n");
    write(&repo, "dir/b.txt", b"bravo\n");
    write(&repo, "bin.dat", b"\xff\xfe\x00\x01");
    let r = report(&repo, &[]);
    assert_eq!(
        statuses(&r),
        vec![
            ("a.txt".to_string(), "added".to_string()),
            ("bin.dat".to_string(), "added".to_string()),
            ("dir/b.txt".to_string(), "added".to_string()),
        ]
    );
    assert_eq!(r.get("from").get("point").as_str(), "heads/main");
    assert!(r.get("from").get("target_block_id").is_null());
    assert_eq!(
        r.get("from").get("queued_patches"),
        &Value::Number("0".to_string())
    );
    let a = entries(&r)
        .into_iter()
        .find(|e| e.get("path").as_str() == "a.txt")
        .unwrap();
    assert_eq!(
        support::apply_unified_hunks("", &[a.get("hunks").as_array()[0].as_str().to_string()])
            .unwrap(),
        "alpha\n"
    );
    let prose = stdout_of(&repo, &["diff"]);
    assert!(
        prose.contains("from: heads/main (not published: the empty state)\n"),
        "{prose}"
    );
    // What it showed is what `commit` authored: nothing left to show once the commit is queued.
    ok(&repo, &["commit", "-m", "first"]);
    assert!(entries(&report(&repo, &[])).is_empty());

    refuses(
        &repo,
        &["diff", "--from", "heads/nope"],
        1,
        "error: precondition not met: ref heads/nope does not exist in this repository",
    );
    refuses(
        &repo,
        &["diff", "--to", "heads/main"],
        2,
        "diff --to needs --from as well",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control W5: a `.prikkignore`d path never appears -- not as an entry, not as an unsupported path -- and an
/// ignore rule cannot hide an already-tracked path (RFC 124 §4.4): a tracked file that a later rule covers still
/// shows its edit.
#[test]
fn w5_ignored_paths_never_appear_and_cannot_hide_a_tracked_one() {
    let repo = support::unique_repo("rfc153-wt-w5");
    ok(&repo, &["setup", "."]);
    write(&repo, "kept.txt", b"tracked\n");
    write(&repo, "later.txt", b"tracked, then ignored\n");
    let _ = commit_and_seal(&repo, "base");

    write(&repo, ".prikkignore", b"build\nlater.txt\n");
    write(&repo, "build/output.txt", b"generated\n");
    write(&repo, "build/deep/more.txt", b"generated\n");
    write(&repo, "later.txt", b"edited after the rule\n");
    write(&repo, "src.txt", b"real\n");
    let r = report(&repo, &[]);
    assert_eq!(
        statuses(&r),
        vec![
            (".prikkignore".to_string(), "added".to_string()),
            ("later.txt".to_string(), "modified".to_string()),
            ("src.txt".to_string(), "added".to_string()),
        ]
    );
    assert!(r.get("unsupported_paths").as_array().is_empty());
    let prose = stdout_of(&repo, &["diff"]);
    // (`.prikkignore` itself is an ordinary added file and its own hunk says `build`: it is the ignored files'
    // names that must be absent.)
    assert!(
        !prose.contains("output.txt") && !prose.contains("more.txt") && !prose.contains("build/"),
        "{prose}"
    );
    // And an ignored directory holding an entry `commit` cannot author is never even opened.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("kept.txt", repo.join("build/link")).unwrap();
        assert!(
            report(&repo, &[])
                .get("unsupported_paths")
                .as_array()
                .is_empty()
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control W6: **`worktree-status` agreement.** Its `untracked` files are `added` here (or a rename's
/// destination), its `missing` ones are `deleted` (or a rename's source), its `modified` ones are `modified` or
/// `binary` -- and nothing `diff` reports as a content change is one `worktree-status` does not.
#[test]
fn w6_worktree_status_and_diff_agree() {
    let w = worktree_fixture("rfc153-wt-w6");
    let status = run(&w.repo, &["worktree-status", "--format", "json"]);
    let status = support::json::parse(&String::from_utf8_lossy(&status.stdout));
    let r = report(&w.repo, &[]);
    let diff = statuses(&r);
    let status_of = |path: &str| {
        diff.iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s.as_str())
    };
    let renamed_from: Vec<String> = entries(&r)
        .iter()
        .filter(|e| has_key(e, "from_path"))
        .map(|e| e.get("from_path").as_str().to_string())
        .collect();

    let mut seen = 0;
    for change in status.get("changes").as_array() {
        let (path, kind) = (change.get("path").as_str(), change.get("kind").as_str());
        seen += 1;
        match kind {
            "untracked" => assert!(
                matches!(status_of(path), Some("added" | "renamed")),
                "untracked {path} must be added here, got {:?}",
                status_of(path)
            ),
            "missing" => assert!(
                status_of(path) == Some("deleted") || renamed_from.iter().any(|old| old == path),
                "missing {path} must be deleted (or a rename's source), got {:?}",
                status_of(path)
            ),
            "modified" => assert!(
                matches!(status_of(path), Some("modified" | "binary")),
                "modified {path}, got {:?}",
                status_of(path)
            ),
            other => panic!("unexpected status kind {other} for {path}"),
        }
    }
    assert!(
        seen >= 5,
        "the fixture has at least five changes status can see, saw {seen}"
    );
    // The converse: every content-level entry here is a change status sees too. (`mode` and the rename itself
    // are declarations and permission bits status does not report.)
    for (path, status_name) in &diff {
        let known = status
            .get("changes")
            .as_array()
            .iter()
            .any(|c| c.get("path").as_str() == path);
        match status_name.as_str() {
            "mode" => {}
            _ => assert!(known, "{path} ({status_name}) is not in worktree-status"),
        }
    }
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W7 (Unix): an entry `commit` refuses -- a new symlink, and a tracked file replaced by one -- is named
/// **once**, in `unsupported_paths`, with **`commit`'s own refusal** and no content; it is not also an entry (a
/// replaced file is not `deleted`), and the rest of the diff is still shown.
#[cfg(unix)]
#[test]
fn w7_an_entry_commit_refuses_is_named_with_its_refusal_and_no_content() {
    let w = worktree_fixture("rfc153-wt-w7");
    std::os::unix::fs::symlink("keep.txt", w.repo.join("link")).unwrap();
    std::fs::remove_file(w.repo.join("keep.txt")).unwrap();
    std::os::unix::fs::symlink("edit.txt", w.repo.join("keep.txt")).unwrap();

    let r = report(&w.repo, &[]);
    let unsupported: Vec<(String, String)> = r
        .get("unsupported_paths")
        .as_array()
        .iter()
        .map(|u| {
            (
                u.get("path").as_str().to_string(),
                u.get("refusal").as_str().to_string(),
            )
        })
        .collect();
    assert_eq!(
        unsupported
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>(),
        vec!["keep.txt", "link"],
        "named in path order, once each"
    );
    let paths: Vec<String> = statuses(&r).into_iter().map(|(p, _)| p).collect();
    assert!(
        !paths.iter().any(|p| p == "keep.txt" || p == "link"),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "edit.txt"),
        "the rest of the diff is still shown: {paths:?}"
    );
    // `commit`'s own refusal, byte for byte: what it prints is `error: ` + the refusal of the path it met first.
    let committed = run(&w.repo, &["commit", "-m", "refused"]);
    assert_eq!(committed.status.code(), Some(1), "{}", text(&committed));
    let stderr = String::from_utf8_lossy(&committed.stderr)
        .trim_end()
        .to_string();
    assert!(
        unsupported
            .iter()
            .any(|(_, refusal)| stderr == format!("error: {refusal}")),
        "commit said {stderr:?}, diff named {unsupported:?}"
    );
    // The same from a point: the point still has `keep.txt`, and the worktree's is refused, so it must be named
    // once and not reported `deleted` -- a second road to the same mistake, guarded separately.
    let from_base = report(&w.repo, &["--from", &w.base]);
    let named_from_base: Vec<String> = from_base
        .get("unsupported_paths")
        .as_array()
        .iter()
        .map(|u| u.get("path").as_str().to_string())
        .collect();
    assert_eq!(named_from_base, vec!["keep.txt", "link"]);
    let paths_from_base: Vec<String> = statuses(&from_base).into_iter().map(|(p, _)| p).collect();
    assert!(
        !paths_from_base
            .iter()
            .any(|p| p == "keep.txt" || p == "link"),
        "{paths_from_base:?}"
    );
    let prose = stdout_of(&w.repo, &["diff"]);
    assert!(prose.contains("unsupported paths: 2\n"), "{prose}");
    assert!(
        prose.contains(&format!("  link: {}", unsupported[1].1)),
        "{prose}"
    );
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W8: **read-only.** No write under `.prikk/` -- not a cache, not an index, not a lock -- for a bare
/// diff, for `--from`, with commits queued, **with the active lock held by someone else** (which refuses a
/// `commit`, and must not refuse a diff), and **while the worktree is provisional** (which refuses `commit`
/// too, and must not stop a look, nor clear the marker).
#[test]
fn w8_diff_is_read_only_under_a_lock_and_a_provisional_worktree() {
    let w = worktree_fixture("rfc153-wt-w8");
    ok(&w.repo, &["commit", "-m", "queued"]);
    write(&w.repo, "late.txt", b"after the queue\n");
    let before = metadata_tree(&w.repo);

    let plain = report(&w.repo, &[]);
    assert_eq!(
        statuses(&plain),
        vec![("late.txt".to_string(), "added".to_string())]
    );
    ok(&w.repo, &["diff"]);
    ok(&w.repo, &["diff", "--from", &w.base]);
    ok(&w.repo, &["diff", "--from", &w.base, "--path", "edit.txt"]);
    assert_eq!(
        metadata_tree(&w.repo),
        before,
        "a diff must write nothing under .prikk/"
    );

    // Someone else holds the active lock: `commit` conflicts, `diff` does not wait for it.
    let layout = prikk_store::RepositoryLayout::open(&w.repo).unwrap();
    let held = prikk_store::ActiveLock::acquire(&layout, prikk_store::DEFAULT_ACTIVE_NAME).unwrap();
    let locked_out = run(&w.repo, &["commit", "-m", "blocked"]);
    assert_ne!(
        locked_out.status.code(),
        Some(0),
        "the lock must really be held: {}",
        text(&locked_out)
    );
    let with_lock = metadata_tree(&w.repo);
    assert_eq!(report(&w.repo, &[]).get("entries"), plain.get("entries"));
    assert_eq!(
        metadata_tree(&w.repo),
        with_lock,
        "no write while another process holds the lock"
    );
    drop(held);

    // A provisional worktree: `commit` refuses to sign it, and the marker is what says so.
    let marker = w.repo.join(".prikk/worktree.provisional");
    let record = format!("PRIKK-PROVISIONAL-WORKTREE-v1 heads/main {}\n", w.base);
    std::fs::write(&marker, &record).unwrap();
    let refused = run(&w.repo, &["commit", "-m", "must refuse"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains("is not replay-verified"),
        "{}",
        text(&refused)
    );
    let provisional = metadata_tree(&w.repo);
    assert_eq!(report(&w.repo, &[]).get("entries"), plain.get("entries"));
    ok(&w.repo, &["diff", "--from", &w.base]);
    assert_eq!(
        metadata_tree(&w.repo),
        provisional,
        "no write while the worktree is provisional"
    );
    assert_eq!(
        std::fs::read(&marker).unwrap(),
        record.as_bytes(),
        "the marker is untouched"
    );
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W13: the refusals that depend on **content** and on **names**, named with `commit`'s own words. An
/// existing text file that is no longer valid UTF-8 cannot be authored (its node's kind is authoritative), and a
/// name that is not a safe repository path cannot be a path at all; `diff` names each, shows no content, and
/// still shows the rest.
#[test]
fn w13_a_content_or_name_commit_refuses_is_named_with_commits_words() {
    let repo = support::unique_repo("rfc153-wt-w13");
    ok(&repo, &["setup", "."]);
    write(&repo, "text.txt", b"plain text\n");
    write(&repo, "other.txt", b"other\n");
    let _ = commit_and_seal(&repo, "base");
    write(&repo, "text.txt", b"no longer \xff\xfe text\n");
    write(&repo, "other.txt", b"other, edited\n");

    let r = report(&repo, &[]);
    let unsupported: Vec<(String, String)> = r
        .get("unsupported_paths")
        .as_array()
        .iter()
        .map(|u| {
            (
                u.get("path").as_str().to_string(),
                u.get("refusal").as_str().to_string(),
            )
        })
        .collect();
    assert_eq!(unsupported.len(), 1, "{unsupported:?}");
    assert_eq!(unsupported[0].0, "text.txt");
    assert_eq!(
        statuses(&r),
        vec![("other.txt".to_string(), "modified".to_string())],
        "the refused path is not also an entry, and the rest is shown"
    );
    let prose = stdout_of(&repo, &["diff"]);
    assert!(
        !prose.contains("no longer"),
        "no content of a refused path: {prose}"
    );
    let committed = run(&repo, &["commit", "-m", "refused"]);
    assert_eq!(committed.status.code(), Some(1), "{}", text(&committed));
    assert_eq!(
        String::from_utf8_lossy(&committed.stderr).trim_end(),
        format!("error: {}", unsupported[0].1),
        "commit's own refusal, byte for byte"
    );

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"bad\xffname");
        // `cfg(unix)` is a statement about the **API surface** (`OsStr::from_bytes`, creating a file at
        // all) -- never a promise that a given filesystem will accept a given name. APFS (macOS) refuses to
        // create a non-UTF-8 name outright, so the write itself is the capability probe, not an assumption:
        // on a filesystem that refuses it, this half is skipped with a printed reason rather than assumed
        // to work everywhere `cfg(unix)` compiles. Where it does work (most Linux filesystems, including
        // CI's), the assertions below still run.
        match std::fs::write(repo.join(name), b"x") {
            Ok(()) => {
                let r = report(&repo, &[]);
                assert_eq!(
                    r.get("unsupported_paths").as_array().len(),
                    2,
                    "the unrepresentable name is named too"
                );
                let named: Vec<String> = r
                    .get("unsupported_paths")
                    .as_array()
                    .iter()
                    .map(|u| u.get("refusal").as_str().to_string())
                    .collect();
                let committed = run(&repo, &["commit", "-m", "refused"]);
                let stderr = String::from_utf8_lossy(&committed.stderr)
                    .trim_end()
                    .to_string();
                assert!(
                    named
                        .iter()
                        .any(|refusal| stderr == format!("error: {refusal}")),
                    "commit said {stderr:?}, diff named {named:?}"
                );
            }
            Err(err) => {
                println!(
                    "skipping the non-UTF-8 name half of this control: the filesystem refused to create \
                     the name ({err})"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control W14: the interrupted-materialization marker. `commit` refuses while it is set (a file the interrupted
/// write never created would be signed as a deletion); `diff` still looks, shows that file as `deleted` -- which
/// is exactly what `commit` would have signed -- and leaves the marker as it found it.
#[test]
fn w14_diff_looks_while_the_interrupted_materialization_marker_is_set() {
    let w = worktree_fixture("rfc153-wt-w14");
    let marker = w.repo.join(".prikk/worktree.marker");
    std::fs::write(&marker, b"PRIKK-WORKTREE-DIRTY\n").unwrap();
    let refused = run(&w.repo, &["commit", "-m", "must refuse"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains("materialization was interrupted"),
        "{}",
        text(&refused)
    );
    let before = metadata_tree(&w.repo);
    let r = report(&w.repo, &[]);
    assert_eq!(statuses(&r), expected_statuses());
    assert_eq!(metadata_tree(&w.repo), before);
    assert_eq!(std::fs::read(&marker).unwrap(), b"PRIKK-WORKTREE-DIRTY\n");
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W15: **a diff writes nothing, not even the rebuildable cache.** Right after a `seal` the baseline cache
/// is stale, which is the state in which the derivation shared with `commit` and `worktree-status` refreshes it;
/// `diff` shares that derivation and must leave `.prikk/` byte-identical anyway -- for a bare diff and for
/// `--from`, with a live declaration in the worktree.
#[test]
fn w15_a_diff_right_after_a_seal_writes_no_cache() {
    let w = worktree_fixture("rfc153-wt-w15");
    // The fixture sealed once and then only touched the worktree: the cache still describes the block before.
    let before = metadata_tree(&w.repo);
    let bare = report(&w.repo, &[]);
    assert_eq!(statuses(&bare), expected_statuses());
    let from = report(&w.repo, &["--from", &w.base]);
    assert_eq!(from.get("entries"), bare.get("entries"));
    assert_eq!(
        metadata_tree(&w.repo),
        before,
        "a diff must not refresh .prikk/cache/lifecycle-state.v1 or write anything else"
    );
    // The contrast that makes this control able to fail: `worktree-status` shares the derivation and refreshes.
    let _ = run(&w.repo, &["worktree-status"]);
    assert_ne!(
        metadata_tree(&w.repo),
        before,
        "precondition of this control: in this very state the shared derivation DOES refresh the cache (the \
         status command wrote it), so the equality above is a real test and not a vacuous one"
    );
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W9: `--path` filters the worktree report -- entries **and** unsupported paths -- and a rename is
/// found by either of its paths.
#[test]
fn w9_path_filters_the_worktree_report() {
    let w = worktree_fixture("rfc153-wt-w9");
    let only = report(&w.repo, &["--path", "edit.txt"]);
    assert_eq!(
        statuses(&only),
        vec![("edit.txt".to_string(), "modified".to_string())]
    );
    for wanted in ["moved.txt", "old.txt"] {
        let renamed = report(&w.repo, &["--path", wanted]);
        assert_eq!(
            statuses(&renamed),
            vec![("moved.txt".to_string(), "renamed".to_string())],
            "{wanted}"
        );
    }
    let prose = stdout_of(&w.repo, &["diff", "--path", "new.txt"]);
    assert!(
        prose.contains("added new.txt") && prose.contains("entries: 1\n"),
        "{prose}"
    );
    assert!(!prose.contains("edit.txt"), "{prose}");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("keep.txt", w.repo.join("link")).unwrap();
        assert_eq!(
            report(&w.repo, &["--path", "link"])
                .get("unsupported_paths")
                .as_array()
                .len(),
            1
        );
        assert!(
            report(&w.repo, &["--path", "edit.txt"])
                .get("unsupported_paths")
                .as_array()
                .is_empty()
        );
    }
    let _ = std::fs::remove_dir_all(&w.repo);
}

/// Control W10: a **refused rename declaration** refuses the diff with `commit`'s own words -- a diff of a commit
/// that cannot happen has no honest answer -- and prints nothing.
#[test]
fn w10_a_declaration_commit_refuses_refuses_the_diff_the_same_way() {
    let repo = support::unique_repo("rfc153-wt-w10");
    ok(&repo, &["setup", "."]);
    write(&repo, "a.txt", b"alpha\n");
    let _ = commit_and_seal(&repo, "base");
    ok(&repo, &["mv", "a.txt", "b.txt"]);
    write(&repo, "a.txt", b"the source is back\n");

    let committed = run(&repo, &["commit", "-m", "refused"]);
    assert_eq!(committed.status.code(), Some(1), "{}", text(&committed));
    let message = String::from_utf8_lossy(&committed.stderr)
        .trim_end()
        .to_string();
    assert!(
        message.contains("a.txt -> b.txt: both paths exist in the worktree"),
        "{message}"
    );
    let refused = run(&repo, &["diff"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(refused.stdout.is_empty());
    assert_eq!(String::from_utf8_lossy(&refused.stderr).trim_end(), message);
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control W11 (owed by Stage 1, §6.4): a **multi-hop rename** A -> B -> C is one `renamed` entry, A paired with
/// C; and a node **deleted and recreated under a new node id** is never `renamed` -- at a new path it is
/// `deleted` + `added`, at the same path it is `modified`.
#[test]
fn w11_a_multi_hop_rename_is_one_entry_and_a_recreated_node_is_never_renamed() {
    let repo = support::unique_repo("rfc153-wt-w11");
    ok(&repo, &["setup", "."]);
    write(&repo, "a.txt", b"the traveller\nsecond line\n");
    write(&repo, "x.txt", b"first life\n");
    write(&repo, "y.txt", b"first life of y\n");
    let b1 = commit_and_seal(&repo, "one");

    ok(&repo, &["mv", "a.txt", "b.txt"]);
    std::fs::remove_file(repo.join("x.txt")).unwrap();
    std::fs::remove_file(repo.join("y.txt")).unwrap();
    let b2 = commit_and_seal(&repo, "two");

    ok(&repo, &["mv", "b.txt", "c.txt"]);
    write(&repo, "z.txt", b"first life\n"); // x's bytes at a new path, new node
    write(&repo, "y.txt", b"second life of y\n"); // y's path again, new node
    let b3 = commit_and_seal(&repo, "three");

    let r = report(&repo, &["--from", &b1, "--to", &b3]);
    assert_eq!(
        statuses(&r),
        vec![
            ("c.txt".to_string(), "renamed".to_string()),
            ("x.txt".to_string(), "deleted".to_string()),
            ("y.txt".to_string(), "modified".to_string()),
            ("z.txt".to_string(), "added".to_string()),
        ]
    );
    let moved = entries(&r)
        .into_iter()
        .find(|e| e.get("path").as_str() == "c.txt")
        .unwrap();
    assert_eq!(
        moved.get("from_path").as_str(),
        "a.txt",
        "A is paired with C, not with the intermediate B"
    );
    assert_eq!(
        entries(&r)
            .iter()
            .filter(|e| e.get("status").as_str() == "renamed")
            .count(),
        1
    );
    // The middle block alone shows the first hop, so the pairing is per pair of points, not remembered.
    let hop = report(&repo, &["--from", &b1, "--to", &b2]);
    assert_eq!(
        hop.get("entries")
            .as_array()
            .iter()
            .find(|e| e.get("status").as_str() == "renamed")
            .unwrap()
            .get("path")
            .as_str(),
        "b.txt"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control W12 -- **the cost bound, end to end** (RFC 153 §6a C): a file reversed above the work bound is diffed
/// promptly, its entry says `minimal: false` in JSON and carries a note in the prose, its hunks still apply
/// byte for byte, and every other entry says `minimal: true`. The unit tests pin the algorithm; this is the
/// wiring.
#[test]
fn w12_above_the_bound_the_entry_says_so_and_still_applies() {
    let repo = support::unique_repo("rfc153-wt-w12");
    ok(&repo, &["setup", "."]);
    let lines: Vec<String> = (0..7_000).map(|i| format!("line {i}\n")).collect();
    let left: String = lines.concat();
    let right: String = lines.iter().rev().cloned().collect();
    write(&repo, "big.txt", left.as_bytes());
    write(&repo, "small.txt", b"one\ntwo\n");
    let _ = commit_and_seal(&repo, "base");
    write(&repo, "big.txt", right.as_bytes());
    write(&repo, "small.txt", b"one\nTWO\n");

    let r = report(&repo, &[]);
    let by_path = |path: &str| {
        entries(&r)
            .into_iter()
            .find(|e| e.get("path").as_str() == path)
            .unwrap()
    };
    let big = by_path("big.txt");
    assert_eq!(big.get("status").as_str(), "modified");
    assert_eq!(big.get("minimal"), &Value::Bool(false));
    let hunks: Vec<String> = big
        .get("hunks")
        .as_array()
        .iter()
        .map(|h| h.as_str().to_string())
        .collect();
    assert_eq!(
        support::apply_unified_hunks(&left, &hunks).unwrap(),
        right,
        "above the bound the hunks are larger, and they still reproduce the right side byte for byte"
    );
    let small = by_path("small.txt");
    assert_eq!(
        small.get("minimal"),
        &Value::Bool(true),
        "a small change is still the shortest script"
    );
    let prose = stdout_of(&repo, &["diff"]);
    assert!(
        prose.contains("modified big.txt\n  note: not the shortest edit script"),
        "{prose}"
    );
    assert_eq!(
        prose.matches("note: not the shortest").count(),
        1,
        "only the entry that hit the bound"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control W16 (RFC 153 §6b's corollary, the round's F1): the left side of a bare diff with **queued, unsealed
/// commits** is the tip *plus* that queue -- a state no block names. A binary entry's `prikk cat` hint must not
/// print a `--ref <block>` line for that side (it would read the sealed tip's bytes, not the side the entry
/// describes); it names the queue instead. The right side's hint is unaffected.
#[test]
fn w16_the_cat_hint_names_the_queue_instead_of_a_block_it_would_lie_about() {
    let w = worktree_fixture("rfc153-wt-w16");
    // Queue one commit -- the worktree now matches the queue, so nothing is left to diff yet.
    ok(&w.repo, &["commit", "-m", "queued"]);
    assert!(entries(&report(&w.repo, &[])).is_empty());
    // A fresh, uncommitted binary change on top of the queue: something for the entry list to show while
    // `queued_patches` is still 1.
    write(&w.repo, "img.bin", b"\xff\x00CCCCCCCC");

    let r = report(&w.repo, &[]);
    assert_eq!(
        r.get("from").get("queued_patches"),
        &Value::Number("1".to_string()),
        "precondition of this control: the left side must carry the queue"
    );
    let entry = entries(&r)
        .into_iter()
        .find(|e| e.get("path").as_str() == "img.bin")
        .expect("img.bin is a binary entry");
    assert_eq!(entry.get("status").as_str(), "binary");

    let prose = stdout_of(&w.repo, &["diff"]);
    assert!(
        prose.contains("the left side includes 1 queued commit that no block names"),
        "{prose}"
    );
    assert!(
        prose.contains("can be read only after `prikk seal`"),
        "{prose}"
    );
    // No `--ref <block-id>` line printed for img.bin on the left side: scan the binary-hint block for a `--ref`
    // that is not immediately followed by `worktree` (which would be the right side's line, absent here since
    // the worktree has no block at all -- confirmed separately by control W2/W7's binary assertions).
    let hint_block = prose
        .split("read a side with")
        .nth(1)
        .expect("img.bin's binary hint block");
    // Skip the fixed header line (`` `prikk cat --path <p> --ref <block-id>`: ``), which always contains the
    // literal text `--ref`; check only the per-side lines under it.
    let per_side_lines = hint_block
        .split_once('\n')
        .expect("lines under the header")
        .1;
    assert!(
        !per_side_lines.contains("--ref "),
        "no side of this entry should print a `--ref <block>` hint: {per_side_lines}"
    );
    let _ = std::fs::remove_dir_all(&w.repo);
}
