use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{BoundaryError, PRODUCTS, push};
use crate::command_scan::{
    Invocation, REQUIRED_CI_POLICY_STEPS, scan, scan_shell, scan_yaml, yaml_run_scripts,
};
use crate::error::{Error, Result};
use crate::json;

const INVENTORY: &str = "release/publication-command-inventory-v1.json";
const CI_WORKFLOW: &str = ".github/workflows/ci.yml";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema_version: String,
    packages: Vec<String>,
    procedures: Vec<Procedure>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Procedure {
    phase: String,
    argv: Vec<String>,
}

pub(super) fn check(root: &Path, errors: &mut Vec<BoundaryError>) -> Result<()> {
    let value = json::parse(&fs::read(root.join(INVENTORY))?)
        .map_err(|error| Error::new(format!("publication inventory JSON: {error}")))?;
    let inventory: Inventory = serde_json::from_value(value)?;
    for detail in verify(&inventory) {
        push(errors, "publication-allowlist", detail);
    }
    for detail in scan_procedure_files(root, &inventory)? {
        push(errors, "publication-allowlist", detail);
    }
    for detail in required_ci_policy_steps(root) {
        push(errors, "publication-allowlist", detail);
    }
    Ok(())
}

/// RFC 141 §2.4: every step in `REQUIRED_CI_POLICY_STEPS` must be a whole `run:` script in
/// `ci.yml`. The allowlist above accepts what a workflow runs; this is the other direction -- what
/// CI must run -- so removing a policy gate from the job is a finding naming that gate, not silence.
fn required_ci_policy_steps(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join(CI_WORKFLOW)) else {
        return vec![format!("required-procedure-unreadable:{CI_WORKFLOW}")];
    };
    let scripts = match yaml_run_scripts(&text) {
        Ok(scripts) => scripts,
        Err(error) => return vec![format!("unparseable-procedure:{CI_WORKFLOW}:{error}")],
    };
    REQUIRED_CI_POLICY_STEPS
        .iter()
        .filter(|step| !scripts.iter().any(|script| script == *step))
        .map(|step| format!("required-procedure-missing:{CI_WORKFLOW}:{step}"))
        .collect()
}

fn scan_procedure_files(root: &Path, inventory: &Inventory) -> Result<Vec<String>> {
    let accepted: Vec<Vec<String>> = inventory
        .procedures
        .iter()
        .map(|procedure| procedure.argv.clone())
        .collect();
    let mut files = Vec::new();
    for directory in [".github", "scripts", "release"] {
        let path = root.join(directory);
        if path.is_dir() {
            collect_files(&path, &mut files)?;
        }
    }
    let mut errors = Vec::new();
    for path in files {
        if path.ends_with(INVENTORY) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let scan = match path.extension().and_then(|value| value.to_str()) {
            Some("sh") => scan_shell(&text),
            Some("yml" | "yaml") => scan_yaml(&text),
            _ => scan(&text),
        };
        if governed_procedure_file(&path) {
            errors.extend(
                scan.errors.iter().map(|error| {
                    format!("unparseable-procedure:{}:{error}", relative(root, &path))
                }),
            );
        }
        for invocation in scan.invocations {
            let Invocation::Publication { argv, .. } = invocation else {
                continue;
            };
            if !accepted.contains(&argv) {
                errors.push(format!(
                    "unregistered-procedure:{}:{}",
                    relative(root, &path),
                    argv.join(" ")
                ));
            }
        }
    }
    errors.sort();
    Ok(errors)
}

fn governed_procedure_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("sh" | "yml" | "yaml")
    )
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_files(&entry.path(), files)?;
        } else {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn verify(inventory: &Inventory) -> Vec<String> {
    let expected: Vec<String> = PRODUCTS
        .iter()
        .map(|(package, _)| (*package).to_owned())
        .collect();
    let mut errors = Vec::new();
    if inventory.schema_version != "publication-command-inventory-v1" {
        errors.push("inventory-version".to_owned());
    }
    if inventory.packages != expected {
        errors.push("package-order".to_owned());
    }
    let expected_procedures: Vec<(String, Vec<String>)> = ["package", "publish"]
        .into_iter()
        .flat_map(|phase| {
            expected.iter().map(move |package| {
                (
                    phase.to_owned(),
                    vec![
                        "cargo".to_owned(),
                        phase.to_owned(),
                        "--locked".to_owned(),
                        "-p".to_owned(),
                        package.clone(),
                    ],
                )
            })
        })
        .collect();
    let actual: Vec<(String, Vec<String>)> = inventory
        .procedures
        .iter()
        .map(|procedure| (procedure.phase.clone(), procedure.argv.clone()))
        .collect();
    if actual != expected_procedures {
        errors.push("procedure-grammar-or-order".to_owned());
    }
    if inventory.procedures.iter().any(|procedure| {
        procedure
            .argv
            .iter()
            .any(|argument| argument == "--workspace")
            || procedure
                .argv
                .iter()
                .any(|argument| argument == "prikk-release-policy")
    }) {
        errors.push("forbidden-package-or-workspace".to_owned());
    }
    errors
}

#[cfg(test)]
#[path = "publication/tests.rs"]
mod tests;
