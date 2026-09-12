//! `prikk compact` — reclaim stale records from the three genuine compaction targets (RFC 102 Stage
//! 6 Step 2). No confirmation prompt, unlike `prikk unlock`: see `prikk_store::compact`'s own module
//! doc for why compaction has no operator-only fact the tool cannot check itself.
//!
//! A bare `prikk compact` names no target and refuses rather than defaulting to `--all` -- the one
//! place in this command where "the tool decides" has a cheap, obvious alternative.

use std::path::PathBuf;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::arg_scan::unknown_argument;
use crate::commands::CliError;
use crate::stdout::println;
use prikk_store::{
    CompactionReport, compact_received_index, compact_ref_pointer_index, compact_trust_policy,
    plan_compact_received_index, plan_compact_ref_pointer_index, plan_compact_trust_policy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    PointerIndex,
    ReceivedIndex,
    TrustPolicy,
}

const ALL_TARGETS: [Target; 3] = [
    Target::PointerIndex,
    Target::ReceivedIndex,
    Target::TrustPolicy,
];

pub(crate) fn run_compact(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut targets: Vec<Target> = Vec::new();
    let mut plan_only = false;
    let push_target = |targets: &mut Vec<Target>, flag: &str, target: Target| {
        if targets.contains(&target) {
            return Err(CliError::Usage(format!("duplicate {flag} flag")));
        }
        targets.push(target);
        Ok(())
    };
    for arg in args {
        match arg.as_str() {
            "--pointer-index" => {
                push_target(&mut targets, "--pointer-index", Target::PointerIndex)?
            }
            "--received-index" => {
                push_target(&mut targets, "--received-index", Target::ReceivedIndex)?
            }
            "--trust-policy" => push_target(&mut targets, "--trust-policy", Target::TrustPolicy)?,
            "--all" => {
                if !targets.is_empty() {
                    return Err(CliError::Usage(
                        "compact --all cannot be combined with another target flag".to_string(),
                    ));
                }
                targets.extend(ALL_TARGETS);
            }
            "--plan-only" => {
                if plan_only {
                    return Err(CliError::Usage("duplicate --plan-only flag".to_string()));
                }
                plan_only = true;
            }
            other => return Err(unknown_argument("compact", other)),
        }
    }
    if targets.is_empty() {
        return Err(CliError::Usage(
            "compact requires a target: --pointer-index, --received-index, --trust-policy, or \
             --all (add --plan-only to preview without writing)"
                .to_string(),
        ));
    }

    let layout = crate::open_repository(root)?;
    for target in targets {
        let report = run_one(&layout, target, plan_only).map_err(|err| err.to_string())?;
        print_report(&report, plan_only);
    }
    Ok(())
}

fn run_one(
    layout: &prikk_store::RepositoryLayout,
    target: Target,
    plan_only: bool,
) -> prikk_error::Result<CompactionReport> {
    match (target, plan_only) {
        (Target::PointerIndex, false) => compact_ref_pointer_index(layout),
        (Target::PointerIndex, true) => plan_compact_ref_pointer_index(layout),
        (Target::ReceivedIndex, false) => compact_received_index(layout),
        (Target::ReceivedIndex, true) => plan_compact_received_index(layout),
        (Target::TrustPolicy, false) => compact_trust_policy(layout),
        (Target::TrustPolicy, true) => plan_compact_trust_policy(layout),
    }
}

fn print_report(report: &CompactionReport, plan_only: bool) {
    let name = match report.container {
        prikk_store::LockableContainer::RefPointerIndex => "pointer-index",
        prikk_store::LockableContainer::ReceivedIndex => "received-index",
        prikk_store::LockableContainer::TrustPolicy => "trust-policy",
        prikk_store::LockableContainer::RefLog => "ref-log",
        // Unreachable in practice -- the object store is not a compaction target and no
        // `CompactionReport` is ever built for it (`compact.rs`'s own `Target` has three variants).
        // Spelled out rather than reached by a wildcard so that adding a sixth `LockableContainer`
        // still breaks this match, which is exactly how the object-store variant itself was caught.
        prikk_store::LockableContainer::ObjectStore => "object-store",
    };
    let verb = if plan_only {
        "would reclaim"
    } else {
        "reclaimed"
    };
    let reclaimed = report.entries_before.saturating_sub(report.entries_after);
    println!(
        "{name}: {reclaimed} {verb} ({} -> {} live records)",
        report.entries_before, report.entries_after
    );
}
