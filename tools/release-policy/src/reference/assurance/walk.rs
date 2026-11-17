//! Which test functions are built, and on which operating systems: the module walk and the `cfg` evaluator.
//!
//! **The walk.** From every test root of every workspace member (`src/lib.rs`, `src/main.rs`, `src/bin/*.rs`, `tests/*.rs`,
//! `tests/*/main.rs`) it follows `mod x;` (to `x.rs` or `x/mod.rs` under the declaring file's directory, or `#[path = "…"]`),
//! reading each declaration's `#[cfg]`, each file's inner `#![cfg]`, and each inline `mod x { … }` block's. A function's
//! conditions are the conjunction of every one of them and its own. A file can be reached from more than one root (a shared
//! `#[path]` module): it is built wherever any inclusion is.
//!
//! **What it refuses (a gate failure, never a guess).** A `cfg` atom outside the list in [`atom_value`]; a `cfg_attr` that
//! mentions `ignore`; a `[[test]]`, `autotests`, custom `[lib]`/`[[bin]]` path or `required-features` in a member's
//! `Cargo.toml` (the walk assumes Cargo's own discovery); a `mod x;` inside an inline module; a file whose braces do not balance
//! after comments and strings are masked (the lexer lost its place).
//!
//! **Features.** `feature = "…"` is true for a crate when some workspace member's dependency edge on it enables the feature
//! (dev-dependencies included), directly or through the crate's own `[features]` table: that is what `cargo test --workspace`
//! unifies. `prikk-cli` depends on `prikk-store` with `test-support`, so the store's own tests build with it. Read from the
//! manifests, not typed.
//!
//! **What it cannot see** (the page says so, and the gate checks the one it can): a test that branches at run time on the OS with
//! `cfg!(…)` is built everywhere. [`Resolved::runtime_cfg`] reports it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use super::Os;
use crate::boundary::coupling::cfg_expr::{self, CfgExpr};
use crate::boundary::coupling::graph::{blank, classify, find_matching_brace};

/// What the gate knows of one cited test.
#[derive(Debug)]
pub(super) struct Resolved {
    /// The OSes (of the job map) on which the test is built.
    pub(super) platforms: BTreeSet<Os>,
    pub(super) ignored: bool,
    /// The test's body mentions `cfg!(`: it compiles everywhere and may assert something different on each OS.
    pub(super) runtime_cfg: bool,
}

pub(super) struct Tree {
    root: PathBuf,
    oses: BTreeSet<Os>,
    features: BTreeMap<String, BTreeSet<String>>,
    /// `file -> every way a test root includes it`.
    inclusions: BTreeMap<PathBuf, Vec<Inclusion>>,
    scans: BTreeMap<PathBuf, Scan>,
}

struct Inclusion {
    krate: String,
    cfgs: Vec<CfgExpr>,
}

#[derive(Clone)]
struct Item {
    kind: Kind,
    name: String,
    attrs: Vec<String>,
    /// The inline modules that enclose it, outermost first (indices into `Scan::items`).
    within: Vec<usize>,
    body_has_cfg_macro: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    ModFile,
    ModInline,
    Fn,
}

struct Scan {
    inner_cfgs: Vec<CfgExpr>,
    items: Vec<Item>,
}

pub(super) fn build(root: &Path, oses: &BTreeSet<Os>) -> Result<Tree, String> {
    let manifest = read_toml(&root.join("Cargo.toml"))?;
    let members = workspace_members(root, &manifest)?;
    let features = enabled_features(&manifest, &members)?;
    let mut tree = Tree {
        root: root.to_path_buf(),
        oses: oses.clone(),
        features,
        inclusions: BTreeMap::new(),
        scans: BTreeMap::new(),
    };
    for member in &members {
        let dir = root.join(&member.path);
        let krate = member.name.clone();
        for test_root in test_roots(&dir, &member.manifest)? {
            let mut stack = Vec::new();
            tree.walk(&test_root, &krate, Vec::new(), true, &mut stack)?;
        }
    }
    Ok(tree)
}

impl Tree {
    /// The one `#[test]` function `name` in `file`, with its platforms.
    pub(super) fn resolve(&mut self, file: &str, name: &str) -> Result<Resolved, String> {
        let path = self.root.join(file);
        let Some(inclusions) = self.inclusions.get(&path) else {
            return Err(format!(
                "{file} is not part of any test target (no `mod` chain from a test root reaches it)"
            ));
        };
        let inclusions: Vec<(String, Vec<CfgExpr>)> = inclusions
            .iter()
            .map(|inclusion| (inclusion.krate.clone(), inclusion.cfgs.clone()))
            .collect();
        let scan = self.scan(&path)?;
        let hits: Vec<&Item> = scan
            .items
            .iter()
            .filter(|item| item.kind == Kind::Fn && item.name == name)
            .collect();
        let tests: Vec<&Item> = hits
            .iter()
            .copied()
            .filter(|item| item.attrs.iter().any(|attr| attr == "test"))
            .collect();
        let item = match (hits.len(), tests.as_slice()) {
            (0, _) => return Err(format!("no `fn {name}` in {file}")),
            (_, [only]) if hits.len() == 1 => *only,
            (_, []) => return Err(format!("`fn {name}` in {file} has no `#[test]`")),
            _ => return Err(format!("`fn {name}` is defined more than once in {file}")),
        };
        let mut own = Vec::new();
        let mut ignored = false;
        for attr in &item.attrs {
            if let Some(inner) = attr_call(attr, "cfg") {
                own.push(parse_cfg(&inner, file)?);
            }
            if attr_call(attr, "cfg_attr").is_some_and(|inner| inner.contains("ignore")) {
                return Err(format!(
                    "`fn {name}` in {file} has a `cfg_attr` that mentions `ignore`: the gate cannot tell where it runs"
                ));
            }
            ignored |=
                attr == "ignore" || attr.starts_with("ignore ") || attr.starts_with("ignore=");
        }
        let mut enclosing = Vec::new();
        for index in &item.within {
            let module = scan
                .items
                .get(*index)
                .ok_or_else(|| "internal: inline module index".to_owned())?;
            for attr in &module.attrs {
                if let Some(inner) = attr_call(attr, "cfg") {
                    enclosing.push(parse_cfg(&inner, file)?);
                }
            }
        }
        let inner_cfgs = scan.inner_cfgs.clone();
        let runtime_cfg = item.body_has_cfg_macro;
        let mut platforms = BTreeSet::new();
        for (krate, cfgs) in &inclusions {
            for os in &self.oses {
                let mut holds = true;
                for cfg in cfgs.iter().chain(&inner_cfgs).chain(&enclosing).chain(&own) {
                    if !self.holds(cfg, *os, krate, file)? {
                        holds = false;
                        break;
                    }
                }
                if holds {
                    platforms.insert(*os);
                }
            }
        }
        Ok(Resolved {
            platforms,
            ignored,
            runtime_cfg,
        })
    }

    fn holds(&self, cfg: &CfgExpr, os: Os, krate: &str, file: &str) -> Result<bool, String> {
        let mut atoms = BTreeSet::new();
        cfg_expr::atoms(cfg, &mut atoms);
        let empty = BTreeSet::new();
        let features = self.features.get(krate).unwrap_or(&empty);
        let mut assignment = BTreeMap::new();
        for atom in atoms {
            let value = atom_value(&atom, os, features).ok_or_else(|| {
                format!("{file}: `cfg` atom `{atom}` is not one the gate evaluates")
            })?;
            assignment.insert(atom, value);
        }
        Ok(cfg_expr::eval(cfg, &assignment))
    }

    fn walk(
        &mut self,
        file: &Path,
        krate: &str,
        cfgs: Vec<CfgExpr>,
        is_root: bool,
        stack: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        if stack.iter().any(|seen| seen == file) {
            return Err(format!("{}: `mod` cycle", file.display()));
        }
        let (mut conditions, declarations) = {
            let scan = self.scan(file)?;
            let declarations: Vec<(String, Vec<String>, bool)> = scan
                .items
                .iter()
                .filter(|item| item.kind == Kind::ModFile)
                .map(|item| {
                    (
                        item.name.clone(),
                        item.attrs.clone(),
                        !item.within.is_empty(),
                    )
                })
                .collect();
            (cfgs.clone(), declarations)
        };
        let display = file.display().to_string();
        conditions.extend(self.scan(file)?.inner_cfgs.clone());
        self.inclusions
            .entry(file.to_path_buf())
            .or_default()
            .push(Inclusion {
                krate: krate.to_owned(),
                cfgs: conditions.clone(),
            });
        let base = file
            .parent()
            .ok_or_else(|| format!("{display} has no parent"))?;
        let stem = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("{display} has no stem"))?;
        let children = if is_root || stem == "mod" {
            base.to_path_buf()
        } else {
            base.join(stem)
        };
        stack.push(file.to_path_buf());
        for (name, attrs, inside_inline) in declarations {
            if inside_inline {
                return Err(format!(
                    "{display}: `mod {name};` inside an inline module is not read by the gate"
                ));
            }
            let mut child_cfgs = conditions.clone();
            for attr in &attrs {
                if let Some(inner) = attr_call(attr, "cfg") {
                    child_cfgs.push(parse_cfg(&inner, &display)?);
                }
            }
            let child = match attrs.iter().find_map(|attr| path_attr(attr)) {
                Some(relative) => base.join(relative),
                None => {
                    let flat = children.join(format!("{name}.rs"));
                    if flat.exists() {
                        flat
                    } else {
                        children.join(&name).join("mod.rs")
                    }
                }
            };
            if !child.exists() {
                return Err(format!(
                    "{display}: `mod {name};` resolves to {}, which does not exist",
                    child.display()
                ));
            }
            self.walk(&child, krate, child_cfgs, false, stack)?;
        }
        stack.pop();
        Ok(())
    }

    fn scan(&mut self, file: &Path) -> Result<&Scan, String> {
        if !self.scans.contains_key(file) {
            let scan = scan_file(file)?;
            self.scans.insert(file.to_path_buf(), scan);
        }
        self.scans
            .get(file)
            .ok_or_else(|| "internal: scan cache".to_owned())
    }
}

// ---------------------------------------------------------------------------------------------- source scan

fn scan_file(file: &Path) -> Result<Scan, String> {
    let raw =
        fs::read_to_string(file).map_err(|error| format!("read {}: {error}", file.display()))?;
    let kinds = classify(&raw);
    let with_strings = blank(&raw, &kinds, true, false);
    let masked = blank(&raw, &kinds, true, true);
    let opens = masked.bytes().filter(|byte| *byte == b'{').count();
    let closes = masked.bytes().filter(|byte| *byte == b'}').count();
    if opens != closes {
        return Err(format!(
            "{}: braces do not balance after masking comments and strings ({opens} open, {closes} close): the lexer lost its place",
            file.display()
        ));
    }
    let mut inner_cfgs = Vec::new();
    let inner = regex_inner_attr();
    for found in inner.find_iter(&masked) {
        let open = found.end();
        let Some(close) = find_bracket_close(&masked, open) else {
            return Err(format!("{}: unbalanced `#![`", file.display()));
        };
        let text = with_strings.get(open..close).unwrap_or_default().trim();
        if let Some(inner_text) = attr_call(text, "cfg") {
            inner_cfgs.push(parse_cfg(&inner_text, &file.display().to_string())?);
        }
    }
    let item_pattern = regex_item();
    let mut items: Vec<Item> = Vec::new();
    // Inline modules still open at the current offset: (index into `items`, offset of its closing brace).
    let mut open_modules: Vec<(usize, usize)> = Vec::new();
    for found in item_pattern.captures_iter(&masked) {
        let (Some(whole), Some(keyword), Some(name)) = (found.get(0), found.get(1), found.get(2))
        else {
            continue;
        };
        let start = whole.start();
        while open_modules.last().is_some_and(|(_, close)| *close < start) {
            open_modules.pop();
        }
        let attrs = attributes_before(&masked, &with_strings, start);
        let within: Vec<usize> = open_modules.iter().map(|(index, _)| *index).collect();
        let after = whole.end();
        let (kind, body) = if keyword.as_str() == "mod" {
            match next_block_or_semicolon(&masked, after) {
                Some((b'{', open)) => {
                    let close = find_matching_brace(masked.as_bytes(), open)
                        .ok_or_else(|| format!("{}: unbalanced module block", file.display()))?;
                    (Kind::ModInline, Some((open, close)))
                }
                _ => (Kind::ModFile, None),
            }
        } else {
            match next_block_or_semicolon(&masked, after) {
                Some((b'{', open)) => {
                    let close = find_matching_brace(masked.as_bytes(), open)
                        .ok_or_else(|| format!("{}: unbalanced function body", file.display()))?;
                    (Kind::Fn, Some((open, close)))
                }
                _ => (Kind::Fn, None),
            }
        };
        let body_has_cfg_macro = body
            .and_then(|(open, close)| masked.get(open..close))
            .is_some_and(|text| text.contains("cfg!("));
        items.push(Item {
            kind,
            name: name.as_str().to_owned(),
            attrs,
            within,
            body_has_cfg_macro,
        });
        if kind == Kind::ModInline {
            if let Some((_, close)) = body {
                open_modules.push((items.len() - 1, close));
            }
        }
    }
    Ok(Scan { inner_cfgs, items })
}

fn regex_item() -> Regex {
    // The pattern is a constant, so compilation cannot fail; a failure would be a defect in this file.
    Regex::new(
        r"\b(?:pub(?:\s*\([^)]*\))?\s+)?(?:(?:async|const|unsafe|extern)\s+)*(mod|fn)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .unwrap_or_else(|error| panic!("assurance item pattern: {error}"))
}

fn regex_inner_attr() -> Regex {
    Regex::new(r"#!\[").unwrap_or_else(|error| panic!("assurance inner-attribute pattern: {error}"))
}

/// The byte after `from` (past generics and parameters) that opens the item's body or ends its declaration: `{` or `;`.
fn next_block_or_semicolon(masked: &str, from: usize) -> Option<(u8, usize)> {
    let bytes = masked.as_bytes();
    let mut paren = 0_i32;
    let mut square = 0_i32;
    let mut index = from;
    while let Some(byte) = bytes.get(index).copied() {
        match byte {
            b'(' => paren += 1,
            b')' => paren -= 1,
            b'[' => square += 1,
            b']' => square -= 1,
            b'{' if paren <= 0 && square <= 0 => return Some((b'{', index)),
            b';' if paren <= 0 && square <= 0 => return Some((b';', index)),
            _ => {}
        }
        index += 1;
    }
    None
}

/// The index of the `]` that closes the `[` just before `from`.
fn find_bracket_close(masked: &str, from: usize) -> Option<usize> {
    crate::boundary::coupling::graph::find_matching_bracket(masked.as_bytes(), from)
}

/// The outer attributes directly above the item at `start`, as the text inside `#[ … ]`, outermost first.
fn attributes_before(masked: &str, with_strings: &str, start: usize) -> Vec<String> {
    let bytes = masked.as_bytes();
    let mut attrs = Vec::new();
    let mut cursor = start;
    loop {
        let mut end = cursor;
        while end > 0 && bytes.get(end - 1).is_some_and(u8::is_ascii_whitespace) {
            end -= 1;
        }
        if end == 0 || bytes.get(end - 1) != Some(&b']') {
            break;
        }
        let mut depth = 0_i32;
        let mut open = None;
        let mut index = end - 1;
        loop {
            match bytes.get(index) {
                Some(b']') => depth += 1,
                Some(b'[') => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(index);
                        break;
                    }
                }
                _ => {}
            }
            if index == 0 {
                break;
            }
            index -= 1;
        }
        let Some(open) = open else { break };
        // `#[` is an outer attribute; `#![` belongs to the file or module, not to this item.
        if open == 0 || bytes.get(open - 1) != Some(&b'#') {
            break;
        }
        attrs.push(
            with_strings
                .get(open + 1..end - 1)
                .unwrap_or_default()
                .trim()
                .to_owned(),
        );
        cursor = open - 1;
    }
    attrs.reverse();
    attrs
}

/// `name(args)` -> `args`, for an attribute's text.
fn attr_call(text: &str, name: &str) -> Option<String> {
    let rest = text
        .trim()
        .strip_prefix(name)?
        .trim_start()
        .strip_prefix('(')?;
    rest.trim_end().strip_suffix(')').map(str::to_owned)
}

fn path_attr(text: &str) -> Option<String> {
    let rest = text
        .trim()
        .strip_prefix("path")?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    rest.strip_prefix('"')?.strip_suffix('"').map(str::to_owned)
}

fn parse_cfg(inner: &str, file: &str) -> Result<CfgExpr, String> {
    cfg_expr::parse(inner).ok_or_else(|| format!("{file}: cannot read `cfg({inner})`"))
}

// ---------------------------------------------------------------------------------------------- cfg atoms

/// The atoms the repository's tests use (all of them, by grep). `None` for anything else: the gate refuses rather than guesses.
fn atom_value(atom: &str, os: Os, features: &BTreeSet<String>) -> Option<bool> {
    let unix = os != Os::Windows;
    match atom {
        "test" => return Some(true),
        "unix" => return Some(unix),
        "windows" => return Some(!unix),
        _ => {}
    }
    let (key, value) = atom.split_once('=')?;
    let key = key.trim();
    let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
    match key {
        "target_os" => Some(value == os.target_os()),
        "target_family" => Some(value == if unix { "unix" } else { "windows" }),
        "target_vendor" => Some(
            value
                == if os == Os::Macos {
                    "apple"
                } else if unix {
                    "unknown"
                } else {
                    "pc"
                },
        ),
        "feature" => Some(features.contains(value)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------- the workspace

struct Member {
    name: String,
    path: String,
    manifest: toml::Value,
}

fn read_toml(path: &Path) -> Result<toml::Value, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn workspace_members(root: &Path, root_manifest: &toml::Value) -> Result<Vec<Member>, String> {
    let list = root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or("Cargo.toml: workspace.members missing")?;
    let mut members = Vec::new();
    for entry in list {
        let path = entry
            .as_str()
            .ok_or("Cargo.toml: a workspace member is not a string")?;
        if path.contains('*') {
            return Err(format!(
                "Cargo.toml: workspace member `{path}` is a glob; the gate reads an explicit list"
            ));
        }
        let manifest = read_toml(&root.join(path).join("Cargo.toml"))?;
        let name = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("{path}/Cargo.toml: package.name missing"))?
            .to_owned();
        members.push(Member {
            name,
            path: path.to_owned(),
            manifest,
        });
    }
    Ok(members)
}

fn workspace_dependencies_entry_has_features(root_manifest: &toml::Value, name: &str) -> bool {
    root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|all| all.get(name))
        .is_some_and(|entry| entry.get("features").is_some())
}

fn string_list(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(toml::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Per crate, the features enabled in a `cargo test --workspace` build: its `default` set, those enabled by any member's
/// dependency edge on it, and everything those enable through the `[features]` tables.
fn enabled_features(
    root_manifest: &toml::Value,
    members: &[Member],
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let names: BTreeSet<&str> = members.iter().map(|member| member.name.as_str()).collect();
    let mut enabled: BTreeMap<String, BTreeSet<String>> = members
        .iter()
        .map(|member| (member.name.clone(), BTreeSet::new()))
        .collect();
    for member in members {
        let targets = member
            .manifest
            .get("target")
            .and_then(toml::Value::as_table);
        for (condition, target) in targets.into_iter().flatten() {
            for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
                let table = target.get(table_name).and_then(toml::Value::as_table);
                // An edge that enables no feature changes nothing the gate evaluates; one that does is per-platform.
                let enables = |key: &String, spec: &toml::Value| {
                    names.contains(key.as_str())
                        && (spec.get("features").is_some()
                            || workspace_dependencies_entry_has_features(root_manifest, key))
                };
                if table.is_some_and(|table| table.iter().any(|(key, spec)| enables(key, spec))) {
                    return Err(format!(
                        "{}: `[target.{condition}.{table_name}]` enables a feature of a workspace member; the gate does not read per-platform feature edges",
                        member.path
                    ));
                }
            }
        }
    }
    let workspace_dependencies = root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"));
    for member in members {
        for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let Some(table) = member
                .manifest
                .get(table_name)
                .and_then(toml::Value::as_table)
            else {
                continue;
            };
            for (dependency, spec) in table {
                let Some(spec) = spec.as_table() else {
                    continue;
                };
                if spec.contains_key("package") {
                    return Err(format!(
                        "{}: dependency `{dependency}` is renamed; the gate does not follow renames",
                        member.path
                    ));
                }
                if !names.contains(dependency.as_str()) {
                    continue;
                }
                let mut turned_on = string_list(spec.get("features"));
                if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                    turned_on.extend(string_list(
                        workspace_dependencies
                            .and_then(|all| all.get(dependency))
                            .and_then(|entry| entry.get("features")),
                    ));
                }
                if let Some(set) = enabled.get_mut(dependency) {
                    set.extend(turned_on);
                }
            }
        }
    }
    // Every selected member's default features are on.
    for member in members {
        let defaults = string_list(
            member
                .manifest
                .get("features")
                .and_then(|features| features.get("default")),
        );
        if let Some(set) = enabled.get_mut(&member.name) {
            set.extend(defaults);
        }
    }
    loop {
        let mut changed = false;
        for member in members {
            let current: Vec<String> = enabled
                .get(&member.name)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();
            for feature in current {
                let implied = string_list(
                    member
                        .manifest
                        .get("features")
                        .and_then(|features| features.get(&feature)),
                );
                for entry in implied {
                    if let Some((dependency, other)) = entry.split_once('/') {
                        let dependency = dependency.trim_end_matches('?');
                        if let Some(set) = enabled.get_mut(dependency) {
                            changed |= set.insert(other.to_owned());
                        }
                    } else if !entry.starts_with("dep:") {
                        if let Some(set) = enabled.get_mut(&member.name) {
                            changed |= set.insert(entry);
                        }
                    }
                }
            }
        }
        if !changed {
            return Ok(enabled);
        }
    }
}

/// The files Cargo compiles as test targets for one member, by its own discovery. A manifest that customises discovery is refused.
fn test_roots(dir: &Path, manifest: &toml::Value) -> Result<Vec<PathBuf>, String> {
    let package = manifest.get("package");
    if manifest.get("test").is_some()
        || package
            .and_then(|package| package.get("autotests"))
            .is_some()
        || package
            .and_then(|package| package.get("autobins"))
            .is_some()
    {
        return Err(format!(
            "{}: `[[test]]`, `autotests` or `autobins` customises discovery; the gate reads Cargo's default",
            dir.display()
        ));
    }
    if let Some(lib) = manifest.get("lib") {
        let path = lib.get("path").and_then(toml::Value::as_str);
        if path.is_some_and(|path| path != "src/lib.rs") || lib.get("required-features").is_some() {
            return Err(format!(
                "{}: a custom `[lib]` is not read by the gate",
                dir.display()
            ));
        }
    }
    for bin in manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = bin
            .get("path")
            .and_then(toml::Value::as_str)
            .unwrap_or("src/main.rs");
        if !(path == "src/main.rs" || path.starts_with("src/bin/"))
            || bin.get("required-features").is_some()
        {
            return Err(format!(
                "{}: `[[bin]]` path `{path}` is not under Cargo's discovery",
                dir.display()
            ));
        }
    }
    let mut roots = Vec::new();
    for candidate in ["src/lib.rs", "src/main.rs"] {
        let path = dir.join(candidate);
        if path.exists() {
            roots.push(path);
        }
    }
    for folder in ["src/bin", "tests"] {
        let Ok(entries) = fs::read_dir(dir.join(folder)) else {
            continue;
        };
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                found.push(path);
            } else if path.is_dir() && path.join("main.rs").exists() {
                found.push(path.join("main.rs"));
            }
        }
        found.sort();
        roots.extend(found);
    }
    Ok(roots)
}
