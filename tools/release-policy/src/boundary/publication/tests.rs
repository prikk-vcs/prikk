#![allow(clippy::unwrap_used)]

use std::fs;

use super::{
    CI_WORKFLOW, Inventory, Procedure, REQUIRED_CI_POLICY_STEPS, required_ci_policy_steps,
    scan_procedure_files, verify,
};

fn workflow_with_steps(steps: &[&str]) -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    fs::create_dir_all(temporary.path().join(".github/workflows")).unwrap();
    let mut text = String::from("jobs:\n  policy:\n    steps:\n");
    for step in steps {
        text.push_str(&format!("      - run: {step}\n"));
    }
    fs::write(temporary.path().join(CI_WORKFLOW), text).unwrap();
    temporary
}

#[test]
fn the_real_workflow_runs_every_required_policy_step() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap();
    assert_eq!(required_ci_policy_steps(root), Vec::<String>::new());
}

/// RFC 141 §2.4: each of the four steps is required on its own -- deleting any one fails, naming
/// exactly that one.
#[test]
fn deleting_any_one_policy_step_fails_naming_it() {
    let complete = workflow_with_steps(&REQUIRED_CI_POLICY_STEPS);
    assert_eq!(
        required_ci_policy_steps(complete.path()),
        Vec::<String>::new()
    );
    for missing in REQUIRED_CI_POLICY_STEPS {
        let remaining: Vec<&str> = REQUIRED_CI_POLICY_STEPS
            .into_iter()
            .filter(|step| *step != missing)
            .collect();
        let workflow = workflow_with_steps(&remaining);
        assert_eq!(
            required_ci_policy_steps(workflow.path()),
            [format!(
                "required-procedure-missing:{CI_WORKFLOW}:{missing}"
            )]
        );
    }
}

/// A step only counts as a whole `run:` script: commented out, or carrying an extra argument, it is
/// missing.
#[test]
fn a_commented_or_altered_policy_step_does_not_count() {
    let temporary = workflow_with_steps(&REQUIRED_CI_POLICY_STEPS[..3]);
    let path = temporary.path().join(CI_WORKFLOW);
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("      # - run: cargo run --locked -p prikk-release-policy -- size-check\n");
    text.push_str("      - run: cargo run --locked -p prikk-release-policy -- size-check --json\n");
    fs::write(&path, text).unwrap();
    assert_eq!(
        required_ci_policy_steps(temporary.path()),
        [format!(
            "required-procedure-missing:{CI_WORKFLOW}:{}",
            REQUIRED_CI_POLICY_STEPS[3]
        )]
    );
}

#[test]
fn a_missing_workflow_is_reported() {
    let temporary = tempfile::tempdir().unwrap();
    assert_eq!(
        required_ci_policy_steps(temporary.path()),
        [format!("required-procedure-unreadable:{CI_WORKFLOW}")]
    );
}

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
