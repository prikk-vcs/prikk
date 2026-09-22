//! `prikk diff` (RFC 153 §2, §3, as amended by §6a and §7): prose with unified hunks, or `diff-report-v1`.
//!
//! **Three shapes, one report.** `--from A --to B` compares two points; a bare `prikk diff` compares the current
//! branch's tip (with its queued commits) against the worktree; `--from A` alone compares `A` against the
//! worktree. Every shape prints through the same code, and the worktree side is a point named `worktree`.
//! A lone `--to` is refused: RFC 153 §2 gives it a left side ("the current branch's tip") but does not say
//! whether that tip carries its queued commits, and this does not guess.

use std::path::PathBuf;

use prikk_store::{DiffEntry, DiffPoint, DiffReport, DiffStatus, PointEntry, WORKTREE_POINT};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::args::optional_path_or_current;
use crate::commands::CliError;
use crate::output::verification::escape_json_string;
use crate::stdout::{print, println};
use crate::tree::push_entry_attributes;
use crate::{current_branch, open_repository, warn_anchor_fallbacks};

struct DiffArgs {
    root: PathBuf,
    from: Option<String>,
    to: Option<String>,
    paths: Vec<String>,
    format_json: bool,
}

/// A `--from`/`--to` value: a ref name or a bare block id, and nothing else (RFC 153 §2).
fn point_argument(flag: &str, value: &str) -> std::result::Result<(), CliError> {
    if prikk_store::is_point_name(value) {
        return Ok(());
    }
    Err(CliError::Usage(format!(
        "diff {flag} {value:?} is neither a ref name (heads/…, tags/… or remotes/…) nor a block id (64 \
         lowercase hex characters)"
    )))
}

fn parse_diff_args(args: Vec<String>) -> std::result::Result<DiffArgs, CliError> {
    let mut root = None;
    let mut from = None;
    let mut to = None;
    let mut paths = Vec::new();
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--from" => {
                let value = flag_value(&mut iter, "diff --from")?;
                point_argument("--from", &value)?;
                from.set_once("--from", value)?;
            }
            "--to" => {
                let value = flag_value(&mut iter, "diff --to")?;
                point_argument("--to", &value)?;
                to.set_once("--to", value)?;
            }
            "--path" => {
                let value = flag_value(&mut iter, "diff --path")?;
                prikk_store::RepoPath::parse(&value).map_err(|err| {
                    CliError::Usage(format!(
                        "diff --path {value:?} is not a valid repository-relative path ({err})"
                    ))
                })?;
                paths.push(value);
            }
            "--format" => {
                let value = flag_value(&mut iter, "diff --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "diff --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("diff", other)),
            _ => {
                if root.is_some() {
                    return Err(CliError::Usage("diff accepts at most one path".to_string()));
                }
                root = Some(arg);
            }
        }
    }
    if to.is_some() && from.is_none() {
        return Err(CliError::Usage(
            "diff --to needs --from as well: without --to the right side is the worktree, and a lone --to \
             has no defined left side (compare two points with --from and --to, or a point with the \
             worktree with --from alone)"
                .to_string(),
        ));
    }
    Ok(DiffArgs {
        root: optional_path_or_current(root)?,
        from,
        to,
        paths,
        format_json,
    })
}

/// `prikk diff [path] [--from <ref|block-id>] [--to <ref|block-id>] [--path <p>]... [--format json]`.
pub(crate) fn run_diff(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_diff_args(args)?;
    let layout = open_repository(args.root)?;
    // RFC 153 §7.1: each side is a point -- a ref, a received ref (read, not adopted), or a bare block id.
    let resolve = |name: &str| {
        prikk_store::resolve_point(&layout, name, prikk_store::ReceivedRefs::Read)
            .map_err(|err| err.to_string())
    };
    let (report, fallbacks) = match (&args.from, &args.to) {
        (Some(from), Some(to)) => {
            let (from, to) = (resolve(from)?, resolve(to)?);
            prikk_store::diff_points_reporting_anchor(&layout, &from, &to, &args.paths)
        }
        (from, _) => {
            // The worktree is the right side. It is read against the *current branch's* baseline -- what
            // `commit` would author it against -- whichever point the left side is. An explicit `--from`
            // naming an absent ref refuses through the resolver; the implicit left side of a fresh
            // repository is the empty state (RFC 153 §7.3).
            let branch = current_branch::resolve_ref(&layout, None)?;
            let from = match from {
                Some(name) => Some(resolve(name)?),
                None => None,
            };
            prikk_store::diff_worktree_reporting_anchor(
                &layout,
                &branch,
                from.as_ref(),
                &args.paths,
            )
        }
    }
    .map_err(|err| err.to_string())?;
    warn_anchor_fallbacks(fallbacks.iter());
    if args.format_json {
        println!("{}", diff_report_json(&report));
    } else {
        print_report(&layout, &report);
    }
    Ok(())
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn describe_point(point: &DiffPoint) -> String {
    if point.worktree {
        return WORKTREE_POINT.to_string();
    }
    let queued = match point.queued_patches {
        Some(count) if count > 0 => Some(plural(count, "queued commit", "queued commits")),
        _ => None,
    };
    match (point.target_block_id, queued) {
        (Some(id), _) if point.point == id.to_string() => format!("block {id}"),
        (Some(id), None) => format!("{} (block {id})", point.point),
        (Some(id), Some(queued)) => {
            format!("{} (block {id}, plus {queued} not yet sealed)", point.point)
        }
        (None, None) => format!("{} (not published: the empty state)", point.point),
        (None, Some(queued)) => format!(
            "{} (not published: the empty state, plus {queued} not yet sealed)",
            point.point
        ),
    }
}

fn octal(entry: &PointEntry) -> String {
    format!("{:06o}", entry.mode)
}

/// The parenthesised particulars of one entry line: a mode change, and a binary side's size and id.
fn details(entry: &DiffEntry) -> String {
    let mut parts = Vec::new();
    if let (Some(from), Some(to)) = (&entry.from, &entry.to) {
        if from.mode != to.mode {
            parts.push(format!("mode {} -> {}", octal(from), octal(to)));
        }
    }
    let binary = |side: &Option<PointEntry>| {
        side.as_ref().and_then(|side| {
            side.content_id
                .map(|id| format!("{} bytes, id {id}", side.size))
        })
    };
    match (binary(&entry.from), binary(&entry.to)) {
        (Some(from), Some(to)) => parts.push(format!("binary: {from} -> {to}")),
        (Some(from), None) => parts.push(format!("binary: {from}")),
        (None, Some(to)) => parts.push(format!("binary: {to}")),
        (None, None) => {}
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join("; "))
    }
}

/// One `prikk cat` hint line for one side of a binary entry, printed under the "read a side with…" header
/// (RFC 153 §6b corollary, the round's F1). **A hint must reproduce the state its report describes, or it
/// must not be printed** — the same rule Stage 1 applied to a hint that named a movable ref instead of the
/// block it resolved to.
///
/// A worktree side is read as the file itself, never through `cat`. A left side that carries queued, unsealed
/// commits is the sealed tip **plus** that queue — a state no block names — so no `--ref` line is printed for
/// it; the reader is told why instead, so they are never handed a command that reads different bytes from the
/// side the entry describes. Every other side names the block it resolved to.
fn print_cat_hint(point: &DiffPoint, path: &str) {
    if point.worktree {
        println!("    (the worktree side is the file itself, at {path})");
        return;
    }
    if let Some(count) = point.queued_patches {
        if count > 0 {
            println!(
                "    the left side includes {} that no block names, so {path} can be read only after \
                 `prikk seal`",
                plural(count, "queued commit", "queued commits")
            );
            return;
        }
    }
    if let Some(block) = point.target_block_id {
        println!("    prikk cat --path {path} --ref {block}");
    }
    // `target_block_id: None` and no queue is an unpublished branch's empty left side, which has no entries
    // to carry a binary hint for in the first place.
}

fn print_entry(report: &DiffReport, entry: &DiffEntry) {
    let old_path = entry.from.as_ref().map(|side| side.path.as_str());
    let head = match (entry.status, old_path) {
        (DiffStatus::Renamed, Some(old)) => format!("renamed {old} -> {}", entry.path),
        (status, _) => format!("{} {}", status.as_str(), entry.path),
    };
    println!("{head}{}", details(entry));
    if !entry.minimal {
        // RFC 153 §6a C 4: a reader must never have to guess whether it got the shortest script. Printed
        // **before** the `---` line, so it can never be mistaken for a line of the hunks.
        println!(
            "  note: not the shortest edit script: the search reached its work bound and stopped, so the \
             region it had not resolved is shown as deleted and re-added; the hunks still apply"
        );
    }
    if !entry.hunks.is_empty() {
        // `patch(1)` reads these two lines to name the file; `/dev/null` says which side is empty.
        match (&entry.from, old_path) {
            (Some(_), Some(old)) => println!("--- a/{old}"),
            _ => println!("--- /dev/null"),
        }
        match &entry.to {
            Some(_) => println!("+++ b/{}", entry.path),
            None => println!("+++ /dev/null"),
        }
        for hunk in &entry.hunks {
            print!("{hunk}");
        }
    }
    let binary_sides = [&entry.from, &entry.to]
        .iter()
        .any(|side| side.as_ref().is_some_and(|side| side.content_id.is_some()));
    if binary_sides {
        // §7.6: `diff` never prints binary bytes; `cat` is the bounded command whose job is bytes.
        println!("  read a side with `prikk cat --path <p> --ref <block-id>`:");
        if let Some(from) = &entry.from {
            print_cat_hint(&report.from, &from.path);
        }
        if let Some(to) = &entry.to {
            print_cat_hint(&report.to, &to.path);
        }
    }
}

fn print_report(layout: &prikk_store::RepositoryLayout, report: &DiffReport) {
    println!("diff repository: {}", layout.prikk_dir().display());
    println!("from: {}", describe_point(&report.from));
    println!("to: {}", describe_point(&report.to));
    if report.entries.is_empty() && report.unsupported_paths.is_empty() {
        println!("no differences");
        return;
    }
    println!("entries: {}", report.entries.len());
    for entry in &report.entries {
        print_entry(report, entry);
    }
    if !report.unsupported_paths.is_empty() {
        // RFC 147 §3f: named with `commit`'s own refusal, and no content.
        println!("unsupported paths: {}", report.unsupported_paths.len());
        for unsupported in &report.unsupported_paths {
            println!("  {}: {}", unsupported.path, unsupported.refusal);
        }
    }
}

/// `"target_block_id"`: the block id, the string `"worktree"` for the worktree (RFC 153 §3), or `null` for an
/// unpublished branch. `queued_patches` appears only on the implicit left side of a bare diff.
fn point_json(point: &DiffPoint) -> String {
    let target = if point.worktree {
        escape_json_string(WORKTREE_POINT)
    } else {
        match point.target_block_id {
            Some(id) => escape_json_string(&id.to_string()),
            None => "null".to_string(),
        }
    };
    let queued = match point.queued_patches {
        Some(count) => format!(", \"queued_patches\": {count}"),
        None => String::new(),
    };
    format!(
        "{{\"point\": {}, \"target_block_id\": {target}{queued}}}",
        escape_json_string(&point.point)
    )
}

fn push_side(json: &mut String, key: &str, side: &PointEntry) {
    json.push_str(&format!(",\n      \"{key}\": {{"));
    push_entry_attributes(json, side);
    json.push('}');
}

/// `diff-report-v1` (RFC 153 §3, as amended by §6a and §7): the two points as named and as resolved, the entries
/// in canonical path order (each with `minimal`, always present), and `unsupported_paths`. There is no `untracked`
/// bucket: a new worktree file is `added`, with its content (§7.2).
fn diff_report_json(report: &DiffReport) -> String {
    let mut json = String::new();
    json.push_str("{\n  \"schema_version\": \"diff-report-v1\",\n  \"from\": ");
    json.push_str(&point_json(&report.from));
    json.push_str(",\n  \"to\": ");
    json.push_str(&point_json(&report.to));
    json.push_str(",\n  \"entries\": [");
    for (index, entry) in report.entries.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\n      \"path\": ");
        json.push_str(&escape_json_string(&entry.path));
        json.push_str(",\n      \"status\": ");
        json.push_str(&escape_json_string(entry.status.as_str()));
        if entry.status == DiffStatus::Renamed {
            if let Some(from) = &entry.from {
                json.push_str(",\n      \"from_path\": ");
                json.push_str(&escape_json_string(&from.path));
            }
        }
        if let Some(from) = &entry.from {
            push_side(&mut json, "from", from);
        }
        if let Some(to) = &entry.to {
            push_side(&mut json, "to", to);
        }
        json.push_str(&format!(",\n      \"minimal\": {}", entry.minimal));
        json.push_str(",\n      \"hunks\": [");
        for (hunk_index, hunk) in entry.hunks.iter().enumerate() {
            if hunk_index > 0 {
                json.push_str(", ");
            }
            json.push_str(&escape_json_string(hunk));
        }
        json.push_str("]\n    }");
    }
    if !report.entries.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("],\n  \"unsupported_paths\": [");
    for (index, unsupported) in report.unsupported_paths.iter().enumerate() {
        if index > 0 {
            json.push_str(", ");
        }
        json.push_str(&format!(
            "{{\"path\": {}, \"refusal\": {}}}",
            escape_json_string(&unsupported.path),
            escape_json_string(&unsupported.refusal)
        ));
    }
    json.push_str("]\n}");
    json
}
