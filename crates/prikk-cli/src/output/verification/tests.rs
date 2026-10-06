#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{escape_json_string, print_verify_report_json, remnant_warning};

/// Windows hostile-test fix handoff Sec 2: the hostile-input proof used to live only in an
/// integration test that planted a directory whose name carried these exact bytes -- a double
/// quote, a backslash, and every C0 control are forbidden in a Win32 filename, so the directory
/// could never be created on that platform and the control never reached the escaper at all.
/// `escape_json_string` is a pure `&str -> String` function; proving it here needs no filesystem
/// and accepts input no filesystem would ever allow.
#[test]
fn hostile_input_is_escaped_correctly() {
    let hostile = "quote\"back\\slash\nnewline\ttab\u{1}control";
    let escaped = escape_json_string(hostile);
    assert_eq!(
        escaped,
        r#""quote\"back\\slash\nnewline\ttab\u0001control""#
    );
}

/// RFC 121 §5: a `RepositoryVerification` missing an outcome for a declared stage used to panic
/// here. `verify_repository_with_options` itself always covers every `VerificationStage::ALL`
/// member, so this hand-breaks a real, fully-populated report (rather than hand-constructing one
/// field-by-field, which would drift the moment the struct gains a field this test does not know
/// about) by removing one entry after the fact, then confirms the emitter now refuses instead of
/// aborting the process.
#[test]
fn missing_stage_outcome_is_refused_not_panicked() {
    let root = std::env::temp_dir().join(format!(
        "prikk-cli-rfc121-json-panic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let layout = prikk_store::RepositoryLayout::init(root.clone()).expect("init");
    let mut report =
        prikk_store::verify_repository_with_options(&layout, prikk_store::VerifyOptions::default())
            .expect("a freshly initialized repository verifies");
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        !report.stage_outcomes.is_empty(),
        "a real report must cover at least one stage"
    );
    report.stage_outcomes.pop();

    let err = print_verify_report_json(&report)
        .expect_err("a report missing a declared stage outcome must be refused, not panic");
    assert!(err.contains("missing an outcome"), "{err}");
}

/// Audit P3: the remnant warning uses the canonical form, with no type prefix before the owner.
#[test]
fn a_remnant_warning_names_the_owner_and_role_without_a_type_prefix() {
    assert_eq!(
        remnant_warning(&"abc123", "parent block", &"def456"),
        "warning: object abc123 references missing parent block def456 -- re-run the import if \
         you still have the bundle; otherwise it is harmless"
    );
}
