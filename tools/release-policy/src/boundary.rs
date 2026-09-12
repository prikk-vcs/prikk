mod changelog_history;
mod coupling;
mod open_work_index;
mod package;
mod placement;
mod publication;
mod rfc_naming;
mod unsafe_boundary;

pub(crate) use coupling::GraphReport;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cargo_metadata::{Metadata, MetadataCommand, Package, PackageId, TargetKind};
use serde::Serialize;

use crate::error::{Error, Result};

const PRODUCTS: [(&str, &str); 8] = [
    ("prikk-error", "crates/prikk-error/Cargo.toml"),
    ("prikk-ffi", "crates/prikk-ffi/Cargo.toml"),
    ("prikk-hash", "crates/prikk-hash/Cargo.toml"),
    ("prikk-crypto", "crates/prikk-crypto/Cargo.toml"),
    ("prikk-object", "crates/prikk-object/Cargo.toml"),
    ("prikk-replay", "crates/prikk-replay/Cargo.toml"),
    ("prikk-store", "crates/prikk-store/Cargo.toml"),
    ("prikk", "crates/prikk-cli/Cargo.toml"),
];
const CATEGORY_ORDER: [&str; 17] = [
    "workspace-members",
    "default-members",
    "tool-metadata",
    "lockfile-boundary",
    "dependency-boundary",
    "dependency-placement",
    "unsafe-boundary",
    "rfc-naming",
    "open-work-index",
    "changelog-history",
    "module-coupling",
    "publication-allowlist",
    "package-contents",
    "package-description",
    "package-readme",
    "package-readme-duplication",
    "source-archive-contents",
];

#[derive(Debug, Serialize)]
pub(crate) struct BoundaryReport {
    schema_version: &'static str,
    pub(crate) valid: bool,
    errors: Vec<BoundaryError>,
}

#[derive(Debug, Serialize)]
struct BoundaryError {
    category: &'static str,
    detail: String,
}

/// `boundary-check --graph` (RFC 131 §6e step 0): emit the module coupling graph the gate builds.
///
/// Read-only, and it can fail nothing. It runs the same `graph::build` and applies the same
/// `HUB_THRESHOLD` as [`run`]'s own hub check, so a census taken from this output and the gate's own
/// verdict cannot disagree — which is the entire reason it exists rather than the census being
/// assembled by reading imports, which is how the two came to disagree before.
pub(crate) fn graph(root: &Path) -> std::result::Result<GraphReport, String> {
    coupling::graph_report(root)
}

pub(crate) fn run(root: &Path) -> Result<BoundaryReport> {
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .other_options(vec!["--locked".to_owned(), "--offline".to_owned()])
        .exec()
        .map_err(|error| Error::new(format!("cargo metadata failed: {error}")))?;
    let mut errors = Vec::new();
    check_members(root, &metadata, &mut errors);
    check_tool(root, &metadata, &mut errors)?;
    check_dependencies(&metadata, &mut errors);
    placement::check(root, &mut errors);
    unsafe_boundary::check(root, &mut errors);
    rfc_naming::check(root, &mut errors);
    open_work_index::check(root, &mut errors);
    changelog_history::check(root, &mut errors)?;
    coupling::check(root, &mut errors);
    package::check(root, &mut errors)?;
    publication::check(root, &mut errors)?;
    errors.sort_by(|left, right| {
        category_rank(left.category)
            .cmp(&category_rank(right.category))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    Ok(BoundaryReport {
        schema_version: "release-policy-boundary-v1",
        valid: errors.is_empty(),
        errors,
    })
}

fn check_members(root: &Path, metadata: &Metadata, errors: &mut Vec<BoundaryError>) {
    let expected: BTreeMap<&str, &str> = PRODUCTS
        .into_iter()
        .chain([
            ("prikk-release-policy", "tools/release-policy/Cargo.toml"),
            // RFC 126 §5 increment A / owner's §6 ruling (option 4): criterion lives in its own
            // member, outside `PRODUCTS`, so `placement.rs`'s dependency-placement scan (which
            // iterates `PRODUCTS`) never sees it -- adding it here, not to `PRODUCTS`, is the whole
            // mechanism that keeps it out of the shipped dependency graph.
            ("prikk-benchmarks", "tools/benchmarks/Cargo.toml"),
            // RFC 139 increment 1: the measurement-corpus profile format and extractor. Same
            // treatment as `prikk-benchmarks` above and for the same reason -- `publish = false`,
            // outside `default-members`, declares `toml`/`serde` with literal versions rather than
            // through `[workspace.dependencies]` (handoff §5: tools are outside `PRODUCTS`'
            // dependency-placement scan by construction), and ships to nobody.
            ("prikk-corpus", "tools/corpus/Cargo.toml"),
        ])
        .collect();
    let members = packages_by_id(metadata, &metadata.workspace_members);
    let actual: BTreeMap<&str, String> = members
        .iter()
        .map(|package| {
            (
                package.name.as_str(),
                relative(root, package.manifest_path.as_std_path()),
            )
        })
        .collect();
    if actual
        != expected
            .iter()
            .map(|(name, path)| (*name, (*path).to_owned()))
            .collect()
    {
        push(errors, "workspace-members", format!("{actual:?}"));
    }
    let defaults: BTreeSet<&str> = packages_by_id(metadata, &metadata.workspace_default_members)
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let expected_defaults: BTreeSet<&str> = PRODUCTS.iter().map(|(name, _)| *name).collect();
    if defaults != expected_defaults {
        push(errors, "default-members", format!("{defaults:?}"));
    }
}

fn check_tool(root: &Path, metadata: &Metadata, errors: &mut Vec<BoundaryError>) -> Result<()> {
    let Some(tool) = metadata
        .packages
        .iter()
        .find(|package| package.name.to_string() == "prikk-release-policy")
    else {
        push(errors, "tool-metadata", "tool package absent".to_owned());
        return Ok(());
    };
    for (valid, detail) in [
        (
            tool.publish.as_ref().is_some_and(std::vec::Vec::is_empty),
            "publish",
        ),
        (
            tool.rust_version.as_ref().is_some_and(|version| {
                version.major == 1 && version.minor == 85 && version.patch == 0
            }),
            "rust-version",
        ),
        (tool.edition.to_string() == "2024", "edition"),
        (tool.targets.len() == 1, "target-count"),
        (
            tool.targets
                .first()
                .is_some_and(|target| target.name == "prikk-release-policy"),
            "target-name",
        ),
        (
            tool.targets
                .first()
                .is_some_and(|target| target.kind.contains(&TargetKind::Bin)),
            "target-kind",
        ),
    ] {
        if !valid {
            push(errors, "tool-metadata", detail.to_owned());
        }
    }
    let manifest_text = std::fs::read_to_string(root.join("tools/release-policy/Cargo.toml"))?;
    let manifest: toml::Value = toml::from_str(&manifest_text)
        .map_err(|error| Error::new(format!("tool manifest parse: {error}")))?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| Error::new("tool manifest package table missing"))?;
    for name in ["version", "edition", "rust-version"] {
        if package
            .get(name)
            .and_then(toml::Value::as_table)
            .and_then(|value| value.get("workspace"))
            .and_then(toml::Value::as_bool)
            != Some(true)
        {
            push(errors, "tool-metadata", format!("{name}.workspace"));
        }
    }
    // RFC 119 track B: a `lints.workspace` check for this crate used to live here too. Removed --
    // `unsafe_boundary::check` already asserts, for every workspace member including this one,
    // that it either inherits `[lints] workspace = true` in full or is on the (single-entry)
    // unsafe-code exemption list re-declaring the guard locally; this crate is not on that list,
    // so the only way it can pass `unsafe-boundary` at all is `lints.workspace = true`. The two
    // checks tested the identical condition on the identical file; this one was pure duplication.
    if root.join("tools/release-policy/Cargo.lock").exists() {
        push(errors, "lockfile-boundary", "nested Cargo.lock".to_owned());
    }
    Ok(())
}

fn check_dependencies(metadata: &Metadata, errors: &mut Vec<BoundaryError>) {
    let Some(resolve) = &metadata.resolve else {
        push(
            errors,
            "dependency-boundary",
            "resolve graph absent".to_owned(),
        );
        return;
    };
    let local: BTreeSet<&PackageId> = metadata
        .packages
        .iter()
        .filter(|package| package.source.is_none())
        .map(|package| &package.id)
        .collect();
    let Some(tool) = metadata
        .packages
        .iter()
        .find(|package| package.name.to_string() == "prikk-release-policy")
    else {
        return;
    };
    for node in &resolve.nodes {
        if node.id == tool.id {
            if node
                .deps
                .iter()
                .any(|dependency| local.contains(&dependency.pkg))
            {
                push(
                    errors,
                    "dependency-boundary",
                    "tool to product edge".to_owned(),
                );
            }
        } else if local.contains(&node.id)
            && reaches(resolve, &node.id, &tool.id, &mut BTreeSet::new())
        {
            push(
                errors,
                "dependency-boundary",
                format!("product reaches tool: {}", node.id),
            );
        }
    }
}

fn packages_by_id<'a>(metadata: &'a Metadata, ids: &[PackageId]) -> Vec<&'a Package> {
    ids.iter()
        .filter_map(|id| metadata.packages.iter().find(|package| &package.id == id))
        .collect()
}

fn reaches<'a>(
    resolve: &'a cargo_metadata::Resolve,
    current: &'a PackageId,
    target: &PackageId,
    visited: &mut BTreeSet<&'a PackageId>,
) -> bool {
    if !visited.insert(current) {
        return false;
    }
    resolve
        .nodes
        .iter()
        .find(|node| &node.id == current)
        .is_some_and(|node| {
            node.deps.iter().any(|dependency| {
                &dependency.pkg == target || reaches(resolve, &dependency.pkg, target, visited)
            })
        })
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn push(errors: &mut Vec<BoundaryError>, category: &'static str, detail: String) {
    errors.push(BoundaryError { category, detail });
}

fn category_rank(category: &str) -> usize {
    CATEGORY_ORDER
        .iter()
        .position(|candidate| *candidate == category)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
#[path = "boundary/tests.rs"]
mod tests;
