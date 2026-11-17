//! `#[cfg(...)]` boolean-formula parsing and satisfiability, for classifying a module as
//! production or test-only (RFC 130 §4.2 item 4 / v2 handoff §3.5 item 4).
//!
//! **A module is test-only only if its `cfg` formula cannot be satisfied with `test` false.** A
//! substring check on the text (does it contain the word `"test"`) gets this wrong:
//! `fsutil/anchored.rs`'s `none` module is gated on
//! `any(all(test, not(target_os = "windows")), not(any(target_os = "linux", target_os = "macos",
//! target_os = "windows")))`, which contains `test` but is still satisfiable with `test = false`
//! (on any platform that is none of linux/macos/windows) — genuine production code on those
//! platforms, wrongly excluded by a naive check.

use std::collections::BTreeSet;

/// One `#[cfg(...)]` boolean formula.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CfgExpr {
    /// A bare predicate (`test`, `unix`, `windows`) or a `key = "value"` predicate
    /// (`target_os = "linux"`), kept as its exact source text so distinct predicates never collide
    /// under a shared assumed name.
    Atom(String),
    All(Vec<CfgExpr>),
    Any(Vec<CfgExpr>),
    Not(Box<CfgExpr>),
}

/// Parse the inside of a `#[cfg(...)]` attribute -- the text between the outer parentheses.
/// Returns `None` on anything this grammar does not recognise, which callers must treat as "cannot
/// prove test-only," never as "is test-only" (see [`is_possibly_production`]'s own doc).
pub(crate) fn parse(input: &str) -> Option<CfgExpr> {
    let mut chars = input.trim().chars().peekable();
    let expr = parse_expr(&mut chars)?;
    skip_ws(&mut chars);
    if chars.next().is_some() {
        return None; // trailing garbage
    }
    Some(expr)
}

fn skip_ws(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
        chars.next();
    }
}

fn parse_ident(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut ident = String::new();
    while chars
        .peek()
        .is_some_and(|c| c.is_alphanumeric() || *c == '_')
    {
        if let Some(next) = chars.next() {
            ident.push(next);
        }
    }
    ident
}

fn parse_string_literal(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    if chars.next() != Some('"') {
        return None;
    }
    let mut value = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(value),
            other => value.push(other),
        }
    }
}

/// One comma-separated argument list inside `all(...)`/`any(...)`.
fn parse_arg_list(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<Vec<CfgExpr>> {
    skip_ws(chars);
    let mut args = vec![parse_expr(chars)?];
    loop {
        skip_ws(chars);
        match chars.peek() {
            Some(',') => {
                chars.next();
                skip_ws(chars);
                if chars.peek() == Some(&')') {
                    break; // tolerate a trailing comma before the close paren
                }
                args.push(parse_expr(chars)?);
            }
            _ => break,
        }
    }
    skip_ws(chars);
    if chars.next() != Some(')') {
        return None;
    }
    Some(args)
}

fn parse_expr(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<CfgExpr> {
    skip_ws(chars);
    let ident = parse_ident(chars);
    if ident.is_empty() {
        return None;
    }
    skip_ws(chars);
    match ident.as_str() {
        "all" if chars.peek() == Some(&'(') => {
            chars.next();
            Some(CfgExpr::All(parse_arg_list(chars)?))
        }
        "any" if chars.peek() == Some(&'(') => {
            chars.next();
            Some(CfgExpr::Any(parse_arg_list(chars)?))
        }
        "not" if chars.peek() == Some(&'(') => {
            chars.next();
            skip_ws(chars);
            let inner = parse_expr(chars)?;
            skip_ws(chars);
            if chars.next() != Some(')') {
                return None;
            }
            Some(CfgExpr::Not(Box::new(inner)))
        }
        _ if chars.peek() == Some(&'=') => {
            chars.next();
            skip_ws(chars);
            let value = parse_string_literal(chars)?;
            Some(CfgExpr::Atom(format!("{ident} = \"{value}\"")))
        }
        _ => Some(CfgExpr::Atom(ident)),
    }
}

/// Every distinct atom text appearing in `expr`.
pub(crate) fn atoms(expr: &CfgExpr, out: &mut BTreeSet<String>) {
    match expr {
        CfgExpr::Atom(text) => {
            out.insert(text.clone());
        }
        CfgExpr::All(items) | CfgExpr::Any(items) => {
            for item in items {
                atoms(item, out);
            }
        }
        CfgExpr::Not(inner) => atoms(inner, out),
    }
}

pub(crate) fn eval(expr: &CfgExpr, assignment: &std::collections::BTreeMap<String, bool>) -> bool {
    match expr {
        CfgExpr::Atom(text) => assignment.get(text).copied().unwrap_or(false),
        CfgExpr::All(items) => items.iter().all(|item| eval(item, assignment)),
        CfgExpr::Any(items) => items.iter().any(|item| eval(item, assignment)),
        CfgExpr::Not(inner) => !eval(inner, assignment),
    }
}

/// The five mutually-exclusive real-platform "worlds" this codebase's `cfg` vocabulary
/// distinguishes: three named target triples, plus a Unix-like "other" (e.g. a BSD) and a
/// non-Unix "other" (e.g. WASM) -- so `not(any(linux, macos, windows))` has a genuine witness
/// world on both sides of `cfg(unix)`, which `fsutil/anchored.rs`'s own `none` module gate depends
/// on for its Windows-vs-rest split elsewhere. Each world fixes every OS-shaped atom this crate's
/// `cfg` attributes actually use; an atom outside this list is a free variable (see
/// [`is_possibly_production`]), not silently assumed false.
fn os_worlds() -> Vec<Vec<(&'static str, bool)>> {
    let linux = vec![
        ("target_os = \"linux\"", true),
        ("target_os = \"macos\"", false),
        ("target_os = \"windows\"", false),
        ("unix", true),
        ("windows", false),
        ("target_family = \"unix\"", true),
    ];
    let macos = vec![
        ("target_os = \"linux\"", false),
        ("target_os = \"macos\"", true),
        ("target_os = \"windows\"", false),
        ("unix", true),
        ("windows", false),
        ("target_family = \"unix\"", true),
    ];
    let windows = vec![
        ("target_os = \"linux\"", false),
        ("target_os = \"macos\"", false),
        ("target_os = \"windows\"", true),
        ("unix", false),
        ("windows", true),
        ("target_family = \"unix\"", false),
    ];
    let other_unix = vec![
        ("target_os = \"linux\"", false),
        ("target_os = \"macos\"", false),
        ("target_os = \"windows\"", false),
        ("unix", true),
        ("windows", false),
        ("target_family = \"unix\"", true),
    ];
    let other_non_unix = vec![
        ("target_os = \"linux\"", false),
        ("target_os = \"macos\"", false),
        ("target_os = \"windows\"", false),
        ("unix", false),
        ("windows", false),
        ("target_family = \"unix\"", false),
    ];
    vec![linux, macos, windows, other_unix, other_non_unix]
}

/// Whether `expr` can be satisfied with `test` forced false -- the test-only classification RFC
/// 130 requires. **A `None` from [`parse`] (a `cfg` shape this parser does not recognise) must be
/// treated as "cannot prove test-only," i.e. this function returns `true` for it** -- refusing to
/// exclude a module this analysis does not understand is the fail-safe direction: it can only make
/// the graph too large (a stray test module counted as production, at worst adding a false edge a
/// human reviewer will notice), never silently drop a real production module from the graph.
pub(crate) fn is_possibly_production(expr: Option<&CfgExpr>) -> bool {
    let Some(expr) = expr else {
        return true;
    };
    let mut free_atoms = BTreeSet::new();
    atoms(expr, &mut free_atoms);
    free_atoms.remove("test");
    let known_os_atoms: BTreeSet<&str> = [
        "target_os = \"linux\"",
        "target_os = \"macos\"",
        "target_os = \"windows\"",
        "unix",
        "windows",
        "target_family = \"unix\"",
    ]
    .into_iter()
    .collect();
    let other_atoms: Vec<String> = free_atoms
        .iter()
        .filter(|atom| !known_os_atoms.contains(atom.as_str()))
        .cloned()
        .collect();

    for world in os_worlds() {
        for other_assignment in power_set_assignments(&other_atoms) {
            let mut assignment: std::collections::BTreeMap<String, bool> = world
                .iter()
                .map(|(key, value)| ((*key).to_owned(), *value))
                .collect();
            assignment.extend(other_assignment);
            assignment.insert("test".to_owned(), false);
            if eval(expr, &assignment) {
                return true;
            }
        }
    }
    false
}

/// Every boolean assignment of `atoms` (each independently true/false) -- `feature = "..."` flags
/// and anything else this module's `cfg` vocabulary does not name as an OS predicate.
fn power_set_assignments(atoms: &[String]) -> Vec<std::collections::BTreeMap<String, bool>> {
    let mut assignments = vec![std::collections::BTreeMap::new()];
    for atom in atoms {
        let mut next = Vec::with_capacity(assignments.len() * 2);
        for partial in &assignments {
            let mut with_true = partial.clone();
            with_true.insert(atom.clone(), true);
            next.push(with_true);
            let mut with_false = partial.clone();
            with_false.insert(atom.clone(), false);
            next.push(with_false);
        }
        assignments = next;
    }
    assignments
}

#[cfg(test)]
#[path = "cfg_expr/tests.rs"]
mod tests;
