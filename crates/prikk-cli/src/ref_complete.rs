//! `prikk ref complete <ref>` (RFC 165 R4) -- the one way out of an interrupted publication that
//! does not require the original signer: [`prikk_store::plan_ref_completion`] decides whether
//! `<ref>` is a completable lead, and this module is the thin CLI shell around it plus
//! [`prikk_store::complete_ref_publication`], the write.
//!
//! `--plan-only` and a real run share one code path up to the write itself (K1): both resolve the
//! signer, compute the plan, and print the same fields in the same order before a real run commits
//! anything -- `--plan-only` simply never reaches [`prikk_store::complete_ref_publication`].

use std::path::PathBuf;

use prikk_store::{ActiveLock, DEFAULT_ACTIVE_NAME, MaintainerSigner, ObjectWriteSession};

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;

use crate::arg_scan::{mark_seen, unknown_argument};
use crate::commands::CliError;

/// Dispatch `prikk ref [complete]`.
pub fn run_ref(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    match iter.next().as_deref() {
        Some("complete") => run_complete(root, iter.collect()),
        Some(other) => Err(CliError::Usage(format!(
            "unknown ref subcommand: {other} (expected complete)"
        ))),
        None => Err(CliError::Usage(
            "ref requires a subcommand: complete".to_string(),
        )),
    }
}

struct CompleteArgs {
    ref_name: String,
    plan_only: bool,
}

fn parse_complete_args(args: Vec<String>) -> std::result::Result<CompleteArgs, CliError> {
    let mut ref_name = None;
    let mut plan_only = false;
    for arg in args {
        match arg.as_str() {
            "--plan-only" => mark_seen(&mut plan_only, "--plan-only")?,
            other if other.starts_with('-') => {
                return Err(unknown_argument("ref complete", other));
            }
            _ => {
                if ref_name.is_some() {
                    return Err(CliError::Usage(
                        "ref complete accepts at most one ref".to_string(),
                    ));
                }
                ref_name = Some(arg);
            }
        }
    }
    let Some(ref_name) = ref_name else {
        return Err(CliError::Usage("ref complete requires <ref>".to_string()));
    };
    Ok(CompleteArgs {
        ref_name,
        plan_only,
    })
}

/// `<ref>` must already be a well-formed local branch or tag ref -- `plan_ref_completion` itself
/// does not care which namespace it is in (the rule is the same for both), but a malformed name
/// should refuse here, before a signer is even built, exactly like every other ref-naming command
/// in this crate (AUD-10).
fn validate_ref_name(ref_name: &str) -> std::result::Result<String, CliError> {
    if ref_name.starts_with("heads/") {
        return prikk_store::validate_local_branch_ref(ref_name)
            .map_err(|err| err.to_string().into());
    }
    if ref_name.starts_with("tags/") {
        return prikk_store::validate_local_tag_ref(ref_name).map_err(|err| err.to_string().into());
    }
    Err(format!(
        "ref {ref_name} is not a local branch or tag ref; expected heads/<name> or tags/<name>"
    )
    .into())
}

fn run_complete(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_complete_args(args)?;
    let ref_name = validate_ref_name(&parsed.ref_name)?;
    let layout = crate::open_repository(root)?;
    // K1: `--plan-only` and a real run report the identical "completing key" field, so the signer
    // is resolved before the plan is even computed, regardless of which mode this is.
    let signer = crate::maintainer_signer_from_env()?;

    let plan = prikk_store::plan_ref_completion(&layout, &ref_name)
        .map_err(|err| err.to_string())?
        .map_err(|refusal| format!("{ref_name} {refusal}"))?;

    print_plan(&plan, &signer);

    if parsed.plan_only {
        return Ok(());
    }

    let active_lock =
        ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).map_err(|err| err.to_string())?;
    let mut object_store = ObjectWriteSession::open(&layout).map_err(|err| err.to_string())?;
    let completed_id = prikk_store::complete_ref_publication(
        &layout,
        &mut object_store,
        &active_lock,
        &plan,
        &signer,
    )
    .map_err(|err| err.to_string())?;
    drop(object_store);
    drop(active_lock);
    println!("{ref_name} completed: RefState {completed_id}");
    Ok(())
}

fn print_plan(plan: &prikk_store::CompletionPlan, signer: &impl MaintainerSigner) {
    println!("ref: {}", plan.ref_name);
    println!("leading RefState: {}", plan.leading_ref_state_id);
    println!("original signer: {}", plan.original_signer_key_id);
    println!("target: {} ({:?})", plan.target_object_id, plan.kind);
    match plan.log_tip {
        Some(tip) => println!("log tip: {tip}"),
        None => println!("log tip: (none -- first publication for this ref)"),
    }
    println!("sequence: {}", plan.next_sequence);
    println!("completing key: {}", signer.key_id());
    println!(
        "partial tail to remove: {} byte(s)",
        plan.removes_partial_tail_bytes
    );
}
