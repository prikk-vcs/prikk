//! `prikk extract` (0.54.0 step 3 Part 2), through the compiled binary.
//!
//! The trust rule, the pre-walk refusal, and the no-replace-rename control are pinned directly at
//! the `prikk-store` level (`crates/prikk-store/src/extract/tests.rs`,
//! `crates/prikk-store/src/patch_replay/extract/tests.rs`), where a fixture's own signing/
//! state-root crafting is reachable; this file covers what only the compiled binary and a real
//! filesystem can show: byte-identity against `cat --output`/`tree`, the write-safety refusals, and
//! that a successful run never touches `.prikk/`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn extract(repo: &Path, point: &str, target: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["extract", "--ref", point, target.to_str().unwrap()];
    args.extend_from_slice(extra);
    support::prikk(repo).args(args).output().unwrap()
}

fn sha256_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

/// `extract`'s own output must be byte-for-byte identical to `cat --output` of every path `tree`
/// lists at the same point -- text, binary, a file edited after the anchor, a renamed file, and a
/// deleted file correctly absent -- with no extra files.
#[test]
fn extract_matches_cat_and_tree_across_text_binary_edit_rename_and_delete() {
    let repo = support::unique_repo("extract-byte-identity");

    support::init(&repo);
    std::fs::write(repo.join("text.txt"), b"line one\nline two\n").unwrap();
    std::fs::write(
        repo.join("bin.dat"),
        [0_u8, 1, 2, 255, 254, 253]
            .iter()
            .chain(b"binary payload")
            .copied()
            .collect::<Vec<u8>>(),
    )
    .unwrap();
    std::fs::write(repo.join("old_name.txt"), b"rename me, unedited after\n").unwrap();
    std::fs::write(repo.join("to_delete.txt"), b"this one goes away\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "commit genesis",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    std::fs::write(repo.join("text.txt"), b"line one EDITED\nline two\n").unwrap();
    support::ok(
        &support::prikk(&repo)
            .args(["mv", "old_name.txt", "new_name.txt"])
            .output()
            .unwrap(),
        "mv",
    );
    std::fs::remove_file(repo.join("to_delete.txt")).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "edit, rename, delete"),
        "commit edits",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal edits");

    // The point's own listing, from `tree`.
    let tree_output = support::prikk(&repo)
        .args(["tree", "--ref", "heads/main", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&tree_output, "tree");
    let listing = support::json::parse(&stdout(&tree_output));
    let entries = listing.get("entries").as_array();
    assert!(
        entries.len() >= 3,
        "fixture sanity: expected at least text.txt, bin.dat and new_name.txt, got {entries:?}"
    );
    let listed_paths: Vec<String> = entries
        .iter()
        .map(|entry| entry.get("path").as_str().to_string())
        .collect();
    assert!(listed_paths.contains(&"text.txt".to_string()));
    assert!(listed_paths.contains(&"bin.dat".to_string()));
    assert!(listed_paths.contains(&"new_name.txt".to_string()));
    assert!(!listed_paths.contains(&"old_name.txt".to_string()));
    assert!(!listed_paths.contains(&"to_delete.txt".to_string()));

    // Each listed path's own `cat --output` bytes, the oracle.
    let oracle_dir = support::unique_repo("extract-byte-identity-cat-oracle");
    let mut expected = BTreeMap::new();
    for path in &listed_paths {
        let cat_destination = oracle_dir.join(path.replace('/', "__"));
        let _ = std::fs::remove_file(&cat_destination);
        let cat_output = support::prikk(&repo)
            .args([
                "cat",
                "--path",
                path,
                "--ref",
                "heads/main",
                "--output",
                cat_destination.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        support::ok(&cat_output, &format!("cat --path {path}"));
        expected.insert(
            PathBuf::from(path),
            std::fs::read(&cat_destination).unwrap(),
        );
        let _ = std::fs::remove_file(&cat_destination);
    }

    let out = support::unique_repo("extract-byte-identity-out");
    std::fs::remove_dir_all(&out).unwrap(); // unique_repo creates it; extract must create it itself
    let extract_output = extract(&repo, "heads/main", &out, &[]);
    support::ok(&extract_output, "extract");
    let report = stdout(&extract_output);
    assert!(report.contains("provisional: no"), "{report}");
    assert!(report.contains("files written: 3"), "{report}");

    let actual = sha256_tree(&out);
    assert_eq!(
        actual, expected,
        "extract's own output must be byte-for-byte identical to cat --output of every path tree \
         lists, with no extra files"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&oracle_dir);
}

/// A bare block id resolves and extracts identically to the ref that names the same block.
#[test]
fn extract_accepts_a_bare_block_id() {
    let repo = support::unique_repo("extract-block-id");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let tree_output = support::prikk(&repo)
        .args(["tree", "--ref", "heads/main", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&tree_output, "tree");
    let listing = support::json::parse(&stdout(&tree_output));
    let block_id = listing.get("target_block_id").as_str().to_string();

    let out = support::unique_repo("extract-block-id-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&repo, &block_id, &out, &[]);
    support::ok(&output, "extract by block id");
    assert_eq!(std::fs::read(out.join("README.md")).unwrap(), b"hello\n");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
}

/// Write safety: an existing target refuses, writes nothing, and never touches `.prikk/`.
#[test]
fn extract_refuses_an_existing_target_and_touches_nothing() {
    let repo = support::unique_repo("extract-existing-target");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-existing-target-out"); // already exists
    let before = support::store_bytes(&repo);

    let output = extract(&repo, "heads/main", &out, &[]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("refusing to extract into an existing path"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = support::store_bytes(&repo);
    assert_eq!(
        before, after,
        ".prikk/ must be byte-identical after a refused extract"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
}

/// Write safety: an existing temporary sibling from a previous run refuses by name, rather than
/// being silently reused or deleted.
#[test]
fn extract_refuses_an_existing_temporary_sibling() {
    let repo = support::unique_repo("extract-existing-sibling");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-existing-sibling-out");
    std::fs::remove_dir_all(&out).unwrap();
    let sibling = out.with_file_name(format!(
        "{}.prikk-extract-tmp",
        out.file_name().unwrap().to_str().unwrap()
    ));
    let _ = std::fs::remove_dir_all(&sibling);
    std::fs::create_dir(&sibling).unwrap();

    let output = extract(&repo, "heads/main", &out, &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("temporary sibling"), "{stderr}");
    assert!(stderr.contains(sibling.to_str().unwrap()), "{stderr}");
    assert!(!out.exists(), "the target must not have been created");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&sibling);
}

/// A successful extract never writes into the repository: every byte under `.prikk/` is identical
/// before and after.
#[test]
fn extract_never_touches_the_repository() {
    let repo = support::unique_repo("extract-prikk-untouched");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let before = support::store_bytes(&repo);
    let out = support::unique_repo("extract-prikk-untouched-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&repo, "heads/main", &out, &[]);
    support::ok(&output, "extract");
    let after = support::store_bytes(&repo);

    assert_eq!(
        before, after,
        ".prikk/ must be byte-identical before and after a successful extract run"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
}

/// `--allow-full-replay` parses and is accepted on an ordinary point (the deep pre-walk-refusal
/// shape itself is pinned at the `prikk-store` level, where a 70-block signed fixture is cheap to
/// build directly -- building one through 70 real `commit`/`seal` subprocess round trips here would
/// cost minutes for no additional coverage).
#[test]
fn extract_accepts_the_allow_full_replay_flag() {
    let repo = support::unique_repo("extract-allow-full-replay-flag");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-allow-full-replay-flag-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&repo, "heads/main", &out, &["--allow-full-replay"]);
    support::ok(&output, "extract --allow-full-replay");
    assert_eq!(std::fs::read(out.join("README.md")).unwrap(), b"hello\n");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
}

/// `--plan-only` (0.54.0 step 3 Part 3): reports file count, total bytes and `provisional`, and
/// writes nothing -- no target, no temporary sibling either.
#[test]
fn extract_plan_only_writes_nothing() {
    let repo = support::unique_repo("extract-plan-only");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-plan-only-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&repo, "heads/main", &out, &["--plan-only"]);
    support::ok(&output, "extract --plan-only");
    let report = stdout(&output);
    assert!(
        report.contains("plan only: nothing was written"),
        "{report}"
    );
    assert!(report.contains("files: 1"), "{report}");
    assert!(report.contains("total content bytes: 6"), "{report}");
    assert!(!out.exists(), "--plan-only must not create the target");
    let sibling = out.with_file_name(format!(
        "{}.prikk-extract-tmp",
        out.file_name().unwrap().to_str().unwrap()
    ));
    assert!(
        !sibling.exists(),
        "--plan-only must not create a temporary sibling either"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `--max-files`/`--max-bytes` (0.54.0 step 3 Part 3): refuse the whole point, by name, before the
/// first byte is written -- the target and its temporary sibling are both absent afterward.
#[test]
fn extract_refuses_over_max_files_and_max_bytes() {
    let repo = support::unique_repo("extract-max-limits");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    std::fs::write(repo.join("b.txt"), b"world\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out_files = support::unique_repo("extract-max-files-out");
    std::fs::remove_dir_all(&out_files).unwrap();
    let output = extract(&repo, "heads/main", &out_files, &["--max-files", "1"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("over --max-files 1"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out_files.exists());

    let out_bytes = support::unique_repo("extract-max-bytes-out");
    std::fs::remove_dir_all(&out_bytes).unwrap();
    let output = extract(&repo, "heads/main", &out_bytes, &["--max-bytes", "5"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("over --max-bytes 5"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out_bytes.exists());

    // A limit that is not exceeded still extracts normally.
    let out_ok = support::unique_repo("extract-max-limits-ok-out");
    std::fs::remove_dir_all(&out_ok).unwrap();
    let output = extract(
        &repo,
        "heads/main",
        &out_ok,
        &["--max-files", "2", "--max-bytes", "1000"],
    );
    support::ok(&output, "extract within the limits");
    assert!(out_ok.join("a.txt").exists());

    let _ = std::fs::remove_dir_all(&repo);
}

/// `--format json` (0.54.0 step 3 Part 3): `extract-report-v1`, `written: true` for a real write.
#[test]
fn extract_format_json_reports_written_true_for_a_real_extract() {
    let repo = support::unique_repo("extract-json-written");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-json-written-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&repo, "heads/main", &out, &["--format", "json"]);
    support::ok(&output, "extract --format json");
    let report = stdout(&output);
    let json = support::json::parse(&report);
    assert_eq!(json.get("schema_version").as_str(), "extract-report-v1");
    assert!(json.get("written").as_bool());
    assert!(report.contains("\"file_count\": 1"), "{report}");
    assert!(out.join("a.txt").exists());

    let _ = std::fs::remove_dir_all(&repo);
}

/// `--ref` and `--claim` are mutually exclusive (0.54.0 step 3 Part 3).
#[test]
fn extract_refuses_both_ref_and_claim() {
    let repo = support::unique_repo("extract-ref-and-claim");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-ref-and-claim-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = support::prikk(&repo)
        .args([
            "extract",
            "--ref",
            "heads/main",
            "--claim",
            &"0".repeat(64),
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not both"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
