//! `prikk mv <old> <new>` -- declared move and rename authoring (RFC 144 §4o.1).
//!
//! Both moves and declares, never one or the other, and keys the four worktree states on existence
//! only -- content is never compared to decide anything here (§4g's declaration-only ruling).

use std::path::PathBuf;

use prikk_store::{
    DeclarationRecordOutcome, RepoPath, RepositoryLayout, record_rename_declaration,
};

use crate::args::current_dir;
use crate::commands::CliError;
use crate::open_repository;
// RFC 121 §2.1: shadows the prelude's `println!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;

pub(crate) fn run_mv(args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut positionals = Vec::with_capacity(2);
    for arg in args {
        if arg.starts_with('-') {
            return Err(unknown_mv_argument(&arg));
        }
        positionals.push(arg);
    }
    let mut iter = positionals.into_iter();
    let (Some(old_arg), Some(new_arg)) = (iter.next(), iter.next()) else {
        return Err(CliError::Usage(
            "mv requires exactly two arguments: <old> <new>".to_string(),
        ));
    };
    if let Some(extra) = iter.next() {
        return Err(CliError::Usage(format!("unknown mv argument: {extra}")));
    }

    // DC-72: the same repository-relative path validation every other path-taking surface goes
    // through -- not hand-rolled. Constructed before any filesystem check below, so a malformed
    // argument is refused before the existence check ever runs.
    let old_repo_path = RepoPath::parse(&old_arg).map_err(|err| err.to_string())?;
    let new_repo_path = RepoPath::parse(&new_arg).map_err(|err| err.to_string())?;

    let root = current_dir()?;
    let layout = open_repository(root)?;
    layout
        .require_current_format()
        .map_err(|err| err.to_string())?;

    let old_disk_path = worktree_path(&layout, &old_repo_path);
    let new_disk_path = worktree_path(&layout, &new_repo_path);
    let old_exists = std::fs::symlink_metadata(&old_disk_path).is_ok();
    let new_exists = std::fs::symlink_metadata(&new_disk_path).is_ok();

    // §4o.1's four worktree states, keyed on existence only -- content is never inspected to decide
    // among them (§4g).
    let outcome = match (old_exists, new_exists) {
        (true, false) => {
            std::fs::rename(&old_disk_path, &new_disk_path)
                .map_err(|err| format!("failed to rename {old_arg} to {new_arg}: {err}"))?;
            let outcome =
                record_rename_declaration(&layout, old_repo_path.as_str(), new_repo_path.as_str())
                    .map_err(|err| err.to_string())?;
            println!("moved {old_arg} -> {new_arg}");
            outcome
        }
        (false, true) => {
            // The realistic flow this row exists for: a shell `mv` already happened, and the user
            // is only now telling prikk about it. Touch no bytes -- record the declaration alone.
            let outcome =
                record_rename_declaration(&layout, old_repo_path.as_str(), new_repo_path.as_str())
                    .map_err(|err| err.to_string())?;
            println!("declared {old_arg} -> {new_arg} (already moved on disk; no bytes touched)");
            outcome
        }
        (true, true) => {
            return Err(format!(
                "refusing to move {old_arg} to {new_arg}: both paths exist, and which one is the \
                 tracked node is not prikk's to guess"
            )
            .into());
        }
        (false, false) => {
            return Err(format!(
                "refusing to move {old_arg} to {new_arg}: neither path exists in the worktree"
            )
            .into());
        }
    };
    // RFC 144 §4p.2: this call's own chain-collapse may have resolved to something other than the
    // rename just asserted -- say so here, since the declaration store never carries a round trip
    // forward for `commit` to disclose later (it is dropped the moment it nets to no move, right
    // here, not at the next commit).
    match outcome {
        DeclarationRecordOutcome::Recorded { .. } => {
            println!(
                "note: this declaration is authored into the next `prikk commit`; see `prikk \
                 worktree-status` to review it first"
            );
        }
        DeclarationRecordOutcome::NetsToNoMove { origin, via } => {
            println!("declaration {origin} -> {via} -> {origin}: nets to no move, dropped");
        }
    }
    Ok(())
}

fn worktree_path(layout: &RepositoryLayout, path: &RepoPath) -> PathBuf {
    let mut out = layout.root().to_path_buf();
    for component in path.as_str().split('/') {
        out.push(component);
    }
    out
}

fn unknown_mv_argument(arg: &str) -> CliError {
    CliError::Usage(format!("unknown mv argument: {arg}"))
}
