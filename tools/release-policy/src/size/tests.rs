//! RFC 130 §8 controls.
//!
//! **Every one of these moves something.** A size gate that is only ever run at its production
//! threshold against its production allowlist has never been seen to fail, and "it passes" would
//! then be a statement about the repository rather than about the gate.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::{DECLARED_LARGE_FILES, DeclaredLargeFile, LINE_LIMIT, SizeReport, run, run_with};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn details(report: &SizeReport) -> String {
    report
        .errors
        .iter()
        .map(|error| format!("[{}] {}", error.category, error.detail))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_real_repository_passes() {
    let report = run(&repo_root()).expect("size-check");
    assert!(report.valid, "{}", details(&report));
    assert_eq!(report.line_limit, LINE_LIMIT);
    assert_eq!(
        report.over_limit.len(),
        DECLARED_LARGE_FILES.len(),
        "every file over the line is declared and every declaration is over the line"
    );
    assert!(report.over_limit.iter().all(|file| file.declared));
}

/// Control 1: **the threshold does something.** At 1,000, `foundation/layout.rs` (1,054 lines,
/// comfortably under the real limit) crosses, is undeclared, and is named.
#[test]
fn lowering_the_threshold_names_the_file_that_crosses() {
    let report = run_with(&repo_root(), 1000, DECLARED_LARGE_FILES).expect("size-check");
    assert!(
        !report.valid,
        "a 1,000-line limit must fail this repository"
    );
    let detail = details(&report);
    assert!(
        detail.contains("crates/prikk-store/src/foundation/layout.rs"),
        "{detail}"
    );
    assert!(detail.contains("1000-line limit"), "{detail}");
    assert!(
        report
            .over_limit
            .iter()
            .any(|file| file.path.ends_with("foundation/layout.rs") && !file.declared),
        "and it appears in over_limit as undeclared"
    );
}

/// Control 2a: **an undeclared large file is named.** Removing `verify.rs`'s entry is the same
/// event as a new 1,700-line file appearing, and must read the same way.
#[test]
fn removing_a_declaration_names_its_file() {
    let thinned: Vec<DeclaredLargeFile> = DECLARED_LARGE_FILES
        .iter()
        .filter(|entry| !entry.path.ends_with("verify.rs"))
        .map(|entry| DeclaredLargeFile {
            path: entry.path,
            reason: entry.reason,
            what_would_split_it: entry.what_would_split_it,
        })
        .collect();
    let report = run_with(&repo_root(), LINE_LIMIT, &thinned).expect("size-check");
    assert!(!report.valid);
    let detail = details(&report);
    assert!(
        detail.contains("crates/prikk-store/src/verify.rs"),
        "{detail}"
    );
    assert!(
        detail.contains("1677"),
        "the line count is in the message: {detail}"
    );
    assert!(
        detail.contains("DECLARED_LARGE_FILES"),
        "and the message says where to record the decision: {detail}"
    );
}

/// Control 2b: **a stale declaration is named.** An entry for a file that is not over the line —
/// split, shrunk, moved, deleted, or never large — is a permission nobody asked for.
#[test]
fn a_declaration_for_a_file_that_is_not_over_the_line_is_stale() {
    let mut entries: Vec<DeclaredLargeFile> = DECLARED_LARGE_FILES
        .iter()
        .map(|entry| DeclaredLargeFile {
            path: entry.path,
            reason: entry.reason,
            what_would_split_it: entry.what_would_split_it,
        })
        .collect();
    entries.push(DeclaredLargeFile {
        path: "crates/prikk-store/src/nothing_like_this_exists.rs",
        reason: "A reason long enough to pass the well-formedness check, and entirely fictional.",
        what_would_split_it: "Nothing: the file does not exist, which is the point of this control.",
    });
    let report = run_with(&repo_root(), LINE_LIMIT, &entries).expect("size-check");
    assert!(!report.valid);
    let detail = details(&report);
    assert!(detail.contains("nothing_like_this_exists.rs"), "{detail}");
    assert!(
        detail.contains("may not outlive its cause"),
        "and says why a stale entry is a failure: {detail}"
    );
}

/// Control 2c: an entry that records nothing fails as surely as a missing one.
#[test]
fn a_placeholder_reason_is_not_a_recorded_decision() {
    let entries = [DeclaredLargeFile {
        path: "crates/prikk-store/src/verify.rs",
        reason: "TODO",
        what_would_split_it: "",
    }];
    let report = run_with(&repo_root(), LINE_LIMIT, &entries).expect("size-check");
    assert!(!report.valid);
    let detail = details(&report);
    assert!(
        detail.contains("reason is empty or a placeholder"),
        "{detail}"
    );
    assert!(
        detail.contains("what_would_split_it is empty or a placeholder"),
        "{detail}"
    );
}

/// Control 3: **a `#[cfg(test)]` module inside a production file is not counted.**
///
/// Asserted against the real file rather than a fixture, because the fixture that matters is the
/// one that exists: `node_authoring.rs` is 1,416 physical lines with a 49-line `mode_change_tests`
/// module at the end, and the gate must see 1,367. Counting the test module would not change this
/// file's verdict — it is declared either way — but it would change `layout.rs`'s the moment
/// someone put tests beside it, which is the shape of the mistake.
#[test]
fn an_inline_test_module_is_not_production() {
    let root = repo_root();
    let path = root.join("crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring.rs");
    let physical = std::fs::read_to_string(&path)
        .expect("read")
        .lines()
        .count();
    assert_eq!(
        physical, 1416,
        "the fixture moved; re-read it before trusting this"
    );

    let report = run(&root).expect("size-check");
    let counted = report
        .over_limit
        .iter()
        .find(|file| file.path.ends_with("node_authoring.rs"))
        .expect("node_authoring.rs is over the line");
    assert_eq!(counted.production_lines, 1367);
    assert_eq!(
        physical - counted.production_lines,
        49,
        "exactly the inline test module's own lines are excluded"
    );
}

/// Control 3b: the whole `#[cfg(test)]` subtree is invisible, not merely uncounted.
///
/// `test_gates/signature_contract_tests/vectors.rs` is 1,085 lines and would be the fourth-largest
/// "production" file under any directory-shaped scan. It is not in the report at all.
#[test]
fn a_cfg_test_subtree_is_never_walked() {
    let report = run(&repo_root()).expect("size-check");
    assert!(
        !report
            .over_limit
            .iter()
            .any(|file| file.path.contains("test_gates")),
        "test_gates must not appear"
    );
    let store = report
        .crates
        .iter()
        .find(|entry| entry.name == "prikk-store")
        .expect("prikk-store");
    // 130 graph nodes plus `lib.rs`, which is a file the gate weighs and not a node the graph has.
    assert_eq!(store.production_files, 131);
}

/// Control 4: the report serialises, and its verdict is the one the exit code is taken from.
#[test]
fn the_json_is_valid_and_carries_the_verdict() {
    let report = run(&repo_root()).expect("size-check");
    let rendered = serde_json::to_string_pretty(&report).expect("serialise");
    let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
    assert_eq!(parsed["schema_version"], "release-policy-size-v1");
    assert_eq!(parsed["valid"], report.valid);
    assert_eq!(parsed["line_limit"], LINE_LIMIT);
    assert!(parsed["crates"].as_array().expect("crates").len() >= 11);
    assert!(parsed["errors"].as_array().expect("errors").is_empty());
}

/// Every workspace member is scanned. A member added to `Cargo.toml` and not to `MEMBERS` would be
/// silently unmeasured, which is the failure this gate is least able to notice about itself.
#[test]
fn every_workspace_member_is_covered() {
    let root = repo_root();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest");
    let members: Vec<String> = manifest
        .lines()
        .skip_while(|line| !line.starts_with("members = ["))
        .skip(1)
        .take_while(|line| !line.starts_with(']'))
        .filter_map(|line| line.trim().trim_end_matches(',').strip_prefix('"'))
        .filter_map(|line| line.strip_suffix('"'))
        .map(str::to_owned)
        .collect();
    assert!(members.len() >= 11, "parsed {members:?}");
    for member in &members {
        assert!(
            super::MEMBERS.contains(&member.as_str()),
            "{member} is a workspace member but is not scanned by size-check"
        );
    }
    assert_eq!(members.len(), super::MEMBERS.len());
}

/// The per-crate delta is not always zero: measured across a release it reports real movement.
///
/// Pinned against `0.40.0`, which is behind the current tag, so this asserts the mechanism rather
/// than today's happens-to-be-zero. A repository with no tags reports `None` and fails nothing.
#[test]
fn the_line_delta_measures_something() {
    let root = repo_root();
    let cli = root.join("crates/prikk-cli/src/key_status.rs");
    let delta = super::line_delta(
        &root,
        "0.40.0",
        &["crates/prikk-cli/src/key_status.rs".to_owned()],
    );
    match delta {
        Some(value) => {
            let lines = std::fs::read_to_string(&cli).expect("read").lines().count();
            assert_eq!(
                value,
                i64::try_from(lines).expect("line count fits"),
                "key_status.rs did not exist at 0.40.0, so its whole length is the delta"
            );
        }
        None => panic!("0.40.0 must be readable in this repository"),
    }
}
