//! Handoff §6 control 1, the rigorous form: same input, two **separate process invocations** of
//! the compiled `extract-profile` binary, byte-identical stdout.
//!
//! An in-process double call of the library function would not actually catch the realistic way
//! this breaks: Rust's default `HashMap` hasher is randomized *per process*, not per call, so two
//! calls to a `HashMap`-using extractor inside the same test process would still agree with each
//! other even if the implementation were nondeterministic across real runs. Two child processes
//! each get their own randomized hasher state, so this is the form that would actually have
//! failed had `extract.rs` used `HashMap` instead of `BTreeMap` anywhere in the histogram-building
//! path -- confirmed by temporarily introducing one during development and watching this test fail
//! (see the implementation report).

#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn control1_two_separate_processes_produce_byte_identical_output() {
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_extract-profile"))
            .arg(fixture("log.txt"))
            .arg(fixture("ls-tree.txt"))
            .arg(fixture("context.toml"))
            .output()
            .expect("extract-profile must run");
        assert!(
            output.status.success(),
            "extract-profile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };

    let first = run();
    let second = run();
    assert_eq!(
        first, second,
        "two separate process invocations of the same input must produce byte-identical output"
    );
}

/// RFC 139 increment 2, handoff §2.2/§6 control 1: the **planner's** own determinism, proven the
/// same rigorous way -- two separate process invocations of the `plan-corpus` binary, byte-identical
/// stdout. No `prikk` binary and no repository are involved on either side, which is exactly what
/// §5a.2's "the determinism test compares the builder's action manifest, not sealed heads" buys: this
/// test could not exist in this form (in the ordinary suite, no `#[ignore]`) if planning required
/// executing anything.
#[test]
fn control1_planner_two_separate_processes_produce_byte_identical_manifests() {
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_plan-corpus"))
            .arg(fixture("tiny-profile.toml"))
            .arg("6")
            .output()
            .expect("plan-corpus must run");
        assert!(
            output.status.success(),
            "plan-corpus failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };

    let first = run();
    let second = run();
    assert!(
        !first.is_empty(),
        "plan-corpus must produce a non-empty manifest"
    );
    assert_eq!(
        first, second,
        "two separate process invocations of the same profile and target depth must produce a \
         byte-identical manifest"
    );
}
