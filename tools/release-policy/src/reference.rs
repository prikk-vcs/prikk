mod msrv;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command_scan::{Invocation, invocations, scan, scan_shell, scan_yaml};
use crate::error::{Error, Result};
use crate::json;

const INVENTORY_PATH: &str = "release/release-policy-command-inventory-v1.json";
// RFC 141 increment 4: CI's `policy` job runs `check` as a step, so the workflow is a live
// invocation site like the three documents, and required like them -- a step deleted from the job
// fails this gate rather than leaving the policy gates unrun in CI again.
const REQUIRED_LIVE_PATHS: [&str; 4] = [
    ".github/workflows/ci.yml",
    "docs/src/contributing/development.md",
    "docs/src/reference/release-compatibility.md",
    "release/README.md",
];
// RFC 119 track B: `PYTHON_PRIMARY` removed along with `differential-check` and the Python it
// invoked -- `release/check-policy.py` no longer exists, and this tool's own `check` (below) was
// already the only `primary_executable` the real inventory ever declared.
const RUST_PRIMARY: AuthorityDescriptor = AuthorityDescriptor {
    path: "tools/release-policy/Cargo.toml",
    command: "cargo run --locked -p prikk-release-policy -- check",
};

#[derive(Clone, Copy)]
struct AuthorityDescriptor {
    path: &'static str,
    command: &'static str,
}

fn required_markers() -> Vec<String> {
    [
        ["cargo run --locked -p prikk-release-policy -- ", "check"].concat(),
        ["cargo run -p prikk-release-policy -- ", "check"].concat(),
    ]
    .to_vec()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema_version: String,
    primary_executable: Executable,
    invocation_markers: Vec<String>,
    references: Vec<Reference>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Executable {
    path: String,
    command: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    path: String,
    classification: Classification,
    command: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
enum Classification {
    LiveInvocation,
    HistoricalOrExplanatory,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReferenceReport {
    schema_version: &'static str,
    pub(crate) valid: bool,
    errors: Vec<String>,
}

pub(crate) fn run(root: &Path) -> Result<ReferenceReport> {
    let bytes = fs::read(root.join(INVENTORY_PATH))?;
    let value = json::parse(&bytes)
        .map_err(|error| Error::new(format!("reference inventory JSON: {error}")))?;
    let inventory: Inventory = serde_json::from_value(value)?;
    let errors = verify(root, &inventory)?;
    Ok(ReferenceReport {
        schema_version: "release-policy-reference-result-v1",
        valid: errors.is_empty(),
        errors,
    })
}

fn verify(root: &Path, inventory: &Inventory) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    if inventory.schema_version != "release-policy-command-inventory-v1" {
        errors.push("inventory-version".to_owned());
    }
    let selected = authority_descriptor(&inventory.primary_executable);
    if selected.is_none()
        || !selected.is_some_and(|descriptor| regular_file(root.join(descriptor.path)))
    {
        errors.push("primary-executable".to_owned());
    }
    let markers = required_markers();
    if inventory.invocation_markers != markers {
        errors.push("invocation-markers".to_owned());
    }
    let registered: BTreeSet<_> = inventory
        .references
        .iter()
        .map(|reference| {
            (
                reference.path.as_str(),
                reference.command.as_str(),
                reference.classification,
            )
        })
        .collect();
    if registered.len() != inventory.references.len() {
        errors.push("duplicate-reference".to_owned());
    }
    for path in REQUIRED_LIVE_PATHS {
        let live: Vec<_> = inventory
            .references
            .iter()
            .filter(|reference| {
                reference.path == path && reference.classification == Classification::LiveInvocation
            })
            .collect();
        if live.len() != 1
            || !selected.is_some_and(|descriptor| {
                live.first()
                    .is_some_and(|reference| reference.command == descriptor.command)
            })
        {
            errors.push(format!("required-live-reference:{path}"));
        }
    }
    for reference in &inventory.references {
        let classification_valid = match reference.classification {
            Classification::LiveInvocation => {
                REQUIRED_LIVE_PATHS.contains(&reference.path.as_str())
                    && selected.is_some_and(|descriptor| reference.command == descriptor.command)
            }
            Classification::HistoricalOrExplanatory => {
                (reference.path.starts_with("rfcs/")
                    || reference.path == "tools/release-policy/README.md")
                    && authority_command(&reference.command)
            }
        };
        if !classification_valid {
            errors.push(format!("classification:{}", reference.path));
        }
    }
    let mut files = Vec::new();
    collect_text_files(root, root, &mut files)?;
    for path in files {
        let relative = relative(root, &path);
        if relative == INVENTORY_PATH {
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
        if executable_reference_file(&path) {
            errors.extend(
                scan.errors
                    .iter()
                    .map(|error| format!("unparseable-reference:{relative}:{error}")),
            );
        }
        for invocation in scan.invocations {
            if matches!(invocation, Invocation::RustPolicy)
                && !registered.iter().any(|(known_path, command, _)| {
                    *known_path == relative && command_matches(command, &invocation)
                })
            {
                errors.push(format!(
                    "unregistered-reference:{relative}:{}",
                    invocation_name(&invocation)
                ));
            }
        }
    }
    for reference in &inventory.references {
        let path = root.join(&reference.path);
        let expected = command_invocation(&reference.command);
        let present = fs::read_to_string(path).is_ok_and(|text| {
            expected
                .as_ref()
                .is_some_and(|expected| invocations(&text).iter().any(|item| item == expected))
        });
        if !present {
            errors.push(format!(
                "missing-reference:{}:{}",
                reference.path, reference.command
            ));
        }
    }
    msrv::check(root, &mut errors)?;
    errors.sort();
    Ok(errors)
}

fn authority_descriptor(executable: &Executable) -> Option<AuthorityDescriptor> {
    (executable.path == RUST_PRIMARY.path && executable.command == RUST_PRIMARY.command)
        .then_some(RUST_PRIMARY)
}

fn authority_command(command: &str) -> bool {
    command == RUST_PRIMARY.command
}

fn regular_file(path: PathBuf) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn command_invocation(command: &str) -> Option<Invocation> {
    invocations(command).into_iter().next()
}

fn command_matches(command: &str, invocation: &Invocation) -> bool {
    command_invocation(command).as_ref() == Some(invocation)
}

fn invocation_name(invocation: &Invocation) -> &'static str {
    match invocation {
        Invocation::RustPolicy => "rust-policy",
        Invocation::Publication { .. } => "publication",
    }
}

fn collect_text_files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = relative(root, &path);
        if entry.file_type()?.is_dir() {
            if matches!(
                relative.as_str(),
                ".git" | ".git-exclude" | "target" | "docs/book"
            ) {
                continue;
            }
            collect_text_files(root, &path, output)?;
        } else if scannable_reference_file(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn scannable_reference_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("md" | "yml" | "yaml" | "sh")
    )
}

fn executable_reference_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("yml" | "yaml" | "sh")
    )
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
#[path = "reference/tests.rs"]
mod tests;
