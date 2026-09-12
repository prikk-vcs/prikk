#![allow(clippy::unwrap_used)]

use std::fs;

use super::{Inventory, Procedure, scan_procedure_files, verify};

fn valid() -> Inventory {
    let packages = [
        "prikk-error",
        "prikk-ffi",
        "prikk-hash",
        "prikk-crypto",
        "prikk-object",
        "prikk-replay",
        "prikk-store",
        "prikk",
    ]
    .map(str::to_owned)
    .to_vec();
    let procedures = ["package", "publish"]
        .into_iter()
        .flat_map(|phase| {
            packages.iter().map(move |package| Procedure {
                phase: phase.to_owned(),
                argv: ["cargo", phase, "--locked", "-p"]
                    .map(str::to_owned)
                    .into_iter()
                    .chain([package.clone()])
                    .collect(),
            })
        })
        .collect();
    Inventory {
        schema_version: "publication-command-inventory-v1".to_owned(),
        packages,
        procedures,
    }
}

#[test]
fn unregistered_ci_or_release_procedure_fails() {
    for (index, command) in [
        "run: cargo  publish --workspace\n",
        "run: cargo \\\n  publish --workspace\n",
        "run: env cargo publish --workspace\n",
        "run: command cargo publish -p prikk-release-policy\n",
        "run: cmd=(cargo publish --workspace)\n",
        "run: publish_all() { cargo publish --workspace; }\n",
        "run: >-\n  cargo\n  publish --workspace\n",
        "run: printf '#'; cargo publish --workspace\n",
        "run: printf \"#\"; cargo publish --workspace\n",
        "run: : ''#x; cargo publish --workspace\n",
        "run: : \"\"#x; cargo publish --workspace\n",
        "run: $CARGO publish --workspace\n",
        "run: ${CARGO} publish --workspace\n",
        "run: cargo \"$PHASE\" --workspace\n",
        "run: cargo \"${PHASE}\" --workspace\n",
        "run: cargo \"$ACTION\" --workspace\n",
        "run: cargo \"${ACTION}\" --workspace\n",
        "run: $CARGO \"$PHASE\" --workspace\n",
        "run: $TOOL \"$ACTION\" --workspace\n",
        "run: $TOOL publish --workspace\n",
        "run: env $TOOL publish --workspace\n",
        "run: env MODE=release ${TOOL} \"${ACTION}\" --workspace\n",
        "- run: $TOOL \"$ACTION\" --workspace\n",
        "  - run: $TOOL \"$ACTION\" --workspace\n",
        "- { run: $TOOL \"$ACTION\" --workspace }\n",
        "- {run: $TOOL \"$ACTION\" --workspace}\n",
        "run: env -u UNUSED $TOOL \"$ACTION\" --workspace\n",
        "run: env -C DIR $TOOL \"$ACTION\" --workspace\n",
        "run: env --unset UNUSED --chdir DIR $TOOL \"$ACTION\" --workspace\n",
        "run: command -p $TOOL \"$ACTION\" --workspace\n",
        "run: nice $TOOL \"$ACTION\" --workspace\n",
        "run: exec $TOOL \"$ACTION\" --workspace\n",
        "run: timeout 600 $TOOL \"$ACTION\" --workspace\n",
        "run: xargs $TOOL \"$ACTION\" --workspace\n",
        "run: nohup $TOOL \"$ACTION\" --workspace\n",
        "run: stdbuf -oL $TOOL \"$ACTION\" --workspace\n",
        "run: setsid $TOOL \"$ACTION\" --workspace\n",
        "run: ionice $TOOL \"$ACTION\" --workspace\n",
        "run: time $TOOL \"$ACTION\" --workspace\n",
        "run: project-wrapper $TOOL \"$ACTION\" --workspace\n",
        "- run: nice $TOOL \"$ACTION\" --workspace\n",
        "run: env --unknown VALUE $TOOL \"$ACTION\" --workspace\n",
        "run: env -u\n",
        "run: command --unknown $TOOL \"$ACTION\" --workspace\n",
        "run: `deploy`\n",
        "run: `resolve-tool` publish\n",
        "run: sh -c 'cargo publish'\n",
        "run: bash -c \"cargo package --workspace\"\n",
        "run: bash -lc 'cargo publish'\n",
        "run: /bin/sh -c 'cargo publish'\n",
        "run: dash -c 'cargo publish'\n",
        "run: ksh -c 'cargo publish'\n",
        "run: zsh -c 'cargo publish'\n",
        "run: env sh -c 'cargo publish'\n",
        "run: eval 'cargo publish'\n",
        "run: python3 -c 'import os; os.system(\"cargo publish\")'\n",
        "run: perl -e 'system(\"cargo publish\")'\n",
        "run: node -e 'require(\"child_process\").execSync(\"cargo publish\")'\n",
        "run: ruby -e 'system(\"cargo publish\")'\n",
        "run: nice sh -c 'cargo publish'\n",
        "run: xargs sh -c 'cargo publish'\n",
        "run: timeout 600 bash -c 'cargo publish'\n",
        "run: project-local-wrapper cargo fmt --check\n",
        "-  run: cargo publish\n",
        "-   run: nice sh -c 'cargo publish'\n",
        "- \"run\": cargo publish\n",
        "- 'run': cargo publish\n",
        "- run : cargo publish\n",
        "  \"run\": nice sh -c 'cargo publish'\n",
        "- {name: deploy, run: cargo publish}\n",
        "- { name: deploy, run: cargo publish }\n",
        "- {run: cargo publish, name: deploy}\n",
        "- {name: deploy, run cargo publish}\n",
        "run: cargo 'publish\n",
    ]
    .into_iter()
    .enumerate()
    {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(temporary.path().join(".github/workflows")).unwrap();
        fs::write(
            temporary
                .path()
                .join(format!(".github/workflows/release-{index}.yml")),
            command,
        )
        .unwrap();
        assert!(
            !scan_procedure_files(temporary.path(), &valid())
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn genuine_comment_hides_non_executable_text() {
    for (index, command) in [
        "run: printf ok # cargo publish --workspace\n",
        "run: : '' # cargo publish --workspace\n",
        "run: : \"\" # cargo publish --workspace\n",
    ]
    .into_iter()
    .enumerate()
    {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(temporary.path().join(".github/workflows")).unwrap();
        fs::write(
            temporary
                .path()
                .join(format!(".github/workflows/release-{index}.yml")),
            command,
        )
        .unwrap();
        assert!(
            scan_procedure_files(temporary.path(), &valid())
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn rejects_workspace_internal_tool_and_order_mutations() {
    assert!(verify(&valid()).is_empty());
    for mutation in 0..3 {
        let mut inventory = valid();
        match mutation {
            0 => inventory
                .procedures
                .first_mut()
                .unwrap()
                .argv
                .push("--workspace".to_owned()),
            1 => {
                *inventory
                    .procedures
                    .first_mut()
                    .unwrap()
                    .argv
                    .last_mut()
                    .unwrap() = "prikk-release-policy".to_owned()
            }
            _ => inventory.packages.swap(0, 1),
        }
        assert!(!verify(&inventory).is_empty());
    }
}
