#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use super::{check, number_token};

fn write_proposed(root: &Path, name: &str) {
    let dir = root.join("rfcs/proposed");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), "# stub\n").unwrap();
}

fn make_handoff(root: &Path, name: &str) {
    std::fs::create_dir_all(root.join("rfcs/handoffs").join(name)).unwrap();
}

fn details(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    check(root, &mut errors);
    errors.into_iter().map(|error| error.detail).collect()
}

#[test]
fn real_tree_passes_unchanged() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    assert_eq!(details(root), Vec::<String>::new());
}

#[test]
fn a_proposed_rfc_with_a_handoff_directory_fails_naming_both() {
    let temporary = tempfile::tempdir().unwrap();
    write_proposed(temporary.path(), "133-performance-cost.md");
    make_handoff(temporary.path(), "133-performance-cost-and-its-evidence");
    assert_eq!(
        details(temporary.path()),
        [
            "rfcs/proposed/133-performance-cost.md is proposed but has a handoff directory \
          rfcs/handoffs/133-performance-cost-and-its-evidence"
        ]
    );
}

#[test]
fn a_proposed_dc_rfc_with_a_handoff_directory_fails_across_letter_case() {
    let temporary = tempfile::tempdir().unwrap();
    write_proposed(temporary.path(), "DC-43-RELEASE-SECURITY-CONTROLS.md");
    make_handoff(temporary.path(), "DC-43-release-security-controls");
    assert_eq!(
        details(temporary.path()),
        [
            "rfcs/proposed/DC-43-RELEASE-SECURITY-CONTROLS.md is proposed but has a handoff \
          directory rfcs/handoffs/DC-43-release-security-controls"
        ]
    );
}

#[test]
fn a_prefix_only_collision_does_not_fail() {
    let temporary = tempfile::tempdir().unwrap();
    write_proposed(temporary.path(), "133-performance-cost.md");
    write_proposed(temporary.path(), "13-short.md");
    write_proposed(temporary.path(), "DC-43-release.md");
    make_handoff(temporary.path(), "13-other");
    make_handoff(temporary.path(), "1330-longer");
    make_handoff(temporary.path(), "DC-10-rollback");
    make_handoff(temporary.path(), "DC-430-longer");
    make_handoff(temporary.path(), "143-content-at-a-point");
    let found = details(temporary.path());
    assert_eq!(
        found,
        [
            "rfcs/proposed/13-short.md is proposed but has a handoff directory rfcs/handoffs/13-other"
        ],
        "only the exact 13 token may collide; 133 must not match 13, 1330 or 143, and DC-43 must \
         not match DC-10 or DC-430"
    );
}

#[test]
fn names_without_a_number_identify_no_rfc() {
    let temporary = tempfile::tempdir().unwrap();
    write_proposed(temporary.path(), ".gitkeep");
    make_handoff(temporary.path(), "consolidation");
    assert_eq!(details(temporary.path()), Vec::<String>::new());
    assert_eq!(number_token(".gitkeep"), None);
    assert_eq!(number_token("consolidation"), None);
    assert_eq!(number_token("DC-"), None);
    assert_eq!(number_token("133x-slug.md"), None);
}

#[test]
fn the_token_is_the_whole_number() {
    assert_eq!(number_token("133-slug.md").as_deref(), Some("133"));
    assert_eq!(number_token("133.md").as_deref(), Some("133"));
    assert_eq!(number_token("DC-43-SLUG.md").as_deref(), Some("DC-43"));
    assert_eq!(number_token("dc-43-slug").as_deref(), Some("DC-43"));
    assert_eq!(
        number_token("151-the-current-branch").as_deref(),
        Some("151")
    );
}

#[test]
fn a_missing_handoffs_directory_is_reported() {
    let temporary = tempfile::tempdir().unwrap();
    write_proposed(temporary.path(), "133-performance-cost.md");
    assert_eq!(
        details(temporary.path()),
        ["rfcs/handoffs: directory unreadable"]
    );
}
