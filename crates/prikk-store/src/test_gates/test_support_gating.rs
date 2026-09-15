//! RFC 136 increment 2b review, carried into the checkout-refusal round §0: test-support items must never
//! be compiled into a shipped build, and `prikk-store/Cargo.toml` says a violation goes unseen, because only
//! the `--all-features` gate builds the feature. So this is checked at source, item by item:
//!
//! - every `pub fn …_for_test_support` in any crate's `src/` carries a `cfg` naming
//!   `feature = "test-support"`;
//! - so does every `pub` type named in such a function's signature, defined in the same file, that no
//!   other production file uses outside a gated re-export;
//! - so does every `pub use` that re-exports one of those names.
//!
//! A module-level inner `cfg` is not accepted in place of the item's own: moving the item out of its
//! module must not ungate it. A violation names the file and the item.

use std::path::{Path, PathBuf};

const GATE: &str = "feature = \"test-support\"";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every production `.rs` file under `crates/*/src`: test files and `tests/` directories excluded.
fn production_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates = workspace_root().join("crates");
    let Ok(entries) = std::fs::read_dir(&crates) else {
        return files;
    };
    let mut stack: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join("src"))
        .filter(|src| src.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            if path.is_dir() {
                if name != "tests" && name != "test_gates" {
                    stack.push(path);
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs")
                && name != "tests.rs"
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The attribute lines directly above line `index`, skipping doc comments.
fn attributes_above(lines: &[&str], index: usize) -> Vec<String> {
    let mut attributes = Vec::new();
    let mut at = index;
    while at > 0 {
        at -= 1;
        let line = lines.get(at).map_or("", |line| line.trim());
        if line.starts_with("#[") {
            attributes.push(line.to_string());
        } else if !line.starts_with("///") {
            break;
        }
    }
    attributes
}

fn gated(lines: &[&str], index: usize) -> bool {
    attributes_above(lines, index)
        .iter()
        .any(|attribute| attribute.starts_with("#[cfg") && attribute.contains(GATE))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The identifier after `prefix` on `line`, if the line declares one.
fn declared_name<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix(prefix)?;
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    rest.get(..end).filter(|name| !name.is_empty())
}

/// The test-support items of every production file: the gated functions, and the same-file `pub` types
/// their signatures name, each with its file, line index and whether it carries the gate.
fn test_support_items(files: &[(PathBuf, String)]) -> Vec<(PathBuf, usize, String, bool)> {
    let mut items = Vec::new();
    for (path, text) in files {
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let Some(name) = declared_name(line, "pub fn ") else {
                continue;
            };
            if !name.ends_with("_for_test_support") {
                continue;
            }
            items.push((path.clone(), index, name.to_string(), gated(&lines, index)));
            let signature: String = lines
                .iter()
                .skip(index)
                .take_while(|line| !line.trim_end().ends_with('{'))
                .chain(
                    lines
                        .iter()
                        .skip(index)
                        .find(|line| line.trim_end().ends_with('{')),
                )
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            for (type_index, type_line) in lines.iter().enumerate() {
                let Some(type_name) = declared_name(type_line, "pub enum ")
                    .or_else(|| declared_name(type_line, "pub struct "))
                else {
                    continue;
                };
                let named = signature
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .any(|word| word == type_name);
                if named && !items.iter().any(|(_, _, seen, _)| seen == type_name) {
                    items.push((
                        path.clone(),
                        type_index,
                        type_name.to_string(),
                        gated(&lines, type_index),
                    ));
                }
            }
        }
    }
    items
}

#[test]
fn every_test_support_item_carries_the_feature_gate() {
    let files: Vec<(PathBuf, String)> = production_files()
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(&path).ok().map(|text| (path, text)))
        .collect();
    let items = test_support_items(&files);
    assert!(
        items
            .iter()
            .any(|(_, _, name, _)| name == "publish_snapshot_fixture_for_test_support"),
        "fixture sanity: the scan finds the lying-snapshot fixture"
    );

    let mut violations = Vec::new();
    for (path, index, name, item_gated) in &items {
        let is_type = !name.ends_with("_for_test_support");
        // A type another production file uses outside a gated re-export is shared, not test-support.
        let shared = is_type
            && files.iter().any(|(other, text)| {
                let lines: Vec<&str> = text.lines().collect();
                other != path
                    && lines.iter().enumerate().any(|(at, line)| {
                        let mentions = line
                            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                            .any(|word| word == name);
                        let comment = line.trim_start().starts_with("//");
                        mentions && !comment && !(line.contains("pub use") && gated(&lines, at))
                    })
            });
        if !shared && !item_gated {
            violations.push(format!(
                "{}:{}: `{name}` carries no cfg naming {GATE}",
                relative(path),
                index + 1
            ));
        }
        for (other, text) in &files {
            let lines: Vec<&str> = text.lines().collect();
            for (at, line) in lines.iter().enumerate() {
                let reexports = line.contains("pub use")
                    && line
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .any(|word| word == name);
                if reexports && !shared && !gated(&lines, at) {
                    violations.push(format!(
                        "{}:{}: the re-export of `{name}` carries no cfg naming {GATE}",
                        relative(other),
                        at + 1
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "test-support items compiled without the feature gate:\n{}",
        violations.join("\n")
    );
}
