//! `prikk sync` — RFC 116's negotiation loop, driven from the command line (stage 4 handoff).
//!
//! **No network. No socket. No new dependency.** Every subcommand reads and writes local files
//! only; the file moves between repositories by whatever channel the operator already has (RFC
//! 116 ruling 2). **Every input file is untrusted** -- it arrived by a channel prikk knows nothing
//! about, and each subcommand's own library call is what refuses a bad one. This module adds no
//! parsing or bound-checking of its own beyond argument handling and file I/O.
//!
//! Modelled directly on `bundle.rs`: one dispatcher, one function per subcommand, the same
//! argument-parsing idiom, the same "print counts, then note what to do next" reporting style.
//!
//! The full loop, across two repositories A (has new content) and B (wants it):
//! `A: sync summary` → `B: sync compare` → `B: sync have` → `A: sync build` → `B: sync accept` →
//! `B: sync pending` (optional, observational) → `B: sync seal`. RFC 117 stage 3 adds a tag-adoption
//! side loop after `accept`, independent of sealing: `B: sync tags` (observational) →
//! `B: sync adopt-tag <name>` (explicit, receiver-signed).
//!
//! **Claim ids move by file, like every other step (RFC 116 stage 5 §2).** `sync accept` can
//! write them to `--claims-out <file>`; `sync seal <ref> --claims <file>` reads them back,
//! orders them with [`prikk_store::order_claims_for_sealing`] (parent blocks before children,
//! derived from the claims' own signed `parent_block_ids` -- never from the file's own order or
//! from id order), and seals each in turn, stopping at the first failure and reporting exactly how
//! far it got (§3: no rollback -- each seal is an independent, legitimate act under the receiver's
//! own key). `sync seal <ref> --claim <id>` still exists for the single-claim case.
//!
//! **Received tags move by no file at all (RFC 117 stage 3 §4).** Unlike claims, "which tags has
//! this repository received" needs no channel of its own: [`prikk_store::received_tag_ids`] derives
//! it fresh from the local object store every time (module doc there), so `sync tags`/
//! `sync adopt-tag <name>` take no `--tags-out`/`--tags <file>` pair -- there is nothing to hand
//! back and forth between two invocations on the *same* repository.
//!
//! **All four writes to a user-supplied output path go through `crate::durable_output`, not
//! `std::fs::write`** (DC-44 increment 2's own report surfaced this; `sync-output-durability-
//! handoff-v1.md`). Only two of the four also refuse an existing destination: `sync build`'s
//! artifact and `sync accept --claims-out`'s claim-id file are each the one durable record of an
//! otherwise-expensive or otherwise-unrecoverable step, so both take `--force` with `bundle
//! export`'s own meaning (§3.1 there: proceed past a safety stop). `sync summary` and `sync have`
//! write a derived, freely-regenerable view of local state that a normal loop iteration expects to
//! overwrite on every run, so neither gates on `--force` at all -- gating them would make
//! `--force` mean "do the ordinary thing" in two places and "override a real safety stop" in the
//! other two, which is worse than a flag that simply does not exist for some commands. Both still
//! get the atomic, durable write; only the collision policy differs.

use std::path::PathBuf;

use prikk_object::ObjectId;
use prikk_store::{
    AcceptOptions, ClaimSignatureVerification, DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT,
    DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES, DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT,
    DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES, ObjectReadSnapshot, ReceivedTagResolution,
    SealFromAcceptedOutcome, SyncArtifactOutcome, TagSignatureVerification,
    accept_exchange_artifact, accepted_but_unsealed_patch_ids, adopt_tag, build_have_list,
    build_sync_artifact, build_sync_summary, compare_sync_summary, decode_sync_summary,
    list_received_tags, order_claims_for_sealing, seal_from_accepted_claim,
};

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::maintainer_signer_from_env;

/// Dispatch `prikk sync [summary|compare|have|build|accept|pending|seal|tags|adopt-tag]`.
pub fn run_sync(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("summary") => run_summary(root, iter.collect()),
        Some("compare") => run_compare(root, iter.collect()),
        Some("have") => run_have(root, iter.collect()),
        Some("build") => run_build(root, iter.collect()),
        Some("accept") => run_accept(root, iter.collect()),
        Some("pending") => run_pending(root, iter.collect()),
        Some("seal") => run_seal(root, iter.collect()),
        Some("tags") => run_tags(root, iter.collect()),
        Some("adopt-tag") => run_adopt_tag(root, iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown sync subcommand: {other} (expected summary, compare, have, build, accept, \
             pending, seal, tags, or adopt-tag)"
        ))),
        None => Err(CliError::Usage(
            "sync requires a subcommand: summary, compare, have, build, accept, pending, seal, \
             tags, or adopt-tag"
                .to_string(),
        )),
    }
}

fn run_summary(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_output_args(args, "sync summary")?;
    let layout = crate::open_repository(root)?;
    let bytes = build_sync_summary(&layout).map_err(|err| err.to_string())?;
    let entries = decode_sync_summary(
        &bytes,
        DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES,
        DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT,
    )
    .map_err(|err| err.to_string())?;
    crate::durable_output::write_new_file_durably(&parsed.output, &bytes)?;
    println!("refs: {}", entries.len());
    println!("wrote {}", parsed.output.display());
    Ok(())
}

fn run_compare(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_summary_input_args(args, "sync compare")?;
    let layout = crate::open_repository(root)?;
    let bytes = std::fs::read(&parsed.summary).map_err(|err| {
        format!(
            "failed to read sync summary from {}: {err}",
            parsed.summary.display()
        )
    })?;
    let (max_total_bytes, max_ref_count) = sync_summary_limits_from_env()?;
    let remote = decode_sync_summary(&bytes, max_total_bytes, max_ref_count)
        .map_err(|err| err.to_string())?;
    let comparisons = compare_sync_summary(&layout, &remote).map_err(|err| err.to_string())?;
    for comparison in &comparisons {
        println!("{} {}", comparison.ref_name, comparison.state.as_str());
    }
    println!("refs compared: {}", comparisons.len());
    Ok(())
}

fn run_have(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_ref_and_output_args(args, "sync have")?;
    let layout = crate::open_repository(root)?;
    let bytes = build_have_list(&layout, &parsed.ref_name).map_err(|err| err.to_string())?;
    crate::durable_output::write_new_file_durably(&parsed.output, &bytes)?;
    println!("have-list for {}", parsed.ref_name);
    println!("wrote {}", parsed.output.display());
    Ok(())
}

fn run_build(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_build_args(args)?;
    let layout = crate::open_repository(root)?;
    let have_list_bytes = std::fs::read(&parsed.have).map_err(|err| {
        format!(
            "failed to read have-list from {}: {err}",
            parsed.have.display()
        )
    })?;
    let signer = maintainer_signer_from_env()?;
    let outcome = build_sync_artifact(&layout, &parsed.ref_name, &have_list_bytes, &signer)
        .map_err(|err| err.to_string())?;
    match outcome {
        SyncArtifactOutcome::AlreadyInSync { ref_name } => {
            println!("{ref_name} is already in sync -- no artifact written");
        }
        SyncArtifactOutcome::Artifact { report, bytes } => {
            // The collision check sits here, not before `open_repository` like `bundle export`'s
            // own (handoff §3.3): whether this call writes anything at all depends on `outcome`,
            // which is only known after the delta is computed. Checking any earlier would refuse
            // an `AlreadyInSync` run over a pre-existing file it was never going to touch.
            if !parsed.force && crate::durable_output::destination_exists(&parsed.output) {
                return Err(format!(
                    "refusing to overwrite existing file at {} (pass --force to overwrite it \
                     intentionally)",
                    parsed.output.display()
                )
                .into());
            }
            crate::durable_output::write_new_file_durably(&parsed.output, &bytes)?;
            println!("built sync artifact for {}", report.ref_name);
            println!("delta patches: {}", report.delta_patch_count);
            println!("claims: {}", report.claim_count);
            println!("tags: {}", report.tag_count);
            println!(
                "blobs: {} | author key material: {}",
                report.export_report.blob_count, report.export_report.author_key_count
            );
            println!("wrote {}", parsed.output.display());
            // RFC 116 stage 7 handoff, Part B: prikk guarantees integrity and authenticity, never
            // secrecy -- this file now exists on disk with repository content in the clear. Once,
            // on the command that creates the exposure; no prompt and no flag to silence it, both
            // of which would defeat the point.
            println!(
                "note: this artifact contains repository content in the clear -- prikk does not \
                 encrypt it; move it only over a channel you trust"
            );
        }
    }
    Ok(())
}

fn run_accept(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_accept_args(args)?;
    // Checked before `open_repository`, and therefore before `accept_exchange_artifact` writes
    // any object -- unlike `sync build`, accepting is a real repository mutation (handoff §3.3),
    // so refusing here means a doomed `--claims-out` write never lets that mutation happen at all.
    if let Some(claims_out) = &parsed.claims_out {
        if !parsed.force && crate::durable_output::destination_exists(claims_out) {
            return Err(format!(
                "refusing to overwrite existing file at {} (pass --force to overwrite it \
                 intentionally)",
                claims_out.display()
            )
            .into());
        }
    }
    let layout = crate::open_repository(root)?;
    let bytes = std::fs::read(&parsed.input).map_err(|err| {
        format!(
            "failed to read sync artifact from {}: {err}",
            parsed.input.display()
        )
    })?;
    let options = accept_options_from_env()?;
    let report =
        accept_exchange_artifact(&layout, &bytes, &options).map_err(|err| err.to_string())?;
    println!("accepted sync artifact");
    println!("patches: {}", report.patch_count);
    println!("blobs: {}", report.blob_count);
    println!("claims: {}", report.claim_count);
    println!("tags: {}", report.tag_count);
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
    // Load-bearing output (handoff §3): this is the only way an operator learns what to pass to
    // `sync seal`. Print `Unverifiable` outcomes plainly -- the operator must be able to see they
    // are about to seal on an unattributed order (D6 §11.6), not have it hidden behind a summary.
    for (claim_id, outcome) in &report.claim_signature_outcomes {
        match outcome {
            ClaimSignatureVerification::Sound { key_id } => {
                println!("  claim {claim_id}: Sound ({key_id})");
            }
            ClaimSignatureVerification::Unverifiable { key_id } => {
                println!(
                    "  claim {claim_id}: Unverifiable ({key_id} not adopted here -- its order is \
                     unattributed)"
                );
            }
        }
    }
    // RFC 117 stage 3 §3: a received tag's signature outcome is reported, never gating -- the same
    // treatment claims get above. Adoption (`sync adopt-tag`) is a wholly separate, later act; this
    // print is purely observational, the same role the claim print has before `sync seal`.
    for (tag_id, outcome) in &report.tag_signature_outcomes {
        match outcome {
            TagSignatureVerification::Sound { key_id } => {
                println!("  tag {tag_id}: Sound ({key_id})");
            }
            TagSignatureVerification::Unverifiable { key_id } => {
                println!("  tag {tag_id}: Unverifiable ({key_id} not adopted here)");
            }
        }
    }
    // §2: claim ids move by file, the same as every other step in this loop -- not by
    // string-splitting stdout, the one exception that inconsistency existed for no reason.
    if let Some(claims_out) = &parsed.claims_out {
        let mut content = String::new();
        for (claim_id, _) in &report.claim_signature_outcomes {
            content.push_str(&claim_id.to_string());
            content.push('\n');
        }
        crate::durable_output::write_new_file_durably(claims_out, content.as_bytes())?;
        println!("wrote claim ids to {}", claims_out.display());
    }
    Ok(())
}

fn run_pending(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    require_no_args(args, "sync pending")?;
    let layout = crate::open_repository(root)?;
    let patch_ids = accepted_but_unsealed_patch_ids(&layout).map_err(|err| err.to_string())?;
    println!("pending (accepted, unsealed) patches: {}", patch_ids.len());
    for patch_id in &patch_ids {
        println!("  {patch_id}");
    }
    Ok(())
}

/// `prikk sync tags` -- list received tags with their name, live signature outcome, and current
/// resolution state (RFC 117 stage 3 §4). Purely observational; takes no input file (module doc).
fn run_tags(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    require_no_args(args, "sync tags")?;
    let layout = crate::open_repository(root)?;
    let summaries = list_received_tags(&layout).map_err(|err| err.to_string())?;
    if summaries.is_empty() {
        println!("no received tags");
        return Ok(());
    }
    println!("received tags: {}", summaries.len());
    for summary in &summaries {
        println!("{} ({})", summary.name, summary.tag_id);
        match &summary.signature_outcome {
            TagSignatureVerification::Sound { key_id } => {
                println!("  signature: Sound ({key_id})");
            }
            TagSignatureVerification::Unverifiable { key_id } => {
                println!("  signature: Unverifiable ({key_id} not adopted here)");
            }
        }
        match &summary.resolution {
            ReceivedTagResolution::Resolved(block_id) => {
                println!("  resolution: Resolved {block_id}");
            }
            ReceivedTagResolution::NotHeld => {
                println!(
                    "  resolution: NotHeld (not enough of this repository's history has been \
                     synced yet)"
                );
            }
            ReceivedTagResolution::Ambiguous { detail } => {
                println!("  resolution: Ambiguous -- {detail}");
            }
        }
    }
    Ok(())
}

/// `prikk sync adopt-tag <name>` -- resolve the one received tag named `<name>` to a local block and
/// create a **local**, receiver-signed tag naming it (RFC 117 T4; `tag_travel::adopt_tag`). Refuses
/// on `NotHeld`, on ambiguity (either T2's patch-set ambiguity or two received tags sharing a name),
/// or if a local tag by that name already exists.
fn run_adopt_tag(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    let Some(name) = iter.next() else {
        return Err(CliError::Usage(
            "sync adopt-tag requires <name>".to_string(),
        ));
    };
    if name.starts_with("--") {
        return Err(CliError::Usage(
            "sync adopt-tag requires <name> before any flags".to_string(),
        ));
    }
    require_no_args(iter.collect(), "sync adopt-tag")?;

    let layout = crate::open_repository(root)?;
    let signer = maintainer_signer_from_env()?;
    let created = adopt_tag(&layout, &name, &signer).map_err(|err| err.to_string())?;
    println!("adopted tag {name}");
    println!("tag object: {}", created.tag_object_id);
    println!("RefState: {}", created.ref_state_id);
    Ok(())
}

fn run_seal(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_seal_args(args)?;
    let layout = crate::open_repository(root)?;
    let signer = maintainer_signer_from_env()?;
    match parsed.claims {
        SealClaims::Single(claim_id) => {
            let outcome = seal_from_accepted_claim(&layout, &parsed.ref_name, claim_id, &signer)
                .map_err(|err| err.to_string())?;
            print_seal_outcome(&outcome, claim_id);
        }
        SealClaims::Batch(claims_file) => {
            let content = std::fs::read_to_string(&claims_file).map_err(|err| {
                format!(
                    "failed to read claim ids from {}: {err}",
                    claims_file.display()
                )
            })?;
            let claim_ids: Vec<ObjectId> = content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| {
                    line.parse::<ObjectId>().map_err(|err| {
                        format!(
                            "{line:?} in {} is not a valid object id: {err}",
                            claims_file.display()
                        )
                    })
                })
                .collect::<std::result::Result<_, _>>()?;
            // §1: order by parent_block_ids, derived from signed data -- never by the file's own
            // input order or by id.
            let object_store = ObjectReadSnapshot::open(&layout).map_err(|err| err.to_string())?;
            let ordered = order_claims_for_sealing(&object_store, &claim_ids)
                .map_err(|err| err.to_string())?;
            // §3: stop at the first failure, report it, and leave the successful seals in place --
            // no rollback. Each `?` below exits before attempting the next claim, and every claim
            // already printed stays sealed regardless of what happens after it.
            for claim_id in ordered {
                let outcome =
                    seal_from_accepted_claim(&layout, &parsed.ref_name, claim_id, &signer)
                        .map_err(|err| {
                            println!("claim {claim_id}: FAILED: {err}");
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                            err.to_string()
                        })?;
                print_seal_outcome(&outcome, claim_id);
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
        }
    }
    Ok(())
}

fn print_seal_outcome(outcome: &SealFromAcceptedOutcome, claim_id: ObjectId) {
    match outcome {
        SealFromAcceptedOutcome::Sealed {
            ref_name,
            block_id,
            ref_state_id,
            patch_count,
            claim_signature_outcome,
        } => {
            println!("claim {claim_id}: sealed {patch_count} patch(es) into {ref_name}");
            println!("  block id: {block_id}");
            println!("  {ref_name} RefState: {ref_state_id}");
            match claim_signature_outcome {
                ClaimSignatureVerification::Sound { key_id } => {
                    println!("  claim signature: Sound ({key_id})");
                }
                ClaimSignatureVerification::Unverifiable { key_id } => {
                    println!("  claim signature: Unverifiable ({key_id} not adopted here)");
                }
            }
        }
        SealFromAcceptedOutcome::AlreadySealed { ref_name, .. } => {
            println!("claim {claim_id}: {ref_name} was already fully sealed -- no-op");
        }
    }
}

/// DC-86: `AcceptOptions` from `PRIKK_EXCHANGE_MAX_OBJECTS`/`PRIKK_EXCHANGE_MAX_BYTES`, the same
/// shape `bundle.rs`'s own `bundle_import_options_from_env` gives `BundleImportOptions` -- absent
/// means the documented default; present but non-numeric or zero is a hard error, never a silent
/// fallback.
fn accept_options_from_env() -> std::result::Result<AcceptOptions, String> {
    let max_object_count = parse_limit_env(
        "PRIKK_EXCHANGE_MAX_OBJECTS",
        DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT,
    )?;
    let max_total_bytes = parse_limit_env(
        "PRIKK_EXCHANGE_MAX_BYTES",
        DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES,
    )?;
    Ok(AcceptOptions::default_limits()
        .with_max_object_count(max_object_count)
        .with_max_total_bytes(max_total_bytes))
}

/// `PRIKK_SYNC_SUMMARY_MAX_BYTES`/`PRIKK_SYNC_SUMMARY_MAX_REFS` overrides for decoding a *remote*
/// summary in `sync compare` -- the one sync subcommand whose own decode step takes bound
/// parameters directly, so the same override shape applies here.
fn sync_summary_limits_from_env() -> std::result::Result<(usize, usize), String> {
    let max_total_bytes = parse_limit_env(
        "PRIKK_SYNC_SUMMARY_MAX_BYTES",
        DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES,
    )?;
    let max_ref_count = parse_limit_env(
        "PRIKK_SYNC_SUMMARY_MAX_REFS",
        DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT,
    )?;
    Ok((max_total_bytes, max_ref_count))
}

fn parse_limit_env(name: &str, default: usize) -> std::result::Result<usize, String> {
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

struct OutputArgs {
    output: PathBuf,
}

fn parse_output_args(
    args: Vec<String>,
    command: &str,
) -> std::result::Result<OutputArgs, CliError> {
    let mut output = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--output" => {
                let value = flag_value(&mut iter, &format!("{command} --output"))?;
                output.set_once("--output", PathBuf::from(value))?;
            }
            other => return Err(unknown_argument(command, other)),
        }
    }
    let output = output.ok_or_else(|| CliError::Usage(format!("{command} requires --output")))?;
    Ok(OutputArgs { output })
}

struct SummaryInputArgs {
    summary: PathBuf,
}

fn parse_summary_input_args(
    args: Vec<String>,
    command: &str,
) -> std::result::Result<SummaryInputArgs, CliError> {
    let mut summary = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--summary" => {
                let value = flag_value(&mut iter, &format!("{command} --summary"))?;
                summary.set_once("--summary", PathBuf::from(value))?;
            }
            other => return Err(unknown_argument(command, other)),
        }
    }
    let summary =
        summary.ok_or_else(|| CliError::Usage(format!("{command} requires --summary")))?;
    Ok(SummaryInputArgs { summary })
}

struct AcceptArgs {
    input: PathBuf,
    claims_out: Option<PathBuf>,
    force: bool,
}

fn parse_accept_args(args: Vec<String>) -> std::result::Result<AcceptArgs, CliError> {
    let mut iter = args.into_iter();
    let Some(input) = iter.next() else {
        return Err(CliError::Usage(
            "sync accept requires a file path".to_string(),
        ));
    };
    if input.starts_with("--") {
        return Err(CliError::Usage(
            "sync accept requires a file path before any flags".to_string(),
        ));
    }
    let mut claims_out = None;
    let mut force = false;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--claims-out" => {
                let value = flag_value(&mut iter, "sync accept --claims-out")?;
                claims_out.set_once("--claims-out", PathBuf::from(value))?;
            }
            "--force" => mark_seen(&mut force, "--force")?,
            other => return Err(unknown_argument("sync accept", other)),
        }
    }
    Ok(AcceptArgs {
        input: PathBuf::from(input),
        claims_out,
        force,
    })
}

fn require_no_args(args: Vec<String>, command: &str) -> std::result::Result<(), CliError> {
    if let Some(extra) = args.into_iter().next() {
        return Err(unknown_argument(command, &extra));
    }
    Ok(())
}

struct RefAndOutputArgs {
    ref_name: String,
    output: PathBuf,
}

fn parse_ref_and_output_args(
    args: Vec<String>,
    command: &str,
) -> std::result::Result<RefAndOutputArgs, CliError> {
    let mut iter = args.into_iter();
    let Some(ref_name) = iter.next() else {
        return Err(CliError::Usage(format!("{command} requires a ref name")));
    };
    if ref_name.starts_with("--") {
        return Err(CliError::Usage(format!(
            "{command} requires a ref name before any flags"
        )));
    }
    let mut output = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--output" => {
                let value = flag_value(&mut iter, &format!("{command} --output"))?;
                output.set_once("--output", PathBuf::from(value))?;
            }
            other => return Err(unknown_argument(command, other)),
        }
    }
    let output = output.ok_or_else(|| CliError::Usage(format!("{command} requires --output")))?;
    Ok(RefAndOutputArgs { ref_name, output })
}

struct BuildArgs {
    ref_name: String,
    have: PathBuf,
    output: PathBuf,
    force: bool,
}

fn parse_build_args(args: Vec<String>) -> std::result::Result<BuildArgs, CliError> {
    let mut iter = args.into_iter();
    let Some(ref_name) = iter.next() else {
        return Err(CliError::Usage(
            "sync build requires a ref name".to_string(),
        ));
    };
    if ref_name.starts_with("--") {
        return Err(CliError::Usage(
            "sync build requires a ref name before any flags".to_string(),
        ));
    }
    let mut have = None;
    let mut output = None;
    let mut force = false;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--have" => {
                let value = flag_value(&mut iter, "sync build --have")?;
                have.set_once("--have", PathBuf::from(value))?;
            }
            "--output" => {
                let value = flag_value(&mut iter, "sync build --output")?;
                output.set_once("--output", PathBuf::from(value))?;
            }
            "--force" => mark_seen(&mut force, "--force")?,
            other => return Err(unknown_argument("sync build", other)),
        }
    }
    let have = have.ok_or_else(|| CliError::Usage("sync build requires --have".to_string()))?;
    let output =
        output.ok_or_else(|| CliError::Usage("sync build requires --output".to_string()))?;
    Ok(BuildArgs {
        ref_name,
        have,
        output,
        force,
    })
}

/// §2: `sync seal <ref> --claim <id>` for the single-claim case (kept, not removed), and
/// `sync seal <ref> --claims <file>` for a batch, ordered by `order_claims_for_sealing` before
/// any of them are sealed.
enum SealClaims {
    Single(ObjectId),
    Batch(PathBuf),
}

struct SealArgs {
    ref_name: String,
    claims: SealClaims,
}

fn parse_seal_args(args: Vec<String>) -> std::result::Result<SealArgs, CliError> {
    let mut iter = args.into_iter();
    let Some(ref_name) = iter.next() else {
        return Err(CliError::Usage("sync seal requires a ref name".to_string()));
    };
    if ref_name.starts_with("--") {
        return Err(CliError::Usage(
            "sync seal requires a ref name before any flags".to_string(),
        ));
    }
    let mut claim_id = None;
    let mut claims_file = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--claim" => {
                let value = flag_value(&mut iter, "sync seal --claim")?;
                let parsed = value.parse::<ObjectId>().map_err(|err| {
                    CliError::Usage(format!("--claim {value:?} is not a valid object id: {err}"))
                })?;
                claim_id.set_once("--claim", parsed)?;
            }
            "--claims" => {
                let value = flag_value(&mut iter, "sync seal --claims")?;
                claims_file.set_once("--claims", PathBuf::from(value))?;
            }
            other => return Err(unknown_argument("sync seal", other)),
        }
    }
    let claims = match (claim_id, claims_file) {
        (Some(_), Some(_)) => {
            return Err(CliError::Usage(
                "sync seal accepts either --claim or --claims, not both".to_string(),
            ));
        }
        (Some(claim_id), None) => SealClaims::Single(claim_id),
        (None, Some(claims_file)) => SealClaims::Batch(claims_file),
        (None, None) => {
            return Err(CliError::Usage(
                "sync seal requires --claim <id> or --claims <file>".to_string(),
            ));
        }
    };
    Ok(SealArgs { ref_name, claims })
}
