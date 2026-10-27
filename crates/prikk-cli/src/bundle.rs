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
use crate::bounded_read::{SizeBound, open_bundle_source, render_incoming_error};
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
    // 0.55.0 step 3 Part 3b-i2 (D1/Q2): a regular file stays an open, seekable handle -- memory
    // follows the largest object, not the whole bundle. A non-regular input (a FIFO, a pipe) is
    // still fully buffered and bounded before a byte reaches the store, exactly as before.
    let mut source = open_bundle_source("bundle", &parsed.input, &bundle_total_size_bound()?)?;
    let object_bound = resolve_max_object_bytes(
        parsed.max_object_bytes,
        Some(&layout),
        DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
    )?;
    let options = bundle_import_options(&object_bound)?;
    let report = import_bundle(&layout, &mut source, &options)
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
    // 0.55.0 step 3 Part 3b-iii: a regular file stays an open, seekable handle, the same as
    // `bundle import` -- memory follows the bundle's decoded content, not a second raw-bytes copy
    // of the whole file. A non-regular input (a FIFO, a pipe) is still fully buffered and bounded
    // before a byte reaches the store, exactly as before.
    let mut source = open_bundle_source("bundle", &parsed.input, &bundle_total_size_bound()?)?;
    let object_bound = resolve_max_object_bytes(
        parsed.max_object_bytes,
        Some(&layout),
        DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
    )?;
    let options = bundle_import_options(&object_bound)?;
    // RFC 151 §2.2: the local branch the preview compares against, not anything the bundle carries.
    let ref_name = crate::current_branch::resolve_ref(&layout, parsed.ref_name)?;
    let (report, anchor_fallbacks) =
        prikk_store::preview_bundle_reporting_anchor(&layout, &mut source, &options, &ref_name)
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
    // 0.55.0 step 3 Part 3b-iii: a regular file stays an open, seekable handle (same as `bundle
    // import`/`bundle preview`). A non-regular input (a FIFO, a pipe) is still fully buffered and
    // bounded before a byte reaches the store, exactly as before.
    let mut source = open_bundle_source("bundle", &parsed.input, &bundle_total_size_bound()?)?;
    // No repository (this command's own module doc: `run_verify` never calls `open_repository`),
    // so `prikk config` never applies here and never appears in this refusal (handoff §3).
    let object_bound = resolve_max_object_bytes(
        parsed.max_object_bytes,
        None,
        DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
    )?;
    let options = bundle_import_options(&object_bound)?;
    let report = verify_bundle(&mut source, &options)
        .map_err(|err| render_incoming_error(err, &object_bound))?;
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
/// it came from (RFC 158 Stage A §2) for [`crate::bounded_read::open_bundle_source`]'s own
/// refusal -- shared by all three commands that read a bundle (`import`, `preview`, `verify`,
/// 0.55.0 step 3 Part 3b-iii).
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::run_export;
    use prikk_store::{MaintainerSigner, ObjectWriter};

    /// 0.52.0 step 1 Part A round 2 (A-2): through the real `run_export` path, including the real
    /// `durable_output::write_new_file_durably` call -- not the library-level `export_bundle`,
    /// which never touches a path at all (it returns bytes to its caller). The bound is lowered on
    /// this thread only (`feature = "test-support"`), so an over-bound object can be a tiny,
    /// ordinary sealed `Patch` rather than a real 257 MiB one. The sealed history is built the same
    /// way `archive::tests`' own completable-lead fixture builds one (a real signer, trusted, a
    /// real Block/RefState/RefUpdate), since `export_bundle` requires a genuinely existing,
    /// resolvable ref -- unlike `archive export`'s own over-bound scan, which reads every
    /// persisted object regardless of ref reachability.
    #[test]
    fn bundle_export_writes_nothing_at_the_destination_when_an_object_is_over_bound() {
        let dir = std::env::temp_dir().join(format!(
            "cli-bundle-export-over-bound-nothing-written-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create the repo root dir");
        let layout = prikk_store::RepositoryLayout::init(&dir).expect("init");

        let signer =
            prikk_store::Ed25519MaintainerSigner::from_seed("a2-bundle-maintainer", &[0x73; 32])
                .expect("build signer");
        let public_key_hex = signer
            .public_key_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        prikk_store::add_trusted_maintainer(&layout, signer.key_id(), &public_key_hex)
            .expect("trust the maintainer");

        let mut object_store = prikk_store::FileObjectStore::new(layout.clone());
        object_store
            .write_object(&signed_blob(b"patch fixture\n"))
            .expect("write the patch's own referenced blob");
        let patch_payload = prikk_object::PatchPayload {
            operations: vec![prikk_object::Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: prikk_object::OperationKind::CreateFile(prikk_object::CreateFile {
                    path: "a.txt".to_string(),
                    node_id: prikk_object::NodeId::from_bytes([0x61; 32]),
                    blob_id: signed_blob_id(b"patch fixture\n"),
                    mode: 0o100_644,
                }),
            }],
            intent: None,
            preconditions: Vec::new(),
            purpose: prikk_object::PatchPurpose::Normal,
            message: None,
        };
        let patch_bytes = prikk_object::CanonicalEncode::to_canonical_bytes(&patch_payload)
            .expect("encode patch");
        let patch =
            prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Patch, 1, patch_bytes);
        let patch_id = object_store
            .write_object(&patch)
            .expect("write the patch object");

        let block_payload = prikk_object::BlockPayload {
            parent_block_ids: Vec::new(),
            kind: prikk_object::BlockKind::Root,
            patch_ids: vec![patch_id],
            state_merkle_root: prikk_store::derive_next_state_root(
                &object_store,
                None,
                &[patch_id],
            )
            .expect("derive state root"),
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        };
        let block_bytes = prikk_object::CanonicalEncode::to_canonical_bytes(&block_payload)
            .expect("encode block");
        let mut block_envelope =
            prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Block, 2, block_bytes);
        block_envelope
            .add_signature(
                prikk_store::maintainer_signature(
                    &signer,
                    prikk_object::ObjectType::Block,
                    block_envelope.object_id(),
                )
                .expect("sign block"),
            )
            .expect("attach block signature");
        let block_id = object_store
            .write_object(&block_envelope)
            .expect("write the block");

        let state_payload = prikk_object::RefStatePayload {
            ref_name: "heads/main".to_string(),
            kind: prikk_object::RefKind::Branch,
            target_object_id: block_id,
            update_seq: 1,
            previous_ref_state_id: None,
            required_attestation_ids: Vec::new(),
            closed: false,
        };
        let state_bytes = prikk_object::CanonicalEncode::to_canonical_bytes(&state_payload)
            .expect("encode ref state");
        let mut ref_state = prikk_object::ObjectEnvelope::unsigned(
            prikk_object::ObjectType::RefState,
            1,
            state_bytes,
        );
        let ref_state_id_for_sig = ref_state.object_id();
        ref_state
            .add_signature(
                prikk_store::maintainer_signature(
                    &signer,
                    prikk_object::ObjectType::RefState,
                    ref_state_id_for_sig,
                )
                .expect("sign ref state"),
            )
            .expect("attach ref state signature");
        let ref_state_id = ref_state.object_id();

        let update_payload = prikk_object::RefUpdatePayload {
            ref_name: "heads/main".to_string(),
            old_ref_state_id: None,
            new_ref_state_id: ref_state_id,
            new_target_object_id: block_id,
            update_seq: 1,
            created_at: 0,
            author_key_id: signer.key_id().to_string(),
        };
        let update_bytes = prikk_object::CanonicalEncode::to_canonical_bytes(&update_payload)
            .expect("encode ref update");
        let mut ref_update = prikk_object::ObjectEnvelope::unsigned(
            prikk_object::ObjectType::RefUpdate,
            1,
            update_bytes,
        );
        let ref_update_id_for_sig = ref_update.object_id();
        ref_update
            .add_signature(
                prikk_store::maintainer_signature(
                    &signer,
                    prikk_object::ObjectType::RefUpdate,
                    ref_update_id_for_sig,
                )
                .expect("sign ref update"),
            )
            .expect("attach ref update signature");

        prikk_store::RefStore::new(layout.clone())
            .publish(&prikk_store::RefPublication {
                ref_name: "heads/main".to_string(),
                expected_previous_ref_state_id: None,
                ref_state,
                ref_update,
            })
            .expect("publish heads/main");

        prikk_store::lower_bundle_object_over_bound_threshold_for_test(4);

        let output = dir.join("out.bundle");
        let result = run_export(
            dir.clone(),
            vec![
                "--ref".to_string(),
                "heads/main".to_string(),
                "--output".to_string(),
                output.to_str().unwrap().to_string(),
            ],
        );

        prikk_store::clear_bundle_object_over_bound_threshold_for_test();

        assert!(result.is_err(), "export must refuse over-bound objects");
        assert!(
            !output.exists(),
            "a refused export must leave nothing at the destination path: {}",
            output.display()
        );
        let siblings: Vec<_> = std::fs::read_dir(&dir)
            .expect("read the repo dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .collect();
        assert!(
            !siblings
                .iter()
                .any(|name| name.to_string_lossy().contains("out.bundle")),
            "no temporary file naming the destination may remain: {siblings:?}"
        );
    }

    fn signed_blob(content: &[u8]) -> prikk_object::ObjectEnvelope {
        let payload =
            prikk_object::BlobPayload::new(prikk_object::BlobKind::Text, content.to_vec());
        let bytes =
            prikk_object::CanonicalEncode::to_canonical_bytes(&payload).expect("encode blob");
        prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Blob, 1, bytes)
    }

    fn signed_blob_id(content: &[u8]) -> prikk_object::ObjectId {
        signed_blob(content).object_id()
    }
}
