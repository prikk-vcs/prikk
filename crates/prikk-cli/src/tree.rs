//! `prikk tree` (RFC 157 §3): the present leaf paths at a point, as prose or `tree-listing-v1`.

use std::path::PathBuf;

use prikk_store::{PointEntry, TreeListing};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::args::optional_path_or_current;
use crate::commands::CliError;
use crate::output::point_label;
use crate::output::verification::escape_json_string;
use crate::stdout::println;
use crate::{current_branch, open_repository, warn_anchor_fallbacks};

struct TreeArgs {
    root: PathBuf,
    ref_name: Option<String>,
    prefix: Option<String>,
    format_json: bool,
}

fn parse_tree_args(args: Vec<String>) -> std::result::Result<TreeArgs, CliError> {
    let mut path = None;
    let mut ref_name = None;
    let mut prefix = None;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "tree --ref")?;
                // RFC 153 §2: a point is a ref name or a bare block id, and nothing else.
                if !prikk_store::is_point_name(&value) {
                    return Err(CliError::Usage(format!(
                        "tree --ref {value:?} is neither a ref name (heads/…, tags/… or remotes/…) nor \
                         a block id (64 lowercase hex characters)"
                    )));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--prefix" => {
                let value = flag_value(&mut iter, "tree --prefix")?;
                prikk_store::parse_tree_prefix(&value).map_err(|err| {
                    CliError::Usage(format!(
                        "tree --prefix {value:?} is not a valid repository-relative path ({err})"
                    ))
                })?;
                prefix.set_once("--prefix", value)?;
            }
            "--format" => {
                let value = flag_value(&mut iter, "tree --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "tree --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("tree", other)),
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage("tree accepts at most one path".to_string()));
                }
                path = Some(arg);
            }
        }
    }
    Ok(TreeArgs {
        root: optional_path_or_current(path)?,
        ref_name,
        prefix,
        format_json,
    })
}

/// `prikk tree [path] [--ref <ref|block-id>] [--prefix <p>] [--format json]`.
pub(crate) fn run_tree(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_tree_args(args)?;
    let layout = open_repository(args.root)?;
    let name = current_branch::resolve_ref(&layout, args.ref_name)?;
    // RFC 157 §3, as amended by RFC 147 §2i: an unpublished current branch lists nothing, with exit 0
    // -- naming it explicitly with `--ref` answers exactly as leaving `--ref` off does. Any other
    // absent ref, named explicitly, refuses through the resolver below.
    if current_branch::is_unpublished_current_branch(&layout, &name)? {
        let listing = prikk_store::unpublished_branch_tree_listing(&name, args.prefix.as_deref());
        print_listing(&layout, &listing, false, args.format_json);
        return Ok(());
    }
    // A received ref is read (RFC 157 §2): reading is not adopting.
    let point = prikk_store::resolve_point(&layout, &name, prikk_store::ReceivedRefs::Read)
        .map_err(|err| err.to_string())?;
    let (listing, fallback) =
        prikk_store::list_tree_at_point_reporting_anchor(&layout, &point, args.prefix.as_deref())
            .map_err(|err| err.to_string())?;
    warn_anchor_fallbacks(fallback.iter());
    print_listing(
        &layout,
        &listing,
        point.kind == prikk_store::PointKind::Block,
        args.format_json,
    );
    Ok(())
}

fn print_listing(
    layout: &prikk_store::RepositoryLayout,
    listing: &TreeListing,
    block: bool,
    format_json: bool,
) {
    if format_json {
        println!("{}", tree_listing_json(listing));
        return;
    }
    println!("tree repository: {}", layout.prikk_dir().display());
    println!("{}: {}", point_label(block), listing.point);
    match listing.target_block_id {
        Some(id) => println!("target block: {id}"),
        None => println!("target block: <not published>"),
    }
    if let Some(prefix) = &listing.prefix {
        println!("prefix: {prefix}");
    }
    println!("entries: {}", listing.entries.len());
    for entry in &listing.entries {
        println!(
            "  {:06o} {} {} {}",
            entry.mode,
            entry.encoding.as_str(),
            entry.size,
            entry.path
        );
    }
}

/// One entry's fields, in `tree-listing-v1`'s order. `path-content-v1` carries the same fields.
pub(crate) fn push_entry_fields(json: &mut String, entry: &PointEntry) {
    json.push_str("\"path\": ");
    json.push_str(&escape_json_string(&entry.path));
    json.push_str(", ");
    push_entry_attributes(json, entry);
}

/// An entry's fields **without its path** -- `kind`, `encoding`, `mode`, `size`, and `content_id` for a binary
/// file only -- in `tree-listing-v1`'s order. `diff-report-v1` puts the path at the entry level and gives each
/// side these same attributes, from this same emitter, so a side of a diff cannot disagree with `tree`.
pub(crate) fn push_entry_attributes(json: &mut String, entry: &PointEntry) {
    json.push_str("\"kind\": ");
    json.push_str(&escape_json_string(entry.kind.as_str()));
    json.push_str(", \"encoding\": ");
    json.push_str(&escape_json_string(entry.encoding.as_str()));
    json.push_str(&format!(
        ", \"mode\": {}, \"size\": {}",
        entry.mode, entry.size
    ));
    if let Some(content_id) = entry.content_id {
        json.push_str(", \"content_id\": ");
        json.push_str(&escape_json_string(&content_id.to_string()));
    }
}

fn optional_string(value: Option<&str>) -> String {
    value.map_or_else(|| "null".to_string(), escape_json_string)
}

/// `tree-listing-v1` (RFC 157 §3).
fn tree_listing_json(listing: &TreeListing) -> String {
    let mut json = String::new();
    json.push_str("{\n  \"schema_version\": \"tree-listing-v1\",\n  \"point\": ");
    json.push_str(&escape_json_string(&listing.point));
    json.push_str(",\n  \"target_block_id\": ");
    json.push_str(&optional_string(
        listing.target_block_id.map(|id| id.to_string()).as_deref(),
    ));
    json.push_str(",\n  \"prefix\": ");
    json.push_str(&optional_string(listing.prefix.as_deref()));
    json.push_str(",\n  \"entries\": [");
    for (index, entry) in listing.entries.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {");
        push_entry_fields(&mut json, entry);
        json.push('}');
    }
    if !listing.entries.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n}");
    json
}
