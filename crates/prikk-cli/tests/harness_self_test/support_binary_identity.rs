//! `prikk-test-support`'s `prikk_bin_path()` must name the exact binary this `cargo test`
//! invocation built, not a guess (0.55.0 G2 Part A2, condition A2.2,
//! `.git-exclude/reviewed/0.55.0-g2-A-review-v1.md`).
//!
//! `env!("CARGO_BIN_EXE_prikk")` resolves here -- this file is compiled as part of `prikk`'s own
//! integration tests -- to Cargo's own authoritative path for the binary it built for *this*
//! invocation. `support::prikk_bin_path()` derives the same path independently, from this test
//! executable's own location (`tools/test-support/src/lib.rs`'s `locate_prikk_binary`); this test
//! is the only thing that proves the two agree, in whichever profile this invocation runs under
//! (debug, release, or MSRV's own toolchain), on whichever operating system runs it. Compared by
//! identity (`support::assert_same_path`), never as text, the same discipline every other printed
//! path in this suite follows: a path spelled differently by symlink resolution or canonicalisation
//! could pass a naive string `==` for the wrong reason.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
#[test]
fn prikk_bin_path_names_the_same_binary_cargo_built() {
    let printed = support::prikk_bin_path().display().to_string();
    support::assert_same_path(&printed, std::path::Path::new(env!("CARGO_BIN_EXE_prikk")));
}

// 0.57.0 prep P3b: Cargo 1.100.0 moved test executables from `<profile>/deps/<test>` to
// `<profile>/build/prikk/<hash>/out/<test>`, and the old rule (this executable's parent's parent, plus `prikk`) then named the
// directory `<profile>/build/prikk`: every test failed with `NotFound` at its first `init`. The rule is now: use
// `CARGO_BIN_EXE_prikk` when Cargo sets it (1.100.0 does, 1.85.0 does not -- measured), else the old layout; and check that the
// result is a file. These tests feed the rule each situation, on any toolchain.

use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("p3b-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"x").unwrap();
}

#[test]
fn cargos_own_value_is_used_when_it_names_a_file() {
    let dir = scratch("env-file");
    let bin = dir.join(format!("prikk{}", std::env::consts::EXE_SUFFIX));
    touch(&bin);
    let got =
        support::resolve_prikk_binary(Some(bin.clone()), &dir.join("elsewhere").join("t")).unwrap();
    assert_eq!(got, bin);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_that_names_a_directory_fails_with_a_sentence_naming_what_was_expected() {
    let dir = scratch("env-dir");
    let said = support::resolve_prikk_binary(Some(dir.clone()), &dir.join("t")).unwrap_err();
    assert!(said.contains("expected the prikk executable at"), "{said}");
    assert!(said.contains("that path is a directory"), "{said}");
    assert!(said.contains("CARGO_BIN_EXE_prikk"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_where_nothing_exists_fails_the_same_way() {
    let dir = scratch("env-missing");
    let said = support::resolve_prikk_binary(Some(dir.join("nope")), &dir.join("t")).unwrap_err();
    assert!(said.contains("expected the prikk executable at"), "{said}");
    assert!(said.contains("nothing exists there"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_cargos_value_the_older_layout_is_read() {
    let dir = scratch("old-layout");
    let bin = dir.join(format!("prikk{}", std::env::consts::EXE_SUFFIX));
    touch(&bin);
    let test_exe = dir.join("deps").join("some_test-0123");
    touch(&test_exe);
    let got = support::resolve_prikk_binary(None, &test_exe).unwrap();
    assert_eq!(got, bin);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Cargo 1.100.0 shape, with no value from Cargo: the layout guess lands on the directory `build/prikk`, and the result is
/// a sentence, not `NotFound` at `init` (the failure CI showed).
#[test]
fn the_cargo_1_100_layout_without_cargos_value_is_a_sentence_not_a_not_found() {
    let dir = scratch("new-layout");
    touch(&dir.join(format!("prikk{}", std::env::consts::EXE_SUFFIX)));
    let test_exe = dir
        .join("build")
        .join("prikk")
        .join("8c2b")
        .join("out")
        .join("some_test-8c2b");
    touch(&test_exe);
    // the guess is `<test_exe>/../..` + `prikk`, i.e. `<dir>/build/prikk/8c2b/prikk`: nothing there, or a directory
    let said = support::resolve_prikk_binary(None, &test_exe).unwrap_err();
    assert!(said.contains("expected the prikk executable at"), "{said}");
    assert!(said.contains("CARGO_BIN_EXE_prikk is not set"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}
