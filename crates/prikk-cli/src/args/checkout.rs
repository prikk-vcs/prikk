//! Argument parsing for checkout commands.

use std::path::PathBuf;

use prikk_store::{DEFAULT_CHECKOUT_REF, RepoPath};

use super::optional_path_or_current;
use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;

/// Parsed checkout command arguments.
pub(crate) struct CheckoutArgs {
    /// Repository root.
    pub(crate) root: PathBuf,
    /// Ref to inspect.
    pub(crate) ref_name: String,
    /// Checkout mode.
    pub(crate) mode: CheckoutMode,
    /// Whether `--format json` was given. RFC 143: meaningful only for `--patch-plan`; rejected
    /// at parse time for every other mode rather than silently ignored.
    pub(crate) format_json: bool,
    /// `--content-path <repo-relative path>`, repeatable. RFC 143 §5: content is emitted only for
    /// these paths, never the whole tree by default -- validated with the same `RepoPath::parse`
    /// the object layer itself uses, so a malformed path is a usage error (exit `2`, RFC 121)
    /// caught before any repository work begins, not an `Integrity` failure surfacing later as
    /// exit `1`.
    pub(crate) content_paths: Vec<String>,
}

/// Checkout command mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckoutMode {
    /// Read-only general checkout planning.
    PlanOnly,
    /// Read-only snapshot manifest validation.
    SnapshotPlan,
    /// Opt-in snapshot materialization.
    SnapshotMaterialize,
    /// Read-only supported patch replay planning.
    PatchPlan,
    /// Opt-in materialization from supported patch replay.
    PatchMaterialize,
    /// Read-only deletion plan for explicit patch-removed files.
    PatchDeletePlan,
    /// Opt-in patch materialization plus explicit patch-removed file deletion.
    PatchMaterializeDelete,
}

/// Parse `prikk checkout` arguments.
pub(crate) fn parse_checkout_args(
    args: Vec<String>,
) -> std::result::Result<CheckoutArgs, CliError> {
    let mut mode = None;
    let mut path = None;
    let mut ref_name = None;
    let mut format_json = false;
    let mut content_paths = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--plan-only" => set_checkout_mode(&mut mode, CheckoutMode::PlanOnly)?,
            "--snapshot-plan" => set_checkout_mode(&mut mode, CheckoutMode::SnapshotPlan)?,
            "--snapshot-materialize" => {
                set_checkout_mode(&mut mode, CheckoutMode::SnapshotMaterialize)?
            }
            "--patch-plan" => set_checkout_mode(&mut mode, CheckoutMode::PatchPlan)?,
            "--patch-materialize" => set_checkout_mode(&mut mode, CheckoutMode::PatchMaterialize)?,
            "--patch-delete-plan" => set_checkout_mode(&mut mode, CheckoutMode::PatchDeletePlan)?,
            "--patch-materialize-delete" => {
                set_checkout_mode(&mut mode, CheckoutMode::PatchMaterializeDelete)?
            }
            "--ref" => {
                let value = flag_value(&mut iter, "checkout --ref")?;
                if value.trim().is_empty() {
                    return Err(CliError::Usage(
                        "checkout --ref must not be empty".to_string(),
                    ));
                }
                ref_name.set_once("--ref", value)?;
            }
            "--format" => {
                let value = flag_value(&mut iter, "checkout --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "checkout --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            "--content-path" => {
                let value = flag_value(&mut iter, "checkout --content-path")?;
                RepoPath::parse(&value).map_err(|err| {
                    CliError::Usage(format!(
                        "checkout --content-path {value:?} is not a valid repository-relative \
                         path ({err})"
                    ))
                })?;
                content_paths.push(value);
            }
            other if other.starts_with('-') => return Err(unknown_argument("checkout", other)),
            _ => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "checkout accepts at most one path".to_string(),
                    ));
                }
                path = Some(arg);
            }
        }
    }
    let Some(mode) = mode else {
        return Err(CliError::Usage(
            concat!(
                "checkout requires one mode flag: `--plan-only`, `--snapshot-plan`, ",
                "`--snapshot-materialize`, `--patch-plan`, `--patch-materialize`, ",
                "`--patch-delete-plan`, or `--patch-materialize-delete`",
            )
            .to_string(),
        ));
    };
    if mode != CheckoutMode::PatchPlan && (format_json || !content_paths.is_empty()) {
        return Err(CliError::Usage(
            "checkout --format and --content-path are only meaningful with --patch-plan"
                .to_string(),
        ));
    }
    if !content_paths.is_empty() && !format_json {
        return Err(CliError::Usage(
            "checkout --content-path requires --format json".to_string(),
        ));
    }
    Ok(CheckoutArgs {
        root: optional_path_or_current(path)?,
        ref_name: ref_name.unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string()),
        mode,
        format_json,
        content_paths,
    })
}

fn set_checkout_mode(
    mode: &mut Option<CheckoutMode>,
    next: CheckoutMode,
) -> std::result::Result<(), CliError> {
    if mode.is_some() {
        return Err(CliError::Usage(
            "checkout accepts only one mode flag".to_string(),
        ));
    }
    *mode = Some(next);
    Ok(())
}
