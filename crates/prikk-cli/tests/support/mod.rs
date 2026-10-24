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
