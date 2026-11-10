//! 0.57.0 item (5): third-party licence notices, generated and gated.
//!
//! BSD-3-Clause, MIT, Zlib and Unicode-3.0 each ask that their notice travel with a binary distribution, and a release archive
//! used to ship `prikk` and `LICENSE` only. `THIRD-PARTY-NOTICES`, at the repository root, holds for every crate **linked into the
//! `prikk` binary on any release target**: its name, version and SPDX expression, the release targets that link it, and the full
//! text of each licence file its published package ships. It is generated from the resolved graph and the registry sources, never
//! written by hand, and committed so that it is reviewable, rides in the source tarball the `prikk` PKGBUILD builds from, and can be
//! copied into every archive.
//!
//! **The gate regenerates and compares** ([`check`]): a dependency change cannot land without its notices, and a linked crate whose
//! package ships no licence file fails by name (this module invents no fallback text).
//!
//! ## What "linked" means
//!
//! The normal dependencies of the `prikk` package for one target, excluding proc-macro crates (they run in the compiler, not in the
//! binary), build dependencies and development dependencies, and excluding the workspace's own crates (they are prikk, covered by
//! `LICENSE`). The set is read from `cargo tree -e normal,no-proc-macro --target <triple>`, not from `cargo metadata`'s resolve:
//! metadata resolves features across targets, and measured on 2026-11-10 it lists `errno` for both Linux targets, which the real
//! Linux build never links (rustix uses its own syscall backend there). `cargo metadata` supplies what a crate *is* (its licence
//! expression and where its sources are).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use cargo_metadata::MetadataCommand;

use crate::error::{Error, Result};

/// The committed file's name, at the repository root.
pub(crate) const FILE_NAME: &str = "THIRD-PARTY-NOTICES";

/// The four targets `release.yml` builds and ships. A test holds this list to the workflow's own matrix.
pub(crate) const RELEASE_TARGETS: [&str; 4] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// A top-level file of a package is a licence file when its lower-cased name starts with one of these.
const LICENCE_FILE_PREFIXES: [&str; 6] = [
    "license",
    "licence",
    "copying",
    "unlicense",
    "notice",
    "copyright",
];

/// One crate linked on at least one release target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkedCrate {
    pub(crate) name: String,
    pub(crate) version: String,
    /// The SPDX expression its manifest declares, or `None` (a crate with only a `license-file`).
    pub(crate) license: Option<String>,
    /// The release targets that link it.
    pub(crate) targets: BTreeSet<String>,
    /// Each licence file's name and normalised text, sorted by name.
    pub(crate) files: Vec<(String, String)>,
}

/// `(name, version)` of each registry crate a `cargo tree --prefix none --format "{p}"` listing names. A line ending in a
/// parenthesised path is a workspace crate and is skipped; so is a `(*)` repeat marker.
pub(crate) fn parse_tree(listing: &str) -> BTreeSet<(String, String)> {
    let mut crates = BTreeSet::new();
    for line in listing.lines() {
        let mut words = line.split_whitespace();
        let (Some(name), Some(version)) = (words.next(), words.next()) else {
            continue;
        };
        let Some(version) = version.strip_prefix('v') else {
            continue;
        };
        let source = words.find(|word| *word != "(*)");
        if source.is_some_and(|word| word.starts_with('(') && word.contains('/')) {
            continue;
        }
        crates.insert((name.to_string(), version.to_string()));
    }
    crates
}

/// Text as the notices carry it: LF line endings, no trailing whitespace on any line, one final newline.
pub(crate) fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    for line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// The licence files at the top of `dir`, by name, with their normalised text. `Err` names the crate when there are none.
pub(crate) fn licence_files(
    dir: &Path,
    extra: Option<&str>,
    crate_label: &str,
) -> Result<Vec<(String, String)>> {
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if entry.file_type()?.is_file()
            && LICENCE_FILE_PREFIXES
                .iter()
                .any(|prefix| lower.starts_with(prefix))
        {
            names.insert(name);
        }
    }
    if let Some(extra) = extra {
        if dir.join(extra).is_file() {
            names.insert(extra.to_string());
        }
    }
    if names.is_empty() {
        return Err(Error::new(format!(
            "{crate_label} is linked into the prikk binary and its package ships no licence file; the notices cannot be generated for it \
             (a decision, not a fallback text, is needed)"
        )));
    }
    names
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(dir.join(&name))?;
            let text = String::from_utf8(bytes).map_err(|_| {
                Error::new(format!(
                    "{crate_label}: licence file {name} is not valid UTF-8"
                ))
            })?;
            Ok((name, normalise(&text)))
        })
        .collect()
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Every crate linked on any release target, with its licence files.
pub(crate) fn collect(root: &Path) -> Result<Vec<LinkedCrate>> {
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .other_options(vec!["--locked".to_owned(), "--offline".to_owned()])
        .exec()
        .map_err(|error| Error::new(format!("cargo metadata failed: {error}")))?;
    let packages: BTreeMap<(String, String), &cargo_metadata::Package> = metadata
        .packages
        .iter()
        .map(|package| {
            (
                (package.name.to_string(), package.version.to_string()),
                package,
            )
        })
        .collect();

    let mut targets_of: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for target in RELEASE_TARGETS {
        let output = cargo()
            .current_dir(root)
            .args([
                "tree",
                "-p",
                "prikk",
                "-e",
                "normal,no-proc-macro",
                "--prefix",
                "none",
                "--format",
                "{p}",
                "--locked",
                "--offline",
                "--target",
                target,
            ])
            .output()?;
        if !output.status.success() {
            return Err(Error::new(format!(
                "cargo tree for {target} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        for key in parse_tree(&String::from_utf8_lossy(&output.stdout)) {
            targets_of
                .entry(key)
                .or_default()
                .insert(target.to_string());
        }
    }
    if targets_of.is_empty() {
        return Err(Error::new(
            "cargo tree listed no third-party crate; the parser is probably broken",
        ));
    }

    targets_of
        .into_iter()
        .map(|((name, version), targets)| {
            let package = packages
                .get(&(name.clone(), version.clone()))
                .ok_or_else(|| {
                    Error::new(format!(
                        "{name} {version} is in the tree but not in the metadata"
                    ))
                })?;
            let dir = package
                .manifest_path
                .as_std_path()
                .parent()
                .ok_or_else(|| {
                    Error::new(format!("{name} {version}: manifest has no directory"))
                })?;
            let extra = package
                .license_file
                .as_ref()
                .map(|path| path.as_str().to_string());
            let files = licence_files(dir, extra.as_deref(), &format!("{name} {version}"))?;
            Ok(LinkedCrate {
                name,
                version,
                license: package.license.clone(),
                targets,
                files,
            })
        })
        .collect()
}

const RULE: &str =
    "================================================================================";

/// The file's text for `crates`: sorted by name, then version, so it is deterministic.
pub(crate) fn render(crates: &[LinkedCrate]) -> String {
    let mut sorted: Vec<&LinkedCrate> = crates.iter().collect();
    // By name, then by version as a version (2.0.0 before 10.0.0), the text as the tie-break for one that does not parse.
    sorted.sort_by_cached_key(|linked| {
        (
            linked.name.clone(),
            cargo_metadata::semver::Version::parse(&linked.version).ok(),
            linked.version.clone(),
        )
    });
    let mut out = String::new();
    out.push_str("THIRD-PARTY NOTICES\n\n");
    out.push_str(
        "prikk is licensed under the Apache License, Version 2.0 (see LICENSE). The prikk binary is built from prikk's own\n\
         source and the third-party crates below. For each crate this file gives its name, version and SPDX licence\n\
         expression, the release targets that link it, and the full text of each licence file its published package ships.\n\n",
    );
    out.push_str(&format!(
        "Release targets: {}.\n\n",
        RELEASE_TARGETS.join(", ")
    ));
    out.push_str(
        "This file is generated, not written: `cargo run -p prikk-release-policy -- notices-generate` rebuilds it from the\n\
         resolved dependency graph (the normal dependencies of the `prikk` package for each release target, without\n\
         proc-macro, build or development crates) and the registry sources, and the gate `notices-check` fails when this\n\
         file differs from what that produces. Line endings are LF and trailing whitespace is removed from each licence text.\n",
    );
    for linked in sorted {
        out.push('\n');
        out.push_str(RULE);
        out.push('\n');
        out.push_str(&format!("{} {}\n", linked.name, linked.version));
        out.push_str(&format!(
            "License: {}\n",
            linked
                .license
                .as_deref()
                .unwrap_or("(none declared; see the files below)")
        ));
        out.push_str(&format!(
            "Linked on: {}\n",
            linked
                .targets
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for (name, text) in &linked.files {
            out.push_str(&format!("\n--- {name} ---\n\n"));
            out.push_str(text);
        }
    }
    out
}

/// What the committed file must be, from the repository as it stands.
pub(crate) fn generate(root: &Path) -> Result<String> {
    Ok(render(&collect(root)?))
}

/// The `name version` heading of each crate section in a notices text.
fn crate_headings(text: &str) -> BTreeSet<String> {
    let mut headings = BTreeSet::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line == RULE {
            if let Some(heading) = lines.next() {
                headings.insert(heading.to_string());
            }
        }
    }
    headings
}

/// Compare `committed` with `generated`; the error names the crates added or removed, else the first differing line.
pub(crate) fn compare(generated: &str, committed: &str) -> Result<()> {
    if generated == committed {
        return Ok(());
    }
    let (now, then) = (crate_headings(generated), crate_headings(committed));
    let join = |set: Vec<&String>| set.into_iter().cloned().collect::<Vec<_>>().join(", ");
    let added = join(now.difference(&then).collect());
    let removed = join(then.difference(&now).collect());
    let what = if added.is_empty() && removed.is_empty() {
        let line = generated
            .lines()
            .zip(committed.lines())
            .position(|(a, b)| a != b)
            .map_or_else(
                || generated.lines().count().min(committed.lines().count()) + 1,
                |index| index + 1,
            );
        format!("a section's text differs (first difference at line {line})")
    } else {
        format!(
            "linked now but missing from the file: [{added}]; in the file but no longer linked: [{removed}]"
        )
    };
    Err(Error::new(format!(
        "{FILE_NAME} differs from what the dependency graph produces: {what}. A dependency change must land with its notices: run \
         `cargo run -p prikk-release-policy -- notices-generate` and commit the result"
    )))
}

/// The gate: regenerate, and compare with the committed file.
pub(crate) fn check(root: &Path) -> Result<()> {
    let generated = generate(root)?;
    let committed = std::fs::read_to_string(root.join(FILE_NAME))
        .map_err(|error| Error::new(format!("{FILE_NAME} cannot be read: {error}")))?;
    compare(&generated, &committed)
}

/// Write the file.
pub(crate) fn write(root: &Path) -> Result<()> {
    std::fs::write(root.join(FILE_NAME), generate(root)?)?;
    Ok(())
}

#[cfg(test)]
mod tests;
