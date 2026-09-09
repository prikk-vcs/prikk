//! RFC 118 stage 2 — the join gate (`rfcs/accepted/118-derive-never-transcribe.md` §8).
//!
//! **Ruled: no serialization boundary.** This is a `#[test]` in `prikk-cli` reading
//! [`super::COMMANDS`] directly (the registry is already Rust; the documents are text on disk) —
//! not a `release-policy` check, not an emitted inventory, no build artifact, no parser, no new
//! dependency. The precedent is Gate A (RFC 114's completeness guard), also a `#[test]` colocated
//! with the thing it guards.
//!
//! Two rules (RFC 118 §8):
//! - **(A)** every documented `prikk <command>` in a declared document names a real registry entry.
//! - **(B)** every registry entry is explained somewhere, or declared undocumented with a reason.
//!
//! **What "explained" means, decided and stated (§5)**: a command's name appears in a declared
//! document's *code context* (a fenced ``` block or an inline `` ` `` span — verified empirically,
//! before writing this gate, that every real `prikk <command>` mention across every declared
//! document lives in one of those two forms; bare prose never coincides with a real command name)
//! **outside** `README.md`'s `## Useful Commands` section, which is a bare listing with no prose —
//! the one bare-listing block among the declared documents. A mention there alone does not count
//! as an explanation.

#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use super::COMMANDS;

/// `README.md` plus every `docs/src/` page found (by direct search, not glob) to mention a real
/// `prikk <command>` — declared, not scanned by wildcard (§3), so a new `docs/` file cannot
/// silently escape the gate and a deleted one fails loudly (`every_declared_document_exists`
/// below). **38 files under `docs/src/`** (31 found in the original search, plus
/// `docs/src/guide/status.md`, added along with the page itself to close the one gap this gate
/// found — review v1 §1/§4; plus `docs/src/guide/backup-restore.md`, added along with the page
/// itself for the same reason — DC-44 increment 4's own amendment §7.5; plus
/// `docs/src/reference/git-mapping.md`, added along with the page itself (RFC 128 §5), and four
/// pre-existing pages found mentioning real commands in code context while preparing that same
/// page — `docs/src/guide/ignore.md`, `docs/src/guide/faq.md`,
/// `docs/src/guide/troubleshooting.md`, `docs/src/reference/durability-recovery.md` — none of
/// which had ever been declared here (RFC 128 outward-surface handoff §4). `docs/src/guide/tutorial.md`
/// also mentions real commands in fenced code and remains undeclared — found during the same
/// sweep, reported rather than added since it was outside that handoff's named scope). Plus
/// `docs/landing/index.html`, added along with the page itself (RFC 137 §7 increment 4) — checked
/// by rule (A) like every other declared document, but excluded from rule (B)'s search by
/// `document_text`'s own `LANDING_PAGE_PATH_PREFIX` arm, since naming a command there is not an
/// explanation of it. Plus `docs/src/guide/first-run.md`, added along with the page itself
/// (RFC 135 §4/§9.7) to explain the two new registry entries, `key` and `setup`.
const DECLARED_DOCUMENTS: &[&str] = &[
    "docs/landing/index.html",
    "docs/src/guide/backup-restore.md",
    "docs/src/guide/checkout/checkout.md",
    "docs/src/guide/checkout/snapshot-checkout.md",
    "docs/src/guide/checkout/snapshot-materialization.md",
    "docs/src/guide/faq.md",
    "docs/src/guide/first-run.md",
    "docs/src/guide/history.md",
    "docs/src/guide/ignore.md",
    "docs/src/guide/merge-evidence.md",
    "docs/src/guide/merge.md",
    "docs/src/guide/merge-plan.md",
    "docs/src/guide/patches/declared-move.md",
    "docs/src/guide/patches/patch-deletions.md",
    "docs/src/guide/patches/patch-inverse.md",
    "docs/src/guide/patches/patch-materialization.md",
    "docs/src/guide/patches/patch-replay.md",
    "docs/src/guide/patches/text-edits.md",
    "docs/src/guide/patches/worktree-patch.md",
    "docs/src/guide/rollback/rollback-draft.md",
    "docs/src/guide/rollback/rollback-draft-verify.md",
    "docs/src/guide/rollback/rollback-preview.md",
    "docs/src/guide/rollback/sealed-rollback-history.md",
    "docs/src/guide/security-setup.md",
    "docs/src/guide/show.md",
    "docs/src/guide/status.md",
    "docs/src/guide/sync.md",
    "docs/src/guide/troubleshooting.md",
    "docs/src/guide/tutorial.md",
    "docs/src/guide/worktree-status.md",
    "docs/src/reference/architecture.md",
    "docs/src/reference/commands.md",
    "docs/src/reference/concurrency-locking.md",
    "docs/src/reference/current-state.md",
    "docs/src/reference/data-model-lifecycle.md",
    "docs/src/reference/data-model.md",
    "docs/src/reference/durability-recovery.md",
    "docs/src/reference/git-mapping.md",
    "docs/src/reference/integrity-recovery.md",
    "docs/src/reference/patch-algebra.md",
    "docs/src/reference/path-safety.md",
    "docs/src/reference/platform-support.md",
    "docs/src/reference/release-compatibility.md",
    "docs/src/reference/repository-layout.md",
    "docs/src/reference/trust-threat-model.md",
    "README.md",
];

/// `(command, reason)` pairs rule (B) accepts as deliberately unexplained — the same shape as
/// `signature_contract_tests::vectors::RFC114_ADMITTED_BUT_UNWRITTEN` next to Gate A, per the
/// handoff's explicit model. Every entry needs a real reason, not a placeholder: this is where the
/// gate's own honesty lives, since it is the one place a genuine gap could be quietly declared away
/// instead of fixed.
///
/// **Empty, deliberately.** `status` briefly lived here (review v1 §1): the reviewer ruled that an
/// accidental documentation gap is not what this list is for — `RFC114_ADMITTED_BUT_UNWRITTEN`
/// declares pairs no production code path has ever constructed, a *fact about the code*, not "we
/// have not written the page yet." `docs/src/guide/status.md` was written instead, closing the gap
/// directly. The constant stays, with its shape established, for a genuine future case — a command
/// that *cannot* be documented, not merely one that has not been yet — rather than being removed
/// only to be re-added under pressure the next time a gap appears.
const DECLARED_UNDOCUMENTED: &[(&str, &str)] = &[];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("prikk-cli's manifest dir has a workspace root two levels up")
        .to_path_buf()
}

/// Every declared path must exist -- a deleted declared document must fail loudly, not silently
/// stop being scanned (§3).
#[test]
fn every_declared_document_exists() {
    let root = repo_root();
    for path in DECLARED_DOCUMENTS {
        assert!(
            root.join(path).is_file(),
            "declared document is missing: {path}"
        );
    }
}

/// Every `<tag ...>...</tag>` occurrence in `text`, in the order they start, as
/// `(region_text, (start, end))` pairs. RFC 137 §4.3: taught to `code_regions` so a declared HTML
/// document (the landing page, `docs/landing/`) is checked by rule (A) the same way a Markdown one
/// is, with one definition of "code context" for every declared document rather than a
/// format-conditional branch.
///
/// Recognizes an opening tag only when the tag name is followed immediately by `>` or whitespace,
/// so `<pre` never matches a different tag sharing the prefix (`<precise>`, `<codex>`). Does not
/// handle a literal `>` inside a quoted attribute value (e.g. `title=">"`) -- deliberately simple,
/// matching this function's own no-parser, no-dependency approach for Markdown fences below; no
/// declared document's authored HTML does this. Tag names are matched **case-insensitively**: HTML
/// tag names are case-insensitive by spec, and matching only lowercase would make an `<CODE>` or
/// `<Pre>` in a declared document invisible to this scan -- a vacuous pass, not a missed style
/// nit, since rule (A)/(B) would silently stop checking that block's commands at all rather than
/// flagging anything. Every declared document (including the landing page, `docs/landing/`) is
/// hand-authored in this project using lowercase tags, so this costs nothing today and closes the
/// gap for whatever is authored next (review v1 §7.2).
///
/// An opening tag with no matching closing tag is treated exactly like the fenced-block arm's own
/// unterminated case below: everything from the opening tag onward is one region, and the scan for
/// this tag name stops there -- matching the existing arm's behaviour is the obvious choice, and
/// being deliberate about it is what the handoff asked for, not silence on the question.
fn html_tag_regions<'a>(text: &'a str, tag: &str) -> Vec<(&'a str, (usize, usize))> {
    let lower = text.to_ascii_lowercase();
    let open_needle = format!("<{tag}");
    let close_needle = format!("</{tag}>");
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = lower[offset..].find(open_needle.as_str()) {
        let start = offset + rel;
        let after_name = start + open_needle.len();
        let is_real_tag = lower[after_name..]
            .chars()
            .next()
            .is_some_and(|c| c == '>' || c.is_whitespace());
        if !is_real_tag {
            // A different tag sharing this prefix (e.g. `<precise>` while scanning for `<pre`) --
            // not a match; keep scanning from just past the prefix, not past the whole tag, since
            // we do not know where (or whether) this unrelated tag closes.
            offset = after_name;
            continue;
        }
        let Some(close_rel) = lower[after_name..].find('>') else {
            // The opening tag itself never closes; nothing genuine follows it either.
            break;
        };
        let after_open = after_name + close_rel + 1;
        match lower[after_open..].find(close_needle.as_str()) {
            Some(end_rel) => {
                let end = after_open + end_rel + close_needle.len();
                out.push((&text[start..end], (start, end)));
                offset = end;
            }
            None => {
                out.push((&text[start..], (start, text.len())));
                break;
            }
        }
    }
    out
}

/// Extract every fenced (``` ... ```), inline (`` `...` ``), and HTML (`<pre>`/`<code>`, RFC 137
/// §4.3) code region from `text`, in the order they start. Deliberately simple (byte-offset
/// scanning against `text` directly, no Markdown or HTML parser, no dependency): this project's
/// docs never nest one inside the other, and totality (never panicking on arbitrary text) matters
/// more than handling every theoretical edge case a hand-authored doc page will never actually
/// contain.
fn code_regions(text: &str) -> Vec<&str> {
    let mut fenced_ranges = Vec::new();
    let mut regions = Vec::new();
    let mut offset = 0usize;
    while let Some(rel_start) = text[offset..].find("```") {
        let start = offset + rel_start;
        let after_open = start + 3;
        let Some(rel_end) = text[after_open..].find("```") else {
            // An unterminated fence: everything from here on is still "inside" it as far as this
            // scan can tell, so there is nothing left to inline-scan either.
            regions.push(&text[start..]);
            fenced_ranges.push((start, text.len()));
            break;
        };
        let end = after_open + rel_end + 3;
        regions.push(&text[start..end]);
        fenced_ranges.push((start, end));
        offset = end;
    }

    // `<pre>...</pre>` plays the same container role as a ``` fence -- recorded next, into the
    // same `fenced_ranges`, so an inline `<code>` or backtick span starting inside one is never
    // also scanned separately (the `<pre><code>...</code></pre>` case, the common one). Also skip
    // any `<pre>` that starts inside an already-recorded ``` fence -- the same precedence the
    // `<code>` and backtick arms below already give a ``` fence, so a `<pre>` block sitting inside
    // a Markdown fence is scanned once, not twice.
    for (region, range) in html_tag_regions(text, "pre") {
        if fenced_ranges
            .iter()
            .any(|&(range_start, range_end)| range.0 >= range_start && range.0 < range_end)
        {
            continue;
        }
        regions.push(region);
        fenced_ranges.push(range);
    }

    // Inline spans, skipping any that start inside an already-collected fenced range -- so a
    // `prikk x` sitting inside a fenced block is not also scanned as (or split by) an inline span.
    let mut offset = 0usize;
    while let Some(rel_start) = text[offset..].find('`') {
        let start = offset + rel_start;
        if fenced_ranges
            .iter()
            .any(|&(range_start, range_end)| start >= range_start && start < range_end)
        {
            offset = start + 1;
            continue;
        }
        let Some(rel_end) = text[start + 1..].find('`') else {
            break;
        };
        let end = start + 1 + rel_end + 1;
        regions.push(&text[start..end]);
        offset = end;
    }

    // `<code>...</code>` spans, the same precedence as an inline backtick span: skip any that
    // start inside an already-recorded fenced range (a ``` fence or a `<pre>` block), so
    // `<pre><code>...</code></pre>` is scanned once, not twice.
    for (region, (start, _end)) in html_tag_regions(text, "code") {
        if fenced_ranges
            .iter()
            .any(|&(range_start, range_end)| start >= range_start && start < range_end)
        {
            continue;
        }
        regions.push(region);
    }

    regions
}

/// Every distinct token immediately following `"prikk "` in `region` — a run of ASCII
/// alphanumerics/hyphens starting with a letter, so a meta-arm like `--version` (starts with `-`)
/// never matches.
fn command_tokens(region: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut rest = region;
    while let Some(rel) = rest.find("prikk ") {
        let after = &rest[rel + "prikk ".len()..];
        let token_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(after.len());
        let token = &after[..token_len];
        if token
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        {
            tokens.push(token);
        }
        // Always advance by at least one byte, even when `token_len == 0` (the character right
        // after "prikk " didn't match at all), so an empty token can never stall the loop.
        let advance = token_len.max(1).min(after.len());
        rest = &after[advance..];
    }
    tokens
}

/// `README.md` only: everything inside the `## Useful Commands` section is not a genuine
/// explanation (it is the one bare listing among the declared documents, per this file's own doc
/// comment) -- return the text with that section's body removed so `explained_outside_readme_list`
/// never counts a mention found only there.
fn strip_readme_command_listing(text: &str) -> String {
    let Some(start) = text.find("## Useful Commands") else {
        return text.to_string();
    };
    let after_heading = &text[start + "## Useful Commands".len()..];
    let end = after_heading
        .find("\n## ")
        .map_or(text.len(), |rel| start + "## Useful Commands".len() + rel);
    let mut result = String::with_capacity(text.len());
    result.push_str(&text[..start]);
    result.push_str(&text[end..]);
    result
}

/// RFC 137 §4.3: the landing page names commands but does not explain them -- if it counted for
/// rule (B) (via `is_explained`, below), a command mentioned only there would read as documented,
/// closing a real documentation gap on a marketing sentence. A prefix, not one exact path: RFC 137
/// §3 fixes the directory (`docs/landing/`) but increment 4 (which builds the page) fixes its
/// filename, so matching the directory means this arm needs no further change if a second file
/// were ever added under it. `docs/landing/index.html` was declared in `DECLARED_DOCUMENTS` in the
/// same increment that created it (RFC 137 §7 increment 4), so rule (A) does check it -- only rule
/// (B)'s search is what this arm exempts it from.
const LANDING_PAGE_PATH_PREFIX: &str = "docs/landing/";

fn document_text(root: &Path, path: &str) -> String {
    if path.starts_with(LANDING_PAGE_PATH_PREFIX) {
        return String::new();
    }
    let text = fs::read_to_string(root.join(path))
        .unwrap_or_else(|err| panic!("declared document {path} must read: {err}"));
    if path == "README.md" {
        strip_readme_command_listing(&text)
    } else {
        text
    }
}

/// Rule (A): every `prikk <command>` mentioned in code context in a declared document names a
/// real registry entry.
#[test]
fn rule_a_every_documented_command_names_a_real_registry_entry() {
    let root = repo_root();
    for path in DECLARED_DOCUMENTS {
        let text = fs::read_to_string(root.join(path))
            .unwrap_or_else(|err| panic!("declared document {path} must read: {err}"));
        for region in code_regions(&text) {
            for token in command_tokens(region) {
                assert!(
                    COMMANDS.iter().any(|command| command.name == token),
                    "{path} documents `prikk {token}`, which is not a real registry command name"
                );
            }
        }
    }
}

fn is_explained(root: &Path, name: &str) -> bool {
    DECLARED_DOCUMENTS.iter().any(|path| {
        let text = document_text(root, path);
        code_regions(&text)
            .iter()
            .any(|region| command_tokens(region).contains(&name))
    })
}

/// Rule (B): every registry entry is explained somewhere, or declared undocumented with a reason.
#[test]
fn rule_b_every_registry_entry_is_explained_or_declared_undocumented() {
    let root = repo_root();
    for command in COMMANDS {
        if DECLARED_UNDOCUMENTED
            .iter()
            .any(|(name, _)| *name == command.name)
        {
            continue;
        }
        assert!(
            is_explained(&root, command.name),
            "`{}` is neither explained in a declared document outside README's bare command \
             listing, nor declared undocumented with a reason in DECLARED_UNDOCUMENTED",
            command.name
        );
    }
}

/// Self-guard on the escape hatch itself: every `DECLARED_UNDOCUMENTED` name must be a real
/// registry entry -- a stale or misspelled entry there would silently exempt nothing (or the wrong
/// command) from rule (B) forever.
#[test]
fn declared_undocumented_names_are_real_registry_entries() {
    for (name, reason) in DECLARED_UNDOCUMENTED {
        assert!(
            COMMANDS.iter().any(|command| &command.name == name),
            "DECLARED_UNDOCUMENTED names `{name}`, which is not a real registry command"
        );
        assert!(
            !reason.trim().is_empty(),
            "DECLARED_UNDOCUMENTED entry for `{name}` has no reason"
        );
    }
}

// ---- RFC 137 §4.3: code_regions' HTML arm (`<pre>`/`<code>`) ----

/// Basic recognition: a bare `<code>` span and a bare `<pre>` block each yield one region, and
/// `command_tokens` finds the real command inside either.
#[test]
fn code_regions_finds_bare_html_code_and_pre() {
    let code_only = "See <code>prikk seal</code> for details.";
    let regions = code_regions(code_only);
    assert_eq!(
        regions.len(),
        1,
        "expected exactly one region, got {regions:?}"
    );
    assert_eq!(
        command_tokens(regions.first().expect("checked len == 1 above")),
        vec!["seal"]
    );

    let pre_only = "<pre>prikk verify</pre>";
    let regions = code_regions(pre_only);
    assert_eq!(
        regions.len(),
        1,
        "expected exactly one region, got {regions:?}"
    );
    assert_eq!(
        command_tokens(regions.first().expect("checked len == 1 above")),
        vec!["verify"]
    );
}

/// Attribute forms (§5 control 3): `<code id="x">`, `<code class="a b">`, and `<pre class="term">`
/// are all recognised as real opening tags.
#[test]
fn code_regions_recognizes_attributed_html_tags() {
    for (text, expected) in [
        (r#"<code id="c1">prikk commit</code>"#, "commit"),
        (r#"<code class="a b">prikk log</code>"#, "log"),
        (r#"<pre class="term">prikk doctor</pre>"#, "doctor"),
    ] {
        let regions = code_regions(text);
        assert_eq!(
            regions.len(),
            1,
            "{text:?}: expected one region, got {regions:?}"
        );
        assert_eq!(
            command_tokens(regions.first().expect("checked len == 1 above")),
            vec![expected],
            "input: {text:?}"
        );
    }
}

/// The prefix trap (§5 control 3): `<codex>` and `<precise>` share a prefix with `<code`/`<pre`
/// but are different tags, and must not be matched as opening tags of either.
#[test]
fn code_regions_does_not_match_html_tag_name_prefix() {
    assert_eq!(
        code_regions("<codex>prikk notacommand</codex>"),
        Vec::<&str>::new(),
        "`<codex>` must not be recognised as a `<code>` opening tag"
    );
    assert_eq!(
        code_regions("<precise>prikk notacommand</precise>"),
        Vec::<&str>::new(),
        "`<precise>` must not be recognised as a `<pre>` opening tag"
    );
}

/// `<pre><code>...</code></pre>` (§5 control 2, the common form) must be scanned once, not twice:
/// `<pre>` is recorded first as a container, and the nested `<code>` starts inside it, so the
/// `<code>` arm must skip it. This is also the direct, permanent version of the positive control
/// the handoff asked for by hand: a fixture naming a real command inside this nesting must still
/// be found by rule (A) exactly once (proven here by region count, not by counting panics, since a
/// panicking `assert!` cannot distinguish "found once" from "found twice").
#[test]
fn code_regions_does_not_double_count_pre_code_nesting() {
    let text = "<pre><code>prikk seal</code></pre>";
    let regions = code_regions(text);
    assert_eq!(
        regions.len(),
        1,
        "expected the nested form to yield exactly one region, got {regions:?}"
    );
    let only_region = regions.first().expect("checked len == 1 above");
    assert_eq!(*only_region, text);
    assert_eq!(command_tokens(only_region), vec!["seal"]);
}

/// Review v1 §7.1: a `<pre>` starting inside an already-recorded ``` fence must not be counted a
/// second time -- the `<pre>` arm used to record it without checking `fenced_ranges` first, unlike
/// the `<code>` and backtick arms below it, which already did. Proven the same way as the
/// `<pre><code>` case above: by region count, not by counting panics.
#[test]
fn code_regions_does_not_double_count_pre_inside_a_fence() {
    let text = "```\n<pre>prikk seal</pre>\n```";
    let regions = code_regions(text);
    assert_eq!(
        regions.len(),
        1,
        "expected the fenced form to yield exactly one region, got {regions:?}"
    );
    let only_region = regions.first().expect("checked len == 1 above");
    assert_eq!(*only_region, text);
}

/// Review v1 §7.2: tag names are matched case-insensitively, so an `<CODE>` or `<Pre>` opening
/// tag is still found rather than silently yielding zero regions (a vacuous pass for rule (A)/(B)
/// rather than a mere style nit, since a mismatch here means the block is never scanned at all).
#[test]
fn code_regions_matches_html_tags_case_insensitively() {
    assert_eq!(
        code_regions("<CODE>prikk seal</CODE>"),
        vec!["<CODE>prikk seal</CODE>"]
    );
    assert_eq!(
        code_regions("<Pre>prikk verify</Pre>"),
        vec!["<Pre>prikk verify</Pre>"]
    );
}

/// An opening `<pre>`/`<code>` with no matching closing tag runs to end-of-text and stops the scan
/// for that tag name, mirroring the fenced-block arm's own unterminated behaviour exactly (§4.1 of
/// the handoff: a deliberate choice, not silence on the question).
#[test]
fn code_regions_html_unterminated_tag_runs_to_end_of_text() {
    let text = "before <code>prikk seal";
    let regions = code_regions(text);
    assert_eq!(regions, vec!["<code>prikk seal"]);
}

/// §4.2: an HTML entity cannot appear *inside* a token -- `&`/`#`/`;` are all outside
/// `command_tokens`' own character class, so an entity terminates a token rather than being
/// silently absorbed into one. Not the shape the landing page actually uses (it writes `prikk
/// seal` as literal text, per the handoff), but confirmed rather than assumed, per the handoff's
/// own instruction.
#[test]
fn command_tokens_cannot_absorb_an_html_entity() {
    assert_eq!(
        command_tokens("prikk se&amp;al"),
        vec!["se"],
        "an entity must terminate the token, never be absorbed into it"
    );
}

/// RFC 137 §4.3: a path under the landing page's directory is excluded from rule (B) -- named
/// there, but nothing on it ever counts as an explanation. The path used here does not exist on
/// disk; `document_text` must not attempt to read it, since increment 4 has not built the page yet.
#[test]
fn document_text_returns_nothing_for_the_landing_page() {
    let root = repo_root();
    let text = document_text(&root, "docs/landing/index.html");
    assert_eq!(
        text, "",
        "the landing page must contribute nothing to rule (B)"
    );
}
