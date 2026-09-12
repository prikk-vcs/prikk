//! CLI output helpers.

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;
use prikk_store::{
    CheckoutMaterialization, CheckoutPlan, PatchDeletionPlan, PatchInversePlan,
    PatchMaterializationReport, PatchReplayPlan, RepositoryLayout, RollbackDraftReport,
    RollbackDraftVerification, RollbackPreviewPlan, SnapshotCheckoutPlan,
    SnapshotMaterializationReport,
};

mod branch;
mod bundle_preview;
mod help;
mod merge_evidence;
mod patch_plan;
mod show;
mod status;
mod tag;
mod trust;
pub(crate) mod verification;
mod worktree;

pub(crate) use branch::{BranchListEntry, ReceivedListEntry, print_branch_list_json};
pub(crate) use bundle_preview::{print_bundle_preview_json, print_bundle_preview_plain};
pub(crate) use help::{print_command_help, print_help};
pub(crate) use merge_evidence::{print_merge_evidence, print_merge_plan};
pub(crate) use patch_plan::print_patch_plan_content_json;
pub(crate) use show::{print_show, print_show_json};
pub(crate) use status::{QueueTarget, QueueThresholdStatus, print_status_json};
pub(crate) use tag::{TagListEntry, print_tag_list_json};
pub(crate) use trust::{
    print_trust_check, print_trust_check_json, print_trust_list, print_trust_list_json,
};
pub(crate) use verification::{
    print_active_session_repairs, print_doctor_report, print_verify_report,
    print_verify_report_json,
};
pub(crate) use worktree::{
    print_history, print_history_json, print_worktree_status, print_worktree_status_json,
};

/// Print a checkout plan.
pub(crate) fn print_checkout_plan(layout: &RepositoryLayout, plan: &CheckoutPlan) {
    println!("checkout plan repository: {}", layout.prikk_dir().display());
    println!("ref: {}", plan.ref_name);
    match plan.ref_state_id {
        Some(id) => println!("ref-state: {id}"),
        None => println!("ref-state: <not published>"),
    }
    match plan.block_id {
        Some(id) => println!("target block: {id}"),
        None => println!("target block: <none>"),
    }
    match plan.block_kind {
        Some(kind) => println!("block kind: {kind:?}"),
        None => println!("block kind: <none>"),
    }
    println!("parents: {}", plan.parent_count);
    println!("patches: {}", plan.patch_count);
    match plan.snapshot_blob_ref {
        Some(snapshot) => println!("snapshot blob: {snapshot}"),
        None => println!("snapshot blob: <none>"),
    }
    println!("materialization: {}", plan.materialization.as_str());
    match plan.materialization {
        CheckoutMaterialization::UnpublishedRef => {
            println!("note: publish a ref before checkout can target a block");
        }
        CheckoutMaterialization::NoWorktreeChanges => {
            println!("note: no worktree changes would be needed for this block");
        }
        CheckoutMaterialization::RequiresSnapshotMaterialization => {
            println!(
                "note: use `prikk checkout --snapshot-plan` to validate, or \
                 `--snapshot-materialize` to write safely"
            );
        }
        CheckoutMaterialization::RequiresPatchEngine => {
            println!("note: use `prikk checkout --patch-plan` for supported replay planning");
        }
    }
}

/// Print a snapshot checkout plan.
pub(crate) fn print_snapshot_checkout_plan(layout: &RepositoryLayout, plan: &SnapshotCheckoutPlan) {
    println!(
        "snapshot checkout plan repository: {}",
        layout.prikk_dir().display()
    );
    print_checkout_plan(layout, &plan.checkout);
    println!("snapshot blob: {}", plan.snapshot_blob_id);
    println!("snapshot files: {}", plan.file_count);
    println!("snapshot content bytes: {}", plan.total_content_bytes);
    for path in &plan.paths {
        println!("  file: {path}");
    }
    println!("note: use `prikk checkout --snapshot-materialize` to write validated snapshot files");
}

/// Print a supported patch replay plan.
pub(crate) fn print_patch_replay_plan(layout: &RepositoryLayout, plan: &PatchReplayPlan) {
    println!(
        "patch replay plan repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", plan.ref_name);
    println!("target block: {}", plan.target_block_id);
    println!("blocks replayed: {}", plan.block_count);
    println!("patches replayed: {}", plan.patch_count);
    println!("operations applied: {}", plan.applied_operation_count);
    println!("result files: {}", plan.file_count);
    println!("result content bytes: {}", plan.total_content_bytes);
    for path in &plan.paths {
        println!("  file: {path}");
    }
    println!(
        "note: this replays CreateFile/DeleteNode/EditText/ReplaceBinary/ChangePerm; renames, \
         conflicts, and full patch algebra remain later increments"
    );
}

/// Print a patch checkout deletion plan.
pub(crate) fn print_patch_deletion_plan(layout: &RepositoryLayout, plan: &PatchDeletionPlan) {
    println!(
        "patch deletion plan repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", plan.ref_name);
    println!("planned deletions: {}", plan.planned_deletions);
    println!("deletable files: {}", plan.deletable_files);
    println!("already absent files: {}", plan.already_absent_files);
    println!("deletion conflicts: {}", plan.conflicts.len());
    for path in &plan.deletable_paths {
        println!("  delete: {path}");
    }
    for conflict in &plan.conflicts {
        println!("  refused: {} — {}", conflict.path, conflict.reason);
    }
    println!("note: only explicit patch-deleted files are eligible; extra files are never deleted");
}

/// Print a patch replay materialization report.
pub(crate) fn print_patch_materialization_report(
    layout: &RepositoryLayout,
    report: &PatchMaterializationReport,
) {
    println!(
        "patch materialization repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", report.ref_name);
    println!("blocks replayed: {}", report.block_count);
    println!("patches replayed: {}", report.patch_count);
    println!("operations applied: {}", report.applied_operation_count);
    println!("planned files: {}", report.planned_files);
    println!("written files: {}", report.written_files);
    println!("unchanged files: {}", report.unchanged_files);
    println!("deleted files: {}", report.deleted_files);
    println!(
        "already absent deleted files: {}",
        report.already_absent_deleted_files
    );
    println!("deletion conflicts: {}", report.deletion_conflicts);
    println!("result content bytes: {}", report.total_content_bytes);
    for path in &report.paths {
        println!("  file: {path}");
    }
    println!(
        "note: this materializes the supported patch replay result; opt-in deletion removes only \
         explicit patch-deleted files whose current bytes still match the old blob"
    );
}

/// Print a snapshot materialization report.
pub(crate) fn print_snapshot_materialization_report(
    layout: &RepositoryLayout,
    report: &SnapshotMaterializationReport,
) {
    println!(
        "snapshot materialization repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", report.ref_name);
    println!("planned files: {}", report.planned_files);
    println!("written files: {}", report.written_files);
    println!("unchanged files: {}", report.unchanged_files);
    println!("snapshot content bytes: {}", report.total_content_bytes);
    for path in &report.paths {
        println!("  file: {path}");
    }
    println!("note: this path writes only snapshot-backed files and never applies patches");
}

/// Print a supported patch inverse plan.
pub(crate) fn print_patch_inverse_plan(layout: &RepositoryLayout, plan: &PatchInversePlan) {
    println!(
        "patch inverse plan repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", plan.ref_name);
    println!("target block: {}", plan.target_block_id);
    println!("blocks inspected: {}", plan.block_count);
    println!("patches inspected: {}", plan.patch_count);
    println!("original operations: {}", plan.original_operation_count);
    println!("inverse operations: {}", plan.inverse_operation_count);
    println!(
        "unsigned inverse patch id hint: {}",
        plan.inverse_patch_id_hint
    );
    for operation in &plan.operations {
        println!(
            "  {:04} {} {}",
            operation.op_seq,
            operation.kind.as_str(),
            operation.path
        );
    }
    println!(
        "note: this is a non-mutating unsigned inverse plan for the supported operation subset; \
         rollback refs, authorization, conflicts, and full patch algebra remain later PRs"
    );
}

/// Print a rollback preview plan.
pub(crate) fn print_rollback_preview_plan(layout: &RepositoryLayout, plan: &RollbackPreviewPlan) {
    println!(
        "rollback preview repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", plan.ref_name);
    println!("target block: {}", plan.target_block_id);
    println!("blocks validated: {}", plan.block_count);
    println!("patches validated: {}", plan.patch_count);
    println!("inverse operations: {}", plan.inverse_operation_count);
    println!(
        "unsigned inverse patch id hint: {}",
        plan.inverse_patch_id_hint
    );
    println!("current files: {}", plan.current_file_count);
    println!("current content bytes: {}", plan.current_content_bytes);
    println!("preview files: {}", plan.preview_file_count);
    println!("preview content bytes: {}", plan.preview_content_bytes);
    println!("changes: {}", plan.change_count);
    println!("would create: {}", plan.would_create_files);
    println!("would delete: {}", plan.would_delete_files);
    println!("would replace: {}", plan.would_replace_files);
    for change in &plan.changes {
        match (change.current_bytes, change.preview_bytes) {
            (Some(current), Some(preview)) => println!(
                "  {} {} current-bytes={} preview-bytes={}",
                change.kind.as_str(),
                change.path,
                current,
                preview
            ),
            (Some(current), None) => println!(
                "  {} {} current-bytes={} preview-bytes=<absent>",
                change.kind.as_str(),
                change.path,
                current
            ),
            (None, Some(preview)) => println!(
                "  {} {} current-bytes=<absent> preview-bytes={}",
                change.kind.as_str(),
                change.path,
                preview
            ),
            (None, None) => println!(
                "  {} {} current-bytes=<absent> preview-bytes=<absent>",
                change.kind.as_str(),
                change.path
            ),
        }
    }
    println!(
        "note: this is a non-mutating rollback preview to the latest snapshot baseline; \
         rollback refs, authorization, worktree writes, and full patch algebra remain later PRs"
    );
}

/// Print a rollback draft append report.
pub(crate) fn print_rollback_draft_report(layout: &RepositoryLayout, report: &RollbackDraftReport) {
    println!(
        "rollback draft repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", report.ref_name);
    println!("target block: {}", report.target_block_id);
    println!("inverse patch: {}", report.inverse_patch_id);
    println!("author key: {}", report.author_key_id);
    println!("WAL sequence: {}", report.wal_sequence);
    println!("blocks inspected: {}", report.block_count);
    println!("patches inspected: {}", report.patch_count);
    println!("inverse operations: {}", report.inverse_operation_count);
    println!("preview changes: {}", report.preview_change_count);
    println!("would create: {}", report.would_create_files);
    println!("would delete: {}", report.would_delete_files);
    println!("would replace: {}", report.would_replace_files);
    for operation in &report.operations {
        println!(
            "  {:04} {} {}",
            operation.op_seq,
            operation.kind.as_str(),
            operation.path
        );
    }
    println!(
        "note: this appended a signed inverse Patch draft to the active WAL only; run seal later \
         to publish it, and rollback refs, audit policy, and worktree writes remain later PRs"
    );
}

/// Print a rollback draft verification report.
pub(crate) fn print_rollback_draft_verification(
    layout: &RepositoryLayout,
    report: &RollbackDraftVerification,
) {
    println!(
        "rollback draft verification repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", report.ref_name);
    println!("target block: {}", report.target_block_id);
    println!("draft patch: {}", report.draft_patch_id);
    println!("author key: {}", report.author_key_id);
    println!("WAL sequence: {}", report.wal_sequence);
    println!("blocks inspected: {}", report.block_count);
    println!("patches inspected: {}", report.patch_count);
    println!("inverse operations: {}", report.inverse_operation_count);
    println!("decoded operations: {}", report.decoded_operation_count);
    println!(
        "note: this validates the active rollback draft against the current inverse plan only; \
         seal, rollback refs, authorization, audit policy, and worktree writes remain separate"
    );
}
