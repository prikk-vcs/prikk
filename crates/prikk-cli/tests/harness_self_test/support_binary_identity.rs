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

#[path = "../support/mod.rs"]
mod support;

#[test]
fn prikk_bin_path_names_the_same_binary_cargo_built() {
    let printed = support::prikk_bin_path().display().to_string();
    support::assert_same_path(&printed, std::path::Path::new(env!("CARGO_BIN_EXE_prikk")));
}
