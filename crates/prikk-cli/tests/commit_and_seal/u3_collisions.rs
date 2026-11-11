//! 0.57.0 U3 (Unicode repository paths): collisions are judged by the **canonical caseless key** (`NFD(fold(NFD(path)))`, full
//! Unicode case folding), at every place that protects the disk. The over-refusal is deliberate: `straße` and `strasse`
//! collide.
//!
//! Inventory of the sites, from the code:
//!
//! | site | how it asks | tested |
//! |---|---|---|
//! | `seal` (state root, also `verify`'s block-state check) | `validate_no_path_collisions` in `compute_state_root` | here, through the binary |
//! | `extract` (both plans) | `validate_no_path_collisions` | `prikk-store` `extract/tests.rs` |
//! | `checkout --patch-materialize*`, `--snapshot-materialize` | `refuse_manifest_conflicts` -> `refuse_colliding_manifest_paths` | `prikk-store` `worktree/tests.rs` |
//! | `branch switch` | `refuse_colliding_manifest_paths` | by the shared function (see the report) |
//! | the `.prikk` first-component check | `validate_repo_path` | here (Linux only: elsewhere the name is the metadata directory) |
//! | branch and tag ref names, maintainer key ids | `ascii_fold` | **not changed** (U0 review, Q2) |

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::support;
use std::path::Path;
use std::process::Output;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn names(repo: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(repo)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != ".prikk")
        .collect();
    found.sort();
    found
}

/// Commit `left` and `right` (which `commit` accepts: it has no collision rule), then `seal`. Returns `None` when this file
/// system folds the two names into one entry (so the case cannot be set up here).
fn commit_then_seal(tag: &str, left: &str, right: &str) -> Option<(std::path::PathBuf, Output)> {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    // A first sealed generation, so the colliding one is an ordinary block and not the root block (a root block also encodes a
    // snapshot manifest, which asks the collision rule on its own: two different sites, each with its own control).
    support::generation(&repo, "heads/main", "base.txt", b"base\n", "base");
    std::fs::write(repo.join(left), b"left\n").unwrap();
    let wrote_right = std::fs::write(repo.join(right), b"right\n").is_ok();
    if !wrote_right || names(&repo).len() != 3 {
        eprintln!("skipped {tag}: this file system treats {left:?} and {right:?} as one name");
        let _ = std::fs::remove_dir_all(&repo);
        return None;
    }
    support::ok(
        &support::commit(&repo, "heads/main", "both"),
        "commit accepts both names",
    );
    let sealed = support::seal(&repo, "heads/main");
    Some((repo, sealed))
}

#[test]
fn seal_refuses_two_names_that_are_one_name_under_unicode_case_folding() {
    for (left, right) in [
        ("Stra\u{df}e.txt", "STRASSE.txt"),
        ("\u{c9}cole.md", "\u{e9}cole.md"),
        ("\u{1e9e}.md", "ss.md"),
    ] {
        let Some((repo, sealed)) = commit_then_seal("u3-seal-collision", left, right) else {
            continue;
        };
        let said = text(&sealed);
        assert_eq!(
            sealed.status.code(),
            Some(1),
            "{left:?} / {right:?}: {said}"
        );
        assert!(
            said.contains("case-insensitive path collision involving: "),
            "{said}"
        );
        assert!(
            said.contains(&prikk_error_escape(left)) && said.contains(&prikk_error_escape(right)),
            "both paths are named, the non-ASCII ones escaped: {said}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn seal_accepts_names_that_are_different_names() {
    // The control: the same shape with two names that do not collide seals, so the refusals above are the rule and not seal.
    let (repo, sealed) = commit_then_seal("u3-seal-distinct", "e.md", "\u{e9}.md")
        .expect("these two names are distinct on every file system");
    support::ok(&sealed, "seal of two distinct names");
    let _ = std::fs::remove_dir_all(&repo);
}

/// `.pri` + U+212A (the Kelvin sign, which case-folds to `k`) + `k` is the metadata directory on a case-insensitive,
/// normalizing file system. Linux only: on APFS creating it would open `.prikk` itself.
///
/// `commit` meets the NFC rule first (the Kelvin sign is a singleton: its NFC spelling is the letter `K`), and names the
/// spelling that then meets the metadata-directory rule. The Kelvin spelling reaches the `.prikk` check itself only through
/// decode (`prikk-object`'s `path/tests.rs`: `.pri\u{212a}k/x` is refused as the metadata directory, first component only).
#[cfg(target_os = "linux")]
#[test]
fn a_first_component_that_folds_to_the_metadata_directory_is_refused() {
    let repo = support::unique_repo("u3-prikk-kelvin");
    support::init(&repo);
    std::fs::create_dir_all(repo.join(".pri\u{212a}k")).unwrap();
    std::fs::write(repo.join(".pri\u{212a}k").join("x.md"), b"x\n").unwrap();
    let output = support::commit(&repo, "heads/main", "kelvin");
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains(".pri\\u{212a}k") && said.contains(".priKk/x.md"),
        "the NFC sentence names the Kelvin spelling (escaped) and its NFC spelling: {said}"
    );
    // Follow the sentence: the NFC spelling is the ASCII letter, and that is the metadata directory.
    std::fs::rename(repo.join(".pri\u{212a}k"), repo.join(".priKk")).unwrap();
    let output = support::commit(&repo, "heads/main", "kelvin renamed");
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(said.contains(".prikk metadata directory"), "{said}");
    let _ = std::fs::remove_dir_all(&repo);
}

fn prikk_error_escape(value: &str) -> String {
    prikk_error::escape_non_ascii(value)
}
