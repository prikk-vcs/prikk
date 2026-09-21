//! `prikk diff` between two points (RFC 153 §2, §3): prose with unified hunks, or `diff-report-v1`.
//!
//! **Stage 1 takes two points.** `--from` and `--to` are both required; comparing with the worktree (RFC 153
//! §6.2) is the next increment and is refused here by name rather than guessed at.

use std::path::PathBuf;

use prikk_store::{DiffEntry, DiffPoint, DiffReport, DiffStatus, PointEntry};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::args::optional_path_or_current;
use crate::commands::CliError;
use crate::output::verification::escape_json_string;
use crate::stdout::{print, println};
use crate::tree::push_entry_attributes;
use crate::{open_repository, warn_anchor_fallbacks};

struct DiffArgs {
    root: PathBuf,
    from: String,
    to: String,
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
    let (Some(from), Some(to)) = (from, to) else {
        return Err(CliError::Usage(
            "diff needs both --from <ref|block-id> and --to <ref|block-id>: comparing with the worktree is \
             not built yet"
                .to_string(),
        ));
    };
    Ok(DiffArgs {
        root: optional_path_or_current(root)?,
        from,
        to,
        paths,
        format_json,
    })
}

/// `prikk diff [path] --from <ref|block-id> --to <ref|block-id> [--path <p>]... [--format json]`.
pub(crate) fn run_diff(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_diff_args(args)?;
    let layout = open_repository(args.root)?;
    // RFC 153 §7.1: each side is a point -- a ref, a received ref (read, not adopted), or a bare block id.
    let resolve = |name: &str| {
        prikk_store::resolve_point(&layout, name, prikk_store::ReceivedRefs::Read)
            .map_err(|err| err.to_string())
    };
    let (from, to) = (resolve(&args.from)?, resolve(&args.to)?);
    let (report, fallbacks) =
        prikk_store::diff_points_reporting_anchor(&layout, &from, &to, &args.paths)
            .map_err(|err| err.to_string())?;
    warn_anchor_fallbacks(fallbacks.iter());
    if args.format_json {
        println!("{}", diff_report_json(&report));
    } else {
        print_report(&layout, &report);
    }
    Ok(())
}

fn describe_point(point: &DiffPoint) -> String {
    let id = point.target_block_id.to_string();
    if point.point == id {
        format!("block {id}")
    } else {
        format!("{} (block {id})", point.point)
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

fn print_entry(report: &DiffReport, entry: &DiffEntry) {
    let old_path = entry.from.as_ref().map(|side| side.path.as_str());
    let head = match (entry.status, old_path) {
        (DiffStatus::Renamed, Some(old)) => format!("renamed {old} -> {}", entry.path),
        (status, _) => format!("{} {}", status.as_str(), entry.path),
    };
    println!("{head}{}", details(entry));
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
        // The hint names the block each side resolved to, not the ref it was named by: a ref may move before
        // the reader runs the command, and the hint has to reproduce the state this report describes.
        println!("  read a side with `prikk cat --path <p> --ref <block-id>`:");
        if let Some(from) = &entry.from {
            println!(
                "    prikk cat --path {} --ref {}",
                from.path, report.from.target_block_id
            );
        }
        if let Some(to) = &entry.to {
            println!(
                "    prikk cat --path {} --ref {}",
                to.path, report.to.target_block_id
            );
        }
    }
}

fn print_report(layout: &prikk_store::RepositoryLayout, report: &DiffReport) {
    println!("diff repository: {}", layout.prikk_dir().display());
    println!("from: {}", describe_point(&report.from));
    println!("to: {}", describe_point(&report.to));
    if report.entries.is_empty() {
        println!("no differences");
        return;
    }
    println!("entries: {}", report.entries.len());
    for entry in &report.entries {
        print_entry(report, entry);
    }
}

fn point_json(point: &DiffPoint) -> String {
    format!(
        "{{\"point\": {}, \"target_block_id\": {}}}",
        escape_json_string(&point.point),
        escape_json_string(&point.target_block_id.to_string())
    )
}

fn push_side(json: &mut String, key: &str, side: &PointEntry) {
    json.push_str(&format!(",\n      \"{key}\": {{"));
    push_entry_attributes(json, side);
    json.push('}');
}

/// `diff-report-v1` (RFC 153 §3, as amended by §7): the two points as named and as resolved, the entries
/// in canonical path order, and `unsupported_paths`. There is no `untracked` bucket: a new worktree file is
/// `added`, with its content (§7.2).
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
    for (index, path) in report.unsupported_paths.iter().enumerate() {
        if index > 0 {
            json.push_str(", ");
        }
        json.push_str(&escape_json_string(path));
    }
    json.push_str("]\n}");
    json
}
