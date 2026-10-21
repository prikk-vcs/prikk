//! `prikk extract` (0.54.0 step 3 Part 2): write the files at a ref or a block id into a brand-new,
//! empty, external directory -- no repository history, no worktree state.
//!
//! **Anchoring is the one thing that makes this a different command from `cat`/`tree`, not a third
//! copy of their own read path.** This writes what a CI job tests and what a forge serves for
//! download, so it anchors only where a writer would (`prikk_store::prepare_extract_plan`, under
//! the admitted-anchor rule, RFC 136 §10.3c ruling 2 / RFC 159 §8.2) -- never at any snapshot that
//! merely "passes the loader" the way `cat`/`tree`'s own anchoring does. A `remotes/…` ref under a
//! key this repository has not adopted always replays from genesis; past the trusted distance bound
//! it refuses by name unless `--allow-full-replay` is given.
//!
//! **Streaming.** `ExtractPlan` never holds more than one file's bytes at a time resident past what
//! `prepare_extract_plan` itself needed to hold (`prikk_store::extract`'s own module doc): this
//! module reads each file's content immediately before writing it, from the plan's own `Blob` id or
//! already-resident `Bytes`, and drops it the moment the write completes.
//!
//! **Write safety.** The point's own resolved paths are checked for case-insensitive collisions
//! before the first byte is written (`prepare_extract_plan`'s own call to
//! `validate_no_path_collisions`); the target directory must not already exist; every file is
//! written into a temporary sibling (`<dir>.prikk-extract-tmp`) first, which refuses by name if a
//! previous run's sibling is still there rather than silently reusing or deleting it; and the
//! sibling becomes the real target only through one no-replace rename
//! (`prikk_store::rename_new_directory_no_replace`) -- an ordinary refusal or error cleans the
//! sibling up; only a hard crash can leave it behind.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use prikk_object::ObjectId;
use prikk_store::{
    ExtractPlan, ExtractPlanContent, ExtractRefusalKind, NoCheckpointReason, ObjectReadSnapshot,
    Point, RepoPath,
};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::durable_output::destination_exists;
use crate::output::point_label;
use crate::output::verification::escape_json_string;
use crate::stdout::println;
use crate::{current_branch, open_repository};

/// `refusal-v1`'s own thirteen ids (0.54.0 step 3 Part 5d, `not-a-repository` added in Part 5e) --
/// this project's first machine-readable refusal document, designed as a precedent other commands
/// adopt when next touched. **Exhaustive by construction**: no `match` over this enum anywhere in
/// this module has a wildcard arm, so a fourteenth refusal `extract` grows later, without updating
/// this enum, is a compile error here, not a silently missing id in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtractRefusalId {
    OverMaxBytes,
    OverMaxFiles,
    DestinationExists,
    TemporarySiblingExists,
    PointNotFound,
    ClaimNotFound,
    NotAClaim,
    ClaimHasSeveralParents,
    CaseCollision,
    NoCheckpointInReach,
    DamagedRepository,
    IoFailure,
    NotARepository,
}

impl ExtractRefusalId {
    fn id(self) -> &'static str {
        match self {
            Self::OverMaxBytes => "over-max-bytes",
            Self::OverMaxFiles => "over-max-files",
            Self::DestinationExists => "destination-exists",
            Self::TemporarySiblingExists => "temporary-sibling-exists",
            Self::PointNotFound => "point-not-found",
            Self::ClaimNotFound => "claim-not-found",
            Self::NotAClaim => "not-a-claim",
            Self::ClaimHasSeveralParents => "claim-has-several-parents",
            Self::CaseCollision => "case-collision",
            Self::NoCheckpointInReach => "no-checkpoint-in-reach",
            Self::DamagedRepository => "damaged-repository",
            Self::IoFailure => "io-failure",
            Self::NotARepository => "not-a-repository",
        }
    }
}

/// `no-checkpoint-in-reach`'s own `reason` sub-ids -- set only for that one refusal, `null`
/// otherwise (`print_refusal_json`'s own caller decides that).
fn no_checkpoint_reason_id(reason: NoCheckpointReason) -> &'static str {
    match reason {
        NoCheckpointReason::SignerNotAdopted => "signer-not-adopted",
        NoCheckpointReason::NotRecorded => "not-recorded",
        NoCheckpointReason::NoCheckpoint => "no-checkpoint",
    }
}

/// The fallback classification for an error not already classified at its own raise site: by
/// `PrikkError`'s own variant, never by its message text (`PrikkError` is `#[non_exhaustive]`, so
/// this `match` keeps its own wildcard arm -- the two buckets it chooses between are themselves
/// documented catch-alls, not individually enumerated ids, so this is not the same thing as a
/// wildcard over `ExtractRefusalId`). `Io` is a real I/O or resource failure; everything else here
/// names a problem with persisted data (a malformed object, a type mismatch, an unsupported
/// format, ...) -- structural damage this repository did not expect to find.
fn classify_prikk_error_fallback(error: &prikk_error::PrikkError) -> ExtractRefusalId {
    match error {
        prikk_error::PrikkError::Io { .. } => ExtractRefusalId::IoFailure,
        _ => ExtractRefusalId::DamagedRepository,
    }
}

/// `resolve_point`'s own errors, classified at `run_extract`'s own call site (0.54.0 step 3 Part
/// 5d): every `Precondition`/`InvalidName` `resolve_point` can raise for extract's own call
/// (`ReceivedRefs::Read`, never `Refused`) is already one of "ref ... does not exist", "block ...
/// is not in this repository", "object ... is a ..., not a block", or a malformed `remotes/...`
/// name past `parse_extract_args`'s own coarser check -- every one of these is `point-not-found`'s
/// own "no such ref or block" (the wrong-type-object case is folded in here too: there is no
/// separate id for it, and it is exactly as unreachable through an honest `--ref` as a crafted
/// object id can make it). Anything else falls back to the generic classification.
fn classify_resolve_point_error(error: &prikk_error::PrikkError) -> ExtractRefusalId {
    match error {
        prikk_error::PrikkError::Precondition(_) | prikk_error::PrikkError::InvalidName(_) => {
            ExtractRefusalId::PointNotFound
        }
        prikk_error::PrikkError::Io { .. } => ExtractRefusalId::IoFailure,
        _ => ExtractRefusalId::DamagedRepository,
    }
}

/// `prepare_extract_plan`/`prepare_extract_plan_for_claim`'s own `ExtractError`, classified: the
/// five cases `prikk-store` already tagged at their own raise site (`err.kind`) map directly;
/// everything else (`kind: None`) falls back to [`classify_prikk_error_fallback`]. This `match` is
/// exhaustive over `prikk_store::ExtractRefusalKind` with no wildcard -- that type is deliberately
/// not `#[non_exhaustive]` (its own module doc) so this cross-crate match can make exactly that
/// promise.
fn classify_extract_plan_error(
    error: &prikk_store::ExtractError,
) -> (ExtractRefusalId, Option<&'static str>) {
    match &error.kind {
        Some(ExtractRefusalKind::ClaimNotFound) => (ExtractRefusalId::ClaimNotFound, None),
        Some(ExtractRefusalKind::NotAClaim { .. }) => (ExtractRefusalId::NotAClaim, None),
        Some(ExtractRefusalKind::ClaimHasSeveralParents { .. }) => {
            (ExtractRefusalId::ClaimHasSeveralParents, None)
        }
        Some(ExtractRefusalKind::CaseCollision) => (ExtractRefusalId::CaseCollision, None),
        Some(ExtractRefusalKind::NoCheckpointInReach(reason)) => (
            ExtractRefusalId::NoCheckpointInReach,
            Some(no_checkpoint_reason_id(*reason)),
        ),
        None => (classify_prikk_error_fallback(&error.error), None),
    }
}

/// Print `refusal-v1`'s own document to stdout, when `--format json` was requested (0.54.0 step 3
/// Part 5d) -- stderr and the exit code are unchanged by this: this is the one new, additional
/// thing a refusal does under `--format json`. `message` is the exact text stderr already prints
/// (`CliError::message`'s own text), carried into the JSON verbatim, the same convention
/// `VerdictCondition` already uses for its own message field (`verify_verdict.rs`).
fn print_refusal_json(id: ExtractRefusalId, reason: Option<&'static str>, message: &str) {
    let mut json = String::new();
    json.push_str("{\"schema_version\": \"refusal-v1\", \"command\": \"extract\", \"refusal\": ");
    json.push_str(&escape_json_string(id.id()));
    json.push_str(", \"reason\": ");
    match reason {
        Some(reason) => json.push_str(&escape_json_string(reason)),
        None => json.push_str("null"),
    }
    json.push_str(", \"message\": ");
    json.push_str(&escape_json_string(message));
    json.push('}');
    println!("{json}");
}

thread_local! {
    /// Whether this run of `extract` has already printed a `refusal-v1` document for its own
    /// current `Err` (0.54.0 step 3 Part 5e, review C1) -- set by [`refuse`] itself whenever it
    /// actually prints one, read and cleared by [`run_extract`]'s own structural boundary catch.
    /// The same seam `main.rs`'s own `SILENT_EXIT_CODE` thread-local already uses for an analogous
    /// problem: `Command.run`'s fixed signature has nowhere else to carry this one bit out of
    /// `run_extract_inner` without changing every one of its own call sites' signatures. Race-free
    /// for the identical reason `SILENT_EXIT_CODE` is: a single-threaded CLI process, one run per
    /// process, read immediately after `run_extract_inner` returns.
    static REFUSAL_PRINTED: Cell<bool> = const { Cell::new(false) };
}

/// Print `refusal-v1`'s own document, when `format_json` is set, then return `cli_error` unchanged
/// -- the one seam every already-classified refusal in this module passes through on its way out,
/// so stderr and the exit code stay exactly what `cli_error` already carries (RFC: a usage error,
/// exit 2, never reaches this function at all -- every call site below is on the exit-1 refusal
/// path only). Marks [`REFUSAL_PRINTED`] so [`run_extract`]'s own boundary catch does not print a
/// second, less-specific document over this one.
fn refuse(
    format_json: bool,
    id: ExtractRefusalId,
    reason: Option<&'static str>,
    cli_error: CliError,
) -> CliError {
    if format_json {
        print_refusal_json(id, reason, cli_error.message());
        REFUSAL_PRINTED.with(|cell| cell.set(true));
    }
    cli_error
}

struct ExtractArgs {
    root: PathBuf,
    target: PathBuf,
    ref_name: Option<String>,
    claim_id: Option<ObjectId>,
    allow_full_replay: bool,
    plan_only: bool,
    max_bytes: Option<u64>,
    max_files: Option<usize>,
    format_json: bool,
}

fn parse_extract_args(
    root: PathBuf,
    args: Vec<String>,
) -> std::result::Result<ExtractArgs, CliError> {
    let mut ref_name = None;
    let mut claim_id = None;
    let mut allow_full_replay = false;
    let mut plan_only = false;
    let mut max_bytes = None;
    let mut max_files = None;
    let mut format_json = false;
    let mut target = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "extract --ref")?;
                // RFC 153 §2: a point is a ref name or a bare block id, and nothing else.
                if !prikk_store::is_point_name(&value) {
                    return Err(CliError::Usage(format!(
                        "extract --ref {value:?} is neither a ref name (heads/…, tags/… or \
                         remotes/…) nor a block id (64 lowercase hex characters)"
                    )));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--claim" => {
                let value = flag_value(&mut iter, "extract --claim")?;
                let parsed = value.parse::<ObjectId>().map_err(|err| {
                    CliError::Usage(format!("--claim {value:?} is not a valid object id: {err}"))
                })?;
                claim_id.set_once("--claim", parsed)?;
            }
            "--allow-full-replay" => mark_seen(&mut allow_full_replay, "--allow-full-replay")?,
            "--plan-only" => mark_seen(&mut plan_only, "--plan-only")?,
            "--format" => {
                let value = flag_value(&mut iter, "extract --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "extract --format {value:?} is not supported -- only \"json\" is"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            "--max-bytes" => {
                let value = flag_value(&mut iter, "extract --max-bytes")?;
                let parsed = value.parse::<u64>().map_err(|err| {
                    CliError::Usage(format!(
                        "--max-bytes {value:?} is not a valid number: {err}"
                    ))
                })?;
                max_bytes.set_once("--max-bytes", parsed)?;
            }
            "--max-files" => {
                let value = flag_value(&mut iter, "extract --max-files")?;
                let parsed = value.parse::<usize>().map_err(|err| {
                    CliError::Usage(format!(
                        "--max-files {value:?} is not a valid number: {err}"
                    ))
                })?;
                max_files.set_once("--max-files", parsed)?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("extract", other)),
            _ => {
                if target.is_some() {
                    return Err(CliError::Usage(
                        "extract accepts at most one output directory".to_string(),
                    ));
                }
                target = Some(PathBuf::from(arg));
            }
        }
    }
    let Some(target) = target else {
        return Err(CliError::Usage(
            "extract requires an output directory: prikk extract (--ref <ref|block-id>|--claim \
             <claim-id>) <dir>"
                .to_string(),
        ));
    };
    if ref_name.is_some() && claim_id.is_some() {
        return Err(CliError::Usage(
            "extract accepts --ref or --claim, not both".to_string(),
        ));
    }
    Ok(ExtractArgs {
        root,
        target,
        ref_name,
        claim_id,
        allow_full_replay,
        plan_only,
        max_bytes,
        max_files,
        format_json,
    })
}

/// Either a resolved [`Point`] (a ref or a bare block id) or a `RecognitionClaim` id -- `extract`'s
/// own two ways to name what to read (0.54.0 step 3 Part 3). `point_label`/`target_block_id`'s own
/// reporting already treats both uniformly through the shared [`ExtractPlan`] this produces.
enum ExtractSource {
    Point(Point),
    Claim(ObjectId),
}

/// `prikk extract (--ref <ref|block-id>|--claim <claim-id>) <dir> [--allow-full-replay]
/// [--plan-only] [--max-bytes N] [--max-files N] [--format json]`.
///
/// **The structural boundary catch (0.54.0 step 3 Part 5e, review C1).** The real work is
/// [`run_extract_inner`]; this function is a thin wrapper around it that makes "exit 1 under
/// `--format json` always means a `refusal-v1` document" a guarantee about this function's own
/// boundary, not a property every call site inside it must each remember to uphold. A usage error
/// (exit 2) is unaffected -- `parse_extract_args` returning `Err` here returns straight through,
/// before `--format json` itself is even known, exactly as before this round.
pub fn run_extract(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_extract_args(root, args)?;
    let format_json = args.format_json;
    REFUSAL_PRINTED.with(|cell| cell.set(false));
    let result = run_extract_inner(args);
    if let Err(err) = &result {
        let already_printed = REFUSAL_PRINTED.with(Cell::get);
        if format_json && !already_printed {
            // Reached without ever calling `refuse()` -- a refusal not yet classified at its own
            // raise site (today, only `current_branch::resolve_ref`'s own failure, already a
            // `CliError` with no `PrikkError` left to inspect by the time it gets here; the review's
            // own explicit ruling is that this one is `damaged-repository`). The fallback has no
            // richer type to classify by at this boundary -- by construction, every call site that
            // *can* distinguish `io-failure` from `damaged-repository` already does so itself, via
            // `refuse()`, before this catch is ever reached.
            print_refusal_json(ExtractRefusalId::DamagedRepository, None, err.message());
        }
    }
    result
}

/// `run_extract`'s own real logic, unchanged by the Part 5e boundary catch around it: every one of
/// the already-classified refusals below still calls [`refuse`] at its own raise site, exactly as
/// before this round.
fn run_extract_inner(args: ExtractArgs) -> std::result::Result<(), CliError> {
    let layout = match open_repository(args.root) {
        Ok(layout) => layout,
        Err(err) => {
            return Err(refuse(
                args.format_json,
                ExtractRefusalId::NotARepository,
                None,
                CliError::Failure(err),
            ));
        }
    };

    let source = match args.claim_id {
        Some(claim_id) => ExtractSource::Claim(claim_id),
        // `parse_extract_args` already refuses both `--ref` and `--claim` together; omitting
        // both defaults to the current branch, exactly as `cat`/`tree`'s own `--ref` already does
        // (`current_branch::resolve_ref`'s own doc: "explicit when given, otherwise the current
        // branch").
        None => {
            let name = current_branch::resolve_ref(&layout, args.ref_name)?;
            // Same rule as `cat`/`tree` (RFC 157 §2, RFC 147 §2i): a received ref is read, never
            // adopted; an unpublished current branch has nothing to extract yet.
            let unpublished_current_branch =
                current_branch::is_unpublished_current_branch(&layout, &name)?;
            let point =
                match prikk_store::resolve_point(&layout, &name, prikk_store::ReceivedRefs::Read) {
                    Ok(point) => point,
                    Err(prikk_error::PrikkError::Precondition(_)) if unpublished_current_branch => {
                        return Err(refuse(
                            args.format_json,
                            ExtractRefusalId::PointNotFound,
                            None,
                            CliError::Failure(format!(
                                "precondition not met: {name} has no published history yet -- \
                                 nothing to extract"
                            )),
                        ));
                    }
                    Err(err) => {
                        let id = classify_resolve_point_error(&err);
                        return Err(refuse(
                            args.format_json,
                            id,
                            None,
                            CliError::Failure(err.to_string()),
                        ));
                    }
                };
            ExtractSource::Point(point)
        }
    };

    let reader = match ObjectReadSnapshot::open(&layout) {
        Ok(reader) => reader,
        Err(err) => {
            let id = classify_prikk_error_fallback(&err);
            return Err(refuse(
                args.format_json,
                id,
                None,
                CliError::Failure(err.to_string()),
            ));
        }
    };
    let plan = match &source {
        ExtractSource::Point(point) => {
            match prikk_store::prepare_extract_plan(&layout, &reader, point, args.allow_full_replay)
            {
                Ok(plan) => plan,
                Err(err) => {
                    let (id, reason) = classify_extract_plan_error(&err);
                    return Err(refuse(
                        args.format_json,
                        id,
                        reason,
                        CliError::Failure(err.to_string()),
                    ));
                }
            }
        }
        ExtractSource::Claim(claim_id) => match prikk_store::prepare_extract_plan_for_claim(
            &layout,
            &reader,
            *claim_id,
            args.allow_full_replay,
        ) {
            Ok(plan) => plan,
            Err(err) => {
                let (id, reason) = classify_extract_plan_error(&err);
                return Err(refuse(
                    args.format_json,
                    id,
                    reason,
                    CliError::Failure(err.to_string()),
                ));
            }
        },
    };

    // `--max-bytes`/`--max-files` and `--plan-only`/`--format json` all need the point's own file
    // count and total content bytes -- the documentation states plainly that these limits hold
    // *before the write*, not before the cost of learning what the write would be: a `Blob` entry's
    // own size is learned by reading it once here (the identical read `write_plan` would make
    // anyway for a real write), discarded immediately rather than held. An ordinary extract with
    // none of these flags given pays none of this: `summary` stays `None`, and `write_plan`'s own
    // single read per file is the only one that ever happens.
    let needs_summary =
        args.plan_only || args.format_json || args.max_bytes.is_some() || args.max_files.is_some();
    let summary = if needs_summary {
        match plan_summary(&reader, &plan) {
            Ok(summary) => Some(summary),
            Err(err) => {
                let id = classify_prikk_error_fallback(&err);
                return Err(refuse(
                    args.format_json,
                    id,
                    None,
                    CliError::Failure(err.to_string()),
                ));
            }
        }
    } else {
        None
    };

    if let Some((file_count, total_bytes, _largest_file_bytes)) = summary {
        if let Some(max_files) = args.max_files {
            if file_count > max_files {
                return Err(refuse(
                    args.format_json,
                    ExtractRefusalId::OverMaxFiles,
                    None,
                    CliError::Failure(format!(
                        "precondition not met: this point has {file_count} files, over \
                         --max-files {max_files} -- refusing the whole point, nothing was written"
                    )),
                ));
            }
        }
        if let Some(max_bytes) = args.max_bytes {
            if total_bytes > max_bytes {
                return Err(refuse(
                    args.format_json,
                    ExtractRefusalId::OverMaxBytes,
                    None,
                    CliError::Failure(format!(
                        "precondition not met: this point holds {total_bytes} bytes of content, \
                         over --max-bytes {max_bytes} -- refusing the whole point, nothing was \
                         written"
                    )),
                ));
            }
        }
    }

    if args.plan_only {
        // `needs_summary` is `true` whenever `plan_only` is, so this is always populated; the
        // fallback is unreachable, not a real default.
        let (file_count, total_bytes, largest_file_bytes) =
            summary.unwrap_or((plan.entries.len(), 0, 0));
        if args.format_json {
            print_json_report(
                &layout,
                &source,
                &plan,
                file_count,
                total_bytes,
                largest_file_bytes,
                false,
            );
        } else {
            print_plan_only_report(
                &layout,
                &source,
                &plan,
                file_count,
                total_bytes,
                largest_file_bytes,
            );
        }
        return Ok(());
    }

    refuse_existing_target(args.format_json, &args.target)?;
    let temp_sibling = temp_sibling_path(&args.target)?;
    if destination_exists(&temp_sibling) {
        return Err(refuse(
            args.format_json,
            ExtractRefusalId::TemporarySiblingExists,
            None,
            CliError::Failure(format!(
                "precondition not met: a temporary sibling from a previous run already exists at \
                 {} -- this run never deletes what it cannot prove it created; remove it yourself \
                 once you have confirmed it is not another run still in progress, then try again \
                 (nothing was written)",
                temp_sibling.display()
            )),
        ));
    }

    let (total_content_bytes, largest_file_bytes) =
        match write_plan(&reader, &plan, &temp_sibling, args.format_json) {
            Ok(result) => result,
            Err(err) => {
                let _ = fs::remove_dir_all(&temp_sibling);
                return Err(err);
            }
        };

    // 0.54.0 step 3 Part 2b R1: every file underneath is already synced (`write_file_durably`);
    // sync every directory in the tree -- the sibling root included -- before the one rename that
    // makes the whole tree visible at its real target, so a crash between here and the rename
    // leaves only the (still-incomplete-looking) temporary sibling, never a target whose directory
    // entries outran their own files' durability.
    if let Err(err) = sync_tree_directories(&temp_sibling) {
        let _ = fs::remove_dir_all(&temp_sibling);
        return Err(refuse(
            args.format_json,
            ExtractRefusalId::IoFailure,
            None,
            CliError::Failure(format!(
                "failed to sync the temporary sibling directory tree at {} before moving it into \
                 place: {err}",
                temp_sibling.display()
            )),
        ));
    }

    if let Err(err) = prikk_store::rename_new_directory_no_replace(&temp_sibling, &args.target) {
        let _ = fs::remove_dir_all(&temp_sibling);
        return Err(refuse(
            args.format_json,
            ExtractRefusalId::IoFailure,
            None,
            CliError::Failure(format!(
                "failed to move the completed extract into place at {}: {err}",
                args.target.display()
            )),
        ));
    }

    // The rename is itself the durable, atomic boundary on Windows (`MOVEFILE_WRITE_THROUGH`,
    // `prikk_ffi::rename_no_replace`); on Unix, the directory entry it just created still needs its
    // own parent synced, the same step `durable_output.rs`'s own single-file writes already take.
    if let Err(err) = crate::durable_output::sync_parent_directory(&args.target) {
        return Err(refuse(
            args.format_json,
            ExtractRefusalId::IoFailure,
            None,
            CliError::Failure(format!(
                "extract completed at {}, but failed to sync its parent directory afterward -- \
                 its durability across a crash is not confirmed: {err}",
                args.target.display()
            )),
        ));
    }

    if args.format_json {
        print_json_report(
            &layout,
            &source,
            &plan,
            plan.entries.len(),
            total_content_bytes,
            largest_file_bytes,
            true,
        );
    } else {
        print_report(
            &layout,
            &source,
            &args.target,
            &plan,
            total_content_bytes,
            largest_file_bytes,
        );
    }
    Ok(())
}

/// `file_count`/`total_bytes`/`largest_file_bytes` for `plan`: each `Bytes` entry's own `len()`
/// (already resident, no read), each `Blob` entry read once to learn its size and discarded
/// immediately -- the one real cost `--max-bytes`/`--max-files`/`--plan-only`/`--format json`
/// accept paying, per the documentation's own wording (0.54.0 step 3 Part 3, item 2).
/// `largest_file_bytes` (0.54.0 step 3 Part 5d): the single largest entry's own length, always
/// present alongside the total.
fn plan_summary(
    reader: &ObjectReadSnapshot,
    plan: &ExtractPlan,
) -> std::result::Result<(usize, u64, u64), prikk_error::PrikkError> {
    let mut total_bytes = 0_u64;
    let mut largest_file_bytes = 0_u64;
    for entry in &plan.entries {
        let len = match &entry.content {
            ExtractPlanContent::Bytes(bytes) => bytes.len(),
            ExtractPlanContent::Blob(blob_id) => {
                prikk_store::read_extract_blob_content(reader, *blob_id)?.len()
            }
        };
        total_bytes += len as u64;
        largest_file_bytes = largest_file_bytes.max(len as u64);
    }
    Ok((plan.entries.len(), total_bytes, largest_file_bytes))
}

/// `fsync` every directory under (and including) `root`, deepest first -- so a crash partway
/// through this walk never leaves an already-synced directory whose own, not-yet-synced parent
/// could still lose the directory entry that names it (0.54.0 step 3 Part 2b R1). A no-op on
/// non-Unix platforms ([`crate::durable_output::sync_directory`]'s own doc).
fn sync_tree_directories(root: &Path) -> std::io::Result<()> {
    let mut directories = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path.clone());
                stack.push(path);
            }
        }
    }
    // Deepest first: longest path components sort last among comparable prefixes, and no sibling
    // subtree's ordering matters here (two directories only need relative ordering when one is the
    // other's ancestor).
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        crate::durable_output::sync_directory(&directory)?;
    }
    Ok(())
}

/// The target must not already exist -- extract only ever creates a brand-new directory, never
/// writes into one that already has something in it.
fn refuse_existing_target(format_json: bool, target: &Path) -> std::result::Result<(), CliError> {
    if destination_exists(target) {
        return Err(refuse(
            format_json,
            ExtractRefusalId::DestinationExists,
            None,
            CliError::Failure(format!(
                "precondition not met: refusing to extract into an existing path: {} (nothing \
                 was written)",
                target.display()
            )),
        ));
    }
    Ok(())
}

/// `<target>.prikk-extract-tmp` -- a fixed suffix, never a random one, so a user or a later run can
/// recognize and find it deterministically (design round 2, R-A4/ruling 3).
fn temp_sibling_path(target: &Path) -> std::result::Result<PathBuf, CliError> {
    let file_name = target.file_name().ok_or_else(|| {
        CliError::Usage(format!(
            "extract's output path {} has no file name to write",
            target.display()
        ))
    })?;
    let mut temp_name = file_name.to_os_string();
    temp_name.push(".prikk-extract-tmp");
    Ok(target.with_file_name(temp_name))
}

/// Join a validated repository-relative path onto an external root, component by component --
/// never `Path::join` on the raw string, which would accept a platform separator the path's own
/// validation never promised (the same reasoning `prikk_store`'s own, crate-private
/// `join_repo_path_to_root` documents).
fn join_repo_path(root: &Path, path: &RepoPath) -> PathBuf {
    let mut out = root.to_path_buf();
    for component in path.as_str().split('/') {
        out.push(component);
    }
    out
}

/// Write every planned file into `temp_sibling` (freshly created here), streaming each one's
/// content immediately before writing it and dropping it right after -- never holding more than one
/// file resident past what the plan itself already held. Returns the total bytes written (a free
/// by-product of writing them, not a second pass).
///
/// **Durability (0.54.0 step 3 Part 2b R1).** Each file is `sync_all`'d here, right after its
/// content and mode are both written -- the same point `durable_output.rs`'s own
/// `write_and_sync_temp_file` syncs a single file, before anything depends on it being durable.
/// Every directory this builds (the sibling root and every subdirectory a nested path needs) is
/// synced afterward, as a whole tree, by [`run_extract`]'s own `sync_tree_directories` call, once
/// every file underneath is already synced -- a directory's own durability only needs to follow its
/// entries', not precede each one.
fn write_plan(
    reader: &ObjectReadSnapshot,
    plan: &ExtractPlan,
    temp_sibling: &Path,
    format_json: bool,
) -> std::result::Result<(u64, u64), CliError> {
    // 0.54.0 step 3 Part 5d: `run_extract` already refuses by name, before this is ever called,
    // when `destination_exists(&temp_sibling)` -- so an `AlreadyExists` failure *here* is a genuine
    // race against another run, the identical condition, not a different one; anything else here
    // is a real I/O failure (permissions, a full disk, ...), classified `io-failure`.
    if let Err(err) = fs::create_dir(temp_sibling) {
        let id = if err.kind() == std::io::ErrorKind::AlreadyExists {
            ExtractRefusalId::TemporarySiblingExists
        } else {
            ExtractRefusalId::IoFailure
        };
        return Err(refuse(
            format_json,
            id,
            None,
            CliError::Failure(format!(
                "failed to create the temporary sibling directory {}: {err}",
                temp_sibling.display()
            )),
        ));
    }
    let mut total_content_bytes = 0_u64;
    let mut largest_file_bytes = 0_u64;
    for entry in &plan.entries {
        let destination = join_repo_path(temp_sibling, &entry.path);
        if let Some(parent) = destination.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                return Err(refuse(
                    format_json,
                    ExtractRefusalId::IoFailure,
                    None,
                    CliError::Failure(format!(
                        "failed to create directory {}: {err}",
                        parent.display()
                    )),
                ));
            }
        }
        let bytes = match &entry.content {
            ExtractPlanContent::Bytes(bytes) => bytes.clone(),
            ExtractPlanContent::Blob(blob_id) => {
                match prikk_store::read_extract_blob_content(reader, *blob_id) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        let id = classify_prikk_error_fallback(&err);
                        return Err(refuse(
                            format_json,
                            id,
                            None,
                            CliError::Failure(err.to_string()),
                        ));
                    }
                }
            }
        };
        let len = bytes.len() as u64;
        total_content_bytes += len;
        largest_file_bytes = largest_file_bytes.max(len);
        if let Err(err) = write_file_durably(&destination, &bytes, entry.mode) {
            return Err(refuse(format_json, ExtractRefusalId::IoFailure, None, err));
        }
    }
    Ok((total_content_bytes, largest_file_bytes))
}

/// Write `bytes` to the freshly-created `destination`, set its mode on the still-open handle, and
/// `fsync` it before returning -- both the file's content and its mode are durable before this
/// function returns (0.54.0 step 3 Part 2b, the architect's own start-of-Part-3 correction: setting
/// the mode *after* `sync_all`, as an earlier round did, left a crash window where the content
/// landed durably but the mode bit did not -- `extract` promises the output equals the point, and a
/// mode that isn't covered by the same sync as its file isn't kept durably). `destination`'s parent
/// is assumed to already exist ([`write_plan`]'s own `fs::create_dir_all` call, just above its own
/// call to this function).
fn write_file_durably(
    destination: &Path,
    bytes: &[u8],
    mode: u32,
) -> std::result::Result<(), CliError> {
    use std::io::Write as _;

    let mut file = fs::File::create_new(destination).map_err(|err| {
        CliError::Failure(format!("failed to write {}: {err}", destination.display()))
    })?;
    file.write_all(bytes).map_err(|err| {
        CliError::Failure(format!("failed to write {}: {err}", destination.display()))
    })?;
    set_mode(&file, destination, mode)?;
    file.sync_all().map_err(|err| {
        CliError::Failure(format!(
            "failed to sync {} before moving the extract into place: {err}",
            destination.display()
        ))
    })
}

/// Set `mode` on the still-open `file` -- the same handle [`write_file_durably`] is about to
/// `sync_all`, so the mode change is covered by that one sync rather than needing a second.
/// `destination` is used only for error messages (`File::set_permissions` takes no path of its
/// own).
#[cfg(unix)]
fn set_mode(file: &fs::File, destination: &Path, mode: u32) -> std::result::Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode & 0o7777))
        .map_err(|err| {
            CliError::Failure(format!(
                "failed to set mode on {}: {err}",
                destination.display()
            ))
        })
}

/// No observable POSIX mode on this platform (DC-87 §3.3/§4.3): a documented no-op, the same
/// reasoning `worktree.rs`'s own mode-aware materializer already applies.
#[cfg(not(unix))]
fn set_mode(
    _file: &fs::File,
    _destination: &Path,
    _mode: u32,
) -> std::result::Result<(), CliError> {
    Ok(())
}

/// `point_label`'s own label, extended to a claim source (0.54.0 step 3 Part 3): `"claim"` for one,
/// `point_label`'s own `"block"`/`"ref"` otherwise.
fn source_label(source: &ExtractSource) -> &'static str {
    match source {
        ExtractSource::Point(point) => point_label(point.kind == prikk_store::PointKind::Block),
        ExtractSource::Claim(_) => "claim",
    }
}

fn print_provisional_line(provisional: bool) {
    // 0.54.0 step 3 Part 2b R4: `checkout --snapshot-materialize`'s own wording ("this snapshot's
    // block") does not fit `extract` -- a point up to 63 blocks past its anchor has no single
    // snapshot of its own to name. This instead says what `extract` actually checked: whether an
    // admitted anchor was found and used, or the replay fell back to genesis because none was (for
    // a ref/block point, `AnchorTrust` under `AnchorSite::Extract`; for a claim, the identical rule
    // under `AnchorSite::ExtractClaim`, through `derive_next_state_for_candidate`).
    //
    // 0.54.0 Part 5c: the `provisional: yes` line used to close with `checkout`'s own "run `prikk
    // verify` before committing" -- wrong here, since `extract` writes into a brand-new external
    // directory, never this repository, and has no commit step at all for `verify` to precede.
    //
    // 0.54.0 Part 5c review v1, condition C1: the replacement wording this round shipped still
    // overclaimed -- "this extract already completed correctly regardless" and "verify would only
    // record a checkpoint" both say more than is true. `provisional: yes` means the files match the
    // history that was replayed, but **nothing has checked that history's own integrity or its
    // publication trust** -- the history replayed may be signed by a key this repository has not
    // adopted. `verify` does real checking, not just bookkeeping: it checks the history (not merely
    // "records" it) and records checkpoints, so a later read near this point replays less.
    if provisional {
        println!(
            "provisional: yes -- no trusted checkpoint was in reach: every file was replayed from \
             the start of history. The files match that history. `prikk verify` checks the \
             history and records checkpoints, so a later read near this point replays less"
        );
    } else {
        println!(
            "provisional: no -- every file was reached from a checkpoint this repository has \
             itself replay-verified and whose signer it has adopted"
        );
    }
}

/// (0.54.0 step 3 Part 5d) the `claim parent block: …` line -- always printed, `<none>` unless
/// `plan` is a claim with a declared parent ([`ExtractPlan::claim_parent_block_id`]'s own doc:
/// distinct from `target_block_id`, which is the block a claim *recognizes*, not the block it
/// would be sealed onto).
fn print_claim_parent_block_line(plan: &ExtractPlan) {
    match plan.claim_parent_block_id {
        Some(parent) => println!("claim parent block: {parent}"),
        None => println!("claim parent block: <none>"),
    }
}

fn print_report(
    layout: &prikk_store::RepositoryLayout,
    source: &ExtractSource,
    target: &Path,
    plan: &ExtractPlan,
    total_content_bytes: u64,
    largest_file_bytes: u64,
) {
    println!("extract repository: {}", layout.prikk_dir().display());
    println!("{}: {}", source_label(source), plan.point);
    println!("target block: {}", plan.target_block_id);
    print_claim_parent_block_line(plan);
    println!("output directory: {}", target.display());
    print_provisional_line(plan.provisional);
    println!("files written: {}", plan.entries.len());
    println!("total content bytes: {total_content_bytes}");
    println!("largest file: {largest_file_bytes} bytes");
}

/// `--plan-only`: the point's own file count, total bytes and `provisional` -- planeter's
/// size-first report (0.54.0 step 3 Part 3, item 1). Writes nothing; there is no `output
/// directory`/`files written` line, since nothing was written.
fn print_plan_only_report(
    layout: &prikk_store::RepositoryLayout,
    source: &ExtractSource,
    plan: &ExtractPlan,
    file_count: usize,
    total_bytes: u64,
    largest_file_bytes: u64,
) {
    println!("extract repository: {}", layout.prikk_dir().display());
    println!("{}: {}", source_label(source), plan.point);
    println!("target block: {}", plan.target_block_id);
    print_claim_parent_block_line(plan);
    println!("plan only: nothing was written");
    print_provisional_line(plan.provisional);
    println!("files: {file_count}");
    println!("total content bytes: {total_bytes}");
    println!("largest file: {largest_file_bytes} bytes");
}

/// `--format json` (0.54.0 step 3 Part 3, item 3): follows `tree`'s own hand-rolled-JSON,
/// `schema_version`-named convention (`tree_listing_json`, `crates/prikk-cli/src/tree.rs`) -- no
/// `serde_json` here either, the same long-standing rule `durable_output.rs`'s own module doc
/// states. `written` tells a consumer, without having to infer it from `total_bytes` alone, whether
/// this call actually wrote `target` to disk (`false` for `--plan-only`, or for a refused run that
/// never reaches this point at all).
fn print_json_report(
    layout: &prikk_store::RepositoryLayout,
    source: &ExtractSource,
    plan: &ExtractPlan,
    file_count: usize,
    total_bytes: u64,
    largest_file_bytes: u64,
    written: bool,
) {
    let mut json = String::new();
    json.push_str("{\n  \"schema_version\": \"extract-report-v1\",\n  \"extract_repository\": ");
    json.push_str(&escape_json_string(
        &layout.prikk_dir().display().to_string(),
    ));
    json.push_str(",\n  \"point\": ");
    json.push_str(&escape_json_string(&plan.point));
    json.push_str(",\n  \"point_kind\": ");
    json.push_str(&escape_json_string(source_label(source)));
    json.push_str(",\n  \"target_block_id\": ");
    json.push_str(&escape_json_string(&plan.target_block_id.to_string()));
    json.push_str(",\n  \"claim_parent_block_id\": ");
    match plan.claim_parent_block_id {
        Some(parent) => json.push_str(&escape_json_string(&parent.to_string())),
        None => json.push_str("null"),
    }
    json.push_str(",\n  \"provisional\": ");
    json.push_str(if plan.provisional { "true" } else { "false" });
    json.push_str(",\n  \"written\": ");
    json.push_str(if written { "true" } else { "false" });
    json.push_str(",\n  \"file_count\": ");
    json.push_str(&file_count.to_string());
    json.push_str(",\n  \"total_bytes\": ");
    json.push_str(&total_bytes.to_string());
    json.push_str(",\n  \"largest_file_bytes\": ");
    json.push_str(&largest_file_bytes.to_string());
    json.push_str("\n}");
    println!("{json}");
}

#[cfg(test)]
mod tests;
