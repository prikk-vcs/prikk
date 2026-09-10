//! RFC 130's production module graph, RFC 131 §6c's qualified-name amendment: a node is every
//! production module at every depth, keyed by its qualified path from the crate root
//! (`foundation::layout` is a node distinct from `foundation::fsutil`), and every `crate::` edge
//! between them (v2 handoff §3.5 items 3/4, amended).
//!
//! **A module's text is its own file's, not its descendants'.** Before this amendment every
//! descendant file's text was concatenated into its top-level ancestor's single node -- a node
//! could only be a top-level module, and a cycle wholly inside one could never be seen (RFC 131
//! §6a/§6c). Splitting nodes by depth is the whole point.
//!
//! **Edge extraction (item 3).** Every `crate::<ident>(::<ident>)*` occurrence in a production
//! file's text, in *any* position -- `use` statements and expression-position paths alike -- after
//! comments and string/char literals are blanked out so neither can produce a spurious match.
//! **RFC 131 §6c.1: the edge *vocabulary* does not change, only its resolution.** The captured
//! path resolves to the **deepest existing module** that is a prefix of it --
//! `crate::a::b::C` reaches node `a::b` when `a::b` is a module and node `a` when it is not; an
//! item name is never itself a node. If no prefix at all names a module, the path's own first
//! segment falls back to the pre-existing re-export table -- the single top-level module that
//! re-exports an item of that name from `lib.rs`'s own `pub use` block, unchanged from before this
//! amendment: the path this crate's own `patch_replay -> active` edge only exists through
//! (`active`'s `read_active_ref_metadata`/`ActiveRefMetadata` are `pub use` re-exports;
//! `patch_replay.rs` never writes `crate::active::` anywhere). Deliberately does **not** add
//! `super::`, `self::`, or bare-path scanning -- those are invisible to the gate today, and adding
//! them would change what an edge *means*, not how precisely it is named (RFC 131 §6c.1).
//!
//! **Grouped imports** (`use crate::{a, b, module::c};`) are expanded before the bare-`crate::`
//! scan runs, so each is resolved once rather than the group being treated as a single opaque
//! match, and each element now keeps its own full path (`module::c`, not just `module`) so the same
//! resolution applies to it. No nested grouping (`crate::{a::{b, c}, d}`) exists anywhere in this
//! crate today (re-checked directly against every `use crate::{` site as of this amendment) -- the
//! flat splitter below would mis-parse one if a future change ever added it, which is worth stating
//! rather than leaving implicit. A *prefixed* group (`crate::module::{a, b}`, e.g. this crate's own
//! `crate::text_span::{self, TextSpanResolutionFailure}`) is a different, simpler shape: it is not
//! expanded specially either before or after this amendment, since the plain scan already stops
//! right before the `{` and captures exactly the qualified prefix (`text_span`) as one occurrence --
//! correct today and unchanged by this round; extending capture *into* such a group's own elements
//! was not asked for and does not ride along.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use super::cfg_expr::{self, CfgExpr};

/// One classified byte span of source text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpanKind {
    Code,
    Comment,
    StringOrChar,
}

/// The byte at `i`, or `0` past the end -- `0` never appears in real Rust source (the workspace's
/// own `clippy::indexing_slicing = "deny"` means every scan in this file reads by index this way
/// instead of `bytes[i]`, and every reader here already gates on `i < bytes.len()` around its own
/// loop, so the `0` sentinel is never actually reached in practice; it exists only so a boundary
/// slip degrades to "no match" instead of a panic).
fn byte_at(bytes: &[u8], i: usize) -> u8 {
    bytes.get(i).copied().unwrap_or(0)
}

/// Classify every byte of `text` as code, a comment, or a string/char literal -- one shared scan
/// so blanking comments-only (for finding `mod`/`cfg` declarations, where a `"linux"` literal
/// inside a `cfg` attribute is meaningful) and blanking comments-and-strings (for the edge scan,
/// where a `crate::` substring inside a string or comment must never be mistaken for a real path)
/// can never disagree about where one span ends and the next begins.
fn classify(text: &str) -> Vec<SpanKind> {
    let bytes = text.as_bytes();
    let mut kinds = vec![SpanKind::Code; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        match byte_at(bytes, i) {
            b'/' if byte_at(bytes, i + 1) == b'/' => {
                let start = i;
                while i < bytes.len() && byte_at(bytes, i) != b'\n' {
                    i += 1;
                }
                if let Some(span) = kinds.get_mut(start..i) {
                    span.fill(SpanKind::Comment);
                }
            }
            b'/' if byte_at(bytes, i + 1) == b'*' => {
                let start = i;
                i += 2;
                let mut depth = 1_u32;
                while i < bytes.len() && depth > 0 {
                    if byte_at(bytes, i) == b'/' && byte_at(bytes, i + 1) == b'*' {
                        depth += 1;
                        i += 2;
                    } else if byte_at(bytes, i) == b'*' && byte_at(bytes, i + 1) == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if let Some(span) = kinds.get_mut(start..i) {
                    span.fill(SpanKind::Comment);
                }
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && byte_at(bytes, i) != b'"' {
                    if byte_at(bytes, i) == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                i = (i + 1).min(bytes.len());
                if let Some(span) = kinds.get_mut(start..i) {
                    span.fill(SpanKind::StringOrChar);
                }
            }
            b'\'' if is_char_literal_start(bytes, i) => {
                let start = i;
                i += 1;
                while i < bytes.len() && byte_at(bytes, i) != b'\'' {
                    if byte_at(bytes, i) == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                i = (i + 1).min(bytes.len());
                if let Some(span) = kinds.get_mut(start..i) {
                    span.fill(SpanKind::StringOrChar);
                }
            }
            _ => i += 1,
        }
    }
    kinds
}

/// Distinguishes a char literal (`'a'`, `'\\''`) from a lifetime (`'a`, `'static`) -- a lifetime's
/// closing quote never comes back, a char literal's does, within a short, defined lookahead.
fn is_char_literal_start(bytes: &[u8], quote_pos: usize) -> bool {
    let mut i = quote_pos + 1;
    if i >= bytes.len() {
        return false;
    }
    if byte_at(bytes, i) == b'\\' {
        i += 1; // escape sequence
        while i < bytes.len() && byte_at(bytes, i) != b'\'' {
            i += 1;
            if i - quote_pos > 8 {
                return false;
            }
        }
        return i < bytes.len() && byte_at(bytes, i) == b'\'';
    }
    // A single plain character followed immediately by a closing quote is a char literal;
    // anything else starting with an identifier character and no closing quote nearby is a
    // lifetime.
    i += 1;
    i < bytes.len() && byte_at(bytes, i) == b'\''
}

/// Blank matching spans to ASCII space, byte-for-byte -- **never** char-for-char. Replacing a
/// blanked multi-byte character with one single-byte space would shrink the string, silently
/// shifting every later byte offset out of alignment with the original text (this is exactly the
/// bug an em dash inside a string literal or comment exposed: a downstream byte offset, valid
/// against the *original* text, landed mid-character once a preceding multi-byte character had
/// been shrunk away). [`classify`] assigns one [`SpanKind`] per byte but always uniformly across a
/// whole character's bytes (comments and strings are delimited by single-byte ASCII markers, so a
/// multi-byte character is never split between two spans) -- so replacing every blanked byte with
/// `b' '` independently, leaving `\n` bytes alone, always yields valid UTF-8 of the exact same
/// byte length as the input.
fn blank(text: &str, kinds: &[SpanKind], blank_comments: bool, blank_strings: bool) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for (index, byte) in bytes.iter_mut().enumerate() {
        let kind = kinds.get(index).copied().unwrap_or(SpanKind::Code);
        let should_blank = matches!(
            (kind, blank_comments, blank_strings),
            (SpanKind::Comment, true, _) | (SpanKind::StringOrChar, _, true)
        );
        if should_blank && *byte != b'\n' {
            *byte = b' ';
        }
    }
    // Lossy, never panicking, even though blanking only ever replaces a byte with ASCII space --
    // which by construction always preserves valid UTF-8 -- because this workspace denies
    // `expect`/`unwrap` in production code even for invariants proven by construction.
    String::from_utf8_lossy(&bytes).into_owned()
}

/// One `mod` declaration found in a file's text.
struct ModDecl {
    name: String,
    cfg: Option<CfgExpr>,
    /// `Some((open, close))` byte offsets (into the comment-blanked text this was found in) of an
    /// inline `mod name { ... }` block's braces; `None` for a file-based `mod name;`.
    inline_block: Option<(usize, usize)>,
}

/// Find every top-level `mod name;` / `mod name { ... }` declaration in `text`, paired with the
/// nearest preceding `#[cfg(...)]` attribute (other attributes, such as `#[allow(...)]`, are
/// tolerated between the two -- `text_span/authoring.rs`'s own `#[cfg(test)] #[allow(...)] mod
/// uniqueness_stress_tests` is exactly this shape). Operates on comment-blanked text so a `mod`
/// mentioned only in a comment is never matched; string literals are left intact since a `cfg`
/// attribute's own `"linux"` is meaningful, not noise.
fn find_mod_declarations(comment_blanked: &str) -> Vec<ModDecl> {
    let bytes = comment_blanked.as_bytes();
    let mut decls = Vec::new();
    let mut pending_cfg: Option<CfgExpr> = None;
    let mut i = 0;
    while i < bytes.len() {
        if byte_at(bytes, i).is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if byte_at(bytes, i) == b'#' && byte_at(bytes, i + 1) == b'[' {
            let attr_start = i + 2;
            let Some(attr_end) = find_matching_bracket(bytes, attr_start) else {
                break;
            };
            // `attr_start`/`attr_end` are the byte positions of `#[`'s `[` plus two, and of the
            // matching `]` -- both ASCII delimiters, so both are always real char boundaries; the
            // `unwrap_or_default` only guards a shape this scan cannot actually produce.
            let attr_text = comment_blanked
                .get(attr_start..attr_end)
                .unwrap_or_default();
            if let Some(inner) = attr_text.trim().strip_prefix("cfg(") {
                let inner = inner.strip_suffix(')').unwrap_or(inner);
                pending_cfg = cfg_expr::parse(inner);
            }
            i = attr_end + 1;
            continue;
        }
        // A visibility qualifier between an attribute and its item does NOT end the attribute's
        // reach: `#[cfg(test)]\npub(crate) mod tests;` is one item, not an attribute stranded
        // before an unrelated one. Skipping it here rather than falling through to the reset below
        // is the whole fix -- without it the `cfg` was dropped and the module's `tests/` subtree
        // was aggregated as production text. Found 2026-09-08 by an implementing round whose new
        // test-only import manufactured a spurious production edge (`patch_replay -> block_state`)
        // and failed `boundary-check`. `pub` alone, `pub(crate)`, `pub(super)` and `pub(in ...)`
        // all take this path; the parenthesised form is skipped to its matching `)`.
        if bytes.get(i..).is_some_and(|rest| rest.starts_with(b"pub")) {
            let mut j = i + 3;
            if byte_at(bytes, j) == b'(' {
                match find_matching_paren(bytes, j + 1) {
                    Some(close) => j = close + 1,
                    None => break,
                }
            }
            if byte_at(bytes, j).is_ascii_whitespace() {
                i = j;
                continue;
            }
        }
        // Byte-slice comparison, never a `str` slice: a byte slice is always memory-safe to read
        // regardless of whether `i` sits on a UTF-8 character boundary (only slicing the
        // underlying `&str` at a non-boundary panics, e.g. when scanning has walked byte-by-byte
        // through a multi-byte character inside an ordinary, non-comment string literal such as
        // an em dash in an error message).
        if bytes.get(i..).is_some_and(|rest| rest.starts_with(b"mod ")) {
            let name_start = i + 4;
            let mut name_end = name_start;
            while name_end < bytes.len()
                && (byte_at(bytes, name_end).is_ascii_alphanumeric()
                    || byte_at(bytes, name_end) == b'_')
            {
                name_end += 1;
            }
            // `name_start..name_end` spans only ASCII identifier bytes (or is empty), so this is
            // always valid UTF-8 and always a real char-boundary range.
            let name = bytes
                .get(name_start..name_end)
                .and_then(|slice| std::str::from_utf8(slice).ok())
                .unwrap_or_default()
                .to_owned();
            if !name.is_empty() {
                let mut j = name_end;
                while j < bytes.len() && byte_at(bytes, j).is_ascii_whitespace() {
                    j += 1;
                }
                let inline_block = if byte_at(bytes, j) == b'{' {
                    find_matching_brace(bytes, j).map(|close| (j, close))
                } else {
                    None
                };
                decls.push(ModDecl {
                    name,
                    cfg: pending_cfg.take(),
                    inline_block,
                });
                i = inline_block.map_or(name_end, |(_, close)| close + 1);
                pending_cfg = None;
                continue;
            }
        }
        // Any other real code token resets a pending cfg -- it was meant for whatever followed it,
        // not for a `mod` several items later.
        pending_cfg = None;
        i += 1;
    }
    decls
}

fn find_matching_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1_u32;
    let mut i = open;
    while i < bytes.len() {
        match byte_at(bytes, i) {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Matching `)` for a visibility qualifier's own parenthesis (`pub(crate)`, `pub(in crate::foo)`).
/// Mirrors [`find_matching_bracket`]; separate because a visibility qualifier is the one place this
/// scanner must step over a parenthesised group rather than treat it as a code token.
fn find_matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1_u32;
    let mut i = open;
    while i < bytes.len() {
        match byte_at(bytes, i) {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1_u32;
    let mut i = open + 1;
    while i < bytes.len() {
        match byte_at(bytes, i) {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// A production module's own source text, ready for the edge scan: comments and strings blanked,
/// and any test-only inline `mod name { ... }` block's body blanked too (its own file-based
/// counterpart is simply never read at all -- this is only needed for the inline form, which
/// shares a file with production code).
fn production_edge_text(raw: &str) -> String {
    let kinds = classify(raw);
    let comment_blanked = blank(raw, &kinds, true, false);
    let decls = find_mod_declarations(&comment_blanked);
    // `open`/`close` are byte offsets into `comment_blanked`, which [`blank`] keeps byte-aligned
    // with `raw` -- so blanking directly on `raw`'s own bytes (never `char`s, which are indexed by
    // character position, not byte position) is the only correct way to excise the span.
    let mut bytes = raw.as_bytes().to_vec();
    for decl in &decls {
        if let Some((open, close)) = decl.inline_block {
            if !cfg_expr::is_possibly_production(decl.cfg.as_ref()) {
                for byte in bytes.iter_mut().take(close + 1).skip(open) {
                    if *byte != b'\n' {
                        *byte = b' ';
                    }
                }
            }
        }
    }
    // Lossy, never panicking -- see `blank`'s own doc for why replacing bytes with ASCII space
    // always preserves valid UTF-8 by construction, and why this workspace still avoids `expect`.
    let with_inline_blanked = String::from_utf8_lossy(&bytes).into_owned();
    let kinds = classify(&with_inline_blanked);
    blank(&with_inline_blanked, &kinds, true, true)
}

/// The production module tree, walked from `lib.rs`, keyed by qualified path from the crate root
/// (RFC 131 §6c) -- `foundation::layout` is a distinct key from `foundation`, each holding only its
/// own file's edge-scan text. `mod.rs`-style files and `#[path = "..."]` overrides are not resolved
/// (neither is used anywhere in `prikk-store` today, confirmed directly) -- every child of a file
/// `x.rs` resolves to `x/<name>.rs`, and every top-level child of the crate root resolves directly
/// under `src/`.
fn walk(src_root: &Path) -> Result<BTreeMap<String, String>, String> {
    let lib_rs = src_root.join("lib.rs");
    let raw = fs::read_to_string(&lib_rs)
        .map_err(|error| format!("read {}: {error}", lib_rs.display()))?;
    let kinds = classify(&raw);
    let comment_blanked = blank(&raw, &kinds, true, false);
    let mut modules = BTreeMap::new();
    for decl in find_mod_declarations(&comment_blanked) {
        if decl.inline_block.is_some() {
            return Err(format!(
                "lib.rs declares inline module `{}` -- the walker assumes every top-level module \
                 is file-based",
                decl.name
            ));
        }
        if !cfg_expr::is_possibly_production(decl.cfg.as_ref()) {
            continue;
        }
        let file = src_root.join(format!("{}.rs", decl.name));
        collect_production_modules(&file, decl.name.clone(), &mut modules)?;
    }
    Ok(modules)
}

/// Recursively insert one node per production file reachable from `file`, each keyed by its own
/// qualified path (`qualified_name`) and holding only its own edge-scan text -- never a
/// descendant's (RFC 131 §6c: "a module's text is its own file's, not its descendants'"). Walks
/// further `mod` declarations (file-based only -- see [`walk`]'s doc) relative to `file`'s own
/// stem-named sibling directory, extending `qualified_name` with `::<child>` at each step.
fn collect_production_modules(
    file: &Path,
    qualified_name: String,
    modules: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let raw =
        fs::read_to_string(file).map_err(|error| format!("read {}: {error}", file.display()))?;
    modules.insert(qualified_name.clone(), production_edge_text(&raw));
    let kinds = classify(&raw);
    let comment_blanked = blank(&raw, &kinds, true, false);
    let children_dir = file
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", file.display()))?
        .join(
            file.file_stem()
                .ok_or_else(|| format!("{} has no stem", file.display()))?,
        );
    for decl in find_mod_declarations(&comment_blanked) {
        if decl.inline_block.is_some() {
            // A production inline `mod name { ... }` block's own text stays part of its parent
            // file's text (via `production_edge_text`, which only excises a *test-only* inline
            // block) rather than becoming its own node -- unchanged from before this amendment.
            // No such block exists in `prikk-store` today (checked directly), so this is a
            // documented, deliberate non-goal rather than an untested path.
            continue;
        }
        if !cfg_expr::is_possibly_production(decl.cfg.as_ref()) {
            continue;
        }
        let child_file = children_dir.join(format!("{}.rs", decl.name));
        let child_qualified = format!("{qualified_name}::{}", decl.name);
        collect_production_modules(&child_file, child_qualified, modules)?;
    }
    Ok(())
}

/// `lib.rs`'s own `pub use <module>::{A, B, ...};` / `pub use <module>::Item;` re-export table:
/// item name -> the qualified module that re-exports it. An item re-exported from more than one
/// module is dropped from the map entirely (never resolved), rather than guessed -- correctness
/// here means "no edge" is always safer than "the wrong edge."
///
/// **The module path is captured in full** (`scan_qualified_path`, the same shared scanner
/// `crate_idents`/`extract_grouped_idents` use), not just its first segment -- found necessary at
/// RFC 131 §6d.2 when `active` grouped under `commit_boundary` and `pub use
/// commit_boundary::active::{...}` first exercised a multi-segment module path here. A
/// first-segment-only capture would have registered `"commit_boundary"` (the group, wrong) as the
/// owner of every re-exported item, or -- for the `{`-group case specifically, since a bare
/// first-segment capture leaves `after_module` starting with the *second* segment rather than
/// `{`, missing the group delimiter check entirely -- registered a spurious item named after that
/// second segment instead (`"active"`) and silently dropped every real item name. Latent since RFC
/// 131 §2.2a's own `author::author_signing`/`lifecycle_cache::incremental`/`foundation::layout`
/// re-exports (already two segments) but never manifested as a wrong edge because nothing
/// referenced those particular re-exported names via a bare `crate::<name>` path -- this fixes
/// them too, not only the new grouping.
fn reexports(src_root: &Path) -> Result<BTreeMap<String, String>, String> {
    let lib_rs = src_root.join("lib.rs");
    let raw = fs::read_to_string(&lib_rs)
        .map_err(|error| format!("read {}: {error}", lib_rs.display()))?;
    let kinds = classify(&raw);
    let text = blank(&raw, &kinds, true, true);
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut rest = text.as_str();
    while let Some(rel) = rest.find("pub use ") {
        rest = &rest[rel + "pub use ".len()..];
        let module_len = scan_qualified_path(rest.as_bytes());
        if module_len == 0 {
            break;
        }
        let module = rest[..module_len].to_owned();
        let after_module = rest[module_len..].trim_start();
        let after_module = after_module.strip_prefix("::").unwrap_or(after_module);
        if let Some(group) = after_module.strip_prefix('{') {
            let Some(close) = group.find('}') else { break };
            for item in group[..close].split(',') {
                let item = item.trim();
                let name = item.rsplit("::").next().unwrap_or(item).trim();
                if !name.is_empty() {
                    owners
                        .entry(name.to_owned())
                        .or_default()
                        .insert(module.clone());
                }
            }
        } else {
            let item_len = after_module
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(after_module.len());
            let name = after_module[..item_len].trim();
            if !name.is_empty() {
                owners
                    .entry(name.to_owned())
                    .or_default()
                    .insert(module.clone());
            }
        }
        rest = after_module;
    }
    Ok(owners
        .into_iter()
        .filter_map(|(name, modules)| {
            if modules.len() == 1 {
                modules.into_iter().next().map(|module| (name, module))
            } else {
                None
            }
        })
        .collect())
}

/// The end offset (exclusive) of the `ident(::ident)*` qualified path starting at byte `0` of
/// `bytes` -- shared by the plain `crate::<path>` scan and the grouped-import expander below so a
/// path never resolves differently depending on which one found it (RFC 131 §6c). Stops before a
/// `::` that is not immediately followed by another identifier -- `crate::foo::{...}` and
/// `crate::foo::*` both stop at `foo`, leaving the `::` and everything after it for the caller to
/// treat as it already did (a group boundary, a glob, or simply the end of a bare reference).
fn scan_qualified_path(bytes: &[u8]) -> usize {
    let mut i = 0;
    while i < bytes.len()
        && (byte_at(bytes, i).is_ascii_alphanumeric() || byte_at(bytes, i) == b'_')
    {
        i += 1;
    }
    if i == 0 {
        return 0;
    }
    loop {
        if byte_at(bytes, i) != b':' || byte_at(bytes, i + 1) != b':' {
            return i;
        }
        let segment_start = i + 2;
        let mut j = segment_start;
        while j < bytes.len()
            && (byte_at(bytes, j).is_ascii_alphanumeric() || byte_at(bytes, j) == b'_')
        {
            j += 1;
        }
        if j == segment_start {
            return i; // `::` not followed by an identifier -- stop before it, not after
        }
        i = j;
    }
}

/// Expand grouped `crate::{a, b, module::c}` imports into their individual elements' own full
/// qualified paths (`a`, `b`, `module::c` -- not first-segment-only), then mask the original group
/// text out of `text` so the plain `crate::<path>` scan below never double-counts it.
fn extract_grouped_idents(text: &mut String) -> Vec<String> {
    let mut idents = Vec::new();
    while let Some(rel) = text.find("crate::{") {
        let group_start = rel + "crate::".len();
        let bytes = text.as_bytes();
        let Some(close) = find_matching_brace(bytes, group_start) else {
            break;
        };
        let group_text = text[group_start + 1..close].to_owned();
        for item in group_text.split(',') {
            let item = item.trim();
            let path_len = scan_qualified_path(item.as_bytes());
            let path = &item[..path_len];
            if !path.is_empty() {
                idents.push(path.to_owned());
            }
        }
        let span_len = close + 1 - rel;
        text.replace_range(rel..rel + span_len, &" ".repeat(span_len));
    }
    idents
}

/// Every `crate::<path>` occurrence's full `::`-separated path (e.g. `crate::a::b::C` yields
/// `"a::b::C"`), from grouped imports and plain paths alike -- left for the caller to resolve
/// against known module nodes (RFC 131 §6c: "an item name is not a node").
fn crate_idents(edge_text: &str) -> Vec<String> {
    let mut text = edge_text.to_owned();
    let mut idents = extract_grouped_idents(&mut text);
    let mut rest = text.as_str();
    while let Some(rel) = rest.find("crate::") {
        rest = &rest[rel + "crate::".len()..];
        let path_len = scan_qualified_path(rest.as_bytes());
        let path = &rest[..path_len];
        if !path.is_empty() {
            idents.push(path.to_owned());
        }
        rest = &rest[path_len..];
    }
    idents
}

/// Whether `candidate` is `node` itself or a qualified-path ancestor of it (RFC 131 §6c.4 rule 4:
/// an ancestor/descendant relationship is never coupling). `"refs"` is an ancestor-or-self of
/// `"refs::evidence"` and of `"refs"` itself; it is not an ancestor of `"refs_other"` (a plain
/// string-prefix check would wrongly say otherwise without the `::` boundary).
fn is_ancestor_or_self(candidate: &str, node: &str) -> bool {
    node == candidate || node.starts_with(&format!("{candidate}::"))
}

/// The production module coupling graph: distinct (from, to) module pairs, self-loops excluded.
#[derive(Debug, Clone)]
pub(crate) struct ModuleGraph {
    pub(crate) modules: BTreeSet<String>,
    pub(crate) edges: BTreeSet<(String, String)>,
}

impl ModuleGraph {
    pub(crate) fn fan_in(&self, module: &str) -> usize {
        self.edges.iter().filter(|(_, to)| to == module).count()
    }

    pub(crate) fn fan_out(&self, module: &str) -> usize {
        self.edges.iter().filter(|(from, _)| from == module).count()
    }

    /// RFC 131 §6c.4 rule 2: whether *any* descendant-or-self of `from` has a raw edge to *any*
    /// descendant-or-self of `to` -- the question the pre-`42bcab15` gate answered correctly at
    /// fixed top-level granularity by concatenating descendant text, restored here without that
    /// fixed granularity. Rule 4: an edge between a node and its own ancestor or descendant is not
    /// coupling, so `from`/`to` in an ancestor-or-descendant relationship never depend on each
    /// other by this definition, regardless of what raw edges exist between their subtrees.
    pub(crate) fn subtree_depends(&self, from: &str, to: &str) -> bool {
        if is_ancestor_or_self(from, to) || is_ancestor_or_self(to, from) {
            return false;
        }
        let from_prefix = format!("{from}::");
        let to_prefix = format!("{to}::");
        self.edges.iter().any(|(a, b)| {
            (a == from || a.starts_with(&from_prefix)) && (b == to || b.starts_with(&to_prefix))
        })
    }

    /// Direct children of `node` among this graph's own known modules (one more `::`-segment than
    /// `node`, no further).
    fn children_of(&self, node: &str) -> Vec<&str> {
        let prefix = format!("{node}::");
        self.modules
            .iter()
            .filter(|module| {
                module
                    .strip_prefix(&prefix)
                    .is_some_and(|rest| !rest.contains("::"))
            })
            .map(String::as_str)
            .collect()
    }

    /// RFC 131 §6c.5: strongly-connected components over the [`Self::subtree_depends`] relation
    /// across every module in this graph -- generalizes §6c.4's pairwise rule 2 to a cycle that
    /// closes through any number of intermediate nodes, not just a direct mutual pair. Returned
    /// components have two or more members (Tarjan's own `len() >= 2` filter; `subtree_depends`
    /// never produces a self-loop, since rule 4 already excludes a node depending on itself).
    fn subtree_depends_components(&self) -> Vec<BTreeSet<String>> {
        let nodes: Vec<&str> = self.modules.iter().map(String::as_str).collect();
        let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for &a in &nodes {
            for &b in &nodes {
                if a != b && self.subtree_depends(a, b) {
                    successors.entry(a).or_default().push(b);
                }
            }
        }
        tarjan_scc(&self.modules, &successors)
            .into_iter()
            .map(|component| component.into_iter().map(str::to_owned).collect())
            .collect()
    }

    /// Whether `nodes`, considered alone (edges to/from anything outside `nodes` ignored), forms
    /// exactly one strongly-connected component under `subtree_depends` covering all of `nodes` --
    /// the test [`Self::minimize_component`] uses to check whether swapping one member for one of
    /// its own children still holds the whole set together.
    fn is_one_subtree_scc(&self, nodes: &BTreeSet<String>) -> bool {
        let node_refs: Vec<&str> = nodes.iter().map(String::as_str).collect();
        let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for &a in &node_refs {
            for &b in &node_refs {
                if a != b && self.subtree_depends(a, b) {
                    successors.entry(a).or_default().push(b);
                }
            }
        }
        let components = tarjan_scc(nodes, &successors);
        components.len() == 1
            && components
                .first()
                .is_some_and(|only| only.len() == nodes.len())
    }

    /// RFC 131 §6c.5 rule 3, generalized from pairs to N members: repeatedly replace any member
    /// with one of its own children when doing so still holds the *whole* set together as one SCC
    /// ([`Self::is_one_subtree_scc`]) -- "a member replaceable by one of its own children while the
    /// rest of the component is held fixed is not minimal," checked directly rather than assumed.
    /// Iterates to a fixed point in a fixed, deterministic order (`BTreeSet`/`children_of` are
    /// already sorted): unlike the N=2 pairwise case -- provably order-independent because
    /// checking both sides at once has no search order at all -- this N-member reduction is a
    /// greedy narrowing, and global uniqueness of the minimal set is not proven for N>2. What is
    /// guaranteed is the fixed point itself: on return, no member can be narrowed to a child
    /// without breaking the set's own SCC property, which is exactly rule 3's stated condition.
    fn minimize_component(&self, component: BTreeSet<String>) -> BTreeSet<String> {
        let mut current = component;
        loop {
            let mut narrowed = None;
            'search: for member in &current {
                for child in self.children_of(member) {
                    let mut candidate = current.clone();
                    candidate.remove(member);
                    candidate.insert(child.to_owned());
                    if self.is_one_subtree_scc(&candidate) {
                        narrowed = Some(candidate);
                        break 'search;
                    }
                }
            }
            match narrowed {
                Some(next) => current = next,
                None => return current,
            }
        }
    }

    /// RFC 131 §6c.4/§6c.5's subtree-aware cycle detection: every strongly-connected component of
    /// the `subtree_depends` relation, each reduced to its minimal member set (rule 3), reported as
    /// the *full* induced edge set among those minimal members -- every pair with `subtree_depends`
    /// true between them, both directions where mutual, directly comparable to `DECLARED_CYCLES`'s
    /// own shape (which itself has always meant "every edge found inside the SCC," not a minimal
    /// cycle cover -- the pre-`42bcab15` raw-node check read the same way: every raw edge with both
    /// endpoints in the same component). A 2-member component reduces to exactly the pairwise
    /// `74e6edc2` result -- see `subtree_control2_the_four_current_pairs_still_report_identically`.
    pub(crate) fn subtree_cycles(&self) -> Vec<(String, String)> {
        let mut reported = Vec::new();
        for component in self.subtree_depends_components() {
            let minimized = self.minimize_component(component);
            let members: Vec<&String> = minimized.iter().collect();
            for &a in &members {
                for &b in &members {
                    if a != b && self.subtree_depends(a, b) {
                        reported.push((a.clone(), b.clone()));
                    }
                }
            }
        }
        reported
    }

    /// Every elementary (simple, node-disjoint-except-for-the-repeated-start) cycle in the graph,
    /// each canonicalised to start at its own lexicographically smallest member so it is reported
    /// exactly once regardless of which node a search happens to begin from. Restricted to one
    /// strongly-connected component at a time (via [`tarjan_scc`]) so the search space is always
    /// just the offending cluster, never the whole graph.
    ///
    /// `#[cfg(test)]`, not merely `pub(crate)`: RFC 131 §6c.4 moved `check()`'s own cycle
    /// comparison from raw-node SCCs to [`Self::subtree_cycles`] -- this and
    /// [`strongly_connected_components`] are no longer called by production code, only by tests
    /// studying facts about the *raw* graph directly (e.g. that the pre-`42bcab15` six-module SCC
    /// no longer forms a multi-member component at all). Kept, not deleted: still a real, tested
    /// capability, just not a production call site any more -- the same reasoning already applied
    /// to `production_edge_text_for_tests` and its siblings.
    #[cfg(test)]
    pub(crate) fn elementary_cycles(&self) -> Vec<Vec<String>> {
        let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (from, to) in &self.edges {
            successors
                .entry(from.as_str())
                .or_default()
                .push(to.as_str());
        }
        let mut cycles = Vec::new();
        for component in tarjan_scc(&self.modules, &successors) {
            if component.len() < 2 {
                continue;
            }
            let component_set: BTreeSet<&str> = component.iter().copied().collect();
            let mut remaining: BTreeSet<&str> = component_set.clone();
            while let Some(&start) = remaining.iter().next() {
                find_cycles_through(start, &remaining, &successors, &mut cycles);
                remaining.remove(start);
            }
        }
        cycles
    }
}

#[cfg(test)]
fn find_cycles_through<'a>(
    start: &'a str,
    remaining: &BTreeSet<&'a str>,
    successors: &BTreeMap<&'a str, Vec<&'a str>>,
    cycles: &mut Vec<Vec<String>>,
) {
    let mut path = vec![start];
    let mut visited: BTreeSet<&str> = [start].into_iter().collect();
    search(
        start,
        start,
        remaining,
        successors,
        &mut path,
        &mut visited,
        cycles,
    );
}

#[cfg(test)]
fn search<'a>(
    start: &'a str,
    current: &'a str,
    remaining: &BTreeSet<&'a str>,
    successors: &BTreeMap<&'a str, Vec<&'a str>>,
    path: &mut Vec<&'a str>,
    visited: &mut BTreeSet<&'a str>,
    cycles: &mut Vec<Vec<String>>,
) {
    let Some(next_nodes) = successors.get(current) else {
        return;
    };
    for &next in next_nodes {
        if !remaining.contains(next) {
            continue;
        }
        if next == start {
            cycles.push(path.iter().map(|node| (*node).to_owned()).collect());
        } else if visited.insert(next) {
            path.push(next);
            search(start, next, remaining, successors, path, visited, cycles);
            path.pop();
            visited.remove(next);
        }
    }
}

/// Tarjan's strongly-connected-components algorithm, iterative-free (this crate's graphs are far
/// too small to need it) -- returns every component with two or more members or a self-loop;
/// callers only care about components that can contain a cycle. RFC 131 §6c.5: a production call
/// site again (`ModuleGraph::subtree_depends_components`/`is_one_subtree_scc`), not test-only --
/// unlike `elementary_cycles`/`strongly_connected_components`/`search`/`find_cycles_through`
/// below, which remain `#[cfg(test)]` (still no production caller of their own).
fn tarjan_scc<'a>(
    nodes: &'a BTreeSet<String>,
    successors: &BTreeMap<&'a str, Vec<&'a str>>,
) -> Vec<Vec<&'a str>> {
    struct State<'a> {
        index: BTreeMap<&'a str, usize>,
        low_link: BTreeMap<&'a str, usize>,
        on_stack: BTreeSet<&'a str>,
        stack: Vec<&'a str>,
        next_index: usize,
        components: Vec<Vec<&'a str>>,
    }
    fn strong_connect<'a>(
        node: &'a str,
        successors: &BTreeMap<&'a str, Vec<&'a str>>,
        state: &mut State<'a>,
    ) {
        state.index.insert(node, state.next_index);
        state.low_link.insert(node, state.next_index);
        state.next_index += 1;
        state.stack.push(node);
        state.on_stack.insert(node);

        if let Some(next_nodes) = successors.get(node) {
            for &next in next_nodes {
                if !state.index.contains_key(next) {
                    strong_connect(next, successors, state);
                    if let (Some(&node_low), Some(&next_low)) =
                        (state.low_link.get(node), state.low_link.get(next))
                    {
                        state.low_link.insert(node, node_low.min(next_low));
                    }
                } else if state.on_stack.contains(next) {
                    if let (Some(&node_low), Some(&next_index)) =
                        (state.low_link.get(node), state.index.get(next))
                    {
                        state.low_link.insert(node, node_low.min(next_index));
                    }
                }
            }
        }

        let is_root = matches!(
            (state.low_link.get(node), state.index.get(node)),
            (Some(low), Some(index)) if low == index
        );
        if is_root {
            let mut component = Vec::new();
            while let Some(member) = state.stack.pop() {
                state.on_stack.remove(member);
                component.push(member);
                if member == node {
                    break;
                }
            }
            state.components.push(component);
        }
    }

    let mut state = State {
        index: BTreeMap::new(),
        low_link: BTreeMap::new(),
        on_stack: BTreeSet::new(),
        stack: Vec::new(),
        next_index: 0,
        components: Vec::new(),
    };
    for node in nodes {
        if !state.index.contains_key(node.as_str()) {
            strong_connect(node.as_str(), successors, &mut state);
        }
    }
    state
        .components
        .into_iter()
        .filter(|component| {
            component.len() > 1
                || component.first().is_some_and(|&node| {
                    successors
                        .get(node)
                        .is_some_and(|targets| targets.contains(&node))
                })
        })
        .collect()
}

/// Owned-`String` wrapper over [`tarjan_scc`], for tests that need components outliving the
/// graph's borrow. RFC 131 §6c.4: no longer a production call site (`check()` uses
/// [`ModuleGraph::subtree_cycles`]) -- see that method's own doc for why this stays `#[cfg(test)]`
/// rather than deleted.
#[cfg(test)]
pub(crate) fn strongly_connected_components(graph: &ModuleGraph) -> Vec<Vec<String>> {
    let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in &graph.edges {
        successors
            .entry(from.as_str())
            .or_default()
            .push(to.as_str());
    }
    tarjan_scc(&graph.modules, &successors)
        .into_iter()
        .map(|component| component.into_iter().map(str::to_owned).collect())
        .collect()
}

/// Resolve one captured `crate::` path (`path`, e.g. `"a::b::C"`) to the node it denotes: the
/// longest prefix of its own `::`-separated segments that names a real module wins (RFC 131 §6c --
/// `crate::a::b::C` reaches `a::b` when `a::b` is a module, `a` when it is not; an item name is
/// never itself a node). Every qualified module key's own ancestor chain is always present in
/// `modules` too (`collect_production_modules` inserts a parent before ever recursing into a
/// child), so if no prefix at all matches, not even the first segment does either -- exactly the
/// pre-existing "not a module" case, which falls back to the re-export table keyed by that first
/// segment (the reexported item's own name), regardless of how many further segments follow it (an
/// associated item, variant, or method access -- never itself a module). Unchanged from before this
/// amendment.
fn resolve_target(
    path: &str,
    modules: &BTreeSet<String>,
    owners: &BTreeMap<String, String>,
) -> Option<String> {
    let segments: Vec<&str> = path.split("::").collect();
    for end in (1..=segments.len()).rev() {
        let Some(prefix) = segments.get(..end) else {
            continue;
        };
        let candidate = prefix.join("::");
        if modules.contains(&candidate) {
            return Some(candidate);
        }
    }
    segments
        .first()
        .and_then(|first| owners.get(*first).cloned())
}

/// Build the production module coupling graph for `prikk-store`. `src_root` is that crate's
/// `src/` directory.
pub(crate) fn build(src_root: &Path) -> Result<ModuleGraph, String> {
    let module_texts = walk(src_root)?;
    let owners = reexports(src_root)?;
    let modules: BTreeSet<String> = module_texts.keys().cloned().collect();
    let mut edges = BTreeSet::new();
    for (module, text) in &module_texts {
        for path in crate_idents(text) {
            if let Some(target) = resolve_target(&path, &modules, &owners) {
                if &target != module {
                    edges.insert((module.clone(), target));
                }
            }
        }
    }
    Ok(ModuleGraph { modules, edges })
}

/// Test-only accessors to otherwise-private scan steps, so `graph::tests` can assert on each stage
/// (comment/string stripping, inline-block excision, re-export resolution, module discovery)
/// independently of the assembled [`build`] result. `#[cfg(test)]`, not merely `pub(crate)`: the
/// non-test compilation of this binary (`cargo clippy --all-targets`'s own non-test target) never
/// calls these, and this workspace denies warnings, so an always-`pub(crate)` helper used only
/// under `#[cfg(test)]` would fail that other compilation as dead code.
#[cfg(test)]
pub(crate) fn production_edge_text_for_tests(raw: &str) -> String {
    production_edge_text(raw)
}

#[cfg(test)]
pub(crate) fn reexports_for_tests(src_root: &Path) -> Result<BTreeMap<String, String>, String> {
    reexports(src_root)
}

#[cfg(test)]
pub(crate) fn walk_root_for_tests(src_root: &Path) -> Result<BTreeSet<String>, String> {
    Ok(walk(src_root)?.into_keys().collect())
}

#[cfg(test)]
pub(crate) fn crate_idents_for_tests(edge_text: &str) -> Vec<String> {
    crate_idents(edge_text)
}

#[cfg(test)]
pub(crate) fn resolve_target_for_tests(
    path: &str,
    modules: &BTreeSet<String>,
    owners: &BTreeMap<String, String>,
) -> Option<String> {
    resolve_target(path, modules, owners)
}

#[cfg(test)]
#[path = "graph/tests.rs"]
mod tests;
