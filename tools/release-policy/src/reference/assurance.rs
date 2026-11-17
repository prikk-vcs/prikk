//! The assurance table under the durability page (0.58.0 A): each crash point, the test that holds it by name, its kind, and the
//! platforms that run it. **This gate checks that each cited test exists and where it runs, not that it still asserts the
//! property.** A citation proves a name and a place; what a test proves is the page's prose, and a reader's.
//!
//! **The table is hand-written; the platform column is checked, not trusted.** Every row's test must be exactly one `#[test]`
//! function in the file the row names, reachable from a test target (`walk`); not `#[ignore]`d; and the platforms the row types
//! must equal the platforms derived from the test's `cfg` chain intersected with the operating systems CI runs a whole,
//! unfiltered `cargo test --workspace` on (`jobs`, read from `ci.yml`, not typed here). A run-time `cfg!(…)` in a cited test's body
//! (built everywhere, may assert less somewhere) must be marked in the row's note, and a marker with no `cfg!(` is stale.
//!
//! **Anything the derivation cannot read fails the gate** (a filter on a suite step, a matrix, an unknown `cfg` atom, a customised
//! Cargo target): the table never rests on an assumption. Failures name the row and the test.

mod jobs;
mod walk;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::error::Result;

const PAGE: &str = "docs/src/reference/durability-recovery.md";
const CI_WORKFLOW: &str = ".github/workflows/ci.yml";
const BEGIN: &str = "<!-- assurance-table:begin -->";
const END: &str = "<!-- assurance-table:end -->";
/// The sentence condition 2 requires the page to say, in these words.
const STATEMENT: &str = "not that it still asserts the property";
/// In a row's note: the test branches at run time on the operating system.
const OS_VARIES: &str = "[os-varies]";
const KINDS: [&str; 4] = ["real-kill", "failpoint", "byte-state", "unit"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    fn target_os(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::Macos => "macOS",
            Self::Windows => "Windows",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        [Self::Linux, Self::Macos, Self::Windows]
            .into_iter()
            .find(|os| os.label() == label)
    }
}

fn labels(set: &BTreeSet<Os>) -> String {
    if set.is_empty() {
        return "none".to_owned();
    }
    set.iter()
        .map(|os| os.label())
        .collect::<Vec<_>>()
        .join(", ")
}

struct Row {
    number: usize,
    test: String,
    file: String,
    kind: String,
    platforms: String,
    note: String,
}

pub(super) fn check(root: &Path, errors: &mut Vec<String>) -> Result<()> {
    let Ok(page) = fs::read_to_string(root.join(PAGE)) else {
        errors.push(format!("assurance-page-missing:{PAGE}"));
        return Ok(());
    };
    let flattened = page.split_whitespace().collect::<Vec<_>>().join(" ");
    if !flattened.contains(STATEMENT) {
        errors.push(format!(
            "assurance-statement-missing:{PAGE}:\"{STATEMENT}\""
        ));
    }
    let rows = match rows(&page) {
        Ok(rows) => rows,
        Err(message) => {
            errors.push(format!("assurance-table:{message}"));
            return Ok(());
        }
    };
    let Ok(workflow) = fs::read_to_string(root.join(CI_WORKFLOW)) else {
        errors.push(format!("assurance-job-map:{CI_WORKFLOW} cannot be read"));
        return Ok(());
    };
    let oses = match jobs::suite_oses(&workflow) {
        Ok(oses) => oses,
        Err(message) => {
            errors.push(format!("assurance-job-map:{message}"));
            return Ok(());
        }
    };
    let mut tree = match walk::build(root, &oses) {
        Ok(tree) => tree,
        Err(message) => {
            errors.push(format!("assurance-walk:{message}"));
            return Ok(());
        }
    };
    for row in &rows {
        check_row(row, &oses, &mut tree, errors);
    }
    Ok(())
}

fn check_row(row: &Row, oses: &BTreeSet<Os>, tree: &mut walk::Tree, errors: &mut Vec<String>) {
    let at = format!("row {}:{}", row.number, row.test);
    if !KINDS.contains(&row.kind.as_str()) {
        errors.push(format!(
            "assurance-kind:{at}:`{}` is not one of {KINDS:?}",
            row.kind
        ));
    }
    let mut typed = BTreeSet::new();
    for label in row.platforms.split(',').map(str::trim) {
        match Os::parse(label) {
            Some(os) => {
                typed.insert(os);
            }
            None => errors.push(format!("assurance-platform-unknown:{at}:`{label}`")),
        }
    }
    let resolved = match tree.resolve(&row.file, &row.test) {
        Ok(resolved) => resolved,
        Err(message) => {
            errors.push(format!("assurance-test-missing:{at}:{message}"));
            return;
        }
    };
    if resolved.ignored {
        errors.push(format!(
            "assurance-test-ignored:{at}: an `#[ignore]` test is not run by CI and cannot be cited"
        ));
    }
    if resolved.platforms != typed {
        errors.push(format!(
            "assurance-platform-mismatch:{at}:row says {}, derived {} (CI runs the suite on {})",
            labels(&typed),
            labels(&resolved.platforms),
            labels(oses)
        ));
    }
    let marked = row.note.contains(OS_VARIES);
    if resolved.runtime_cfg && !marked {
        errors.push(format!(
            "assurance-os-varies-unmarked:{at}: the test's body has `cfg!(`, so it is built everywhere and may assert less somewhere; mark the note {OS_VARIES}"
        ));
    }
    if marked && !resolved.runtime_cfg {
        errors.push(format!(
            "assurance-os-varies-stale:{at}: the note says {OS_VARIES} and the test's body has no `cfg!(`"
        ));
    }
}

/// The table's rows, between the two markers: five columns, `| crash point | `test` (`file`) | kind | platforms | note |`.
fn rows(page: &str) -> std::result::Result<Vec<Row>, String> {
    let begin = page
        .find(BEGIN)
        .ok_or_else(|| format!("{PAGE} has no `{BEGIN}`"))?;
    let end = page
        .find(END)
        .ok_or_else(|| format!("{PAGE} has no `{END}`"))?;
    let body = page
        .get(begin + BEGIN.len()..end)
        .ok_or_else(|| format!("{PAGE}: `{END}` comes before `{BEGIN}`"))?;
    let mut rows = Vec::new();
    for line in body
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
    {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let first = cells.first().copied().unwrap_or_default();
        if first.eq_ignore_ascii_case("crash point") || first.starts_with("---") {
            continue;
        }
        let [_, test_cell, kind, platforms, note] = cells.as_slice() else {
            return Err(format!("row `{line}` does not have five columns"));
        };
        let (test, file) = split_test_cell(test_cell).ok_or_else(|| {
            format!("row `{line}`: the test column must read `name` (`path/to/file.rs`)")
        })?;
        rows.push(Row {
            number: rows.len() + 1,
            test,
            file,
            kind: (*kind).to_owned(),
            platforms: (*platforms).to_owned(),
            note: (*note).to_owned(),
        });
    }
    if rows.is_empty() {
        return Err("the table has no rows".to_owned());
    }
    Ok(rows)
}

/// `` `name` (`path`) `` -> (name, path).
fn split_test_cell(cell: &str) -> Option<(String, String)> {
    let rest = cell.strip_prefix('`')?;
    let (name, rest) = rest.split_once('`')?;
    let rest = rest.trim().strip_prefix('(')?.strip_suffix(')')?.trim();
    let path = rest.strip_prefix('`')?.strip_suffix('`')?;
    (!name.is_empty() && !path.is_empty()).then(|| (name.to_owned(), path.to_owned()))
}

#[cfg(test)]
#[path = "assurance/tests.rs"]
mod tests;
