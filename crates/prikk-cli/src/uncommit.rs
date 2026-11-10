//! `prikk uncommit <patch-id> [--plan-only]` (0.57.0 step 3, S5b): take back your own newest unsealed commit.
//!
//! The inverse of `commit` for work that has not been sealed. Nothing is wrong when this runs. The logic, the plan and the
//! recovery entries are `prikk_store::uncommit`'s; this module parses the arguments and prints what the store returns. Text only,
//! with the exit codes of the other repairs (0 done, 1 refused, 2 usage): no `--format json` until the whole CLI's JSON tier is
//! decided (0.59.0), so that no command gets a one-off shape.

use prikk_store::{OperationFate, RecordOutcome, UncommitPlan, plan_uncommit, uncommit};

use crate::arg_scan::{SetOnce, mark_seen, unknown_argument};
use crate::args::current_dir;
use crate::commands::CliError;
use crate::open_repository;
// RFC 121 §2.1: shadows the prelude's `println!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;

struct UncommitArgs {
    patch_id: prikk_object::ObjectId,
    plan_only: bool,
}

fn parse_args(args: Vec<String>) -> std::result::Result<UncommitArgs, CliError> {
    let mut patch_id: Option<String> = None;
    let mut plan_only = false;
    for arg in args {
        match arg.as_str() {
            "--plan-only" => mark_seen(&mut plan_only, "--plan-only")?,
            other if other.starts_with('-') => return Err(unknown_argument("uncommit", other)),
            _ => patch_id.set_once("<patch-id>", arg)?,
        }
    }
    let patch_id = patch_id.ok_or_else(|| {
        CliError::Usage(
            "uncommit requires <patch-id>: the id of your newest queued commit (`prikk status --format json` lists them)"
                .to_string(),
        )
    })?;
    let patch_id = patch_id.parse::<prikk_object::ObjectId>().map_err(|_| {
        CliError::Usage(format!(
            "uncommit: {patch_id:?} is not a patch id (64 hexadecimal characters; `prikk status --format json` lists the queued ones)"
        ))
    })?;
    Ok(UncommitArgs {
        patch_id,
        plan_only,
    })
}

pub(crate) fn run_uncommit(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_args(args)?;
    let root = current_dir()?;
    let layout = open_repository(root)?;
    layout
        .require_current_format()
        .map_err(|err| err.to_string())?;
    let result = if args.plan_only {
        plan_uncommit(&layout, args.patch_id)
    } else {
        uncommit(&layout, args.patch_id)
    };
    let plan = result.map_err(|err| err.to_string())?;
    print_plan(&plan, args.plan_only);
    Ok(())
}

fn fate_words(fate: OperationFate) -> &'static str {
    match fate {
        OperationFate::Reauthored => {
            "the worktree still has it; the next `prikk commit` authors it again"
        }
        OperationFate::RenameDeclared => {
            "put back as a rename declaration; the next `prikk commit` authors the rename again"
        }
        OperationFate::NotInWorktree => {
            "the worktree does not have this change, so the next `prikk commit` does not author it"
        }
        OperationFate::NotAuthoredByCommit => {
            "`prikk commit` does not author this kind from the worktree"
        }
        _ => "this prikk cannot tell what the next `prikk commit` does about it",
    }
}

fn print_plan(plan: &UncommitPlan, plan_only: bool) {
    let queue_after = plan.queue_after.len();
    let (commits, remain) = if queue_after == 1 {
        ("commit", "remains")
    } else {
        ("commits", "remain")
    };
    if plan.finishing {
        match &plan.earlier_run {
            Some(run) => println!(
                "commit {} is already out of the queue; finishing an interrupted `prikk uncommit` (recovery run {run})",
                plan.patch_id
            ),
            // The recovery log has no `uncommit` run for it: the same state is what a lost acknowledged commit looks like.
            None => println!(
                "commit {} is already out of the queue (an interrupted `prikk uncommit`, or an acknowledged commit that was lost); taking it out of the record",
                plan.patch_id
            ),
        }
    } else {
        let on = plan
            .ref_name
            .as_deref()
            .map_or_else(String::new, |name| format!(" on {name}"));
        let sequence = plan
            .sequence
            .map_or_else(String::new, |seq| format!("sequence {seq}"));
        println!("commit {} ({sequence}{on})", plan.patch_id);
        if let Some(message) = &plan.message {
            println!("message: {message}");
        }
        for operation in &plan.operations {
            println!(
                "  {} {} -- {}",
                operation.kind,
                operation.paths.join(" -> "),
                fate_words(operation.fate)
            );
        }
        println!(
            "it leaves the queue and is kept in the recovery log ({} bytes); {queue_after} queued {commits} {remain}",
            plan.removed_bytes
        );
        println!("your files in the worktree are not changed");
        if plan.blobs_left > 0 {
            println!(
                "{} blob(s) it wrote stay in the object store: committing the same file again reuses them; one that nothing else \
                 reaches shows in `prikk verify` as an unreached object until reclamation exists",
                plan.blobs_left
            );
        }
    }
    match plan.record {
        RecordOutcome::Rewritten => {
            println!("the worktree's record is brought up to date with the queue");
        }
        RecordOutcome::LeftAsItWas => {
            println!("the worktree's record was not this queue's, and is left as it was");
        }
        _ => {}
    }
    if plan.clears_owner {
        println!("the queue is empty: its owner is cleared, as `seal` clears it");
    }
    if plan_only {
        println!("plan only -- nothing written");
        return;
    }
    match &plan.recovery_run {
        Some(run) => println!(
            "commit taken back; undo with `prikk doctor --recovery-restore {run}` (it refuses once anything has changed since)"
        ),
        None => println!("commit taken back"),
    }
}
