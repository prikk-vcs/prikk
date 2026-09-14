#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use super::{LEGACY_TAGS_WITHOUT_DATED_HEADINGS, changelog_headings, check, is_release_tag};
use crate::boundary::BoundaryError;

#[test]
fn is_release_tag_accepts_the_unprefixed_x_y_z_shape() {
    assert!(is_release_tag("0.23.0"));
    assert!(is_release_tag("12.0.100"));
}

#[test]
fn is_release_tag_rejects_anything_else() {
    assert!(!is_release_tag("v0.23.0"), "prefixed");
    assert!(!is_release_tag("0.23"), "two components");
    assert!(!is_release_tag("0.23.0.1"), "four components");
    assert!(!is_release_tag("0.23.0-rc1"), "non-numeric suffix");
    assert!(!is_release_tag(""), "empty");
    assert!(!is_release_tag("release"), "not a version at all");
}

const SAMPLE_CHANGELOG: &str = "\
# Changelog

## 0.2.0 — 2026-01-02

Second release.

## 0.1.0 — 2026-01-01

First release.
";

#[test]
fn changelog_headings_extracts_every_version_token_in_file_order() {
    assert_eq!(changelog_headings(SAMPLE_CHANGELOG), vec!["0.2.0", "0.1.0"]);
}

#[test]
fn changelog_headings_keeps_duplicates_rather_than_deduplicating() {
    let changelog = "## 0.1.0 — 2026-01-01\n\nfirst\n\n## 0.1.0 — 2026-01-02\n\nagain\n";
    assert_eq!(changelog_headings(changelog), vec!["0.1.0", "0.1.0"]);
}

/// A scratch git repository with one commit and whichever tags the caller names, so `check`'s own
/// `git tag --list` call has something real to read -- the same "scratch worktree" idea control 2
/// asks for, built as a reusable fixture rather than a one-off manual step.
fn scratch_repo(tags: &[&str]) -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "test"]);
    std::fs::write(root.join("CHANGELOG.md"), "# Changelog\n").unwrap();
    git(&["add", "CHANGELOG.md"]);
    // `-c commit.gpgsign=false`: same reason as the tag below -- a machine with
    // `commit.gpgsign = true` otherwise makes this fixture call gpg (RFC 136 increment 0 review).
    git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "genesis",
    ]);
    for tag in tags {
        // `-c tag.gpgsign=false` overrides a developer machine's own global
        // `tag.gpgsign = true` (which turns a plain `git tag <name>` into a signed tag that
        // requires a message and fails non-interactively with "fatal: no tag message?") -- this
        // fixture needs an ordinary lightweight tag regardless of the caller's global git config.
        git(&["-c", "tag.gpgsign=false", "tag", tag]);
    }
    temporary
}

#[test]
fn check_reports_no_tags_found_rather_than_passing_vacuously() {
    let temporary = scratch_repo(&[]);
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        format!("{errors:?}").contains("no release tags found"),
        "{errors:?}"
    );
}

#[test]
fn check_ignores_a_tag_that_is_not_a_release_version() {
    let temporary = scratch_repo(&["not-a-version"]);
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    // A non-version tag is filtered out entirely, leaving the tag set empty -- same as no tags at
    // all, not a false pass and not a spurious per-tag failure naming a tag this gate never
    // claims to cover.
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        format!("{errors:?}").contains("no release tags found"),
        "{errors:?}"
    );
}

#[test]
fn check_fails_on_the_real_defect_a_tag_with_no_matching_heading() {
    let temporary = scratch_repo(&["0.1.0", "0.2.0"]);
    // Only 0.2.0's heading is present -- 0.1.0's own was overwritten, the exact shape of the real
    // defect this gate exists to catch.
    std::fs::write(
        temporary.path().join("CHANGELOG.md"),
        "# Changelog\n\n## 0.2.0 — 2026-01-02\n\nSecond release.\n",
    )
    .unwrap();
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let detail = format!("{errors:?}");
    assert!(detail.contains("0.1.0"), "{detail}");
    assert!(detail.contains("0 `## 0.1.0"), "{detail}");
}

#[test]
fn check_fails_on_a_duplicated_heading_too() {
    let temporary = scratch_repo(&["0.1.0"]);
    std::fs::write(
        temporary.path().join("CHANGELOG.md"),
        "# Changelog\n\n## 0.1.0 — 2026-01-01\n\nfirst\n\n## 0.1.0 — 2026-01-02\n\nagain\n",
    )
    .unwrap();
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(format!("{errors:?}").contains("2 `## 0.1.0"), "{errors:?}");
}

/// The provisional exemption (see its own doc comment) exists only for these two named tags --
/// this pins that the list stays exactly what the report describes, so a future edit that widens
/// it silently is itself a test failure, not a quiet scope creep.
#[test]
fn legacy_exemption_list_is_exactly_the_two_named_pre_convention_tags() {
    assert_eq!(LEGACY_TAGS_WITHOUT_DATED_HEADINGS, ["0.0.1", "0.1.1"]);
}

/// An exempted tag with no heading at all is not reported, even though it would fail
/// [`check_fails_on_the_real_defect_a_tag_with_no_matching_heading`]'s own assertion if it were
/// not exempt -- proof the exemption actually suppresses the failure, not just that it happens not
/// to trigger one in the real repository.
#[test]
fn check_does_not_report_a_tag_on_the_legacy_exemption_list() {
    let exempt_tag = LEGACY_TAGS_WITHOUT_DATED_HEADINGS
        .first()
        .expect("exemption list is non-empty");
    let temporary = scratch_repo(&["0.2.0", exempt_tag]);
    std::fs::write(
        temporary.path().join("CHANGELOG.md"),
        "# Changelog\n\n## 0.2.0 — 2026-01-02\n\nOnly the non-exempt tag has a heading.\n",
    )
    .unwrap();
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
}

/// The self-guard (RFC 127 §3.3): an exemption must stay true, or the gate must refuse and name
/// the stale entry, rather than keep silently skipping a tag that has since gained a conforming
/// heading. Shown red (heading present) then green (heading removed) -- the amendment's own
/// control 1.
#[test]
fn check_fails_when_a_legacy_exemption_gains_a_conforming_heading() {
    let exempt_tag = LEGACY_TAGS_WITHOUT_DATED_HEADINGS
        .first()
        .expect("exemption list is non-empty");
    let temporary = scratch_repo(&["0.2.0", exempt_tag]);

    // Red: the exempt tag now has a conforming heading -- its exemption is stale.
    std::fs::write(
        temporary.path().join("CHANGELOG.md"),
        format!(
            "# Changelog\n\n## 0.2.0 — 2026-01-02\n\nSecond release.\n\n\
             ## {exempt_tag} — 2026-01-01\n\nNow dated.\n"
        ),
    )
    .unwrap();
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let detail = format!("{errors:?}");
    assert!(detail.contains(exempt_tag), "{detail}");
    assert!(detail.contains("stale"), "{detail}");

    // Green: heading removed again, self-guard clears.
    std::fs::write(
        temporary.path().join("CHANGELOG.md"),
        "# Changelog\n\n## 0.2.0 — 2026-01-02\n\nSecond release.\n",
    )
    .unwrap();
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn check_passes_when_every_tagged_version_has_exactly_one_heading() {
    let temporary = scratch_repo(&["0.1.0", "0.2.0"]);
    std::fs::write(temporary.path().join("CHANGELOG.md"), SAMPLE_CHANGELOG).unwrap();
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(temporary.path(), &mut errors).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
}

/// Control 1/2's own evidence, automated: the real repository's own tags, checked against its own
/// real, now-restored `CHANGELOG.md` — proof the gate passes against real content post-fix, not
/// only synthetic fixtures. If a future release ever reintroduces the RFC 127 defect, this is the
/// test that goes red.
#[test]
fn check_passes_against_the_real_repository() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    let mut errors: Vec<BoundaryError> = Vec::new();
    check(root, &mut errors).expect("git must be available and CHANGELOG.md must be readable");
    // A checkout with no tags (a shallow CI clone before this increment's own `fetch-tags: true`
    // fix) reads as the "no release tags found" case -- not a failure of this test, since it is
    // exactly the ambiguity RFC 127 §3's "fail loudly, not vacuously" rule is about. Skip rather
    // than assert cleanliness when this local clone genuinely has none, and say so.
    if errors.len() == 1 && format!("{errors:?}").contains("no release tags found") {
        eprintln!(
            "skipping: this checkout has no tags (run `git fetch --tags` to exercise the real \
             check)"
        );
        return;
    }
    assert!(errors.is_empty(), "{errors:?}");
}
