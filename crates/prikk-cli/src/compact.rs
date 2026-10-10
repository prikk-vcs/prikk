//! `prikk compact` — reclaim stale records from the three genuine compaction targets (RFC 102 Stage
//! 6 Step 2). No confirmation prompt, unlike `prikk unlock`: see `prikk_store::compact`'s own module
//! doc for why compaction has no operator-only fact the tool cannot check itself.
//!
//! A bare `prikk compact` names no target and refuses rather than defaulting to `--all` -- the one
//! place in this command where "the tool decides" has a cheap, obvious alternative.

use std::path::PathBuf;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::arg_scan::{SetOnce, flag_value, unknown_argument};
use crate::commands::CliError;
use crate::stdout::println;
use prikk_store::{
    CompactionReport, ContainerSlot, KeepSlotReport, compact_received_index,
    compact_received_index_keep_slot, compact_ref_pointer_index, compact_trust_policy,
    compact_trust_policy_keep_slot, plan_compact_received_index,
    plan_compact_received_index_keep_slot, plan_compact_ref_pointer_index,
    plan_compact_trust_policy, plan_compact_trust_policy_keep_slot,
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
    let mut keep_slot: Option<ContainerSlot> = None;
    let push_target = |targets: &mut Vec<Target>, flag: &str, target: Target| {
        if targets.contains(&target) {
            return Err(CliError::Usage(format!("duplicate {flag} flag")));
        }
        targets.push(target);
        Ok(())
    };
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
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
            "--keep-slot" => {
                let value = flag_value(&mut iter, "compact --keep-slot")?;
                let slot = match value.as_str() {
                    "a" => ContainerSlot::A,
                    "b" => ContainerSlot::B,
                    other => {
                        return Err(CliError::Usage(format!(
                            "compact --keep-slot expects a or b, not {other}"
                        )));
                    }
                };
                keep_slot.set_once("--keep-slot", slot)?;
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

    // 0.51.0 step 1 Part C (K9/K10): `--keep-slot` is for the lost-log state, a question the ref
    // pointer index does not have (its rebuild re-derives the index from the ref log directly,
    // never choosing between slots) -- and it names which slot of exactly one container, so it
    // refuses `--all` and more than one explicit target the same way.
    if let Some(slot) = keep_slot {
        if targets.len() != 1 {
            return Err(CliError::Usage(
                "compact --keep-slot requires exactly one container -- --trust-policy or \
                 --received-index"
                    .to_string(),
            ));
        }
        let target = targets[0];
        if target == Target::PointerIndex {
            return Err(CliError::Usage(
                "compact --keep-slot does not apply to --pointer-index; run `prikk doctor \
                 --rebuild-pointer-index --plan-only` instead -- the ref log decides, not either \
                 slot"
                    .to_string(),
            ));
        }
        let report = run_one_keep_slot(&layout, target, slot, plan_only)
            .map_err(|err| err.to_string())?;
        print_keep_slot_report(&report, plan_only);
        return Ok(());
    }

    // RFC 164 round 2 Addendum 1, item 1: every target this run will touch is checked before any of
    // them is compacted -- otherwise a multi-target run (`--all`, or several explicit flags together)
    // could compact an earlier target durably before ever discovering a later one's own torn tail or
    // damage, the review's own finding. A `--plan-only` preview writes nothing and is unaffected by a
    // tail it will never write behind, so it skips this pass entirely, matching every other guarded
    // writer's "refuse only when the operation will append" rule.
    if !plan_only {
        for target in &targets {
            precheck_one(&layout, *target).map_err(|err| err.to_string())?;
        }
    }
    for target in targets {
        let report = run_one(&layout, target, plan_only).map_err(|err| err.to_string())?;
        print_report(&report, plan_only);
    }
    Ok(())
}

fn run_one_keep_slot(
    layout: &prikk_store::RepositoryLayout,
    target: Target,
    chosen: ContainerSlot,
    plan_only: bool,
) -> prikk_error::Result<KeepSlotReport> {
    match (target, plan_only) {
        (Target::PointerIndex, _) => unreachable!("refused by run_compact before this point"),
        (Target::ReceivedIndex, false) => compact_received_index_keep_slot(layout, chosen),
        (Target::ReceivedIndex, true) => plan_compact_received_index_keep_slot(layout, chosen),
        (Target::TrustPolicy, false) => compact_trust_policy_keep_slot(layout, chosen),
        (Target::TrustPolicy, true) => plan_compact_trust_policy_keep_slot(layout, chosen),
    }
}

fn print_keep_slot_report(report: &KeepSlotReport, plan_only: bool) {
    let name = container_name(report.container);
    println!(
        "{name}: slot a, {} entr{} -- {}",
        report.slot_a_entry_count,
        if report.slot_a_entry_count == 1 {
            "y"
        } else {
            "ies"
        },
        if report.slot_a_summary.is_empty() {
            "(empty)".to_string()
        } else {
            report.slot_a_summary.join(", ")
        }
    );
    println!(
        "{name}: slot b, {} entr{} -- {}",
        report.slot_b_entry_count,
        if report.slot_b_entry_count == 1 {
            "y"
        } else {
            "ies"
        },
        if report.slot_b_summary.is_empty() {
            "(empty)".to_string()
        } else {
            report.slot_b_summary.join(", ")
        }
    );
    if !report.only_in_a.is_empty() || !report.only_in_b.is_empty() {
        println!(
            "{name}: only in slot a: {}",
            if report.only_in_a.is_empty() {
                "(none)".to_string()
            } else {
                report.only_in_a.join(", ")
            }
        );
        println!(
            "{name}: only in slot b: {}",
            if report.only_in_b.is_empty() {
                "(none)".to_string()
            } else {
                report.only_in_b.join(", ")
            }
        );
    }
    match report.deduced_slot {
        Some(slot) => println!(
            "{name}: prikk deduced slot {}",
            slot.as_str()
        ),
        None => println!("{name}: ambiguous: prikk will not choose"),
    }
    if let Some(deduced) = report.deduced_slot {
        if deduced != report.chosen_slot {
            println!(
                "{name}: prikk deduced {}; you chose {}",
                deduced.as_str(),
                report.chosen_slot.as_str()
            );
        }
    }
    let verb = if plan_only {
        "would keep"
    } else {
        "kept"
    };
    println!(
        "{name}: {verb} slot {} live, {} record(s); the other slot and the generation log were \
         saved first",
        report.chosen_slot.as_str(),
        report.entries_after
    );
    if !report.wrote {
        println!("{name}: --plan-only, nothing written");
    }
}

fn container_name(container: prikk_store::LockableContainer) -> &'static str {
    match container {
        prikk_store::LockableContainer::RefPointerIndex => "pointer-index",
        prikk_store::LockableContainer::ReceivedIndex => "received-index",
        prikk_store::LockableContainer::TrustPolicy => "trust-policy",
        prikk_store::LockableContainer::RefLog => "ref-log",
        prikk_store::LockableContainer::ObjectStore => "object-store",
    }
}

fn precheck_one(layout: &prikk_store::RepositoryLayout, target: Target) -> prikk_error::Result<()> {
    match target {
        Target::PointerIndex => prikk_store::precheck_ref_pointer_index_before_compaction(layout),
        Target::ReceivedIndex => prikk_store::precheck_received_index_before_compaction(layout),
        Target::TrustPolicy => prikk_store::precheck_trust_policy_before_compaction(layout),
    }
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
    let name = container_name(report.container);
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
