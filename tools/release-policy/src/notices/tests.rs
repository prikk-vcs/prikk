//! The notices generator and its gate: the parser, the licence-file rule, determinism, and the comparison.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("notices-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn linked(name: &str, version: &str, targets: &[&str]) -> LinkedCrate {
    LinkedCrate {
        name: name.to_string(),
        version: version.to_string(),
        license: Some("MIT".to_string()),
        targets: targets.iter().map(|t| (*t).to_string()).collect(),
        files: vec![("LICENSE".to_string(), "text\n".to_string())],
    }
}

#[test]
fn the_tree_parser_lists_every_crate_once_whatever_the_platform_spells_for_a_path() {
    // Linux/macOS and Windows spellings of a workspace crate's path, and a registry crate; a repeat marker; noise.
    let listing = "prikk v0.56.0 (/work/crates/prikk-cli)\n\
                   prikk-store v0.56.0 (D:\\a\\prikk\\prikk\\crates\\prikk-store)\n\
                   ed25519-dalek v3.0.0\n\
                   digest v0.11.3 (*)\n\
                   ed25519-dalek v3.0.0 (*)\n\
                   \n\
                   not a crate line\n";
    assert_eq!(
        parse_tree(listing),
        BTreeSet::from([
            ("prikk".to_string(), "0.56.0".to_string()),
            ("prikk-store".to_string(), "0.56.0".to_string()),
            ("digest".to_string(), "0.11.3".to_string()),
            ("ed25519-dalek".to_string(), "3.0.0".to_string()),
        ]),
        "the parser reads names and versions only: which crates are the workspace's is metadata's to say"
    );
}

/// The Windows failure of 0.57.0 item 5 (CI run 42829516211): the workspace's own crates must not reach the notices however the
/// platform prints their path. The real graph never lists one, on any release target.
#[test]
fn no_workspace_crate_is_ever_listed_as_a_third_party_one() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let crates = collect(&root).unwrap();
    assert!(
        crates.iter().all(|c| !c.name.starts_with("prikk")),
        "{:?}",
        crates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

/// A checkout that turned LF into CRLF (Windows `core.autocrlf`) is still the committed file once read the way the gate reads it.
#[test]
fn a_crlf_checkout_of_the_committed_file_passes_only_through_the_lf_reading() {
    let generated = render(&[linked("alpha", "1.0.0", &["aarch64-apple-darwin"])]);
    let crlf = generated.replace('\n', "\r\n");
    assert!(compare(&generated, &crlf).is_err());
    assert!(compare(&generated, &lf(&crlf)).is_ok());
}

#[test]
fn the_licence_files_are_found_by_name_whatever_the_case_and_the_rest_are_ignored() {
    let dir = temp_dir("names");
    for (name, body) in [
        ("LICENSE-MIT", "mit\r\ntext  \r\n"),
        ("license-apache-2.0", "apache\n"),
        ("COPYRIGHT", "short notice\n"),
        ("README.md", "not a licence"),
        ("Cargo.toml", "[package]"),
    ] {
        std::fs::write(dir.join(name), body).unwrap();
    }
    let files = licence_files(&dir, None, "demo 1.0.0").unwrap();
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec!["COPYRIGHT", "LICENSE-MIT", "license-apache-2.0"]
    );
    assert_eq!(
        files[1].1, "mit\ntext\n",
        "CRLF becomes LF and trailing whitespace goes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_crate_without_a_licence_file_fails_by_name() {
    let dir = temp_dir("none");
    std::fs::write(dir.join("README.md"), "no licence here").unwrap();
    let error = licence_files(&dir, None, "bare 0.1.0")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("bare 0.1.0") && error.contains("no licence file"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_declared_license_file_outside_the_usual_names_is_included() {
    let dir = temp_dir("extra");
    std::fs::write(dir.join("TERMS.txt"), "terms\n").unwrap();
    let files = licence_files(&dir, Some("TERMS.txt"), "odd 2.0.0").unwrap();
    assert_eq!(
        files,
        vec![("TERMS.txt".to_string(), "terms\n".to_string())]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_rendered_file_is_sorted_by_name_then_version_whatever_the_input_order() {
    let a = linked("zeta", "1.0.0", &["x86_64-unknown-linux-gnu"]);
    let b = linked("alpha", "2.0.0", &["x86_64-pc-windows-msvc"]);
    let c = linked("alpha", "10.0.0", &["x86_64-pc-windows-msvc"]);
    let forward = render(&[a.clone(), b.clone(), c.clone()]);
    let reversed = render(&[c, b, a]);
    assert_eq!(forward, reversed);
    let alpha_2 = forward.find("alpha 2.0.0").unwrap();
    let alpha_10 = forward.find("alpha 10.0.0").unwrap();
    let zeta = forward.find("zeta 1.0.0").unwrap();
    assert!(
        alpha_2 < alpha_10 && alpha_10 < zeta,
        "by name, then by version as a version"
    );
    assert!(forward.contains("Linked on: x86_64-unknown-linux-gnu"));
}

#[test]
fn the_comparison_names_the_first_difference_and_the_command_that_fixes_it() {
    let generated = render(&[linked("alpha", "1.0.0", &["aarch64-apple-darwin"])]);
    assert!(compare(&generated, &generated).is_ok());
    let mut stale = generated.clone();
    stale = stale.replace("alpha 1.0.0", "alpha 0.9.0");
    let error = compare(&generated, &stale).unwrap_err().to_string();
    assert!(error.contains("notices-generate"), "{error}");
    assert!(
        error.contains("[alpha 1.0.0]") && error.contains("[alpha 0.9.0]"),
        "the crates added and removed are named: {error}"
    );
    // A committed file missing a whole crate differs too, and names it.
    let missing = render(&[]);
    let error = compare(&generated, &missing).unwrap_err().to_string();
    assert!(error.contains("[alpha 1.0.0]"), "{error}");
    // Same crates, one licence text edited: the first differing line is named.
    let edited = generated.replacen("text\n", "other text\n", 1);
    let error = compare(&generated, &edited).unwrap_err().to_string();
    assert!(error.contains("first difference at line"), "{error}");
}

/// The target list is the one `release.yml` builds: if a target is added or dropped there, the notices must follow.
#[test]
fn the_release_targets_are_the_workflows_matrix() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let workflow = std::fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap();
    let matrix: BTreeSet<String> = workflow
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- target: "))
        .map(str::to_string)
        .collect();
    let ours: BTreeSet<String> = RELEASE_TARGETS.iter().map(|t| (*t).to_string()).collect();
    assert_eq!(matrix, ours);
}

/// The committed file is what the graph produces today (the gate itself, as a test).
#[test]
fn the_committed_notices_are_what_the_graph_produces() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    check(&root).unwrap();
}

/// The real graph: no linked crate lacks a licence file, nothing proc-macro is listed, and the platform-specific crates are on their
/// platforms only.
#[test]
fn the_real_graph_has_licence_files_for_every_linked_crate_and_the_right_platform_sets() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let crates = collect(&root).unwrap();
    assert!(crates.len() > 15, "{} crates", crates.len());
    for linked in &crates {
        assert!(
            !linked.files.is_empty(),
            "{} {}",
            linked.name,
            linked.version
        );
    }
    let find = |name: &str| crates.iter().find(|c| c.name == name).unwrap();
    assert_eq!(
        find("windows-sys").targets,
        BTreeSet::from(["x86_64-pc-windows-msvc".to_string()])
    );
    assert!(!find("libc").targets.contains("x86_64-pc-windows-msvc"));
    assert!(
        !find("errno").targets.contains("x86_64-unknown-linux-gnu"),
        "the Linux build does not link errno (cargo metadata over-reports it)"
    );
    for proc_macro in ["syn", "quote", "proc-macro2", "zeroize_derive"] {
        assert!(
            crates.iter().all(|c| c.name != proc_macro),
            "{proc_macro} is not linked"
        );
    }
}
