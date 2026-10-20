//! Unit tests for `extract`'s own small, pure helpers. End-to-end coverage (byte-identity against
//! `cat`/`tree`, the write-safety controls, the trust rule, and the `.prikk/`-untouched property)
//! lives in `crates/prikk-cli/tests/extract.rs`, which drives the compiled binary.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use prikk_store::RepoPath;

use super::{join_repo_path, sync_tree_directories, temp_sibling_path};

#[test]
fn temp_sibling_path_appends_the_fixed_suffix() {
    let target = Path::new("/tmp/some/where/out");
    let sibling = temp_sibling_path(target).unwrap();
    assert_eq!(sibling, Path::new("/tmp/some/where/out.prikk-extract-tmp"));
}

#[test]
fn join_repo_path_splits_on_forward_slash_only() {
    let path = RepoPath::parse("src/deep/file.txt").unwrap();
    let joined = join_repo_path(Path::new("/out"), &path);
    assert_eq!(joined, Path::new("/out/src/deep/file.txt"));
}

/// 0.54.0 step 3 Part 2b R1: `sync_tree_directories` must reach every directory in a nested tree,
/// not only the root -- the real risk a shallow walk would miss (a crash losing a deep directory
/// entry while its own ancestors already look durable). No fault-injection primitive exists on
/// this path (unlike `durable_output.rs`'s own single-file writer, which this project's own
/// `#[cfg(test)]` hook can interrupt at a named point), so this pins the one thing provable without
/// one: a real, multi-level tree syncs without error, root included.
#[test]
fn sync_tree_directories_reaches_every_level() {
    let root = std::env::temp_dir().join(format!(
        "prikk-extract-sync-tree-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir_all(root.join("a/b/c")).unwrap();
    std::fs::write(root.join("a/b/c/leaf.txt"), b"leaf").unwrap();
    std::fs::write(root.join("a/sibling.txt"), b"sibling").unwrap();

    let result = sync_tree_directories(&root);
    assert!(result.is_ok(), "{result:?}");

    let _ = std::fs::remove_dir_all(&root);
}
