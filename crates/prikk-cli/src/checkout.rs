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
    let ref_name = current_branch::resolve_ref(&layout, args.ref_name)?;
    // 0.53.0 step 1, item 3 (RFC 147 §2i): an unpublished local branch **with queued, unsealed
    // commits** is not absent. Every mode but `--plan-only` (whose own guarded arm below already
    // answers this state correctly) still refuses here -- there is no sealed block to plan from or
    // write -- but must say what is true, not the generic "does not exist"
    // `require_existing_ref`/`resolve_point` give below (right only for a name that never existed,
    // *or* has never had anything queued against it either -- see the zero-queued note just below).
    //
    // **The classifier is `current_branch::is_unpublished_current_branch`, the exact one
    // `--plan-only`'s own guarded arm already calls, not the raw store-level
    // `is_unpublished_local_branch` it wraps.** Confirmed from source, not assumed: the raw
    // predicate only checks "no `RefState`, empty log" -- true both for a genuinely unpublished
    // branch *and* for a name nobody ever created at all (a typo), since `branch create` always
    // writes a `RefState` immediately (confirmed in `branch.rs::run_create`), so the only name that
    // can ever be real *and* have empty state is the repository's own current branch, pre-seal. The
    // wrapper's own first check -- "is `name` the current-branch pointer" -- is exactly the guard
    // that tells the two cases apart; calling the raw predicate directly (an earlier draft of this
    // change did) made `checkout --patch-plan --ref heads/nope` (a pure typo, never created) read as
    // "unpublished" instead of "does not exist" -- caught by actually running it, not assumed
    // correct from the raw predicate's own doc comment alone.
    //
    // **A second real gap, found by running the existing suite, not assumed safe**: a destination
    // that received `remotes/heads/main` (via `bundle import`/`archive import`/`sync accept`) but
    // never locally created `heads/main` at all is *also* "no `RefState`, empty log" for the local
    // name, and also happens to have the current-branch pointer naming it (every repository's own
    // default) -- so this new check, unguarded, pre-empted `require_existing_ref`'s own existing,
    // more specific refusal for that exact shape (which correctly names the received-ref
    // alternative: *"... remotes/heads/main exists: received refs are read by ..."*). That refusal
    // is strictly more useful than this one's generic "no sealed block yet", so it must still win:
    // skip this new check when a `remotes/<ref_name>` twin exists, the same condition
    // `ref_resolution.rs::decide_ref_existence` already checks for the identical reason.
    //
    // **A third real gap, also found by running the existing suite**: a *truly* fresh repository
    // (`init` alone, zero commits ever) is unpublished too, but `rfc132_absent_and_received_refs.rs`'s
    // own `addendum3_implicit_plan_only_keeps_its_answer_in_a_fresh_repository` -- a prior, already-
    // ruled ADDENDUM 3 control, RFC 147 §2i, not this handoff's own item to revisit -- pins exactly
    // this shape still reading "does not exist" for the six refusing modes; only `--plan-only` gets
    // the non-refusing treatment there. The handoff's own ruling and its own measured table are both
    // specifically about a branch that has queued commits ("setup, one commit, no seal"; "an
    // unpublished branch... has queued commits and nothing sealed") -- not the zero-queued shape.
    // So this new wording only fires once the queued count is genuinely nonzero; zero queued falls
    // through to the existing, unchanged "does not exist" path below, preserving Addendum 3's own
    // ruling rather than silently overriding it.
    if args.mode != CheckoutMode::PlanOnly
        && current_branch::is_unpublished_current_branch(&layout, &ref_name)?
        && prikk_store::read_received_pointer(&layout, &format!("remotes/{ref_name}"))
            .map_err(|err| err.to_string())?
            .is_none()
    {
        let queued = queued_commit_count(&layout, &ref_name)?;
        if queued > 0 {
            return Err(unpublished_branch_refusal(&ref_name, queued).into());
        }
    }
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
        // Addendum 3, as amended by RFC 147 §2i: `--plan-only` on the current branch keeps answering
        // `<not published>` in a fresh repository -- a legitimate state, as for `log` and
        // `worktree-status` (rule 4) -- whether the branch is named with `--ref` or left implicit.
        CheckoutMode::PlanOnly
            if current_branch::is_unpublished_current_branch(&layout, &ref_name)? =>
        {
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

/// How many queued, unsealed commits `ref_name` currently holds. Only the repository's one
/// currently-active ref can ever hold any (the single-active-session model) -- a name that is not
/// the active ref reports 0, regardless of whether it has ever been touched at all.
fn queued_commit_count(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> std::result::Result<usize, CliError> {
    match prikk_store::read_active_ref_metadata(layout).map_err(|err| err.to_string())? {
        prikk_store::ActiveRefMetadata::Valid(active) if active == ref_name => Ok(
            prikk_store::enumerate_queued_patches(layout).map_err(|err| err.to_string())?
                .len(),
        ),
        _ => Ok(0),
    }
}

/// The one shared refusal text for a checkout mode that cannot proceed because `ref_name` has
/// `queued` commits and nothing sealed yet (0.53.0 step 1, item 3; RFC 147 §2i). `queued` is always
/// nonzero here -- the one caller only reaches this once [`queued_commit_count`] confirms that.
fn unpublished_branch_refusal(ref_name: &str, queued: usize) -> String {
    let plural = if queued == 1 { "" } else { "s" };
    format!(
        "{ref_name} has no sealed block yet ({queued} commit{plural} queued) — `prikk seal` \
         publishes it"
    )
}
