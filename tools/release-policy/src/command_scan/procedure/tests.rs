#![allow(clippy::unwrap_used)]

use super::{allowed, yaml_scripts};

fn cargo_allowed(line: &str) -> bool {
    let tokens: Vec<String> = line.split(' ').map(str::to_owned).collect();
    allowed(&tokens, 0, "cargo")
}

/// RFC 141 increment 4: CI's `policy` job. `check` is accepted through `rust_policy`; the other three
/// only in exactly the form the job spells, so a flag or a different subcommand is still refused.
#[test]
fn the_four_policy_gates_are_accepted_exactly() {
    for gate in ["check", "boundary-check", "reference-check", "size-check"] {
        assert!(
            cargo_allowed(&format!(
                "cargo run --locked -p prikk-release-policy -- {gate}"
            )),
            "{gate}"
        );
    }
    for refused in [
        "cargo run --locked -p prikk-release-policy -- boundary-check --graph",
        "cargo run --locked -p prikk-release-policy -- size-check extra",
        "cargo run -p prikk-release-policy -- reference-check",
        "cargo run --locked -p prikk-release-policy -- release-evidence",
        "cargo run --locked -p prikk-release-policy -- publish-check",
    ] {
        assert!(!cargo_allowed(refused), "{refused}");
    }
}

#[test]
fn extracts_scalar_sequence_flow_and_blocks() {
    let scripts = yaml_scripts(
        r#"url: ${{ metadata }}
defaults:
  run:
    working-directory: docs
- run: cargo test --workspace
-  run: mdbook build
- "run": cargo fmt --check
- 'run': cargo test --workspace
- run : mdbook build
- {name: book, run: cargo fmt --check}
- {run: cargo test --workspace, name: test}
- name: block step
  run: mdbook build
- run: >-
    cargo
    fmt --check
- run: |
    echo first
    echo second
"#,
    )
    .unwrap();
    assert_eq!(
        scripts,
        [
            "cargo test --workspace",
            "mdbook build",
            "cargo fmt --check",
            "cargo test --workspace",
            "mdbook build",
            "cargo fmt --check",
            "cargo test --workspace",
            "mdbook build",
            "cargo fmt --check",
            "echo first\necho second",
        ]
    );
}

#[test]
fn malformed_run_scalar_fails_closed() {
    assert_eq!(
        yaml_scripts("- run: 'unterminated"),
        Err("unsupported-quoted-yaml-run-scalar")
    );
    assert_eq!(yaml_scripts("- run:"), Err("empty-yaml-run-scalar"));
    assert_eq!(
        yaml_scripts("- {name: deploy, run cargo publish}"),
        Err("malformed-yaml-flow-field")
    );
}
