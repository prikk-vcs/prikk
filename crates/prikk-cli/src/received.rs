//! `prikk received remove <remotes/…> [--plan-only]` (0.54.0 step 3 Part 5, B2a): removes one
//! received ref's own pointer from the received-index container. The objects it reached are not
//! removed, only the pointer -- `prikk verify` reports what nothing reaches any more as an
//! **unreached object** warning, 0.54.0 step 3 Part 1's own mechanism (B1).
//!
//! `received` is a dispatcher, the same shape `sync`'s own sub-command parsing uses, even though
//! `remove` is its only verb today -- the received-index container's own other operations
//! (`compact --received-index`, the recovery-log restore) already live under `compact`/`doctor`,
//! so there is no obvious second verb planned for this group yet; kept as a dispatcher anyway
//! since a "received" verb group reads more naturally for an operator than a single flat command
//! would if a sibling verb is ever added.

use std::path::PathBuf;

use prikk_store::ReceivedRemoveReport;

use crate::arg_scan::{mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::stdout::println;

pub(crate) fn run_received(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    let Some(verb) = iter.next() else {
        return Err(CliError::Usage(
            "received requires a sub-command: remove".to_string(),
        ));
    };
    match verb.as_str() {
        "remove" => run_remove(root, iter.collect()),
        other if other.starts_with('-') => Err(CliError::Usage(
            "received requires a sub-command before any flags: remove".to_string(),
        )),
        other => Err(CliError::Usage(format!(
            "received has no sub-command {other:?}; expected: remove"
        ))),
    }
}

fn run_remove(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut ref_name = None;
    let mut plan_only = false;
    for arg in args {
        match arg.as_str() {
            "--plan-only" => mark_seen(&mut plan_only, "--plan-only")?,
            other if other.starts_with('-') => {
                return Err(unknown_argument("received remove", other));
            }
            _ => {
                if ref_name.is_some() {
                    return Err(CliError::Usage(
                        "received remove accepts exactly one ref name".to_string(),
                    ));
                }
                ref_name = Some(arg);
            }
        }
    }
    let Some(ref_name) = ref_name else {
        return Err(CliError::Usage(
            "received remove requires a ref name: prikk received remove <remotes/…> [--plan-only]"
                .to_string(),
        ));
    };

    let layout = crate::open_repository(root)?;
    let report = if plan_only {
        prikk_store::plan_remove_received_pointer(&layout, &ref_name)
    } else {
        prikk_store::remove_received_pointer(&layout, &ref_name)
    }
    .map_err(|err| err.to_string())?;
    print_report(&report, plan_only);
    Ok(())
}

fn print_report(report: &ReceivedRemoveReport, plan_only: bool) {
    let verb = if plan_only { "would remove" } else { "removed" };
    match &report.removed {
        Some(pointer) => println!(
            "received {verb}: {} (was {})",
            report.ref_name, pointer.ref_state_id
        ),
        None => println!("received {verb}: nothing named {}", report.ref_name),
    }
    println!(
        "received index: {} -> {} live record(s)",
        report.entries_before, report.entries_after
    );
    if let Some(run_id) = &report.run_id {
        println!(
            "recovery: saved as run {run_id} -- `prikk doctor --recovery-restore {run_id}` undoes \
             this, if the slot it would overwrite has not changed since"
        );
    }
}

#[cfg(test)]
mod tests;
