//! Unit tests for `extract`'s own small, pure helpers. End-to-end coverage (byte-identity against
//! `cat`/`tree`, the write-safety controls, the trust rule, and the `.prikk/`-untouched property)
//! lives in `crates/prikk-cli/tests/extract.rs`, which drives the compiled binary.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use prikk_store::RepoPath;

use super::{join_repo_path, temp_sibling_path};

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
