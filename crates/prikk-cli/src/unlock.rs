//! `prikk unlock` — stale-lock recovery (RFC 102 Stage 6 Step 2, design-v1.md §15.7 decision 3).
//!
//! Bare invocation only lists what is currently held — it never clears anything. Clearing one
//! specific lock (`--lock <path>`) requires either an interactive `yes` confirmation or the scripting
//! escape `--yes`; there is no default that removes a lock. **Since 0.57.0 (step 2, L2) a holder that
//! `appears running` is refused unless `--force`, and since 0.58.0 (step 0, part 0a) so is a holder that is
//! `unknown`** (another pid or time namespace, another machine, an older prikk, an incomplete body: external
//! review 050 §2 found a live, frozen holder cleared by `--yes` in each of the first three): `--yes` and a typed
//! `yes` answer the prompt only for a lock whose holder "does not appear to be running", and `--force` alone
//! clears the other two, at the risk of corrupting what that process is writing. The tool never
//! decides a lock is safe to clear on its own -- see `prikk_store::unlock`'s own module doc for why
//! (PID-based auto-stealing was considered and rejected: a false positive there means two writers on
//! one container simultaneously, the exact race Step 2 exists to close).

use std::io::Write as _;
use std::path::PathBuf;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::stdout::{print, println};
use prikk_store::{HeldLock, PidLiveness, clear_lock, find_held_lock, list_held_locks};

pub(crate) fn run_unlock(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut args = args.into_iter();
    let mut target: Option<String> = None;
    let mut yes = false;
    let mut force = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lock" => {
                let value = flag_value(&mut args, "--lock")?;
                target.set_once("--lock", value)?;
            }
            "--yes" => mark_seen(&mut yes, "--yes")?,
            "--force" => mark_seen(&mut force, "--force")?,
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
    // L2 (0.57.0) and 0.58.0 step 0a: only a holder that "does not appear to be running" may be cleared
    // on a `--yes` or a typed `yes`. A holder that appears running, and one whose liveness is unknown,
    // are refused whatever either says; only `--force` clears them. The refusal is an operational failure
    // (exit 1), before anything is asked or written, so a script chaining
    // `prikk unlock --lock X --yes && proceed` never proceeds past a lock whose holder it cannot show
    // to be gone.
    if lock.liveness != PidLiveness::DoesNotAppearRunning && !force {
        return Err(refuse_holder_not_shown_gone(lock).into());
    }
    // RFC 121 §6a: declining -- interactively, or via EOF on a non-interactive stdin, which
    // `confirm_interactively` cannot tell apart from a "no" -- must not exit `0`. A script running
    // `prikk unlock --lock X && proceed` with no input attached would otherwise be told the lock is
    // gone when it is not: uniform across both paths, not special-cased on whether a human was
    // watching. `--force` is an explicit "clear it whatever the holder says" and asks nothing.
    if !(yes || force) && !confirm_interactively(lock) {
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
         defined in one place ({LIVENESS_WORDS_URL}). \
         Clear a lock only if you have independently confirmed the process that created it is gone."
    );
}

/// The URL of the one place the three liveness words are defined: published, so a release installed
/// from a package or an archive can open it (the source-tree path would not exist there).
const LIVENESS_WORDS_URL: &str =
    "https://prikk.org/docs/reference/concurrency-locking.html#what-the-three-liveness-words-mean";

/// The one sentence about what `--force` risks, shared by both refusals so the risk is never worded two ways.
const FORCE_RISK: &str =
    "--force clears it anyway, at the risk of corrupting what that process is writing.";

/// The refusal for a holder that is not shown to be gone: one that appears running (the pid and the start time), or one whose
/// liveness is unknown (the reason the judge gave). Both name `--force` and what it risks through [`FORCE_RISK`], and point at
/// the one definition of the words. No backticks and no source-tree path: an installed release has neither a tree to open nor
/// a markdown renderer.
fn refuse_holder_not_shown_gone(lock: &HeldLock) -> String {
    let (what, state) = if lock.liveness == PidLiveness::AppearsRunning {
        let pid = lock
            .recorded_pid
            .map_or_else(|| "?".to_string(), |pid| pid.to_string());
        let started = lock
            .recorded_start
            .as_deref()
            .map_or_else(String::new, |start| format!(", start time {start}"));
        (
            format!(
                "the process that took {} appears running (pid {pid}{started})",
                lock.path.display()
            ),
            "holder appears running",
        )
    } else {
        (
            format!(
                "whether the process that took {} is still running cannot be established ({})",
                lock.path.display(),
                lock.liveness_reason.sentence()
            ),
            "holder's liveness is unknown",
        )
    };
    format!(
        "precondition not met: {what} -- refusing to clear its lock. --yes does not clear a lock whose {state}. \
         {FORCE_RISK} What these words mean: {LIVENESS_WORDS_URL}"
    )
}

fn confirm_interactively(lock: &HeldLock) -> bool {
    print!(
        "Clearing a lock whose process is still running can corrupt this repository; this \
         holder is {}. Type 'yes' to confirm clearing {}: ",
        lock.liveness_words(),
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
