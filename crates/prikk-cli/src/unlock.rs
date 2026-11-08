//! `prikk unlock` — stale-lock recovery (RFC 102 Stage 6 Step 2, design-v1.md §15.7 decision 3).
//!
//! Bare invocation only lists what is currently held — it never clears anything. Clearing one
//! specific lock (`--lock <path>`) always requires either an interactive `yes` confirmation or the
//! scripting escape `--yes`; there is no default that removes a lock. The tool never decides a lock
//! is safe to clear on its own -- see `prikk_store::unlock`'s own module doc for why (PID-based
//! auto-stealing was considered and rejected: a false positive there means two writers on one
//! container simultaneously, the exact race Step 2 exists to close).

use std::io::Write as _;
use std::path::PathBuf;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::stdout::{print, println};
use prikk_store::{HeldLock, clear_lock, find_held_lock, list_held_locks};

pub(crate) fn run_unlock(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut args = args.into_iter();
    let mut target: Option<String> = None;
    let mut skip_confirmation = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lock" => {
                let value = flag_value(&mut args, "--lock")?;
                target.set_once("--lock", value)?;
            }
            "--yes" | "--force" => mark_seen(&mut skip_confirmation, "--yes/--force")?,
            other => return Err(unknown_argument("unlock", other)),
        }
    }

    let layout = crate::open_repository(root)?;
    let locks = list_held_locks(&layout).map_err(|err| err.to_string())?;

    let Some(target) = target else {
        print_locks(&locks);
        return Ok(());
    };

    // `find_held_lock` resolves both sides before comparing (a path reached through a symlinked temp
    // dir, home, or mount must still match) -- but `print_locks` below already emits every path in its
    // resolved form, so an operator who copies straight from a listing never needed this in the first
    // place. This only ever bites a path constructed independently of that listing.
    let target_path = PathBuf::from(&target);
    let Some(lock) = find_held_lock(&locks, &target_path) else {
        // Name the resolution explicitly when it changes something, rather than silently comparing
        // resolved forms behind the scenes and leaving an operator to wonder why a path that "looks
        // right" still didn't match -- if it resolves to something real, say what, so a raw/resolved
        // mismatch is visible instead of hidden.
        let resolved_note = match std::fs::canonicalize(&target_path) {
            Ok(resolved) if resolved != target_path => {
                format!(
                    " (resolves to {}, which also has no held lock)",
                    resolved.display()
                )
            }
            _ => String::new(),
        };
        return Err(format!(
            "no held lock at {target}{resolved_note} -- run `prikk unlock` with no arguments to list \
             what is currently held"
        )
        .into());
    };

    print_locks(std::slice::from_ref(lock));
    // RFC 121 §6a: declining -- interactively, or via EOF on a non-interactive stdin, which
    // `confirm_interactively` cannot tell apart from a "no" -- must not exit `0`. A script running
    // `prikk unlock --lock X && proceed` with no input attached would otherwise be told the lock is
    // gone when it is not: uniform across both paths, not special-cased on whether a human was
    // watching.
    if !skip_confirmation && !confirm_interactively(lock) {
        return Err("lock not cleared: confirmation was not given"
            .to_string()
            .into());
    }

    clear_lock(&layout, &lock.path).map_err(|err| err.to_string())?;
    println!("cleared: {}", lock.path.display());
    Ok(())
}

fn print_locks(locks: &[HeldLock]) {
    if locks.is_empty() {
        println!("no locks currently held");
        return;
    }
    for lock in locks {
        println!("{}", lock.path.display());
        println!("  kind: {}", lock.kind);
        match lock.recorded_pid {
            Some(pid) => println!("  recorded pid: {pid}"),
            None => println!("  recorded pid: (not read: the lock body is incomplete)"),
        }
        if let Some(start) = &lock.recorded_start {
            println!("  recorded start: {start}");
        }
        println!("  liveness: {}", lock.liveness_words());
    }
    println!();
    println!(
        "liveness is advisory only: \"does not appear to be running\" and \"unknown\" are NOT proof \
         it is safe to clear. What each word means, and what each platform records to decide it, is \
         defined in one place: docs/src/reference/concurrency-locking.md, \"What the three liveness \
         words mean\". Clear a lock only if you have independently confirmed the process that \
         created it is gone."
    );
}

fn confirm_interactively(lock: &HeldLock) -> bool {
    print!(
        "Clearing this lock while its process is still running can corrupt this repository. \
         Type 'yes' to confirm clearing {}: ",
        lock.path.display()
    );
    if std::io::stdout().flush().is_err() {
        return false;
    }
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    input.trim() == "yes"
}
