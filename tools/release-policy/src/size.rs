//! RFC 130 §8: a production file over 1,200 lines is a decision, not a drift.
//!
//! **This gate does not adjudicate size; it forces a recorded decision about it** — the same shape
//! [`crate::boundary::coupling`]'s `DECLARED_CYCLES` and `DECLARED_HUBS` use, and for the same
//! reason. A bare bound would be either too low to pass or too high to mean anything; an allowlist
//! with a reason and *what would split it* makes the next reader's question — "is this file big
//! because it has to be?" — answerable without them having to read all 1,677 lines to guess.
//!
//! **A stale entry fails too.** An allowlist that outlives its cause is worse than none: it records
//! a decision about a file that no longer needs one, and the next person to cross the line inherits
//! a permission they never asked for.
//!
//! ## What counts as a production file
//!
//! Not a definition of its own: [`graph::production_files`] is the walk, shared with the coupling
//! gate. A `mod` declaration whose `cfg` cannot hold in any production configuration is never
//! followed, so `#[cfg(test)]` subtrees are invisible here exactly as they are there — which is why
//! `prikk-store/src/test_gates/signature_contract_tests/vectors.rs` (1,085 lines) is not a finding
//! and a second, directory-shaped scan would have made it one.
//!
//! Lines are **physical lines**, minus the body of any test-only inline module: those lines are
//! test lines that happen to share a file with production code, and counting them would push a
//! file over the line for having tests next to what they test.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::boundary::coupling::graph;
use crate::error::Result;

/// A production file above this many physical lines must be declared.
///
/// **1,200, from RFC 130 §8.** Three files are above it today and the fourth-largest production file
/// sits at 1,054, so the line falls in a real gap rather than through the middle of the
/// distribution — which is the only property that makes a threshold worth having: crossing it is an
/// event, not a rounding difference.
const LINE_LIMIT: usize = 1200;

/// A production file that is over [`LINE_LIMIT`] by a recorded decision.
struct DeclaredLargeFile {
    /// Repository-relative path, as the report prints it.
    path: &'static str,
    /// Why this file is one file.
    reason: &'static str,
    /// The split that exists but has not been taken — so that "should we split it?" starts from a
    /// concrete proposal rather than from scratch.
    what_would_split_it: &'static str,
}

/// The three files over the line today, each read before it was written about.
const DECLARED_LARGE_FILES: &[DeclaredLargeFile] = &[
    DeclaredLargeFile {
        path: "crates/prikk-store/src/verify.rs",
        reason: "One verification pass over one repository, reported as one graded verdict: object \
                 decode, WAL shape, ref reachability, publication trust and signature checking are \
                 not independent passes but one traversal that must not read the same container \
                 twice, and the report type they fill in is a single document whose fields are \
                 cross-checked at the end.",
        what_would_split_it: "The per-category finding constructors and their evidence shapes could \
                              move to `verify/findings.rs` (roughly 400 lines) without touching the \
                              traversal, which is the part that has to stay whole. Nobody has \
                              needed that yet; the file is read top to bottom far more often than \
                              it is edited in one place.",
    },
    DeclaredLargeFile {
        path: "crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring.rs",
        reason: "Node identity is decided once, in one place, on purpose (RFC 134): the rename \
                 witness, the content classification, the span identity and the path effects are \
                 four readings of the same worktree entry, and a bug in this file is nearly always \
                 a disagreement between two of them. Separating them separates the evidence a \
                 reader needs to see side by side.",
        what_would_split_it: "The classification helpers — text/binary detection, permission \
                              reading, symlink refusal — are self-contained and could become \
                              `node_authoring/classify.rs`. That would leave the identity decision \
                              itself intact and take out perhaps 350 lines.",
    },
    DeclaredLargeFile {
        path: "crates/prikk-store/src/bundle.rs",
        reason: "The bundle format's writer, reader and preview all encode the same PBNDL003 \
                 layout, and the format is defined by their agreement: a header field the writer \
                 emits and the reader skips is a defect only visible when both are in view. The \
                 file is long because the format is one artifact.",
        what_would_split_it: "`preview` is the least entangled third — it reads a bundle and \
                              reports, writing nothing — and would move to `bundle/preview.rs` at \
                              around 400 lines. The writer and reader should not be separated from \
                              each other.",
    },
];

#[derive(Debug, Serialize)]
pub(crate) struct SizeReport {
    schema_version: &'static str,
    pub(crate) valid: bool,
    /// The threshold this run applied.
    line_limit: usize,
    errors: Vec<SizeError>,
    /// Every production file over the limit, declared or not — the list a census reads.
    over_limit: Vec<FileSize>,
    /// Per crate, reported and never failed: control 1's number.
    crates: Vec<CrateSize>,
}

#[derive(Debug, Serialize)]
struct SizeError {
    category: &'static str,
    detail: String,
}

#[derive(Debug, Serialize)]
struct FileSize {
    path: String,
    production_lines: usize,
    declared: bool,
}

#[derive(Debug, Serialize)]
struct CrateSize {
    name: String,
    production_files: usize,
    production_lines: usize,
    /// Lines in test-only inline modules inside production files. Test files are not walked at all,
    /// so this is not the crate's test total — it is the test code living in production files.
    inline_test_lines: usize,
    /// Net physical line change in today's production files since `since_tag`, or `null` when the
    /// tag could not be read.
    production_line_delta: Option<i64>,
}

/// Every workspace member, by directory. Kept beside [`crate::boundary`]'s own `PRODUCTS` rather
/// than derived from `cargo metadata`: this gate must also cover the three `tools/` members, which
/// `PRODUCTS` deliberately does not list, and a member that disappears from here without being
/// removed from the workspace is caught by `members_are_covered` below.
const MEMBERS: [&str; 11] = [
    "crates/prikk-cli",
    "crates/prikk-crypto",
    "crates/prikk-error",
    "crates/prikk-ffi",
    "crates/prikk-hash",
    "crates/prikk-object",
    "crates/prikk-replay",
    "crates/prikk-store",
    "tools/benchmarks",
    "tools/corpus",
    "tools/release-policy",
];

pub(crate) fn run(root: &Path) -> Result<SizeReport> {
    run_with(root, LINE_LIMIT, DECLARED_LARGE_FILES)
}

/// The gate, with its two constants supplied.
///
/// **Parameterised so the controls can move them.** A threshold and an allowlist that only ever run
/// at their production values are a rule nobody has watched fail: the controls drive this with the
/// limit at 1,000 and with entries removed and invented, which is the only way to know the gate
/// reports what it claims rather than passing because nothing is wrong.
fn run_with(root: &Path, limit: usize, declared: &[DeclaredLargeFile]) -> Result<SizeReport> {
    let mut errors = Vec::new();
    check_declarations_are_well_formed(declared, &mut errors);

    let since_tag = last_tag(root);
    let mut over_limit = Vec::new();
    let mut crates = Vec::new();
    let mut seen_paths: BTreeSet<String> = BTreeSet::new();

    for member in MEMBERS {
        let member_root = root.join(member);
        let mut production_files = 0usize;
        let mut production_lines = 0usize;
        let mut inline_test_lines = 0usize;
        let mut member_paths = Vec::new();

        for crate_root_file in crate_root_files(&member_root) {
            let files = match graph::production_files(&crate_root_file) {
                Ok(files) => files,
                Err(message) => {
                    push(
                        &mut errors,
                        "file-size-scan",
                        format!("{member}: {message}"),
                    );
                    continue;
                }
            };
            for file in files {
                let relative = relative_path(root, &file.path);
                // A file reachable from two roots (a `src/bin` sharing a module with `main.rs`) is
                // one file, counted once.
                if !seen_paths.insert(relative.clone()) {
                    continue;
                }
                production_files += 1;
                production_lines += file.production_lines;
                inline_test_lines += file.test_lines;
                member_paths.push(relative.clone());
                if file.production_lines > limit {
                    let is_declared = declared_entry(declared, &relative).is_some();
                    if !is_declared {
                        push(
                            &mut errors,
                            "file-size",
                            format!(
                                "{relative} is {} lines, over the {limit}-line limit, and is not \
                                 in DECLARED_LARGE_FILES -- add an entry stating why it is one \
                                 file and what would split it, or split it",
                                file.production_lines
                            ),
                        );
                    }
                    over_limit.push(FileSize {
                        path: relative,
                        production_lines: file.production_lines,
                        declared: is_declared,
                    });
                }
            }
        }

        crates.push(CrateSize {
            name: member.rsplit('/').next().unwrap_or(member).to_owned(),
            production_files,
            production_lines,
            inline_test_lines,
            production_line_delta: since_tag
                .as_deref()
                .and_then(|tag| line_delta(root, tag, &member_paths)),
        });
    }

    check_declarations_are_current(declared, limit, &over_limit, &mut errors);

    Ok(SizeReport {
        schema_version: "release-policy-size-v1",
        valid: errors.is_empty(),
        line_limit: limit,
        errors,
        over_limit,
        crates,
    })
}

/// `src/lib.rs`, `src/main.rs`, and every `src/bin/*.rs` that exists. Each is a crate root in its
/// own right, and `tools/benchmarks` has only the third kind — a member whose production code would
/// otherwise be invisible to this gate while looking covered.
fn crate_root_files(member_root: &Path) -> Vec<PathBuf> {
    let src = member_root.join("src");
    let mut roots = Vec::new();
    for name in ["lib.rs", "main.rs"] {
        let candidate = src.join(name);
        if candidate.is_file() {
            roots.push(candidate);
        }
    }
    if let Ok(entries) = std::fs::read_dir(src.join("bin")) {
        let mut bins: Vec<PathBuf> = entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .collect();
        bins.sort();
        roots.extend(bins);
    }
    roots
}

fn declared_entry<'a>(
    declared: &'a [DeclaredLargeFile],
    path: &str,
) -> Option<&'a DeclaredLargeFile> {
    declared.iter().find(|entry| entry.path == path)
}

/// An entry must say something. Mirrors `coupling::check_allowlists_are_well_formed`: a blank or
/// placeholder reason is an entry that records nothing, which is the failure mode an allowlist has.
fn check_declarations_are_well_formed(declared: &[DeclaredLargeFile], errors: &mut Vec<SizeError>) {
    let mut seen = BTreeSet::new();
    for entry in declared {
        if !seen.insert(entry.path) {
            push(
                errors,
                "file-size-allowlist",
                format!("{} appears twice in DECLARED_LARGE_FILES", entry.path),
            );
        }
        if is_thin(entry.reason) {
            push(
                errors,
                "file-size-allowlist",
                format!("{}: reason is empty or a placeholder", entry.path),
            );
        }
        if is_thin(entry.what_would_split_it) {
            push(
                errors,
                "file-size-allowlist",
                format!(
                    "{}: what_would_split_it is empty or a placeholder",
                    entry.path
                ),
            );
        }
    }
}

/// A declared file that is no longer over the line, or no longer there at all, must lose its entry.
fn check_declarations_are_current(
    declared: &[DeclaredLargeFile],
    limit: usize,
    over_limit: &[FileSize],
    errors: &mut Vec<SizeError>,
) {
    let over: BTreeSet<&str> = over_limit.iter().map(|file| file.path.as_str()).collect();
    for entry in declared {
        if !over.contains(entry.path) {
            push(
                errors,
                "file-size-allowlist",
                format!(
                    "{} is declared in DECLARED_LARGE_FILES but is not over the {limit}-line limit \
                     (it may have been split, shrunk, moved or deleted) -- remove the entry; an \
                     allowlist may not outlive its cause",
                    entry.path
                ),
            );
        }
    }
}

fn is_thin(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.len() < 40 || trimmed.eq_ignore_ascii_case("tbd") || trimmed.contains("TODO")
}

fn push(errors: &mut Vec<SizeError>, category: &'static str, detail: String) {
    errors.push(SizeError { category, detail });
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The most recent release tag, or `None` when there is none to read — a shallow clone, a fresh
/// repository, or a checkout with no tags fetched. **Reported as unknown, never failed**: the delta
/// is context for a reader, and a gate that goes red because CI did not fetch tags would teach
/// people to ignore it.
fn last_tag(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let tag = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if tag.is_empty() { None } else { Some(tag) }
}

/// Net physical lines added to today's production files since `tag`.
///
/// **Restricted to files that are production code now.** A file deleted since the tag has no
/// current production status to weigh, and a test file that has grown is not this number's subject.
/// So this measures "how much did the code we still ship grow", which is the question the per-crate
/// line is asked to answer.
fn line_delta(root: &Path, tag: &str, paths: &[String]) -> Option<i64> {
    if paths.is_empty() {
        return Some(0);
    }
    let output = Command::new("git")
        .args(["diff", "--numstat", &format!("{tag}..HEAD"), "--"])
        .args(paths)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut delta = 0i64;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split('\t');
        let added = fields.next()?.parse::<i64>().ok();
        let removed = fields.next()?.parse::<i64>().ok();
        // `-`/`-` marks a binary file; there are none under `src/`, and skipping is the honest
        // response to one appearing rather than counting it as zero change.
        if let (Some(added), Some(removed)) = (added, removed) {
            delta += added - removed;
        }
    }
    Some(delta)
}

#[cfg(test)]
#[path = "size/tests.rs"]
mod tests;
