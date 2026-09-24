//! RFC 136 increment 2c, item 0: **the corpus instrument's `prikk` is a release build**, decided in source.
//!
//! Every corpus-driven timing before 2c (RFC 139's build-cost curve, the two-measurements ladder, RFC 136
//! increment 3, the warm-cache round) ran a debug binary, because the helper that finds the binary ran
//! `cargo build` without `--release`. Debug is ~30x slower and has different shapes. So the default is release,
//! not a knob, and the control reads Cargo's own `compiler-artifact` record for the binary it hands back.
//!
//! **Perturb:** drop `--release` from `build_args`: the first control goes red at once, and so does the ignored
//! one that builds the binary.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use support::{BuildProfile, artifact_opt_level, build_args, require_optimized};

#[test]
fn the_default_build_is_release_and_the_debug_build_is_a_separate_named_request() {
    let release = build_args(BuildProfile::Release, "Cargo.toml");
    assert!(release.iter().any(|arg| arg == "--release"), "{release:?}");
    let debug = build_args(BuildProfile::DebugForBridgingColumns, "Cargo.toml");
    assert!(!debug.iter().any(|arg| arg == "--release"), "{debug:?}");
    // `--locked` and the JSON output the lookup reads are not optional either.
    for args in [&release, &debug] {
        assert!(args.iter().any(|arg| arg == "--locked"));
        assert!(args.iter().any(|arg| arg == "--message-format=json"));
    }
}

#[test]
fn an_unoptimized_artifact_record_is_refused_and_an_optimized_one_accepted() {
    let record = |opt_level: &str| serde_json::json!({"reason": "compiler-artifact", "profile": {"opt_level": opt_level}});
    assert!(require_optimized(artifact_opt_level(&record("3")).as_deref()).is_ok());
    assert!(require_optimized(artifact_opt_level(&record("s")).as_deref()).is_ok());
    let refused = require_optimized(artifact_opt_level(&record("0")).as_deref()).unwrap_err();
    assert!(refused.contains("debug-build"), "{refused}");
    assert!(
        require_optimized(None).is_err(),
        "no opt_level recorded is not proof of anything"
    );
}

/// The real thing: build the default binary and read the profile Cargo recorded for it. Ignored because it builds
/// a release `prikk`.
#[test]
#[ignore = "builds a release prikk"]
fn the_default_binary_is_built_optimized() {
    let built = support::locate_prikk_binary(BuildProfile::Release).expect("building");
    require_optimized(built.opt_level.as_deref()).expect("the default binary is optimized");
    assert!(
        built
            .path
            .components()
            .any(|part| part.as_os_str() == "release"),
        "{}",
        built.path.display()
    );
    let debug =
        support::locate_prikk_binary(BuildProfile::DebugForBridgingColumns).expect("building");
    assert_eq!(debug.opt_level.as_deref(), Some("0"));
}
