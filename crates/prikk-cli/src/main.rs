#![forbid(unsafe_code)]

//! Prikk command-line entry point.
//!
//! The CLI exposes minimal repository layout commands, active WAL status, node-addressed worktree
//! commit authoring, explicit non-default branch genesis, tag publication, bundle export/import,
//! sync negotiation and tag adoption, deterministic arbitrary-span text edit generation,
//! read-only inverse planning, rollback preview, rollback draft append/verification, sealed rollback classification,
//! supported patch replay planning/materialization, explicit patch deletion planning, a local
//! no-audit seal scaffold, read-only history inspection, checkout planning, conservative snapshot
//! materialization, read-only worktree status, minimal publication trust setup, read-only content
//! display for a sealed block or patch, read-only merge evidence and merge plan review, confluent
//! merge execution, repository verification, doctor diagnostics, and stale lock/index maintenance.
//! Multi-operation text diff minimization, full patch algebra, and audit plugins remain later increments.

use std::path::PathBuf;
use std::process::ExitCode;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` with the closed-pipe-safe versions in
// `stdout.rs` -- see that module's own doc comment.
use stdout::println;

mod arg_scan;
mod args;
mod branch;
mod bundle;
mod commands;
mod compact;
mod durable_output;
mod key;
mod merge;
mod output;
mod seal;
mod setup;
mod stdout;
mod sync;
mod tag;
mod unlock;
mod verify_verdict;

use arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use args::{
    CheckoutMode, MergeEvidenceTargetArg, VerifyOutputFormat, current_dir, parse_checkout_args,
    parse_commit_args, parse_doctor_args, parse_inverse_plan_args, parse_log_args,
    parse_merge_evidence_args, parse_merge_plan_args, parse_rollback_draft_args,
    parse_rollback_draft_verify_args, parse_rollback_preview_args, parse_show_args,
    parse_verify_args, parse_worktree_status_args,
};
use commands::CliError;
use output::{
    QueueTarget, QueueThresholdStatus, print_active_session_repairs, print_checkout_plan,
    print_command_help, print_doctor_report, print_help, print_history, print_merge_evidence,
    print_merge_plan, print_patch_deletion_plan, print_patch_inverse_plan,
    print_patch_materialization_report, print_patch_plan_content_json, print_patch_replay_plan,
    print_rollback_draft_report, print_rollback_draft_verification, print_rollback_preview_plan,
    print_show, print_show_json, print_snapshot_checkout_plan,
    print_snapshot_materialization_report, print_status_json, print_trust_check,
    print_trust_check_json, print_trust_list, print_trust_list_json, print_verify_report,
    print_verify_report_json, print_worktree_status,
};
use prikk_object::Signature;
use prikk_store::{
    ActiveRefMetadata, ActiveSessionRepairStatus, DEFAULT_ACTIVE_NAME, DEFAULT_ACTIVE_PATCH_LIMIT,
    DoctorRepairOptions, Ed25519AuthorSigner, Ed25519MaintainerSigner, MergeEvidenceTarget,
    RefStore, RepositoryLayout, VerifyOptions, Wal, WorktreePatchCommitOptions,
    add_trusted_maintainer, append_rollback_draft, commit_worktree_changes_signed,
    doctor_repository, enumerate_queued_patches, list_received_pointers,
    load_maintainer_trust_policy_or_empty, load_received_ref_history, load_ref_history,
    materialize_patch_checkout, materialize_patch_checkout_with_deletions,
    materialize_snapshot_checkout, plan_patch_checkout_deletions, prepare_checkout_plan,
    prepare_merge_evidence, prepare_merge_plan, prepare_patch_inverse_plan,
    prepare_patch_plan_content_report, prepare_patch_replay_plan, prepare_rollback_preview,
    prepare_snapshot_checkout_plan, read_active_ref_metadata, remove_trusted_maintainer,
    repair_repository, show, verify_active_rollback_draft, verify_repository_with_options,
    worktree_status,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn open_repository(
    root: impl Into<PathBuf>,
) -> std::result::Result<RepositoryLayout, String> {
    RepositoryLayout::open(root).map_err(|err| err.to_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}", err.message());
            ExitCode::from(err.exit_code())
        }
    }
}

fn run() -> std::result::Result<(), CliError> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("--help") | Some("-h") => {
            print_help(VERSION);
            Ok(())
        }
        Some("--version") | Some("-V") => {
            println!("prikk {VERSION}");
            Ok(())
        }
        Some(name) => match commands::find(name) {
            Some(command) => {
                let args: Vec<String> = args.collect();
                // RFC 121 §2.5 v2 amendment §2: round 3's argument hygiene made every parser refuse
                // an unrecognized flag, which now includes `--help`/`-h` -- so this must be
                // recognized *before* the command's own parser ever sees it, not added as a 27th
                // arm to those parsers (the same "one absence, not many places" reasoning round 3
                // itself was built on). Recognized anywhere in the argument list, not only as the
                // first token (matching `git`/`cargo`'s own convention) -- `COMMANDS` is a flat
                // table of top-level names, so this does not distinguish `bundle --help` from
                // `bundle export --help`; both print `bundle`'s full help text (`print_command_help`'s
                // own doc comment).
                if args.iter().any(|arg| arg == "--help" || arg == "-h") {
                    print_command_help(command);
                    Ok(())
                } else {
                    (command.run)(args)
                }
            }
            // RFC 121 §6a: an unrecognized command name is detected before any repository work
            // begins, the exact shape §1's `Usage` variant exists for -- not `.into()`'s default
            // `Failure`, which every other bare error in this file still gets until the argument-
            // hygiene increment (`command-discovery-handoff-v1.md`'s sibling) adds the rest.
            None => Err(CliError::Usage(format!("unknown command: {name}"))),
        },
    }
}

fn run_init(path: Option<String>) -> std::result::Result<(), CliError> {
    let root = match path {
        Some(path) => PathBuf::from(path),
        None => current_dir()?,
    };
    RepositoryLayout::init(root.clone()).map_err(|err| err.to_string())?;
    println!(
        "initialized Prikk repository at {}",
        root.join(".prikk").display()
    );
    Ok(())
}

fn run_commit(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_commit_args(args)?;
    let root = current_dir()?;
    let layout = open_repository(root)?;
    layout
        .require_current_format()
        .map_err(|err| err.to_string())?;
    let thresholds = ActivePatchThresholds::from_env()?;
    let options = if args.text_edits {
        WorktreePatchCommitOptions::prefer_text_edits()
    } else {
        WorktreePatchCommitOptions::file_level()
    }
    .with_active_patch_limit(thresholds.limit);
    let signer = author_signer_from_env()?;
    let report =
        commit_worktree_changes_signed(&layout, &args.ref_name, &args.message, options, &signer)
            .map_err(|err| err.to_string())?;
    println!("recorded worktree patch in active WAL");
    println!("baseline ref: {}", report.ref_name);
    println!("patch id: {}", report.patch_id);
    println!("WAL sequence: {}", report.wal_sequence);
    println!("operations: {}", report.operation_count);
    println!("referenced blobs: {}", report.referenced_blob_count);
    println!("text edits: {}", report.text_edit_count);
    for change in &report.changes {
        println!("  {} {}", change.operation.as_str(), change.path);
    }
    println!(
        "note: multi-operation text diff minimization, patch algebra, rename detection, and audit \
         plugins remain later increments"
    );
    Ok(())
}

fn run_seal(args: Vec<String>) -> std::result::Result<(), CliError> {
    // AUD-10: parse before building the signer, so a bad argument is refused before an unrelated
    // missing-signer environment is ever consulted.
    let ref_name = seal::parse_seal_args(args)?;
    let root = current_dir()?;
    let signer = maintainer_signer_from_env()?;
    let result = seal::run_seal(root, ref_name, &signer)?;
    println!("sealed active WAL into block");
    println!("patches: {}", result.patch_count);
    println!("block id: {}", result.block_id);
    println!("{} RefState: {}", result.ref_name, result.ref_state_id);
    println!("note: audit plugins remain later PRs");
    Ok(())
}

fn run_merge(args: Vec<String>) -> std::result::Result<(), CliError> {
    // AUD-10: parse before building the signer, so a bad argument is refused before an unrelated
    // missing-signer environment is ever consulted.
    let args = crate::args::parse_merge_execute_args(args)?;
    let signer = maintainer_signer_from_env()?;
    let report = merge::run_merge(args, &signer)?;
    println!("merged {} into {}", report.from_ref, report.into_ref);
    println!("baseline block: {}", report.baseline_block_id);
    println!("parent block: {}", report.parent_block_id);
    println!("adopted target block: {}", report.adopted_target_block_id);
    println!("adopted patches: {}", report.adopted_patch_ids.len());
    for patch_id in &report.adopted_patch_ids {
        println!("  {patch_id}");
    }
    println!("block id: {}", report.block_id);
    println!("{} RefState: {}", report.into_ref, report.ref_state_id);
    Ok(())
}

fn run_branch(args: Vec<String>) -> std::result::Result<(), CliError> {
    let root = current_dir()?;
    branch::run_branch(root, args)?;
    Ok(())
}

/// `prikk key` needs no repository -- a visitor must be able to generate a key *before* `init`
/// (RFC 135). No `root` to inject, but wrapped anyway for the same reason every other entry in
/// `COMMANDS` is: one uniform `crate::run_*` shape at the call site.
fn run_key(args: Vec<String>) -> std::result::Result<(), CliError> {
    key::run_key(args)
}

/// `prikk setup` takes its own repository path as a positional argument (mirroring `init`'s own
/// `[path]`, since `setup` performs `init` as its first step) rather than resolving the current
/// directory, so it also needs no `root` injected here.
fn run_setup(args: Vec<String>) -> std::result::Result<(), CliError> {
    setup::run_setup(args)
}

fn run_tag(args: Vec<String>) -> std::result::Result<(), CliError> {
    let root = current_dir()?;
    tag::run_tag(root, args)?;
    Ok(())
}

fn run_bundle(args: Vec<String>) -> std::result::Result<(), CliError> {
    let root = current_dir()?;
    bundle::run_bundle(root, args)?;
    Ok(())
}

fn run_sync(args: Vec<String>) -> std::result::Result<(), CliError> {
    let root = current_dir()?;
    sync::run_sync(root, args)?;
    Ok(())
}

fn run_unlock(args: Vec<String>) -> std::result::Result<(), CliError> {
    let root = current_dir()?;
    unlock::run_unlock(root, args)?;
    Ok(())
}

fn run_compact(args: Vec<String>) -> std::result::Result<(), CliError> {
    let root = current_dir()?;
    compact::run_compact(root, args)?;
    Ok(())
}

fn run_trust(args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut args = args.into_iter();
    match (args.next().as_deref(), args.next().as_deref()) {
        (Some("maintainer"), Some("add")) => {
            let mut key_id = None;
            let mut public_key = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--key-id" => {
                        let value = flag_value(&mut args, "--key-id")?;
                        key_id.set_once("--key-id", value)?;
                    }
                    "--public-key" => {
                        let value = flag_value(&mut args, "--public-key")?;
                        public_key.set_once("--public-key", value)?;
                    }
                    other => return Err(unknown_argument("trust maintainer add", other)),
                }
            }
            let key_id = key_id.ok_or_else(|| {
                CliError::Usage("trust maintainer add requires --key-id".to_string())
            })?;
            let public_key = public_key.ok_or_else(|| {
                CliError::Usage("trust maintainer add requires --public-key".to_string())
            })?;
            let root = current_dir()?;
            let layout = open_repository(root)?;
            let (adopted, newly_added) = add_trusted_maintainer(&layout, &key_id, &public_key)
                .map_err(|err| err.to_string())?;
            if newly_added {
                println!("trusted maintainer key: {}", adopted.key_id);
            } else {
                println!("maintainer key already trusted: {}", adopted.key_id);
            }
            // RFC 138 §4.2 carried-defects B: a derived count, not the `policy: required=1`
            // literal that used to print here -- `MaintainerTrustPolicy` holds a `Vec` and nothing
            // else, so this line's job is to say how many, not to restate trust semantics that
            // `prikk trust maintainer list` already states correctly.
            let policy =
                load_maintainer_trust_policy_or_empty(&layout).map_err(|err| err.to_string())?;
            println!("adopted maintainer keys: {}", policy.keys.len());
            Ok(())
        }
        // RFC 102 Stage 5, design-v1.md §14.9: a supported interface for the revocation capability
        // that already existed as an undocumented hand-edit of the retired `policy.toml`. Not a new
        // capability -- see `trust_index.rs`'s own module doc for why the snapshot-container shape
        // makes this representable without a tombstone record.
        (Some("maintainer"), Some("remove")) => {
            let mut key_id = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--key-id" => {
                        let value = flag_value(&mut args, "--key-id")?;
                        key_id.set_once("--key-id", value)?;
                    }
                    other => return Err(unknown_argument("trust maintainer remove", other)),
                }
            }
            let key_id = key_id.ok_or_else(|| {
                CliError::Usage("trust maintainer remove requires --key-id".to_string())
            })?;
            let root = current_dir()?;
            let layout = open_repository(root)?;
            let removed =
                remove_trusted_maintainer(&layout, &key_id).map_err(|err| err.to_string())?;
            if removed {
                println!("revoked maintainer key: {key_id}");
            } else {
                println!("maintainer key was not trusted: {key_id}");
            }
            Ok(())
        }
        // RFC 138: a read surface over a policy already loaded on every seal -- no new read, no
        // new state, no change to adoption, revocation, or what `seal` requires. An empty policy
        // is a successful empty result (§7.3), never an error, which is why this reads through
        // `load_maintainer_trust_policy_or_empty` rather than `load_maintainer_trust_policy`.
        (Some("maintainer"), Some("list")) => {
            let mut format_json = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--format" => {
                        let value = flag_value(&mut args, "trust maintainer list --format")?;
                        if value != "json" {
                            return Err(CliError::Usage(format!(
                                "trust maintainer list --format does not support {value:?}"
                            )));
                        }
                        mark_seen(&mut format_json, "--format")?;
                    }
                    other => return Err(unknown_argument("trust maintainer list", other)),
                }
            }
            let root = current_dir()?;
            let layout = open_repository(root)?;
            let policy =
                load_maintainer_trust_policy_or_empty(&layout).map_err(|err| err.to_string())?;
            if format_json {
                print_trust_list_json(&policy);
            } else {
                print_trust_list(&policy);
            }
            Ok(())
        }
        // RFC 138 §3: exits `0` whichever way the question resolves -- "key X is not trusted" is
        // an answer, not a failure or a refusal (RFC 121's exit-code vocabulary has no slot for a
        // negative answer to a question that was successfully asked). `1`/`2` keep their ruled
        // meanings: an unreadable policy is `1`, a missing or malformed `--key-id` is `2`.
        (Some("maintainer"), Some("check")) => {
            let mut key_id = None;
            let mut format_json = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--key-id" => {
                        let value = flag_value(&mut args, "--key-id")?;
                        key_id.set_once("--key-id", value)?;
                    }
                    "--format" => {
                        let value = flag_value(&mut args, "trust maintainer check --format")?;
                        if value != "json" {
                            return Err(CliError::Usage(format!(
                                "trust maintainer check --format does not support {value:?}"
                            )));
                        }
                        mark_seen(&mut format_json, "--format")?;
                    }
                    other => return Err(unknown_argument("trust maintainer check", other)),
                }
            }
            let key_id = key_id.ok_or_else(|| {
                CliError::Usage("trust maintainer check requires --key-id".to_string())
            })?;
            Signature::validate_key_id(&key_id).map_err(|err| CliError::Usage(err.to_string()))?;
            let root = current_dir()?;
            let layout = open_repository(root)?;
            let policy =
                load_maintainer_trust_policy_or_empty(&layout).map_err(|err| err.to_string())?;
            let found = policy.keys.iter().find(|key| key.key_id == key_id);
            if format_json {
                print_trust_check_json(&key_id, found);
            } else {
                print_trust_check(&key_id, found);
            }
            Ok(())
        }
        _ => Err(CliError::Usage(
            "usage: prikk trust maintainer add --key-id <key-id> --public-key <64-hex>\n       \
             prikk trust maintainer remove --key-id <key-id>\n       \
             prikk trust maintainer list [--format json]\n       \
             prikk trust maintainer check --key-id <key-id> [--format json]"
                .to_string(),
        )),
    }
}

/// `status: `--format json` (RFC 140) edits `status`'s own long-standing "accept no arguments"
/// ruling (RFC 121 §3) rather than opening a gap in it -- `run_status_adapter`
/// (`commands.rs`) still refuses everything except this one flag. Dispatches to the prose form
/// (byte-identical to before RFC 140, and paying nothing this RFC added: RFC 140 §5) or the JSON
/// form (`run_status_json`, below), never both.
fn run_status(format_json: bool) -> std::result::Result<(), CliError> {
    if format_json {
        return run_status_json();
    }
    let root = current_dir()?;
    let layout = open_repository(root)?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().map_err(|err| err.to_string())?;
    println!("prikk repository: {}", layout.prikk_dir().display());
    let ref_store = RefStore::new(layout.clone());
    let main_ref = ref_store
        .read_current_ref_state_id("heads/main")
        .map_err(|err| err.to_string())?;
    println!("active WAL records: {}", replay.records.len());
    println!(
        "trailing partial WAL bytes: {}",
        replay.trailing_partial_bytes
    );
    match main_ref {
        Some(id) => println!("heads/main RefState: {id}"),
        None => println!("heads/main RefState: <not published>"),
    }
    // DC-66 criterion 7: report the queued patch count and the ref the queue targets, distinct from
    // `replay.records.len()` (a raw count with no ownership) and `heads/main RefState` (the last
    // *sealed* state, not what an active queue is targeting).
    if replay.records.is_empty() {
        println!("queued patches: 0");
    } else {
        let target = match read_active_ref_metadata(&layout).map_err(|err| err.to_string())? {
            ActiveRefMetadata::Valid(ref_name) => ref_name,
            ActiveRefMetadata::Missing => "<missing metadata>".to_string(),
            ActiveRefMetadata::Invalid(_) => "<malformed metadata>".to_string(),
        };
        println!(
            "queued patches: {} targeting {target}",
            replay.records.len()
        );
        // DC-57 (NFR-PERF-02): extends DC-66's existing queue report rather than inventing a second
        // reporting path, per app-requirements §6.3 ("status must recommend sealing").
        let thresholds = ActivePatchThresholds::from_env()?;
        if replay.records.len() >= thresholds.limit {
            println!(
                "warning: active patches ({}) at or above the configured hard limit ({}); \
                 commit is blocked until you run `prikk seal`",
                replay.records.len(),
                thresholds.limit
            );
        } else if replay.records.len() >= thresholds.warn {
            println!(
                "warning: active patches ({}) at or above the recommended threshold ({}); \
                 consider running `prikk seal`",
                replay.records.len(),
                thresholds.warn
            );
        }
    }
    println!("status: multi-operation text diff minimization and plugins not yet implemented");
    Ok(())
}

/// `status --format json` (RFC 140 §2/§6): `status-report-v1`, carrying everything the prose form
/// above does, plus the queue enumeration the prose form cannot (RFC 140's own reason to exist).
/// Independent of `run_status`'s prose body above by design, the same way
/// `print_trust_list`/`print_trust_list_json` are independent functions rather than one refactored
/// into the other (RFC 138) -- so a future prose-wording change cannot silently reach into this
/// function's own JSON shape, and vice versa.
fn run_status_json() -> std::result::Result<(), CliError> {
    let root = current_dir()?;
    let layout = open_repository(root)?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().map_err(|err| err.to_string())?;
    let ref_store = RefStore::new(layout.clone());
    let main_ref = ref_store
        .read_current_ref_state_id("heads/main")
        .map_err(|err| err.to_string())?;

    if replay.records.is_empty() {
        print_status_json(
            &layout,
            replay.records.len(),
            replay.trailing_partial_bytes,
            main_ref,
            None,
            None,
            &[],
        );
        return Ok(());
    }

    let target = match read_active_ref_metadata(&layout).map_err(|err| err.to_string())? {
        ActiveRefMetadata::Valid(ref_name) => QueueTarget::Ref(ref_name),
        ActiveRefMetadata::Missing => QueueTarget::MissingMetadata,
        ActiveRefMetadata::Invalid(_) => QueueTarget::MalformedMetadata,
    };
    // DC-57 (NFR-PERF-02): the same thresholds the prose form reads, rendered as a status rather
    // than prose text.
    let thresholds = ActivePatchThresholds::from_env()?;
    let threshold_status = if replay.records.len() >= thresholds.limit {
        QueueThresholdStatus::HardLimit
    } else if replay.records.len() >= thresholds.warn {
        QueueThresholdStatus::Warn
    } else {
        QueueThresholdStatus::None
    };

    // RFC 140 §5: this is the derivation the prose path above never performs. Reached only because
    // `--format json` was requested and the queue is non-empty.
    let patches = enumerate_queued_patches(&layout).map_err(|err| err.to_string())?;

    print_status_json(
        &layout,
        replay.records.len(),
        replay.trailing_partial_bytes,
        main_ref,
        Some(&target),
        Some((&threshold_status, thresholds.warn, thresholds.limit)),
        &patches,
    );
    Ok(())
}

/// DC-57 (NFR-PERF-02): active-patch warn/hard-block thresholds, read once from the environment and
/// validated together — a warn threshold above the hard limit, a non-numeric value, or zero for
/// either is rejected rather than silently kept at the default (the same fail-closed precedent as
/// `PRIKK_AUTHOR_KEY_ID`/`PRIKK_AUTHOR_SEED`). Per-invocation only; never persisted in the
/// repository — a durable policy belongs to a future general configuration increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActivePatchThresholds {
    warn: usize,
    limit: usize,
}

/// NFR-PERF-02's default warn threshold. The hard-block default,
/// [`prikk_store::DEFAULT_ACTIVE_PATCH_LIMIT`], is owned by `prikk-store` since it is also the
/// fallback baked into `WorktreePatchCommitOptions::file_level`/`prefer_text_edits`; this constant
/// has no store-side counterpart to share, since only the CLI's `status` output ever consults it.
const DEFAULT_ACTIVE_PATCH_WARN: usize = 800;

impl ActivePatchThresholds {
    fn from_env() -> std::result::Result<Self, String> {
        let warn =
            parse_active_patch_threshold_env("PRIKK_ACTIVE_PATCH_WARN", DEFAULT_ACTIVE_PATCH_WARN)?;
        let limit = parse_active_patch_threshold_env(
            "PRIKK_ACTIVE_PATCH_LIMIT",
            DEFAULT_ACTIVE_PATCH_LIMIT,
        )?;
        if warn > limit {
            return Err(format!(
                "PRIKK_ACTIVE_PATCH_WARN ({warn}) must not exceed PRIKK_ACTIVE_PATCH_LIMIT ({limit})"
            ));
        }
        Ok(Self { warn, limit })
    }
}

/// Parse one active-patch threshold environment variable, failing closed (never silently defaulting)
/// on anything present but malformed: non-numeric, or zero.
fn parse_active_patch_threshold_env(
    name: &str,
    default: usize,
) -> std::result::Result<usize, String> {
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

fn run_log(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_log_args(args)?;
    let layout = open_repository(args.root)?;
    // Received refs (DC-78 ruling 4) live in their own container (RFC 102 Stage 5:
    // received_index.rs, formerly refs/received/), not the local-ref pointer index, and their
    // RefState objects carry the origin's own name rather than the local "remotes/"-prefixed one
    // — load_ref_history's pointer/name-match check can't resolve them, so route separately.
    let history = if args.ref_name.starts_with("remotes/") {
        load_received_ref_history(&layout, &args.ref_name, args.limit)
    } else {
        load_ref_history(&layout, &args.ref_name, args.limit)
    }
    .map_err(|err| err.to_string())?;
    print_history(&layout, &history);
    Ok(())
}

fn run_checkout(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_checkout_args(args)?;
    let layout = open_repository(args.root)?;
    match args.mode {
        CheckoutMode::PlanOnly => {
            let plan =
                prepare_checkout_plan(&layout, &args.ref_name).map_err(|err| err.to_string())?;
            print_checkout_plan(&layout, &plan);
        }
        CheckoutMode::SnapshotPlan => {
            let plan = prepare_snapshot_checkout_plan(&layout, &args.ref_name)
                .map_err(|err| err.to_string())?;
            print_snapshot_checkout_plan(&layout, &plan);
        }
        CheckoutMode::SnapshotMaterialize => {
            let report = materialize_snapshot_checkout(&layout, &args.ref_name)
                .map_err(|err| err.to_string())?;
            print_snapshot_materialization_report(&layout, &report);
        }
        CheckoutMode::PatchPlan => {
            if args.format_json {
                let report =
                    prepare_patch_plan_content_report(&layout, &args.ref_name, &args.content_paths)
                        .map_err(|err| err.to_string())?;
                print_patch_plan_content_json(&report);
            } else {
                let plan = prepare_patch_replay_plan(&layout, &args.ref_name)
                    .map_err(|err| err.to_string())?;
                print_patch_replay_plan(&layout, &plan);
            }
        }
        CheckoutMode::PatchMaterialize => {
            let report = materialize_patch_checkout(&layout, &args.ref_name)
                .map_err(|err| err.to_string())?;
            print_patch_materialization_report(&layout, &report);
        }
        CheckoutMode::PatchDeletePlan => {
            let plan = plan_patch_checkout_deletions(&layout, &args.ref_name)
                .map_err(|err| err.to_string())?;
            print_patch_deletion_plan(&layout, &plan);
            if !plan.is_safe_to_apply() {
                return Err("patch deletion plan has unsafe candidates"
                    .to_string()
                    .into());
            }
        }
        CheckoutMode::PatchMaterializeDelete => {
            let report = materialize_patch_checkout_with_deletions(&layout, &args.ref_name)
                .map_err(|err| err.to_string())?;
            print_patch_materialization_report(&layout, &report);
        }
    }
    Ok(())
}

fn run_merge_evidence(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_merge_evidence_args(args)?;
    let layout = open_repository(args.root)?;
    let report = prepare_merge_evidence(
        &layout,
        args.baseline_block_id,
        merge_target_from_arg(args.left_target),
        merge_target_from_arg(args.right_target),
    )
    .map_err(|err| err.to_string())?;
    print_merge_evidence(&report);
    Ok(())
}

/// `prikk show` (RFC 142): what a block or patch changed. Exit `0` whichever way it resolves
/// (RFC 121) -- a block that changed nothing (zero patches) is a valid answer, not a failure.
fn run_show(args: Vec<String>) -> std::result::Result<(), CliError> {
    let show_args = parse_show_args(args)?;
    let layout = open_repository(current_dir()?)?;
    let patches = show(&layout, show_args.id).map_err(|err| err.to_string())?;
    if show_args.format_json {
        print_show_json(&patches);
    } else {
        print_show(&patches);
    }
    Ok(())
}

fn run_merge_plan(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_merge_plan_args(args)?;
    let layout = open_repository(args.root)?;
    let plan = prepare_merge_plan(
        &layout,
        args.baseline_block_id,
        merge_target_from_arg(args.left_target),
        merge_target_from_arg(args.right_target),
    )
    .map_err(|err| err.to_string())?;
    print_merge_plan(&plan);
    Ok(())
}

fn merge_target_from_arg(target: MergeEvidenceTargetArg) -> MergeEvidenceTarget {
    match target {
        MergeEvidenceTargetArg::Block(block_id) => MergeEvidenceTarget::Block(block_id),
        // DC-85: a `--left-ref`/`--right-ref` value can name a received ref too, so evidence/plan
        // previews reach the same source a `merge` invocation would — same prefix-routing shape as
        // `run_log`'s `remotes/` dispatch, not `run_verify`/`branch list`'s list-both shape, since
        // this resolves one specific input rather than showing everything.
        MergeEvidenceTargetArg::Ref(ref_name) if ref_name.starts_with("remotes/") => {
            MergeEvidenceTarget::ReceivedRef(ref_name)
        }
        MergeEvidenceTargetArg::Ref(ref_name) => MergeEvidenceTarget::Ref(ref_name),
    }
}

fn run_inverse_plan(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_inverse_plan_args(args)?;
    let layout = open_repository(args.root)?;
    let plan =
        prepare_patch_inverse_plan(&layout, &args.ref_name).map_err(|err| err.to_string())?;
    print_patch_inverse_plan(&layout, &plan);
    Ok(())
}

fn run_rollback_preview(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_rollback_preview_args(args)?;
    let layout = open_repository(args.root)?;
    let plan = prepare_rollback_preview(&layout, &args.ref_name).map_err(|err| err.to_string())?;
    print_rollback_preview_plan(&layout, &plan);
    Ok(())
}

fn run_rollback_draft(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_rollback_draft_args(args)?;
    let layout = open_repository(args.root)?;
    layout
        .require_current_format()
        .map_err(|err| err.to_string())?;
    let signer = author_signer_from_env()?;
    let report = append_rollback_draft(&layout, &args.ref_name, &args.message, &signer)
        .map_err(|err| err.to_string())?;
    print_rollback_draft_report(&layout, &report);
    Ok(())
}

fn run_rollback_draft_verify(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_rollback_draft_verify_args(args)?;
    let layout = open_repository(args.root)?;
    let report =
        verify_active_rollback_draft(&layout, &args.ref_name).map_err(|err| err.to_string())?;
    print_rollback_draft_verification(&layout, &report);
    Ok(())
}

fn run_worktree_status(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_worktree_status_args(args)?;
    let layout = open_repository(args.root)?;
    let report = worktree_status(&layout, &args.ref_name).map_err(|err| err.to_string())?;
    print_worktree_status(&layout, &report);
    if report.is_clean() {
        Ok(())
    } else {
        Err("worktree has changes against the baseline"
            .to_string()
            .into())
    }
}

fn run_verify(args: Vec<String>) -> std::result::Result<(), CliError> {
    let verify_args = parse_verify_args(args)?;
    let layout = open_repository(verify_args.root)?;
    let options = VerifyOptions {
        stop_on_first_error: verify_args.stop_on_first_error,
    };
    let report = verify_repository_with_options(&layout, options).map_err(|err| err.to_string())?;
    // RFC 118 stage 5: `--format json` emits exactly one JSON document and nothing else -- the
    // default prose path's received-refs lines below are additive presentation, not part of the
    // schema, and would corrupt a JSON stream if interleaved with it, so this branch replaces the
    // whole default body rather than adding to it.
    if verify_args.format == VerifyOutputFormat::Json {
        print_verify_report_json(&report)?;
    } else {
        print_verify_report(&layout, &report);
        // Received refs (DC-78 ruling 4) are never read by verify_repository itself — every object
        // they point at is already checked by the ordinary type-based object scan regardless of
        // which ref (if any) points to it, so this is purely additive presentation, not a new check.
        let received = list_received_pointers(&layout).map_err(|err| err.to_string())?;
        println!("received refs: {}", received.len());
        for pointer in &received {
            println!(
                "received-ref {}: {}",
                pointer.ref_name, pointer.ref_state_id
            );
        }
    }
    // This used to be a nine-arm hand-written else-if chain. It is now one lookup into
    // `verify_verdict::VERDICT_CONDITIONS` -- the same declaration `--format json` above reads --
    // so the exit code and the JSON verdict cannot silently diverge. First-match-wins order and
    // every message are unchanged from the prior chain.
    match verify_verdict::first_true_condition(&report) {
        Some(condition) => Err(condition.message.to_string().into()),
        None => Ok(()),
    }
}

fn run_doctor(args: Vec<String>) -> std::result::Result<(), CliError> {
    let doctor_args = parse_doctor_args(args)?;
    let layout = open_repository(doctor_args.root)?;
    if doctor_args.repair_wal_tail || doctor_args.repair_main_ref {
        let options = DoctorRepairOptions {
            truncate_wal_tail: doctor_args.repair_wal_tail,
            reconstruct_main_ref: doctor_args.repair_main_ref,
        };
        let repair = repair_repository(&layout, options).map_err(|err| err.to_string())?;
        println!("doctor repository: {}", layout.prikk_dir().display());
        // RFC 108 increment 3d review v1 §1's condition: `repair.wal_repair`'s own two lines
        // (still present in the struct, unchanged, for `default`-only callers) used to be the only
        // thing printed here -- silently omitting every other active session's own outcome, and
        // omitting even `default`'s own reason when it was `Skipped` rather than repaired.
        // `print_active_session_repairs` replaces them, covering every active session
        // (`default` included) from the one field that already carries the full, honest answer.
        print_active_session_repairs(&repair.active_repairs);
        print_doctor_report(&layout, &repair.after);
        // Exit-rule adjudication (review v1 §1): "every skip is a failure" is the correct rule
        // *here*, not as a general principle, but because this branch is only reached when
        // `doctor_args.repair_wal_tail` requested exactly this repair -- `repair_main_ref` always
        // refuses inside `repair_repository` itself, before any active session is even attempted, so
        // reaching this line means `truncate_wal_tail: true` was requested unconditionally. A
        // `Skipped` entry in `active_repairs` is therefore never vacuous here: it always means a WAL
        // tail truncation the operator asked for did not happen. (A hypothetical caller of
        // `repair_repository` that requests no repair at all, `DoctorRepairOptions::none()`, would
        // need a different rule -- skips would be expected, not a failure -- but no CLI path reaches
        // this line with that combination, so that rule is not built here.)
        let any_active_session_skipped = repair
            .active_repairs
            .iter()
            .any(|outcome| matches!(outcome.status, ActiveSessionRepairStatus::Skipped { .. }));
        if any_active_session_skipped {
            Err(
                "doctor repair skipped one or more active sessions; see the per-active outcomes \
                 above for which and why"
                    .to_string()
                    .into(),
            )
        } else if repair.after.is_healthy() {
            Ok(())
        } else {
            Err("doctor repair finished but repository health errors remain"
                .to_string()
                .into())
        }
    } else {
        let report = doctor_repository(&layout);
        println!("doctor repository: {}", layout.prikk_dir().display());
        print_doctor_report(&layout, &report);
        if report.is_healthy() {
            Ok(())
        } else {
            Err("doctor found repository health errors".to_string().into())
        }
    }
}

/// Build the AUTHOR signer from caller-supplied key material in the environment, failing closed if
/// none is configured. This is deliberately minimal key *input* — no trust store, key file, rotation,
/// or persistence (those are later phases). Real authoring requires:
///   - `PRIKK_AUTHOR_KEY_ID`: non-empty key identifier recorded in the signature;
///   - `PRIKK_AUTHOR_SEED`: 64 hex characters (a 32-byte Ed25519 secret seed).
fn author_signer_from_env() -> Result<Ed25519AuthorSigner, String> {
    let key_id = std::env::var("PRIKK_AUTHOR_KEY_ID").map_err(|_| {
        "author signing is required: set PRIKK_AUTHOR_KEY_ID (no signing key configured)"
            .to_string()
    })?;
    if key_id.trim().is_empty() {
        return Err("PRIKK_AUTHOR_KEY_ID must not be empty".to_string());
    }
    let seed_hex = std::env::var("PRIKK_AUTHOR_SEED").map_err(|_| {
        "author signing is required: set PRIKK_AUTHOR_SEED (64 hex chars; no signing key configured)"
            .to_string()
    })?;
    let seed = decode_seed_hex(&seed_hex, "PRIKK_AUTHOR_SEED")?;
    Ed25519AuthorSigner::from_seed(key_id, &seed).map_err(|err| err.to_string())
}

/// Build the MAINTAINER signer from caller-supplied key material in the environment, failing closed
/// if none is configured.
pub(crate) fn maintainer_signer_from_env() -> Result<Ed25519MaintainerSigner, String> {
    let key_id = std::env::var("PRIKK_MAINTAINER_KEY_ID").map_err(|_| {
        "maintainer signing is required: set PRIKK_MAINTAINER_KEY_ID (no signing key configured)"
            .to_string()
    })?;
    if key_id.trim().is_empty() {
        return Err("PRIKK_MAINTAINER_KEY_ID must not be empty".to_string());
    }
    let seed_hex = std::env::var("PRIKK_MAINTAINER_SEED").map_err(|_| {
        "maintainer signing is required: set PRIKK_MAINTAINER_SEED (64 hex chars; no signing key configured)"
            .to_string()
    })?;
    let seed = decode_seed_hex(&seed_hex, "PRIKK_MAINTAINER_SEED")?;
    Ed25519MaintainerSigner::from_seed(key_id, &seed).map_err(|err| err.to_string())
}

/// Read a 32-byte Ed25519 secret seed from the named environment variable (RFC 135 §9.3: the
/// caller passes the variable's *name* on argv, never the seed itself). Shared by `prikk key
/// public --seed-env` and any future command that must derive from a seed the user already holds.
pub(crate) fn read_seed_env(var_name: &str) -> std::result::Result<[u8; 32], CliError> {
    let value = std::env::var(var_name)
        .map_err(|_| CliError::Usage(format!("environment variable {var_name} is not set")))?;
    decode_seed_hex(&value, var_name).map_err(CliError::Usage)
}

/// Decode exactly 64 hex characters into a 32-byte Ed25519 secret seed.
fn decode_seed_hex(hex: &str, env_name: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!(
            "{env_name} must be 64 hex characters, got {}",
            hex.len()
        ));
    }
    let mut seed = [0_u8; 32];
    for (slot, pair) in seed.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let hi = pair
            .first()
            .copied()
            .ok_or_else(|| format!("{env_name} truncated"))?;
        let lo = pair
            .get(1)
            .copied()
            .ok_or_else(|| format!("{env_name} truncated"))?;
        *slot = (hex_nibble(hi, env_name)? << 4) | hex_nibble(lo, env_name)?;
    }
    Ok(seed)
}

/// Convert one ASCII hex character to its 4-bit value.
fn hex_nibble(c: u8, env_name: &str) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(format!("{env_name} contains a non-hex character")),
    }
}
