//! `prikk bundle` — export/import/verify a verifiable subset of history (DC-78 §D4/§D6; `verify`
//! is DC-44 increment 1, `bundle-offline-verify-handoff-v1.md`).
//!
//! `bundle export` writes a self-contained file: the exported ref's own RefState plus every object
//! reachable from its target Block back to genesis. **Refuses an existing file at the destination
//! unless `--force` is passed** (DC-44 increment 2,
//! `bundle-export-durability-handoff-v1.md` §3.1 — destroying a backup must never be silent), and
//! writes atomically and durably (§3.2 — `crate::durable_output`, not the anchored contract, which
//! is confined to repository-internal paths and does not apply to an arbitrary destination). `bundle
//! import` writes those objects into the
//! local object store and records a `received` pointer (`remotes/<origin ref name>`) — it never
//! touches `refs/by-id/`, never advances a local ref, and never adopts a MAINTAINER key into the
//! local trust policy. Imported history stays present but untrusted until the operator explicitly
//! runs `trust maintainer add` for the key that sealed it; the way to gain confidence in what was
//! imported is an ordinary `prikk verify`, unmodified — the bundle format adds no new verification
//! path. Turning a received ref into local history is an ordinary `merge`, using machinery that
//! already exists; this module does not add a "pull" concept.
//!
//! `bundle verify` answers "is this backup any good?" without restoring it: it reads a bundle file
//! and reports whether it is structurally sound and internally consistent, writing nothing and
//! needing no repository — `run_verify` below never calls `crate::open_repository`, unlike
//! `run_export`/`run_import`. It shares `import`'s own decode and closure-validation path
//! (`prikk_store::verify_bundle`, DC-44 increment 1 §2) rather than a second decoder, so the two
//! cannot silently drift apart on what counts as well-formed.
//!
//! **`bundle export` and `bundle verify` both print the bundle's self-describing manifest**
//! (DC-44 increment 3, `bundle-manifest-handoff-v1.md`): the on-disk repository format and tool
//! version that produced it, and a fixed note stating this bundle is one ref's closure only --
//! other refs in the source repository, if any, are not included, and the bundle makes no claim
//! about them. A `PBNDL001`/`PBNDL002` bundle predates the manifest section, so `verify` says so
//! plainly rather than printing absent fields as if they were checked and found empty.

use std::path::PathBuf;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::bounded_read::{SizeBound, read_bounded_file, render_incoming_error};
use crate::commands::CliError;
use crate::config::resolve_max_object_bytes;
use crate::output::{print_bundle_preview_json, print_bundle_preview_plain};
use crate::stdout::println;
use prikk_store::{
    BundleImportOptions, BundleManifest, BundleScope, DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
    DEFAULT_BUNDLE_MAX_OBJECT_COUNT, DEFAULT_BUNDLE_MAX_TOTAL_BYTES, export_bundle, import_bundle,
    verify_bundle,
};

/// Dispatch `prikk bundle [export|import|preview|verify]`.
pub fn run_bundle(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("export") => run_export(root, iter.collect()),
        Some("import") => run_import(root, iter.collect()),
        Some("preview") => run_preview(root, iter.collect()),
        Some("verify") => run_verify(iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown bundle subcommand: {other} (expected export, import, preview, or verify)"
        ))),
        None => Err(CliError::Usage(
            "bundle requires a subcommand: export, import, preview, or verify".to_string(),
        )),
    }
}

fn run_export(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_export_args(args)?;
    // DC-44 increment 2 §3.1: checked before opening the repository at all, so the common
    // "there is already a backup at this path" case fails fast rather than after a read. Not
    // race-free against a file created after this check -- see `durable_output`'s own doc
    // comment for why that is an accepted, stated limit rather than a gap this pretends to close.
    if !parsed.force && crate::durable_output::destination_exists(&parsed.output) {
        return Err(format!(
            "refusing to overwrite existing file at {} (pass --force to overwrite it \
             intentionally)",
            parsed.output.display()
        )
        .into());
    }
    let layout = crate::open_repository(root)?;
    let (report, bytes) =
        export_bundle(&layout, &parsed.ref_name).map_err(|err| err.to_string())?;
    // DC-44 increment 2 §3.2: atomic and durable -- a failure here leaves whatever was previously
    // at `parsed.output` untouched, never a partial file. See `durable_output`'s own doc comment
    // for exactly what this does and does not guarantee.
    crate::durable_output::write_new_file_durably(&parsed.output, &bytes)?;
    println!("exported {}", report.ref_name);
    println!("tip block: {}", report.tip_block_id);
    println!("objects: {}", report.object_count);
    println!(
        "author key material: {} included (continuity only, not a trust decision)",
        report.author_key_count
    );
    print_manifest(&report.manifest);
    println!("wrote {}", parsed.output.display());
    Ok(())
}

fn run_import(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_import_args(args)?;
    let layout = crate::open_repository(root)?;
    // RFC 158 Stage A §1: refused before a byte is read, on the open handle's own metadata.
    let bytes = read_bounded_file("bundle", &parsed.input, &bundle_total_size_bound()?)?;
    let object_bound = resolve_max_object_bytes(
        parsed.max_object_bytes,
        Some(&layout),
        DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
    )?;
    let options = bundle_import_options(&object_bound)?;
    let report = import_bundle(&layout, &bytes, &options)
        .map_err(|err| render_incoming_error(err, &object_bound))?;
    println!("received {}", report.ref_name);
    println!("RefState: {}", report.ref_state_id);
    println!("objects: {}", report.object_count);
    println!("new objects: {}", report.written_object_count);
    // RFC 156: shown only when it happened, so an ordinary import reads exactly as before.
    if report.merged_object_count > 0 {
        println!("objects gaining signatures: {}", report.merged_object_count);
    }
    for dropped in &report.dropped_signatures {
        println!(
            "signature not stored: {:?} {} on {} {} -- {}",
            dropped.signer_role,
            dropped.key_id,
            dropped.object_type,
            dropped.object_id,
            dropped.reason.describe()
        );
    }
    println!(
        "author key material: {} recorded (continuity only, not a trust decision)",
        report.recorded_author_key_count
    );
    // The note says what the import gave and stops there. It used to advise `merge`, which refuses in a
    // fresh repository (`ref heads/main is not published`): a received ref cannot become a local branch
    // today, by any route, and `docs/src/guide/backup-restore.md` says so.
    println!(
        "note: no local ref was created or advanced, and no MAINTAINER key was trusted; this \
         repository now holds the received history at {ref_name}, which `prikk log --ref {ref_name}` \
         and `prikk show` read, and `prikk verify` checks once `trust maintainer add` trusts the \
         sealing key",
        ref_name = report.ref_name
    );
    Ok(())
}

fn run_preview(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_preview_args(args)?;
    let layout = crate::open_repository(root)?;
    // RFC 158 Stage A §1: refused before a byte is read, on the open handle's own metadata.
    let bytes = read_bounded_file("bundle", &parsed.input, &bundle_total_size_bound()?)?;
    let object_bound = resolve_max_object_bytes(
        parsed.max_object_bytes,
        Some(&layout),
        DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
    )?;
    let options = bundle_import_options(&object_bound)?;
    // RFC 151 §2.2: the local branch the preview compares against, not anything the bundle carries.
    let ref_name = crate::current_branch::resolve_ref(&layout, parsed.ref_name)?;
    let (report, anchor_fallbacks) =
        prikk_store::preview_bundle_reporting_anchor(&layout, &bytes, &options, &ref_name)
            .map_err(|err| render_incoming_error(err, &object_bound))?;
    // RFC 136 §10.3b.4: a snapshot the preview could not anchor at is named on stderr.
    crate::warn_anchor_fallbacks(anchor_fallbacks.iter());
    if parsed.format_json {
        print_bundle_preview_json(&report);
    } else {
        print_bundle_preview_plain(&report);
    }
    Ok(())
}

struct PreviewArgs {
    input: PathBuf,
    ref_name: Option<String>,
    format_json: bool,
    max_object_bytes: Option<u64>,
}

fn parse_preview_args(args: Vec<String>) -> std::result::Result<PreviewArgs, CliError> {
    let mut input = None;
    let mut ref_name = None;
    let mut format_json = false;
    let mut max_object_bytes = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--input" => {
                let value = flag_value(&mut iter, "bundle preview --input")?;
                input.set_once("--input", PathBuf::from(value))?;
            }
            "--max-object-bytes" => {
                let value = flag_value(&mut iter, "bundle preview --max-object-bytes")?;
                let parsed = crate::bounded_read::parse_max_object_bytes_value(&value)?;
                max_object_bytes.set_once("--max-object-bytes", parsed)?;
            }
            "--ref" => {
                let value = flag_value(&mut iter, "bundle preview --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "bundle preview --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--format" => {
                let value = flag_value(&mut iter, "bundle preview --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "bundle preview --format only accepts \"json\", got {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other => return Err(unknown_argument("bundle preview", other)),
        }
    }
    let input =
        input.ok_or_else(|| CliError::Usage("bundle preview requires --input".to_string()))?;
    Ok(PreviewArgs {
        input,
        ref_name,
        format_json,
        max_object_bytes,
    })
}

fn run_verify(args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_verify_args(args)?;
    // RFC 158 Stage A §1: refused before a byte is read, on the open handle's own metadata.
    let bytes = read_bounded_file("bundle", &parsed.input, &bundle_total_size_bound()?)?;
    // No repository (this command's own module doc: `run_verify` never calls `open_repository`),
    // so `prikk config` never applies here and never appears in this refusal (handoff §3).
    let object_bound = resolve_max_object_bytes(
        parsed.max_object_bytes,
        None,
        DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
    )?;
    let options = bundle_import_options(&object_bound)?;
    let report =
        verify_bundle(&bytes, &options).map_err(|err| render_incoming_error(err, &object_bound))?;
    println!("bundle verifies: {}", report.ref_name);
    println!("RefState: {}", report.ref_state_id);
    println!("tip block: {}", report.tip_block_id);
    println!("objects: {}", report.object_count);
    println!(
        "author key material: {} present (continuity only, not a trust decision)",
        report.author_key_count
    );
    match &report.manifest {
        Some(manifest) => print_manifest(manifest),
        None => println!(
            "manifest: not present (this bundle predates the PBNDL003 manifest section -- \
             repository format, tool version, and scope are unknown)"
        ),
    }
    println!(
        "note: this checks structural and internal consistency only -- no signature is \
         cryptographically verified (a standalone bundle carries no trust material to check one \
         against), and this bundle's own author-key section is recorded here, never \
         independently verified, the same as at import. A verified bundle is not yet a trusted \
         one -- import it and run `prikk verify` for that."
    );
    Ok(())
}

/// DC-44 increment 3 §4.3: printed by both `bundle export` and `bundle verify` for a
/// manifest-bearing (`PBNDL003`) bundle. `BundleScope` has exactly one variant today, so the
/// `match` prints one fixed sentence -- kept as a match, not an `if let`, so a future second
/// variant forces this print site to be revisited rather than silently keep the old wording.
fn print_manifest(manifest: &BundleManifest) {
    println!("repository format: {}", manifest.repository_format);
    println!("tool version: {}", manifest.tool_version);
    match manifest.scope {
        BundleScope::SingleRef => println!(
            "note: this bundle contains one ref's closure only -- other refs in the source \
             repository, if any, are not included, and this bundle makes no claim about them"
        ),
    }
}

struct VerifyArgs {
    input: PathBuf,
    max_object_bytes: Option<u64>,
}

fn parse_verify_args(args: Vec<String>) -> std::result::Result<VerifyArgs, CliError> {
    let mut input = None;
    let mut max_object_bytes = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--input" => {
                let value = flag_value(&mut iter, "bundle verify --input")?;
                input.set_once("--input", PathBuf::from(value))?;
            }
            "--max-object-bytes" => {
                let value = flag_value(&mut iter, "bundle verify --max-object-bytes")?;
                let parsed = crate::bounded_read::parse_max_object_bytes_value(&value)?;
                max_object_bytes.set_once("--max-object-bytes", parsed)?;
            }
            other => return Err(unknown_argument("bundle verify", other)),
        }
    }
    let input =
        input.ok_or_else(|| CliError::Usage("bundle verify requires --input".to_string()))?;
    Ok(VerifyArgs {
        input,
        max_object_bytes,
    })
}

/// DC-86: the bundle's total-byte bound from `PRIKK_BUNDLE_MAX_BYTES`, with a description of where
/// it came from (RFC 158 Stage A §2) for [`crate::bounded_read::read_bounded_file`]'s own refusal.
fn bundle_total_size_bound() -> std::result::Result<SizeBound, CliError> {
    Ok(SizeBound::from_env(
        "PRIKK_BUNDLE_MAX_BYTES",
        DEFAULT_BUNDLE_MAX_TOTAL_BYTES,
    )?)
}

/// `BundleImportOptions` from `PRIKK_BUNDLE_MAX_OBJECTS` (DC-86, unchanged) and the already-resolved
/// per-object bound (RFC 158 Stage A §3) — `object_bound.bytes` came from `--max-object-bytes`,
/// `prikk config`, or the default, resolved once by [`resolve_max_object_bytes`], not re-derived
/// here.
fn bundle_import_options(
    object_bound: &SizeBound,
) -> std::result::Result<BundleImportOptions, CliError> {
    let max_object_count =
        parse_bundle_limit_env("PRIKK_BUNDLE_MAX_OBJECTS", DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let max_total_bytes =
        parse_bundle_limit_env("PRIKK_BUNDLE_MAX_BYTES", DEFAULT_BUNDLE_MAX_TOTAL_BYTES)?;
    Ok(BundleImportOptions::default_limits()
        .with_max_object_count(max_object_count)
        .with_max_total_bytes(max_total_bytes)
        .with_max_object_bytes(usize::try_from(object_bound.bytes).unwrap_or(usize::MAX)))
}

fn parse_bundle_limit_env(name: &str, default: usize) -> std::result::Result<usize, String> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    let value: usize = trimmed
        .parse()
        .map_err(|_| format!("{name} must be a positive integer, got {raw:?}"))?;
    if value == 0 {
        return Err(format!("{name} must be greater than zero, got 0"));
    }
    Ok(value)
}

struct ExportArgs {
    ref_name: String,
    output: PathBuf,
    force: bool,
}

fn parse_export_args(args: Vec<String>) -> std::result::Result<ExportArgs, CliError> {
    let mut ref_name = None;
    let mut output = None;
    let mut force = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "bundle export --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "bundle export --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--output" => {
                let value = flag_value(&mut iter, "bundle export --output")?;
                output.set_once("--output", PathBuf::from(value))?;
            }
            "--force" => mark_seen(&mut force, "--force")?,
            other => return Err(unknown_argument("bundle export", other)),
        }
    }
    let ref_name =
        ref_name.ok_or_else(|| CliError::Usage("bundle export requires --ref".to_string()))?;
    let output =
        output.ok_or_else(|| CliError::Usage("bundle export requires --output".to_string()))?;
    Ok(ExportArgs {
        ref_name,
        output,
        force,
    })
}

struct ImportArgs {
    input: PathBuf,
    max_object_bytes: Option<u64>,
}

fn parse_import_args(args: Vec<String>) -> std::result::Result<ImportArgs, CliError> {
    let mut input = None;
    let mut max_object_bytes = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--input" => {
                let value = flag_value(&mut iter, "bundle import --input")?;
                input.set_once("--input", PathBuf::from(value))?;
            }
            "--max-object-bytes" => {
                let value = flag_value(&mut iter, "bundle import --max-object-bytes")?;
                let parsed = crate::bounded_read::parse_max_object_bytes_value(&value)?;
                max_object_bytes.set_once("--max-object-bytes", parsed)?;
            }
            other => return Err(unknown_argument("bundle import", other)),
        }
    }
    let input =
        input.ok_or_else(|| CliError::Usage("bundle import requires --input".to_string()))?;
    Ok(ImportArgs {
        input,
        max_object_bytes,
    })
}
