#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use super::{check, check_exemption_list_size, check_member};

fn write_manifest(root: &Path, relative: &str, body: &str) {
    let full = root.join(relative);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
}

const MEMBER_MANIFESTS: [(&str, &str); 9] = [
    ("prikk-error", "crates/prikk-error/Cargo.toml"),
    ("prikk-ffi", "crates/prikk-ffi/Cargo.toml"),
    ("prikk-hash", "crates/prikk-hash/Cargo.toml"),
    ("prikk-crypto", "crates/prikk-crypto/Cargo.toml"),
    ("prikk-object", "crates/prikk-object/Cargo.toml"),
    ("prikk-replay", "crates/prikk-replay/Cargo.toml"),
    ("prikk-store", "crates/prikk-store/Cargo.toml"),
    ("prikk", "crates/prikk-cli/Cargo.toml"),
    ("prikk-release-policy", "tools/release-policy/Cargo.toml"),
];

const ROOT_MANIFEST_WITH_LINT: &str = "\
[workspace]
members = []

[workspace.lints.clippy]
undocumented_unsafe_blocks = \"forbid\"
";

fn write_baseline(root: &Path) {
    write_manifest(root, "Cargo.toml", ROOT_MANIFEST_WITH_LINT);
    for (_, manifest_path) in MEMBER_MANIFESTS {
        write_manifest(root, manifest_path, "[lints]\nworkspace = true\n");
    }
}

#[test]
fn real_tree_passes_unchanged() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    let mut errors = Vec::new();
    check(root, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn baseline_tree_passes() {
    let temporary = tempfile::tempdir().unwrap();
    write_baseline(temporary.path());
    let mut errors = Vec::new();
    check(temporary.path(), &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn non_exempt_member_dropping_inheritance_fails() {
    let temporary = tempfile::tempdir().unwrap();
    write_baseline(temporary.path());
    write_manifest(
        temporary.path(),
        "crates/prikk-object/Cargo.toml",
        "[package]\nname = \"prikk-object\"\n",
    );
    let mut errors = Vec::new();
    check(temporary.path(), &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].detail,
        "prikk-object: does not inherit workspace lints and is not in the exemption list"
    );
}

#[test]
fn root_manifest_missing_the_self_guarding_lint_fails() {
    let temporary = tempfile::tempdir().unwrap();
    write_baseline(temporary.path());
    write_manifest(
        temporary.path(),
        "Cargo.toml",
        "[workspace]\nmembers = []\n",
    );
    let mut errors = Vec::new();
    check(temporary.path(), &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].detail,
        "root workspace.lints.clippy.undocumented_unsafe_blocks != \"forbid\""
    );
}

#[test]
fn root_manifest_with_the_lint_set_to_the_wrong_level_fails() {
    let temporary = tempfile::tempdir().unwrap();
    write_baseline(temporary.path());
    write_manifest(
        temporary.path(),
        "Cargo.toml",
        "[workspace]\nmembers = []\n\n[workspace.lints.clippy]\nundocumented_unsafe_blocks = \"warn\"\n",
    );
    let mut errors = Vec::new();
    check(temporary.path(), &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn unreadable_member_manifest_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    write_baseline(temporary.path());
    std::fs::remove_file(temporary.path().join("crates/prikk-hash/Cargo.toml")).unwrap();
    let mut errors = Vec::new();
    check(temporary.path(), &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].detail, "prikk-hash: manifest unreadable");
}

#[test]
fn unparseable_member_manifest_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    write_baseline(temporary.path());
    write_manifest(temporary.path(), "crates/prikk-hash/Cargo.toml", "[lints\n");
    let mut errors = Vec::new();
    check(temporary.path(), &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].detail, "prikk-hash: manifest unparseable");
}

#[test]
fn exemption_list_with_two_entries_fails() {
    let mut errors = Vec::new();
    check_exemption_list_size(&["prikk-ffi", "prikk-ffi-2"], &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].detail,
        "exemption list has 2 entries; at most one is allowed"
    );
}

#[test]
fn exemption_list_with_one_entry_passes() {
    let mut errors = Vec::new();
    check_exemption_list_size(&["prikk-ffi"], &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn exemption_list_empty_passes() {
    let mut errors = Vec::new();
    check_exemption_list_size(&[], &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
}

/// The negative control DC-90's ruling asked for by name: the exempt crate opting out of workspace
/// lint inheritance, without re-declaring the self-guarding lint locally, must fail. This is
/// dependency-injected -- `check_member` takes the exemption list as a parameter -- rather than
/// requiring the real, empty `UNSAFE_EXEMPT_CRATES` to be temporarily populated for the test.
#[test]
fn exempt_crate_opting_out_without_redeclaring_the_lint_fails() {
    let mut errors = Vec::new();
    check_member(
        "prikk-ffi",
        "[package]\nname = \"prikk-ffi\"\n",
        &["prikk-ffi"],
        &mut errors,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].detail,
        "prikk-ffi: exempt from workspace lint inheritance but does not locally re-declare \
         lints.clippy.undocumented_unsafe_blocks = \"forbid\""
    );
}

/// Review-found (DC-90 implementation-review-v1 §2): re-declaring the lint at `"deny"` is not
/// enough. `deny` can be locally overridden by an inner `#[allow(...)]`, which would let the exempt
/// crate satisfy this check and then silently remove its own guard in source -- the escape route the
/// review demonstrated by building it. Only `"forbid"` closes it, since `#[allow]` against a
/// `forbid`-level lint is a hard compile error. Pinned here so a future reader who sees "deny" and
/// "forbid" as interchangeable-looking strings does not pick the familiar one.
#[test]
fn exempt_crate_redeclaring_the_lint_at_deny_still_fails() {
    let mut errors = Vec::new();
    check_member(
        "prikk-ffi",
        "[package]\nname = \"prikk-ffi\"\n\n[lints.clippy]\nundocumented_unsafe_blocks = \"deny\"\n",
        &["prikk-ffi"],
        &mut errors,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].detail,
        "prikk-ffi: exempt from workspace lint inheritance but does not locally re-declare \
         lints.clippy.undocumented_unsafe_blocks = \"forbid\""
    );
}

/// The companion positive: an exempt crate that opts out but re-declares the lint locally passes.
#[test]
fn exempt_crate_opting_out_with_the_lint_redeclared_passes() {
    let mut errors = Vec::new();
    check_member(
        "prikk-ffi",
        "[package]\nname = \"prikk-ffi\"\n\n[lints.clippy]\nundocumented_unsafe_blocks = \"forbid\"\n",
        &["prikk-ffi"],
        &mut errors,
    );
    assert!(errors.is_empty(), "{errors:?}");
}

/// A crate outside the exemption list gets no benefit from locally re-declaring the lint -- opting
/// out of inheritance is the failure, regardless of what it declares in its place.
#[test]
fn non_exempt_crate_opting_out_with_the_lint_redeclared_still_fails() {
    let mut errors = Vec::new();
    check_member(
        "prikk-object",
        "[package]\nname = \"prikk-object\"\n\n[lints.clippy]\nundocumented_unsafe_blocks = \"forbid\"\n",
        &[],
        &mut errors,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].detail,
        "prikk-object: does not inherit workspace lints and is not in the exemption list"
    );
}

/// An exempt crate that keeps full workspace inheritance is fine -- being named in the list is
/// permission, not a requirement to actually exercise it.
#[test]
fn exempt_crate_keeping_full_inheritance_passes() {
    let mut errors = Vec::new();
    check_member(
        "prikk-ffi",
        "[lints]\nworkspace = true\n",
        &["prikk-ffi"],
        &mut errors,
    );
    assert!(errors.is_empty(), "{errors:?}");
}
