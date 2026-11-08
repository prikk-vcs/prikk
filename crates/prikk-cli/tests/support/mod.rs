//! Thin re-export of `prikk-test-support` (0.55.0 G2 Part A).
//!
//! The actual harness moved to its own crate, `tools/test-support` (package `prikk-test-support`),
//! a dev-dependency of `prikk`: before this round, `mod support;` meant this file (then 906 lines
//! combined with `json.rs`) was textually included into, and recompiled as part of, every one of
//! `prikk`'s own ~141 separate integration-test binaries. As a real crate it compiles once.
//!
//! This file stays, as a two-line re-export, so every existing `mod support;` declaration across
//! those test files (whether written as plain `mod support;` or explicitly as
//! `#[path = "support/mod.rs"] mod support;`), and `merge_evidence_support/mod.rs`'s own
//! `#[path = "../support/mod.rs"] mod support;`, keeps resolving at the identical path -- no test
//! file's call sites moved.
#![allow(dead_code)]

pub use prikk_test_support::*;

/// Set `path`'s modification time a day into the past.
///
/// 0.57.0 step 0c (git's racy-entry rule): `commit` does not record a commit-index entry for a file
/// whose mtime is not older than the file system's clock when the commit took its lock, because a
/// rewrite in the same timestamp tick could reproduce the recorded stat. A test that needs the entry
/// -- to damage it, to count its reads -- must therefore write a file that is not "too recent" on
/// *any* clock, not only on this machine's fine-grained one (a coarse kernel tick, NTFS, FAT).
#[allow(clippy::expect_used)]
pub fn backdate_mtime(path: &std::path::Path) {
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open to backdate")
        .set_modified(std::time::SystemTime::now() - day)
        .expect("set the modification time");
}
