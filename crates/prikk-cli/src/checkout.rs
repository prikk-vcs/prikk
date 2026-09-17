//! `prikk checkout`: plans and materializations of a point (RFC 153 §7.1 for the point).
//!
//! The read-only modes resolve their point once -- a ref or a bare block id -- and take it to the
//! store's point-taking readers. The modes that write the worktree take a branch or tag by name, and the
//! store refuses a bare block id there itself.

use prikk_store::{RepositoryLayout, materialize_snapshot_checkout, prepare_checkout_plan};

use crate::args::{CheckoutMode, parse_checkout_args};
use crate::commands::CliError;
use crate::output::{
    self, print_checkout_plan, print_patch_deletion_plan, print_patch_materialization_report,
    print_patch_plan_content_json, print_patch_replay_plan, print_snapshot_checkout_plan,
    print_snapshot_materialization_report,
};
use crate::{current_branch, open_repository, warn_anchor_fallbacks};

pub(crate) fn run_checkout(args: Vec<String>) -> std::result::Result<(), CliError> {
    let args = parse_checkout_args(args)?;
    let layout = open_repository(args.root)?;
    let explicit_ref = args.ref_name.is_some();
    let ref_name = current_branch::resolve_ref(&layout, args.ref_name)?;
    // RFC 132 refusal sweep, rules 1-3: every checkout mode refuses an absent or received target first,
    // before any question of what the target holds (the snapshot modes' "not a checkpoint").
    match args.mode {
        CheckoutMode::SnapshotMaterialize
        | CheckoutMode::PatchMaterialize
        | CheckoutMode::PatchMaterializeDelete => {
            prikk_store::require_existing_ref(
                &layout,
                &ref_name,
                prikk_store::ReceivedRefs::Refused,
            )
            .map_err(|err| err.to_string())?;
            return run_checkout_write(&layout, args.mode, &ref_name);
        }
        // Addendum 3: `--plan-only` on the implicit current branch keeps answering `<not published>` in a
        // fresh repository -- a legitimate state, as for `log` and `worktree-status` (rule 4).
        CheckoutMode::PlanOnly if !explicit_ref => {
            let plan = prepare_checkout_plan(&layout, &ref_name).map_err(|err| err.to_string())?;
            print_checkout_plan(&layout, &plan, output::point_label(false));
            return Ok(());
        }
        _ => {}
    }
    // RFC 153 §7.1: the read-only modes resolve their point once -- a ref, or a bare block id.
    let point = prikk_store::resolve_point(&layout, &ref_name, prikk_store::ReceivedRefs::Refused)
        .map_err(|err| err.to_string())?;
    let label = output::point_label(point.kind == prikk_store::PointKind::Block);
    match args.mode {
        CheckoutMode::PlanOnly => {
            let plan = prikk_store::prepare_checkout_plan_at_point(&layout, &point)
                .map_err(|err| err.to_string())?;
            print_checkout_plan(&layout, &plan, label);
        }
        CheckoutMode::SnapshotPlan => {
            let plan = prikk_store::prepare_snapshot_checkout_plan_at_point(&layout, &point)
                .map_err(|err| err.to_string())?;
            print_snapshot_checkout_plan(&layout, &plan, label);
        }
        // RFC 136 increment 2a: read-only reports may anchor at a snapshot. One that fails
        // validation is reported on stderr; stdout is unchanged (§10.3b.4).
        CheckoutMode::PatchPlan => {
            if args.format_json {
                let (report, fallback) =
                    prikk_store::prepare_patch_plan_content_report_at_point_reporting_anchor(
                        &layout,
                        &point,
                        &args.content_paths,
                    )
                    .map_err(|err| err.to_string())?;
                warn_anchor_fallbacks(fallback.iter());
                print_patch_plan_content_json(&report);
            } else {
                let (plan, fallback) =
                    prikk_store::prepare_patch_replay_plan_at_point_reporting_anchor(
                        &layout, &point,
                    )
                    .map_err(|err| err.to_string())?;
                warn_anchor_fallbacks(fallback.iter());
                print_patch_replay_plan(&layout, &plan, label);
            }
        }
        CheckoutMode::PatchDeletePlan => {
            let (plan, fallback) =
                prikk_store::plan_patch_checkout_deletions_at_point_reporting_anchor(
                    &layout, &point,
                )
                .map_err(|err| err.to_string())?;
            warn_anchor_fallbacks(fallback.iter());
            print_patch_deletion_plan(&layout, &plan, label);
            if !plan.is_safe_to_apply() {
                return Err("patch deletion plan has unsafe candidates"
                    .to_string()
                    .into());
            }
        }
        // Returned above.
        CheckoutMode::SnapshotMaterialize
        | CheckoutMode::PatchMaterialize
        | CheckoutMode::PatchMaterializeDelete => {}
    }
    Ok(())
}

/// The `checkout` modes that write the worktree. Each takes a branch or tag by name; a bare block id is
/// refused by the store function itself (RFC 153 point-resolver handoff §2.4).
fn run_checkout_write(
    layout: &RepositoryLayout,
    mode: CheckoutMode,
    ref_name: &str,
) -> std::result::Result<(), CliError> {
    match mode {
        CheckoutMode::SnapshotMaterialize => {
            let report =
                materialize_snapshot_checkout(layout, ref_name).map_err(|err| err.to_string())?;
            print_snapshot_materialization_report(layout, &report);
        }
        CheckoutMode::PatchMaterialize => {
            let (report, fallback) =
                prikk_store::materialize_patch_checkout_reporting_anchor(layout, ref_name)
                    .map_err(|err| err.to_string())?;
            warn_anchor_fallbacks(fallback.iter());
            print_patch_materialization_report(layout, &report);
        }
        CheckoutMode::PatchMaterializeDelete => {
            let (report, fallback) =
                prikk_store::materialize_patch_checkout_with_deletions_reporting_anchor(
                    layout, ref_name,
                )
                .map_err(|err| err.to_string())?;
            warn_anchor_fallbacks(fallback.iter());
            print_patch_materialization_report(layout, &report);
        }
        CheckoutMode::PlanOnly
        | CheckoutMode::SnapshotPlan
        | CheckoutMode::PatchPlan
        | CheckoutMode::PatchDeletePlan => {}
    }
    Ok(())
}
