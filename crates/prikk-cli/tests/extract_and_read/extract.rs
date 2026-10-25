//! `prikk extract` (0.54.0 step 3 Part 2), through the compiled binary.
//!
//! The trust rule, the pre-walk refusal, and the no-replace-rename control are pinned directly at
//! the `prikk-store` level (`crates/prikk-store/src/extract/tests.rs`,
//! `crates/prikk-store/src/patch_replay/extract/tests.rs`), where a fixture's own signing/
//! state-root crafting is reachable; this file covers what only the compiled binary and a real
//! filesystem can show: byte-identity against `cat --output`/`tree`, the write-safety refusals, and
//! that a successful run never touches `.prikk/`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
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

/// Asserts a `destination-inside-repository` refusal: exits 1, names the repository root, and
/// creates neither the target nor its temporary sibling (0.55.0 step 1 item 1, external review 035
/// §3.2).
fn assert_refuses_inside_repository(repo: &Path, target: &Path, output: &Output) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("inside this repository's own root"),
        "{stderr}"
    );
    assert!(!target.exists(), "the target must not have been created");
    let sibling = target.with_file_name(format!(
        "{}.prikk-extract-tmp",
        target.file_name().unwrap().to_str().unwrap()
    ));
    assert!(
        !sibling.exists(),
        "the temporary sibling must not have been created either"
    );
    let _ = repo;
}

/// An absolute path inside the worktree refuses, naming the repository root, with nothing created.
#[test]
fn extract_refuses_a_destination_inside_the_worktree_absolute() {
    let repo = support::unique_repo("extract-inside-worktree-absolute");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let target = repo.join("inside-the-worktree");
    let output = extract(&repo, "heads/main", &target, &[]);
    assert_refuses_inside_repository(&repo, &target, &output);

    let _ = std::fs::remove_dir_all(&repo);
}

/// The same refusal for a relative path, resolved against the process's own current directory
/// (`support::prikk` sets it to `repo`) -- the destination never resolves differently just because
/// it was spelled relatively.
#[test]
fn extract_refuses_a_destination_inside_the_worktree_relative() {
    let repo = support::unique_repo("extract-inside-worktree-relative");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let relative_target = PathBuf::from("inside-the-worktree-relative");
    let output = extract(&repo, "heads/main", &relative_target, &[]);
    let resolved_target = repo.join(&relative_target);
    assert_refuses_inside_repository(&repo, &resolved_target, &output);

    let _ = std::fs::remove_dir_all(&repo);
}

/// A path under `.prikk/` refuses -- `.prikk` is always nested under the repository root, so the
/// same root-identity check already covers it; this proves it does, not just by construction.
#[test]
fn extract_refuses_a_destination_under_the_prikk_directory() {
    let repo = support::unique_repo("extract-inside-dot-prikk");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let target = repo.join(".prikk").join("extracted-into-prikk-dir");
    let output = extract(&repo, "heads/main", &target, &[]);
    assert_refuses_inside_repository(&repo, &target, &output);

    let _ = std::fs::remove_dir_all(&repo);
}

/// A destination spelled with a literal `..` component that, once resolved, lands back inside the
/// repository still refuses -- comparing resolved paths by identity, never as text, is exactly what
/// defeats this.
#[test]
fn extract_refuses_a_destination_reached_through_dot_dot() {
    let repo = support::unique_repo("extract-inside-via-dotdot");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let repo_name = repo.file_name().unwrap().to_str().unwrap();
    // Relative to the process's own cwd (`repo`, per `support::prikk`): up to the parent, back
    // down through the repository's own directory name, landing inside it again.
    let relative_target = PathBuf::from("..").join(repo_name).join("via-dot-dot");
    let output = extract(&repo, "heads/main", &relative_target, &[]);
    let resolved_target = repo.join("via-dot-dot");
    assert_refuses_inside_repository(&repo, &resolved_target, &output);

    let _ = std::fs::remove_dir_all(&repo);
}

/// A symlinked parent directory that points inside the repository refuses too -- `canonicalize`
/// follows every symlink on the way to the nearest existing ancestor, so a destination reached only
/// through a symlink resolves to where it actually lands.
#[cfg(unix)]
#[test]
fn extract_refuses_a_destination_through_a_symlinked_parent() {
    let repo = support::unique_repo("extract-inside-via-symlink");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let link_container = support::unique_repo("extract-inside-via-symlink-link-container");
    let link = link_container.join("points-into-the-repo");
    std::os::unix::fs::symlink(&repo, &link).unwrap();
    let target = link.join("via-symlink");

    let output = extract(&repo, "heads/main", &target, &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("inside this repository's own root"),
        "{stderr}"
    );
    assert!(!target.exists(), "the target must not have been created");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&link_container);
}

/// Windows-only: a case-different spelling of the repository root still resolves to the same
/// repository, by the file system's own canonical on-disk casing (`GetFinalPathNameByHandleW`),
/// not by the casing the caller typed. The first real execution of this test is CI's Windows job --
/// no Windows host exists in this development environment.
#[cfg(windows)]
#[test]
fn extract_refuses_a_destination_under_a_case_different_spelling_of_the_repository_root() {
    let repo = support::unique_repo("extract-inside-case-different");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let repo_str = repo.to_str().unwrap();
    let flipped: String = repo_str
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect();
    let target = PathBuf::from(flipped).join("case-different-target");
    let output = extract(&repo, "heads/main", &target, &[]);
    let resolved_target = repo.join("case-different-target");
    assert_refuses_inside_repository(&repo, &resolved_target, &output);

    let _ = std::fs::remove_dir_all(&repo);
}

/// A sibling directory of the repository whose name starts with the repository's own name
/// (`repo-out` beside `repo`) is genuinely outside it and must still succeed -- proving the check
/// compares path components, never a bare string prefix.
#[test]
fn extract_succeeds_into_a_sibling_whose_name_starts_with_the_repositorys_own_name() {
    let repo = support::unique_repo("extract-sibling-prefix");
    support::init(&repo);
    std::fs::write(repo.join("README.md"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let mut sibling_name = repo.file_name().unwrap().to_os_string();
    sibling_name.push("-out");
    let out = repo.with_file_name(sibling_name);
    let _ = std::fs::remove_dir_all(&out);

    let output = extract(&repo, "heads/main", &out, &[]);
    support::ok(&output, "extract into a same-prefix sibling must succeed");
    assert_eq!(std::fs::read(out.join("README.md")).unwrap(), b"hello\n");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
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
    // 0.54.0 step 3 Part 3b (R3): one fixed key, "point", across every point-reading command's own
    // JSON -- the same shape `tree-listing-v1`/`path-content-v1` already use -- with the third kind
    // `extract` alone has (a claim) named by a second field, "point_kind", rather than by the key.
    assert_eq!(json.get("point").as_str(), "heads/main");
    assert_eq!(json.get("point_kind").as_str(), "ref");
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
            "--format",
            "json",
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
    // 0.54.0 step 3 Part 5d: a usage error (exit 2) prints no `refusal-v1` document, even under
    // `--format json`.
    assert_eq!(output.status.code(), Some(2), "a usage error exits 2");
    assert!(
        stdout(&output).is_empty(),
        "a usage error prints no stdout at all, refusal-v1 document included: {}",
        stdout(&output)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `refusal-v1` (0.54.0 step 3 Part 5d): this project's first machine-readable refusal document.
/// Asserts its exact shape -- `schema_version`, `command`, `refusal`, `reason`, `message` -- and
/// that stderr and the exit code are unchanged from before this round.
fn assert_refusal_json(
    output: &Output,
    expected_refusal: &str,
    expected_reason: Option<&str>,
    message_contains: &str,
) {
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1), "a refusal exits 1, not 2");
    let stderr_text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr_text.contains(message_contains),
        "stderr must still carry the ordinary message: {stderr_text}"
    );
    let report = stdout(output);
    let json = support::json::parse(&report);
    assert_eq!(
        json.get("schema_version").as_str(),
        "refusal-v1",
        "{report}"
    );
    assert_eq!(json.get("command").as_str(), "extract", "{report}");
    assert_eq!(json.get("refusal").as_str(), expected_refusal, "{report}");
    match expected_reason {
        Some(reason) => assert_eq!(json.get("reason").as_str(), reason, "{report}"),
        None => assert!(json.get("reason").is_null(), "{report}"),
    }
    assert!(
        json.get("message").as_str().contains(message_contains),
        "{report}"
    );
    // The JSON `message` is the exact stderr text (the handoff's own wording), modulo `main.rs`'s
    // own generic `"error: "` framing (`eprintln!("error: {}", err.message())`) applied uniformly
    // to every command's error, not something `extract`'s own refusal text ever included itself --
    // the JSON carries `CliError::message()`'s own text, the same text stderr's one `error: ` line
    // wraps.
    let stderr_message = stderr_text
        .trim_end()
        .strip_prefix("error: ")
        .unwrap_or(stderr_text.trim_end());
    assert_eq!(json.get("message").as_str(), stderr_message, "{report}");
}

/// `point-not-found` (0.54.0 step 3 Part 5d): an explicit `--ref` naming a branch that was never
/// created.
#[test]
fn extract_refuses_point_not_found_with_refusal_v1() {
    let repo = support::unique_repo("extract-refusal-point-not-found");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-refusal-point-not-found-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&repo, "heads/nope", &out, &["--format", "json"]);
    assert_refusal_json(&output, "point-not-found", None, "does not exist");

    let _ = std::fs::remove_dir_all(&repo);
}

/// `claim-not-found` (0.54.0 step 3 Part 5d, item 4 first bullet): an id that names no object at
/// all -- was `integrity error` before this round, now `Precondition`, classified `claim-not-found`.
#[test]
fn extract_refuses_claim_not_found_with_refusal_v1() {
    let repo = support::unique_repo("extract-refusal-claim-not-found");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-refusal-claim-not-found-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = support::prikk(&repo)
        .args([
            "extract",
            "--claim",
            &"ab".repeat(32),
            "--format",
            "json",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_refusal_json(&output, "claim-not-found", None, "does not exist");

    let _ = std::fs::remove_dir_all(&repo);
}

/// `not-a-claim` (0.54.0 step 3 Part 5d, item 4 second bullet): a real object of another type (a
/// `Block` id) named by `--claim` -- used to read `object type mismatch` straight from
/// `read_typed`'s own shared default, now caught at `prepare_extract_plan_for_claim`'s own call
/// site and reclassified.
#[test]
fn extract_refuses_a_block_id_as_claim_not_a_claim_with_refusal_v1() {
    let repo = support::unique_repo("extract-refusal-not-a-claim");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let tree_output = support::prikk(&repo)
        .args(["tree", "--ref", "heads/main", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&tree_output, "tree");
    let listing = support::json::parse(&stdout(&tree_output));
    let block_id = listing.get("target_block_id").as_str().to_string();

    let out = support::unique_repo("extract-refusal-not-a-claim-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = support::prikk(&repo)
        .args([
            "extract",
            "--claim",
            &block_id,
            "--format",
            "json",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_refusal_json(&output, "not-a-claim", None, "is not a recognition claim");

    let _ = std::fs::remove_dir_all(&repo);
}

/// `destination-exists`/`temporary-sibling-exists`/`over-max-files`/`over-max-bytes`, each carrying
/// `refusal-v1` (0.54.0 step 3 Part 5d) -- the write-safety and limit refusals already pinned above
/// for their stderr text, now also checked for their JSON document.
#[test]
fn extract_refusal_ids_for_the_write_safety_and_limit_refusals() {
    let repo = support::unique_repo("extract-refusal-ids-sweep");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    std::fs::write(repo.join("b.txt"), b"world\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    // destination-exists
    let out_existing = support::unique_repo("extract-refusal-ids-existing-out"); // already exists
    let output = extract(&repo, "heads/main", &out_existing, &["--format", "json"]);
    assert_refusal_json(
        &output,
        "destination-exists",
        None,
        "refusing to extract into an existing path",
    );

    // temporary-sibling-exists
    let out_sibling = support::unique_repo("extract-refusal-ids-sibling-out");
    std::fs::remove_dir_all(&out_sibling).unwrap();
    let sibling = out_sibling.with_file_name(format!(
        "{}.prikk-extract-tmp",
        out_sibling.file_name().unwrap().to_str().unwrap()
    ));
    let _ = std::fs::remove_dir_all(&sibling);
    std::fs::create_dir(&sibling).unwrap();
    let output = extract(&repo, "heads/main", &out_sibling, &["--format", "json"]);
    assert_refusal_json(
        &output,
        "temporary-sibling-exists",
        None,
        "temporary sibling",
    );
    let _ = std::fs::remove_dir_all(&sibling);

    // over-max-files
    let out_files = support::unique_repo("extract-refusal-ids-max-files-out");
    std::fs::remove_dir_all(&out_files).unwrap();
    let output = extract(
        &repo,
        "heads/main",
        &out_files,
        &["--max-files", "1", "--format", "json"],
    );
    assert_refusal_json(&output, "over-max-files", None, "over --max-files 1");

    // over-max-bytes
    let out_bytes = support::unique_repo("extract-refusal-ids-max-bytes-out");
    std::fs::remove_dir_all(&out_bytes).unwrap();
    let output = extract(
        &repo,
        "heads/main",
        &out_bytes,
        &["--max-bytes", "5", "--format", "json"],
    );
    assert_refusal_json(&output, "over-max-bytes", None, "over --max-bytes 5");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out_existing);
}

/// `not-a-repository` (0.54.0 step 3 Part 5e, review C1): `open_repository`'s own "no prikk
/// repository here" failure, previously exiting 1 under `--format json` with nothing on stdout at
/// all (the gap the review confirmed real) -- now classified explicitly at its own call site in
/// `run_extract_inner` (`crates/prikk-cli/src/extract.rs`), before any of extract's own logic runs.
#[test]
fn extract_refuses_not_a_repository_with_refusal_v1() {
    // `support::unique_repo` creates the directory itself but never calls `support::init` on it --
    // a plain directory with no `.prikk/` underneath, exactly `open_repository`'s own refusal shape.
    let not_a_repo = support::unique_repo("extract-not-a-repository");

    let out = support::unique_repo("extract-not-a-repository-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&not_a_repo, "heads/main", &out, &["--format", "json"]);
    assert_refusal_json(&output, "not-a-repository", None, "no prikk repository at");
    assert!(!out.exists(), "a refused extract must write nothing");

    let _ = std::fs::remove_dir_all(&not_a_repo);
    let _ = std::fs::remove_dir_all(&out);
}

/// The structural boundary catch itself (0.54.0 step 3 Part 5e, review C1): "a refusal raised
/// outside the eleven [now thirteen] sites still yields a document." A malformed
/// `.prikk/current-branch` pointer fails inside `current_branch::resolve_ref`
/// (`crates/prikk-cli/src/current_branch.rs`), a real code path that never calls this module's own
/// `refuse()` at all -- confirmed from source: `resolve_ref` maps its own error straight to a
/// `CliError` via `.to_string()` (`current_branch.rs:20`), with no `extract`-local classification in
/// between, so by the time it reaches `run_extract_inner` there is no richer `PrikkError` left to
/// classify by variant any more. Getting a `refusal-v1` document here at all proves `run_extract`'s
/// own boundary catch is what printed it, not one of the already-classified call sites -- and the
/// review's own explicit ruling is that this shape is `damaged-repository`, which this also checks.
#[test]
fn extract_boundary_catch_prints_refusal_v1_for_an_unreadable_current_branch_pointer() {
    let repo = support::unique_repo("extract-boundary-catch");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    // No trailing newline: malformed by name, the identical shape
    // `rfc151_current_branch.rs`'s own `a_malformed_pointer_is_refused_naming_the_file` already pins
    // for `worktree-status`.
    std::fs::write(repo.join(".prikk/current-branch"), b"heads/main").unwrap();

    let out = support::unique_repo("extract-boundary-catch-out");
    std::fs::remove_dir_all(&out).unwrap();
    // Deliberately no `--ref`: the default path reads the (now malformed) current-branch pointer,
    // the one way this test reaches `current_branch::resolve_ref` at all (an explicit `--ref` never
    // touches the pointer, `resolve_ref`'s own doc).
    let output = support::prikk(&repo)
        .args(["extract", "--format", "json", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_refusal_json(
        &output,
        "damaged-repository",
        None,
        "current-branch is malformed",
    );
    assert!(!out.exists(), "a refused extract must write nothing");

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
}

/// `largest_file_bytes`/`claim_parent_block_id` (0.54.0 step 3 Part 5d, items 1-2): always present.
/// A ref point has no claim parent (`null`); the largest single file's own length is reported
/// alongside the total, in both the JSON report and the plain-text one (`--plan-only`'s own
/// `largest file: N bytes` wording).
#[test]
fn extract_report_always_carries_largest_file_bytes_and_claim_parent_block_id() {
    let repo = support::unique_repo("extract-new-fields");
    support::init(&repo);
    std::fs::write(repo.join("small.txt"), b"hi\n").unwrap();
    std::fs::write(
        repo.join("large.txt"),
        b"a bigger file than the other one\n",
    )
    .unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let out = support::unique_repo("extract-new-fields-out");
    std::fs::remove_dir_all(&out).unwrap();
    let output = extract(&repo, "heads/main", &out, &["--format", "json"]);
    support::ok(&output, "extract --format json");
    let report = stdout(&output);
    let json = support::json::parse(&report);
    assert!(json.get("claim_parent_block_id").is_null(), "{report}");
    assert!(
        report.contains("\"largest_file_bytes\": 33"),
        "large.txt is 33 bytes: {report}"
    );

    let out_plan_only = support::unique_repo("extract-new-fields-plan-only-out");
    std::fs::remove_dir_all(&out_plan_only).unwrap();
    let output = extract(&repo, "heads/main", &out_plan_only, &["--plan-only"]);
    support::ok(&output, "extract --plan-only");
    let report = stdout(&output);
    assert!(
        report.contains("largest file: 33 bytes"),
        "--plan-only's own text form: {report}"
    );
    assert!(
        report.contains("claim parent block: <none>"),
        "a ref point has no claim parent: {report}"
    );

    let out_text = support::unique_repo("extract-new-fields-text-out");
    std::fs::remove_dir_all(&out_text).unwrap();
    let output = extract(&repo, "heads/main", &out_text, &[]);
    support::ok(&output, "extract");
    let report = stdout(&output);
    assert!(
        report.contains("largest file: 33 bytes"),
        "the ordinary text report: {report}"
    );
    assert!(
        report.contains("claim parent block: <none>"),
        "a ref point has no claim parent, in the ordinary text report too: {report}"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&out_plan_only);
    let _ = std::fs::remove_dir_all(&out_text);
}
