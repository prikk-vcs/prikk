//! **P3 -- no allocation is sized by a decoded length without a bound** (RFC 160 §3.3).
//!
//! Allocation failure in Rust **aborts**: a `Vec::with_capacity(n)` or `vec![0; n]` whose `n` came out of a file the reader has not
//! yet validated turns a damaged (or hostile) record into a process that dies without an error message, in the very tools that exist
//! to diagnose damage. RFC 102's append-length round met it once in review (the ranged read sized by a frame header's `body_len`)
//! and the sweep found it shipped once (`trust_index.rs`, `with_capacity(count as usize)`).
//!
//! This test reads the production sources of `prikk-store` and `prikk-object`, finds every `with_capacity(`, `reserve(`,
//! `reserve_exact(`, `resize(` and `vec![x; n]`, and classifies the size expression:
//! - **constant** -- only literals, `SCREAMING_CASE` constants and their `.len()`;
//! - **in-memory length** -- also `something.len()` of a collection that already exists (its own allocation is some earlier site's
//!   business), with arithmetic on it;
//! - **cursor-bounded** -- `ByteCursor::bounded_capacity(count, min_element_len)`: the capacity is `min(count, remaining bytes /
//!   smallest encoded element)`, so a claimed count can never ask for more than the bytes left could hold;
//! - **unbounded** -- anything else: `as usize` on a value, a bare variable, `usize::try_from(..)`. An unbounded site **fails the
//!   test** unless it is in [`ALLOWED`] with the bound it relies on **and a witness**: text that must appear in the same file (the
//!   bound itself, in the words the code uses, so removing the bound reddens the scan). A listed site whose witness is gone, or that the scan no longer
//!   finds, also fails: the list cannot go stale silently.
//!
//! The scan is checked against itself (`the_scanner_...` tests below) so a scanner that stops matching cannot pass as "no offenders";
//! and [`every_site_is_classified_and_the_scan_sees_a_realistic_number_of_them`] fails if it finds fewer sites, of any class, than the
//! tree is known to hold. **Report mode:** `PRIKK_ALLOC_SCAN_REPORT=<file>` writes every inspected site with its class.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};

/// How one allocation site's size expression is bounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Constant,
    InMemoryLength,
    CursorBounded,
    Unbounded,
}

#[derive(Debug)]
struct Site {
    file: String,
    line: usize,
    kind: &'static str,
    expr: String,
    class: Class,
    /// The file's production text (for a witness lookup).
    file_text: std::rc::Rc<String>,
}

/// One unbounded site that is allowed, and why. `file` is relative to the crate's `src/` (prefixed `prikk-object/` for that crate),
/// `expr` is the size expression with whitespace removed, `bound` says what bounds it, and `witness` is text that must appear in the
/// file's production code -- the bound itself, in the words the code uses -- so that removing the bound reddens the scan.
struct Allowed {
    file: &'static str,
    expr: &'static str,
    bound: &'static str,
    witness: &'static str,
}

const ALLOWED: &[Allowed] = &[
    Allowed {
        file: "foundation/fsutil/anchored/read.rs",
        expr: "len",
        bound: "the ranged reader clamps `len` to what the file holds from `offset` (its `fstat`/metadata size) before the buffer is made \
                (RFC 102 Addendum 1 item 1); the path-only reader reads through `take(min(len, available))`, which reserves nothing",
        witness: "let len = len.min(",
    },
    Allowed {
        file: "line_diff.rs",
        expr: "size",
        bound: "`Diagonals::new(max_d)`: `max_d` is `(n + m + 1) / 2` from the two in-memory line sequences being diffed (`middle_snake`), so the arrays are at most the size of the texts already in memory; nothing here is read from disk",
        witness: "let max = (n + m + 1) / 2;",
    },
    Allowed {
        file: "text_span.rs",
        expr: "text.len()-(end-start)+replacement.len()",
        bound: "the size of the spliced result: `text.len()` less the located range plus `replacement.len()`, both slices in memory, the range having just been checked against `text.len()`",
        witness: "end > text.len()",
    },
];

fn source_roots() -> Vec<(&'static str, PathBuf)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        ("", manifest.join("src")),
        ("prikk-object/", manifest.join("../prikk-object/src")),
    ]
}

/// Files that are not production code (test modules and fixtures); see the module doc.
fn is_test_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name == "tests.rs"
        || name.contains("test_support")
        || name.contains("_tests")
        || name.ends_with("_test.rs")
        || path.components().any(|component| {
            let part = component.as_os_str();
            part == "tests" || part == "test_gates" || part == "caller_tests"
        })
}

/// Blank out comments (so a `//` line naming `with_capacity(` is not a site) and inline `#[cfg(test)] mod x { ... }` blocks, keeping
/// line numbers.
fn production_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    // Pass 1: comments and string literals are kept, comments are blanked.
    let mut in_string = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            out.push(char::from(byte));
            if byte == b'\\' && index + 1 < bytes.len() {
                out.push(char::from(bytes[index + 1]));
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push('"');
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && bytes[index] != b'\n' {
                out.push(' ');
                index += 1;
            }
            continue;
        }
        out.push(char::from(byte));
        index += 1;
    }
    strip_inline_test_modules(&out)
}

/// Replace every `#[cfg(test)] ... mod name { ... }` block (balanced braces) with spaces, keeping newlines.
fn strip_inline_test_modules(text: &str) -> String {
    let mut result = text.to_string();
    let mut search_from = 0;
    while let Some(found) = result[search_from..].find("#[cfg(test)]") {
        let start = search_from + found;
        let rest = &result[start + "#[cfg(test)]".len()..];
        let after_attrs = rest.trim_start();
        let leading = rest.len() - after_attrs.len();
        let is_inline_mod = {
            // The first line that is not another attribute (`#[allow(..)]` may sit between `#[cfg(test)]` and `mod`).
            let line = after_attrs
                .lines()
                .map(str::trim_start)
                .find(|line| !line.starts_with("#["))
                .unwrap_or_default();
            let line = line
                .strip_prefix("pub(crate) ")
                .or_else(|| line.strip_prefix("pub(super) "))
                .or_else(|| line.strip_prefix("pub "))
                .unwrap_or(line);
            line.starts_with("mod ") && line.trim_end().ends_with('{')
        };
        if !is_inline_mod {
            search_from = start + "#[cfg(test)]".len();
            continue;
        }
        let open = start
            + "#[cfg(test)]".len()
            + leading
            + result[start + "#[cfg(test)]".len() + leading..]
                .find('{')
                .unwrap_or(0);
        let mut depth = 0_usize;
        let mut end = open;
        for (offset, ch) in result[open..].char_indices() {
            if ch == '{' {
                depth += 1;
            } else if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    end = open + offset + 1;
                    break;
                }
            }
        }
        let blanked: String = result[start..end]
            .chars()
            .map(|ch| if ch == '\n' { '\n' } else { ' ' })
            .collect();
        result.replace_range(start..end, &blanked);
        search_from = start + blanked.len();
    }
    result
}

/// The text between the balanced delimiters that open at `open` (which must be the opening `(` or `[`), and the index just past the close.
fn balanced(text: &str, open: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let (opener, closer) = match bytes.get(open)? {
        b'(' => (b'(', b')'),
        b'[' => (b'[', b']'),
        _ => return None,
    };
    let mut depth = 0_usize;
    for (offset, byte) in bytes[open..].iter().enumerate() {
        if *byte == opener {
            depth += 1;
        } else if *byte == closer {
            depth -= 1;
            if depth == 0 {
                return Some((&text[open + 1..open + offset], open + offset + 1));
            }
        }
    }
    None
}

/// Split at top-level `sep` (outside nested `()`, `[]`, `{}`).
fn split_top_level(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn is_screaming_constant(token: &str) -> bool {
    let last = token.rsplit("::").next().unwrap_or(token);
    !last.is_empty()
        && last.chars().any(|ch| ch.is_ascii_uppercase())
        && last
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
}

/// Classify a size expression (whitespace already removed).
fn classify(expr: &str) -> Class {
    if expr.contains(".bounded_capacity(") {
        return Class::CursorBounded;
    }
    // Any conversion of a value into a size is a decoded-or-unknown source.
    if expr.contains("asusize") || expr.contains("usize::") {
        return Class::Unbounded;
    }
    let mut saw_len_of_local = false;
    let mut index = 0;
    let bytes = expr.as_bytes();
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'*' | b'/' | b'(' | b')' | b'_') {
            index += 1;
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' || byte == b':' || byte == b'.' {
            // A path token (identifiers, `::`, `.` field access), up to an operator or call parenthesis.
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || matches!(bytes[index], b'_' | b':' | b'.'))
            {
                index += 1;
            }
            let token = &expr[start..index];
            let (receiver, method) = token.rsplit_once('.').unwrap_or((token, ""));
            if method == "len" && bytes.get(index) == Some(&b'(') {
                if !is_screaming_constant(receiver.rsplit('.').next().unwrap_or(receiver)) {
                    saw_len_of_local = true;
                }
                continue;
            }
            if method == "div_ceil" && bytes.get(index) == Some(&b'(') {
                continue;
            }
            if is_screaming_constant(token) {
                continue;
            }
            return Class::Unbounded;
        }
        // Anything else (a cast such as `as`, a comma, a brace) is not in the safe alphabet.
        return Class::Unbounded;
    }
    if saw_len_of_local {
        Class::InMemoryLength
    } else {
        Class::Constant
    }
}

/// Every allocation site of one production source text.
fn sites_in(file: &str, raw: &str) -> Vec<Site> {
    let text = production_text(raw);
    let file_text = std::rc::Rc::new(text.clone());
    let mut sites = Vec::new();
    let line_of = |index: usize| text[..index].bytes().filter(|byte| *byte == b'\n').count() + 1;
    let mut push = |index: usize, kind: &'static str, expr: &str| {
        let expr: String = expr.chars().filter(|ch| !ch.is_whitespace()).collect();
        let line = line_of(index);
        sites.push(Site {
            file: file.to_string(),
            line,
            kind,
            class: classify(&expr),
            expr,
            file_text: file_text.clone(),
        });
    };
    for (needle, kind) in [
        ("with_capacity(", "with_capacity"),
        (".reserve(", "reserve"),
        (".reserve_exact(", "reserve_exact"),
    ] {
        let mut from = 0;
        while let Some(found) = text[from..].find(needle) {
            let at = from + found;
            let open = at + needle.len() - 1;
            if let Some((inner, end)) = balanced(&text, open) {
                push(at, kind, inner);
                from = end;
            } else {
                from = at + needle.len();
            }
        }
    }
    let mut from = 0;
    while let Some(found) = text[from..].find(".resize(") {
        let at = from + found;
        let open = at + ".resize".len();
        if let Some((inner, end)) = balanced(&text, open) {
            let first = split_top_level(inner, ',')[0];
            push(at, "resize", first);
            from = end;
        } else {
            from = at + 1;
        }
    }
    let mut from = 0;
    while let Some(found) = text[from..].find("vec![") {
        let at = from + found;
        let open = at + "vec!".len();
        if let Some((inner, end)) = balanced(&text, open) {
            let parts = split_top_level(inner, ';');
            if parts.len() == 2 {
                push(at, "vec![_; n]", parts[1]);
            }
            from = end;
        } else {
            from = at + 1;
        }
    }
    sites
}

fn all_sites() -> Vec<Site> {
    let mut sites = Vec::new();
    for (prefix, root) in source_roots() {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs")
                    || is_test_file(&path)
                {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                sites.extend(sites_in(&format!("{prefix}{relative}"), &text));
            }
        }
    }
    sites.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
    sites
}

/// Sites that are unbounded and not covered by an allowlist row whose witness is present; and rows nothing matched.
fn offenders(sites: &[Site]) -> (Vec<String>, Vec<String>) {
    let mut bad = Vec::new();
    let mut used = vec![false; ALLOWED.len()];
    for site in sites.iter().filter(|site| site.class == Class::Unbounded) {
        let row = ALLOWED
            .iter()
            .position(|row| row.file == site.file && row.expr == site.expr);
        let Some(position) = row else {
            bad.push(format!(
                "{}:{}: `{}({})` is sized by something the scan cannot show is bounded. Use \
                 `ByteCursor::bounded_capacity`, size it from a collection that already exists, or -- if a bound is genuinely \
                 there -- list the site in `ALLOWED` with the bound and a witness",
                site.file, site.line, site.kind, site.expr
            ));
            continue;
        };
        used[position] = true;
        // One witness per site: a file with two allowlisted sites (the ranged reader has one per platform) must show the bound twice,
        // so removing one platform's clamp is not hidden by the other's.
        let allowed = &ALLOWED[position];
        let sites_of_the_row = sites
            .iter()
            .filter(|other| {
                other.class == Class::Unbounded
                    && other.file == allowed.file
                    && other.expr == allowed.expr
            })
            .count();
        let witnesses = site.file_text.matches(allowed.witness).count();
        if witnesses < sites_of_the_row {
            bad.push(format!(
                "{}:{}: `{}({})` is allowed for `{}`, but its witness `{}` appears {witnesses} time(s) in the file for {sites_of_the_row} allowed site(s): a bound is gone",
                site.file, site.line, site.kind, site.expr, allowed.bound, allowed.witness
            ));
        }
    }
    let stale = ALLOWED
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|(row, _)| {
            format!(
                "{} `{}`: listed, but the scan finds no such unbounded site",
                row.file, row.expr
            )
        })
        .collect();
    (bad, stale)
}

/// **The scan.** Every allocation site in the production sources of `prikk-store` and `prikk-object` is bounded, or is listed with the
/// bound and a witness.
/// **Perturb:** (a) remove the ranged reader's clamp (`let len = len.min(...)`) -- the allowlisted site's witness is gone and this goes
/// red; (b) put `Vec::with_capacity(count as usize)` back in `trust_index.rs` -- an unbounded site with no row: red.
#[test]
fn no_allocation_is_sized_by_a_decoded_length_without_a_bound() {
    let sites = all_sites();
    if let Ok(report) = std::env::var("PRIKK_ALLOC_SCAN_REPORT") {
        let mut lines = String::new();
        for site in &sites {
            lines.push_str(&format!(
                "{}\t{}\t{}\t{:?}\t{}\n",
                site.file, site.line, site.kind, site.class, site.expr
            ));
        }
        let _ = std::fs::write(report, lines);
    }
    let (bad, stale) = offenders(&sites);
    assert!(
        bad.is_empty(),
        "allocation sites sized by a length the scan cannot bound: {bad:#?}"
    );
    assert!(
        stale.is_empty(),
        "stale allocation allowlist rows: {stale:#?}"
    );
}

/// A scan that matches nothing proves nothing: the tree is known to hold well over a hundred allocation sites, and each class has
/// members. (The counts are floors, not a census: adding sites never fails this; the scan silently reading nothing does.)
#[test]
fn every_site_is_classified_and_the_scan_sees_a_realistic_number_of_them() {
    let sites = all_sites();
    let count = |class: Class| sites.iter().filter(|site| site.class == class).count();
    assert!(sites.len() >= 60, "found only {} sites", sites.len());
    assert!(
        count(Class::Constant) >= 10,
        "constant: {}",
        count(Class::Constant)
    );
    assert!(
        count(Class::InMemoryLength) >= 25,
        "in-memory length: {}",
        count(Class::InMemoryLength)
    );
    assert!(
        count(Class::CursorBounded) >= 1,
        "cursor-bounded: {}",
        count(Class::CursorBounded)
    );
    assert!(
        count(Class::Unbounded) >= 1,
        "unbounded (allowlisted): {}",
        count(Class::Unbounded)
    );
    // Both crates were read.
    assert!(
        sites
            .iter()
            .any(|site| site.file.starts_with("prikk-object/"))
    );
    assert!(sites.iter().any(|site| site.file == "trust_index.rs"));
}

/// The scanner is checked against known-bad and known-good text, so "no offenders" cannot come from a scanner that stopped matching.
#[test]
fn the_scanner_flags_decoded_sizes_and_passes_bounded_ones() {
    let bad = r#"
fn decode(cursor: &mut ByteCursor) {
    let count = cursor.read_u32()?;
    let a = Vec::with_capacity(count as usize);
    let b = vec![0_u8; len];
    let mut c = Vec::new();
    c.reserve(n);
    c.reserve_exact(usize::try_from(total)?);
    c.resize(size, 0);
}
"#;
    let found = sites_in("x.rs", bad);
    assert_eq!(found.len(), 5, "{found:#?}");
    assert!(
        found.iter().all(|site| site.class == Class::Unbounded),
        "{found:#?}"
    );

    let good = r#"
fn encode(body: &[u8], items: &[u8]) {
    let a = Vec::with_capacity(HEADER_LEN + body.len());
    let b = Vec::with_capacity(items.len() * 32);
    let c = Vec::with_capacity(3);
    let d = Vec::with_capacity(cursor.bounded_capacity(count as usize, 2));
    let e = vec![false; ids.len()];
    let f = vec![0_u8; 32];
    let g = vec![1, 2, 3];
}
"#;
    let found = sites_in("y.rs", good);
    assert_eq!(
        found.len(),
        6,
        "the list form vec![1, 2, 3] is not an allocation site: {found:#?}"
    );
    assert!(
        found.iter().all(|site| site.class != Class::Unbounded),
        "{found:#?}"
    );
    assert_eq!(
        found
            .iter()
            .filter(|site| site.class == Class::CursorBounded)
            .count(),
        1
    );

    // A site in a comment, and one inside an inline test module, are not production sites.
    let ignored = r#"
// Vec::with_capacity(count as usize)
fn production() {}
#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    fn helper() { let v = vec![0_u8; n]; let w = Vec::with_capacity(count as usize); }
}
fn after_the_test_module() { let x = Vec::with_capacity(count as usize); }
"#;
    let found = sites_in("z.rs", ignored);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].line, 9);
}

/// An allowlist row is worth what its witness is worth: a site with a row but no witness in its file is reported, and a row that no
/// site matches is reported as stale.
#[test]
fn an_allowed_site_whose_witness_is_gone_is_reported() {
    let file = ALLOWED[0].file;
    let expr = ALLOWED[0].expr;
    let with_witness = Site {
        file: file.to_string(),
        line: 10,
        kind: "vec![_; n]",
        expr: expr.to_string(),
        class: Class::Unbounded,
        file_text: std::rc::Rc::new(format!(
            "    {}available)\n    let bytes = vec![0; len];",
            ALLOWED[0].witness
        )),
    };
    let (bad, stale) = offenders(std::slice::from_ref(&with_witness));
    assert!(bad.is_empty(), "{bad:?}");
    // The other rows found no site in this one-site input, so they are reported stale: a listed row nothing matches is an error.
    assert_eq!(stale.len(), ALLOWED.len() - 1, "{stale:?}");
    let without_witness = Site {
        file_text: std::rc::Rc::new("    let bytes = vec![0; len];".to_string()),
        ..with_witness
    };
    let (bad, _) = offenders(std::slice::from_ref(&without_witness));
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].contains("witness"));

    // Two allowed sites in one file (one per platform's reader) but the bound shown once: reported, so one platform's clamp cannot
    // hide behind the other's.
    let twin = |line| Site {
        file: file.to_string(),
        line,
        kind: "vec![_; n]",
        expr: expr.to_string(),
        class: Class::Unbounded,
        file_text: std::rc::Rc::new(format!("    {}available)\n", ALLOWED[0].witness)),
    };
    let (bad, _) = offenders(&[twin(10), twin(20)]);
    assert_eq!(bad.len(), 2, "{bad:?}");
}
