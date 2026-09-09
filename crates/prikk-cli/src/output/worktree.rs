// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;
use prikk_store::{
    RefHistory, RenameDeclaration, RepositoryLayout, WorktreeChangeKind, WorktreeStatusReport,
};

use super::verification::escape_json_string;

/// Print a worktree status report.
pub(crate) fn print_worktree_status(layout: &RepositoryLayout, report: &WorktreeStatusReport) {
    println!(
        "worktree-status repository: {}",
        layout.prikk_dir().display()
    );
    println!("ref: {}", report.ref_name);
    println!("tracked files: {}", report.tracked_files);
    println!("unchanged files: {}", report.unchanged_files);
    println!(
        "missing files: {}",
        report.count_kind(WorktreeChangeKind::Missing)
    );
    println!(
        "modified files: {}",
        report.count_kind(WorktreeChangeKind::Modified)
    );
    println!(
        "untracked files: {}",
        report.count_kind(WorktreeChangeKind::Untracked)
    );
    println!(
        "unsupported paths: {}",
        report.count_kind(WorktreeChangeKind::UnsupportedPath)
    );
    if report.is_clean() {
        println!("worktree: clean against baseline");
    } else {
        println!("worktree: changed against baseline");
        for change in &report.changes {
            println!(
                "  {} {} — {}",
                change.kind.as_str(),
                change.path,
                change.detail
            );
        }
    }
    // RFC 144 §4o.3: a declaration made and forgotten becomes permanent history at the next
    // commit, so it must be visible in the command whose job is what this commit will do.
    if report.declarations.is_empty() {
        println!("live rename declarations: 0");
    } else {
        println!("live rename declarations: {}", report.declarations.len());
        for declaration in &report.declarations {
            println!("  {} -> {}", declaration.old_path, declaration.new_path);
        }
        println!(
            "note: each declaration above is authored into the next `prikk commit` as a \
             RenamePath -- run `prikk mv` again to change it, or move the destination back to the \
             source to clear it"
        );
    }
    if let Some(other_ref) = &report.queued_elsewhere {
        println!(
            "note: the active WAL has queued (unsealed) patches for {other_ref}, not {} -- that \
             is real, committed work, not shown above; any \"untracked\" file here may be exactly \
             that work seen from this ref's own baseline, so do not delete based on this report \
             alone (see `prikk status`)",
            report.ref_name
        );
    }
    println!(
        "note: use `prikk commit -m <message>` to author node-addressed worktree changes; \
         text nodes use deterministic arbitrary-span EditText"
    );
}

/// `prikk worktree-status --format json` (RFC 144 §4o.3): `worktree-status-report-v1`, carrying
/// everything the prose form above does, plus the same live rename declarations as a
/// machine-branchable field rather than prose lines -- the reason this format exists at all.
/// Independent of `print_worktree_status`'s prose body above by design, the same way
/// `status.rs::print_status_json` is independent of `run_status`'s own prose (RFC 140).
pub(crate) fn print_worktree_status_json(layout: &RepositoryLayout, report: &WorktreeStatusReport) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"worktree-status-report-v1\",\n");
    json.push_str(&format!(
        "  \"repository\": {},\n",
        escape_json_string(&layout.prikk_dir().display().to_string())
    ));
    json.push_str(&format!(
        "  \"ref\": {},\n",
        escape_json_string(&report.ref_name)
    ));
    json.push_str(&format!("  \"tracked_files\": {},\n", report.tracked_files));
    json.push_str(&format!(
        "  \"unchanged_files\": {},\n",
        report.unchanged_files
    ));
    json.push_str(&format!("  \"clean\": {},\n", report.is_clean()));
    match &report.queued_elsewhere {
        Some(other_ref) => json.push_str(&format!(
            "  \"queued_elsewhere\": {},\n",
            escape_json_string(other_ref)
        )),
        None => json.push_str("  \"queued_elsewhere\": null,\n"),
    }
    json.push_str("  \"changes\": [");
    for (index, change) in report.changes.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"path\": ");
        json.push_str(&escape_json_string(&change.path));
        json.push_str(", \"kind\": ");
        json.push_str(&escape_json_string(change.kind.as_str()));
        json.push_str(", \"detail\": ");
        json.push_str(&escape_json_string(&change.detail));
        json.push('}');
    }
    if !report.changes.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("],\n");
    json.push_str("  \"declarations\": [");
    for (index, declaration) in report.declarations.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    ");
        push_declaration(&mut json, declaration);
    }
    if !report.declarations.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n");
    json.push('}');
    println!("{json}");
}

fn push_declaration(json: &mut String, declaration: &RenameDeclaration) {
    json.push_str("{\"old_path\": ");
    json.push_str(&escape_json_string(&declaration.old_path));
    json.push_str(", \"new_path\": ");
    json.push_str(&escape_json_string(&declaration.new_path));
    json.push('}');
}

/// Print ref history.
pub(crate) fn print_history(layout: &RepositoryLayout, history: &RefHistory) {
    println!("history repository: {}", layout.prikk_dir().display());
    println!("ref: {}", history.ref_name);
    if history.is_empty() {
        println!("history: <empty>");
        return;
    }
    for entry in &history.entries {
        println!("block {}", entry.block_id);
        println!("  ref-state: {}", entry.ref_state_id);
        println!("  update-seq: {}", entry.update_seq);
        println!("  kind: {:?}", entry.block_kind);
        println!("  rollback-block: {}", entry.is_rollback_block);
        println!("  parents: {}", entry.parent_count);
        println!("  patches: {}", entry.patch_count);
        println!("  rollback-patches: {}", entry.rollback_patch_count);
        println!(
            "  required-attestations: {}",
            entry.required_attestation_count
        );
        // RFC 123 §8: a patch with no message (schema 1/2/3, or a genuinely absent RFC 113
        // import) contributes no line here at all -- absence is the truth, not a placeholder.
        for patch_message in &entry.patch_messages {
            println!(
                "  patch {}: {}",
                patch_message.patch_id, patch_message.message
            );
        }
        match entry.previous_ref_state_id {
            Some(previous) => println!("  previous-ref-state: {previous}"),
            None => println!("  previous-ref-state: <none>"),
        }
    }
}
