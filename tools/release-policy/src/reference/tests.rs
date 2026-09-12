#![allow(clippy::unwrap_used)]

use std::fs;

use super::{
    Classification, Executable, Inventory, RUST_PRIMARY, Reference, required_markers, verify,
};

#[path = "tests/authority.rs"]
mod authority;

fn rust_command() -> String {
    RUST_PRIMARY.command.to_owned()
}

// RFC 119 track B: this used to take a `primary: &str` selecting between a Python and a Rust
// authority descriptor -- `differential-check` and the Python it invoked are gone, and
// `PYTHON_PRIMARY` with them, so `RUST_PRIMARY` is the only descriptor left to build a fixture
// around.
/// Correctly-transcribed MSRV content for whichever of the six live sites (`msrv.rs`) also carries
/// an invocation marker in this fixture -- appended after the marker line, since `msrv::check`
/// (called unconditionally from `verify`) would otherwise report every site as missing.
fn msrv_authority_fixture(temporary: &std::path::Path) {
    fs::create_dir_all(temporary.join(".github/workflows")).unwrap();
    fs::write(
        temporary.join("Cargo.toml"),
        "[workspace.package]\nrust-version = \"1.85\"\n",
    )
    .unwrap();
    fs::write(
        temporary.join(".github/workflows/ci.yml"),
        "jobs:\n  msrv:\n    name: msrv-1.85.0\n    steps:\n      - uses: dtolnay/rust-toolchain@1.85.0\n",
    )
    .unwrap();
    fs::write(
        temporary.join("rfcs/EXECUTION-ORDER.md"),
        "run `cargo +1.85.0 test --workspace --locked` for every candidate\n",
    )
    .unwrap();
}

const MSRV_PROSE_AND_COMMANDS: &str = "\ndeclares Rust 1.85 as its minimum supported version.\ndeclared minimum Rust version is exactly 1.85.0.\ncargo +1.85.0 check --workspace --all-targets --locked\ncargo +1.85.0 test --workspace --locked\ncargo +1.85.0 build --workspace --locked\n";

fn fixture() -> (tempfile::TempDir, Inventory) {
    let temporary = tempfile::tempdir().unwrap();
    fs::create_dir_all(temporary.path().join("release")).unwrap();
    fs::create_dir_all(temporary.path().join("tools/release-policy")).unwrap();
    fs::create_dir_all(temporary.path().join("docs/src/contributing")).unwrap();
    fs::create_dir_all(temporary.path().join("docs/src/reference")).unwrap();
    fs::create_dir_all(temporary.path().join("rfcs")).unwrap();
    fs::write(temporary.path().join("tools/release-policy/Cargo.toml"), "").unwrap();
    msrv_authority_fixture(temporary.path());
    let path = "tools/release-policy/Cargo.toml";
    let command = rust_command();
    let mut references = Vec::new();
    // RFC 141 increment 4: CI's `policy` job is a required live site; the fixture's workflow keeps
    // its MSRV lines and gains the step.
    let workflow = ".github/workflows/ci.yml";
    let mut workflow_text = fs::read_to_string(temporary.path().join(workflow)).unwrap();
    workflow_text.push_str(&format!("  policy:\n    steps:\n      - run: {command}\n"));
    fs::write(temporary.path().join(workflow), workflow_text).unwrap();
    references.push(Reference {
        path: workflow.to_owned(),
        classification: Classification::LiveInvocation,
        command: command.clone(),
    });
    for path in [
        "docs/src/contributing/development.md",
        "docs/src/reference/release-compatibility.md",
        "release/README.md",
    ] {
        let content = if path == "release/README.md" {
            command.clone()
        } else {
            format!("{command}{MSRV_PROSE_AND_COMMANDS}")
        };
        fs::write(temporary.path().join(path), content).unwrap();
        references.push(Reference {
            path: path.to_owned(),
            classification: Classification::LiveInvocation,
            command: command.clone(),
        });
    }
    let history = "rfcs/history.md";
    fs::write(
        temporary.path().join(history),
        ["cargo run --locked -p prikk-release-policy -- ", "check"].concat(),
    )
    .unwrap();
    references.push(Reference {
        path: history.to_owned(),
        classification: Classification::HistoricalOrExplanatory,
        command: ["cargo run --locked -p prikk-release-policy -- ", "check"].concat(),
    });
    (
        temporary,
        Inventory {
            schema_version: "release-policy-command-inventory-v1".to_owned(),
            primary_executable: Executable {
                path: path.to_owned(),
                command,
            },
            invocation_markers: required_markers(),
            references,
        },
    )
}

#[test]
fn marker_omission_and_classification_substitution_fail() {
    let (temporary, mut inventory) = fixture();
    inventory.invocation_markers.pop();
    assert!(
        verify(temporary.path(), &inventory)
            .unwrap()
            .iter()
            .any(|error| error == "invocation-markers")
    );

    let (temporary, mut inventory) = fixture();
    inventory.invocation_markers.swap(0, 1);
    assert!(
        verify(temporary.path(), &inventory)
            .unwrap()
            .iter()
            .any(|error| error == "invocation-markers")
    );

    let (temporary, mut inventory) = fixture();
    inventory.references.first_mut().unwrap().classification =
        Classification::HistoricalOrExplanatory;
    let errors = verify(temporary.path(), &inventory).unwrap();
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("classification:"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("required-live-reference:"))
    );
}

#[test]
fn unregistered_rust_policy_variants_fail() {
    for (index, marker) in [
        "cargo  run --locked -p prikk-release-policy -- check",
        "env cargo run --locked -p prikk-release-policy -- check",
        "cargo \\\n  run -p prikk-release-policy -- check",
        "command cargo run -p prikk-release-policy -- check",
    ]
    .into_iter()
    .enumerate()
    {
        let (temporary, inventory) = fixture();
        let path = format!("docs/unregistered-{index}.md");
        fs::create_dir_all(temporary.path().join("docs")).unwrap();
        fs::write(temporary.path().join(&path), marker).unwrap();
        assert!(
            verify(temporary.path(), &inventory)
                .unwrap()
                .iter()
                .any(|error| error.starts_with(&format!("unregistered-reference:{path}:")))
        );
    }
}

#[test]
fn dynamic_rust_policy_commands_fail_closed_in_executable_files() {
    for (index, command) in [
        "$CARGO run --locked -p prikk-release-policy -- check",
        "${CARGO} run --locked -p prikk-release-policy -- check",
        "$(resolve-cargo) run --locked -p prikk-release-policy -- check",
    ]
    .into_iter()
    .enumerate()
    {
        let (temporary, inventory) = fixture();
        let path = format!("release/dynamic-{index}.sh");
        fs::write(temporary.path().join(&path), command).unwrap();
        assert!(
            verify(temporary.path(), &inventory)
                .unwrap()
                .iter()
                .any(|error| error.starts_with(&format!("unparseable-reference:{path}:")))
        );
    }
}

#[test]
fn genuine_comment_hides_non_executable_reference() {
    for (index, command) in [
        "printf ok # cargo run --locked -p prikk-release-policy -- check\n",
        ": '' # cargo run --locked -p prikk-release-policy -- check\n",
        ": \"\" # cargo run --locked -p prikk-release-policy -- check\n",
    ]
    .into_iter()
    .enumerate()
    {
        let (temporary, inventory) = fixture();
        let path = format!("docs/commented-{index}.md");
        fs::create_dir_all(temporary.path().join("docs")).unwrap();
        fs::write(temporary.path().join(path), command).unwrap();
        assert!(verify(temporary.path(), &inventory).unwrap().is_empty());
    }
}
