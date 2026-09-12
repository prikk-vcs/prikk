//! CLI argument parsing helpers.

use std::path::PathBuf;

use prikk_store::{DEFAULT_CHECKOUT_REF, DEFAULT_HISTORY_LIMIT};

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;

mod checkout;
mod merge_evidence;
mod merge_execute;

pub(crate) use checkout::{CheckoutMode, parse_checkout_args};
pub(crate) use merge_evidence::{
    MergeEvidenceTargetArg, parse_merge_evidence_args, parse_merge_plan_args,
};
pub(crate) use merge_execute::{MergeExecuteArgs, parse_merge_execute_args};

/// Parsed commit command arguments.
pub(crate) struct CommitArgs {
    /// Commit message.
    pub(crate) message: String,
    /// Baseline ref for worktree commits.
    pub(crate) ref_name: String,
    /// Compatibility flag retained for text edit generation.
    pub(crate) text_edits: bool,
}

/// Parsed log command arguments.
pub(crate) struct LogArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Ref to inspect.
    pub(crate) ref_name: String,
    /// Maximum entries to display.
    pub(crate) limit: usize,
    /// RFC 146: `true` for `--format json` (`log-report-v1`).
    pub(crate) format_json: bool,
}

/// Parsed inverse-plan command arguments.
pub(crate) struct InversePlanArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Ref to inspect.
    pub(crate) ref_name: String,
}

/// Parsed rollback-preview command arguments.
pub(crate) struct RollbackPreviewArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Ref to inspect.
    pub(crate) ref_name: String,
}

/// Parsed rollback-draft command arguments.
pub(crate) struct RollbackDraftArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Ref to inspect.
    pub(crate) ref_name: String,
    /// Rollback draft message.
    pub(crate) message: String,
}

/// Parsed rollback-draft-verify command arguments.
pub(crate) struct RollbackDraftVerifyArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Ref to inspect.
    pub(crate) ref_name: String,
}

/// Parsed worktree-status command arguments.
pub(crate) struct WorktreeStatusArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Ref to use as baseline.
    pub(crate) ref_name: String,
    /// RFC 144 §4o.3: `true` for `--format json`, the machine-branchable form that carries live
    /// rename declarations as a real field rather than a prose line.
    pub(crate) format_json: bool,
}

/// Parsed doctor command arguments.
pub(crate) struct DoctorArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Whether to repair incomplete trailing WAL bytes.
    pub(crate) repair_wal_tail: bool,
    /// Whether `--repair-main-ref` was supplied. Always refused -- no repair is implemented.
    pub(crate) repair_main_ref: bool,
    /// Whether to rebuild the object index from the containers (RFC 102's repair round).
    pub(crate) repair_index: bool,
}

/// `prikk verify`'s output format (RFC 118 stage 5). `Prose` is the default and must remain
/// byte-identical to the pre-stage-5 output; `Json` is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifyOutputFormat {
    /// The original, human-readable line-per-fact report (unchanged by RFC 118 stage 5).
    Prose,
    /// `verify-report-v1` (RFC 118 stage 5): schema version, verdict, and one entry per
    /// `VerificationStage::ALL`.
    Json,
}

/// Parsed verify command arguments.
pub(crate) struct VerifyArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// DC-95 Stage 2 Level 1: stop at the first stage that fails or cannot evaluate, rather than
    /// accumulating findings across all twelve. Preserves the pre-Stage-2 bounded-walk behavior for
    /// a large, badly-damaged repository where a full accumulating scan would be costly.
    pub(crate) stop_on_first_error: bool,
    /// Output format. Default `Prose` (RFC 118 stage 5).
    pub(crate) format: VerifyOutputFormat,
}

/// Parse `prikk verify` arguments.
pub(crate) fn parse_verify_args(args: Vec<String>) -> std::result::Result<VerifyArgs, CliError> {
    let mut stop_on_first_error = false;
    let mut format = None;
    let mut path = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--stop-on-first-error" => {
                mark_seen(&mut stop_on_first_error, "--stop-on-first-error")?
            }
            "--format" => {
                let value = flag_value(&mut iter, "verify --format")?;
                let resolved = match value.as_str() {
                    "json" => VerifyOutputFormat::Json,
                    other => {
                        return Err(CliError::Usage(format!(
                            "verify --format does not support {other:?}"
                        )));
                    }
                };
                format.set_once("--format", resolved)?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("verify", other)),
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "verify accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    Ok(VerifyArgs {
        root: optional_path_or_current(path)?,
        stop_on_first_error,
        format: format.unwrap_or(VerifyOutputFormat::Prose),
    })
}

/// Parse `prikk log` arguments.
pub(crate) fn parse_log_args(args: Vec<String>) -> std::result::Result<LogArgs, CliError> {
    let mut path = None;
    let mut ref_name = None;
    let mut limit = None;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "log --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage("log --ref must not be empty".to_string()));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--limit" => {
                let value = flag_value(&mut iter, "log --limit")?;
                let parsed = value.parse::<usize>().map_err(|_| {
                    CliError::Usage("log --limit must be a non-negative integer".to_string())
                })?;
                limit.set_once("--limit", parsed)?;
            }
            // RFC 146: the same restricted-value pattern `worktree-status --format json` already
            // uses -- the only supported value is `json`.
            "--format" => {
                let value = flag_value(&mut iter, "log --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "log --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("log", other)),
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage("log accepts at most one path".to_string()));
                }
                path = Some(arg);
            }
        }
    }
    Ok(LogArgs {
        root: optional_path_or_current(path)?,
        ref_name: ref_name.unwrap_or_else(|| "heads/main".to_string()),
        limit: limit.unwrap_or(DEFAULT_HISTORY_LIMIT),
        format_json,
    })
}

/// Parse `prikk inverse-plan` arguments.
pub(crate) fn parse_inverse_plan_args(
    args: Vec<String>,
) -> std::result::Result<InversePlanArgs, CliError> {
    let mut path = None;
    let mut ref_name = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "inverse-plan --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "inverse-plan --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("inverse-plan", other)),
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "inverse-plan accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    Ok(InversePlanArgs {
        root: optional_path_or_current(path)?,
        ref_name: ref_name.unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string()),
    })
}

/// Parse `prikk rollback-preview` arguments.
pub(crate) fn parse_rollback_preview_args(
    args: Vec<String>,
) -> std::result::Result<RollbackPreviewArgs, CliError> {
    let mut path = None;
    let mut ref_name = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "rollback-preview --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "rollback-preview --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("rollback-preview", other));
            }
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "rollback-preview accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    Ok(RollbackPreviewArgs {
        root: optional_path_or_current(path)?,
        ref_name: ref_name.unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string()),
    })
}

/// Parse `prikk rollback-draft` arguments.
pub(crate) fn parse_rollback_draft_args(
    args: Vec<String>,
) -> std::result::Result<RollbackDraftArgs, CliError> {
    let mut path = None;
    let mut ref_name = None;
    let mut message = None;
    let mut append_inverse = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--append-inverse" => mark_seen(&mut append_inverse, "--append-inverse")?,
            "--ref" => {
                let value = flag_value(&mut iter, "rollback-draft --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "rollback-draft --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            "-m" | "--message" => {
                let value = flag_value(&mut iter, "rollback-draft message option")?;
                message.set_once("-m/--message", value)?;
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("rollback-draft", other));
            }
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "rollback-draft accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    if !append_inverse {
        return Err(CliError::Usage(
            "rollback-draft requires --append-inverse".to_string(),
        ));
    }
    let Some(message) = message else {
        return Err(CliError::Usage(
            "rollback-draft requires -m <message>".to_string(),
        ));
    };
    if message.trim().is_empty() {
        return Err(CliError::Usage(
            "rollback-draft message must not be empty".to_string(),
        ));
    }
    Ok(RollbackDraftArgs {
        root: optional_path_or_current(path)?,
        ref_name: ref_name.unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string()),
        message,
    })
}

/// Parse `prikk rollback-draft-verify` arguments.
pub(crate) fn parse_rollback_draft_verify_args(
    args: Vec<String>,
) -> std::result::Result<RollbackDraftVerifyArgs, CliError> {
    let mut path = None;
    let mut ref_name = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "rollback-draft-verify --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "rollback-draft-verify --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("rollback-draft-verify", other));
            }
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "rollback-draft-verify accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    Ok(RollbackDraftVerifyArgs {
        root: optional_path_or_current(path)?,
        ref_name: ref_name.unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string()),
    })
}

/// Parse `prikk worktree-status` arguments.
pub(crate) fn parse_worktree_status_args(
    args: Vec<String>,
) -> std::result::Result<WorktreeStatusArgs, CliError> {
    let mut path = None;
    let mut ref_name = None;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ref" => {
                let value = flag_value(&mut iter, "worktree-status --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "worktree-status --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            // RFC 144 §4o.3, the same restricted-value pattern `status --format json` already uses
            // (`commands.rs::run_status_adapter`'s own doc): the only supported value is `json`.
            "--format" => {
                let value = flag_value(&mut iter, "worktree-status --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "worktree-status --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("worktree-status", other));
            }
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "worktree-status accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    Ok(WorktreeStatusArgs {
        root: optional_path_or_current(path)?,
        ref_name: ref_name.unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string()),
        format_json,
    })
}

/// Parse `prikk doctor` arguments.
pub(crate) fn parse_doctor_args(args: Vec<String>) -> std::result::Result<DoctorArgs, CliError> {
    let mut repair_wal_tail = false;
    let mut repair_main_ref = false;
    let mut repair_index = false;
    let mut path = None;
    for arg in args {
        match arg.as_str() {
            "--repair-wal-tail" => mark_seen(&mut repair_wal_tail, "--repair-wal-tail")?,
            "--repair-main-ref" => mark_seen(&mut repair_main_ref, "--repair-main-ref")?,
            "--repair-index" => mark_seen(&mut repair_index, "--repair-index")?,
            other if other.starts_with('-') => return Err(unknown_argument("doctor", other)),
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "doctor accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    Ok(DoctorArgs {
        root: optional_path_or_current(path)?,
        repair_wal_tail,
        repair_main_ref,
        repair_index,
    })
}

/// Parse commit arguments. Commit always authors a node-addressed patch from the worktree against the
/// baseline ref; `--from-worktree` is accepted for backward compatibility but is the only behavior.
pub(crate) fn parse_commit_args(args: Vec<String>) -> std::result::Result<CommitArgs, CliError> {
    let mut message = None;
    let mut ref_name = None;
    let mut text_edits = false;
    let mut from_worktree_seen = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            // Accepted for compatibility; worktree authoring is the only commit behavior.
            "--from-worktree" => mark_seen(&mut from_worktree_seen, "--from-worktree")?,
            "--text-edits" => mark_seen(&mut text_edits, "--text-edits")?,
            "--ref" => {
                let value = flag_value(&mut iter, "commit --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "commit --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            "-m" | "--message" => {
                let value = flag_value(&mut iter, "commit message option")?;
                message.set_once("-m/--message", value)?;
            }
            other => return Err(unknown_argument("commit", other)),
        }
    }
    let Some(message) = message else {
        return Err(CliError::Usage(
            "commit requires -m <message> (usage: prikk commit [--from-worktree] [--text-edits] \
             [--ref <name>] -m <message>)"
                .to_string(),
        ));
    };
    if message.trim().is_empty() {
        return Err(CliError::Usage(
            "commit message must not be empty".to_string(),
        ));
    }
    Ok(CommitArgs {
        message,
        ref_name: ref_name.unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string()),
        text_edits,
    })
}

/// Parsed show command arguments (RFC 142). No `[path]` -- unlike most commands, `show` was never
/// specified with one (its own help line names only `<block-id|patch-id> [--format json]`), so
/// `run_show` opens the current directory directly, matching `run_status_adapter`.
pub(crate) struct ShowArgs {
    /// Target block or patch id.
    pub(crate) id: prikk_object::ObjectId,
    /// Whether `--format json` was given.
    pub(crate) format_json: bool,
}

/// Parse `prikk show` arguments.
pub(crate) fn parse_show_args(args: Vec<String>) -> std::result::Result<ShowArgs, CliError> {
    let mut id = None;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--format" => {
                let value = flag_value(&mut iter, "show --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "show --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other if other.starts_with('-') => return Err(unknown_argument("show", other)),
            _ => {
                if id.is_some() {
                    return Err(CliError::Usage(
                        "show accepts exactly one id (usage: prikk show <block-id|patch-id> \
                         [--format json])"
                            .to_string(),
                    ));
                }
                id = Some(arg);
            }
        }
    }
    let Some(id) = id else {
        return Err(CliError::Usage(
            "show requires <block-id|patch-id> (usage: prikk show <block-id|patch-id> [--format \
             json])"
                .to_string(),
        ));
    };
    let id = id.parse::<prikk_object::ObjectId>().map_err(|err| {
        CliError::Usage(format!(
            "show id must be a lowercase 64-hex object id ({err})"
        ))
    })?;
    Ok(ShowArgs { id, format_json })
}

/// Return an optional path or the current working directory.
pub(crate) fn optional_path_or_current(
    path: Option<String>,
) -> std::result::Result<PathBuf, String> {
    match path {
        Some(path) => Ok(PathBuf::from(path)),
        None => current_dir(),
    }
}

/// Return the current working directory.
pub(crate) fn current_dir() -> std::result::Result<PathBuf, String> {
    std::env::current_dir().map_err(|err| err.to_string())
}
