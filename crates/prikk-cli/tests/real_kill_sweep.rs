// SPDX-License-Identifier: Apache-2.0
//! **The real-kill sweep (0.59.0 K3).** For each writer: run it under `ptrace`, kill it with `SIGKILL` at every mutating syscall it issues (each ordinal
//! in turn, on a fresh copy of the prepared repository), then recover as a user would, and require that the repository ends sound.
//!
//! **Linux x86_64 only** (`prikk_test_support::ptrace_sweep`); the Windows and macOS gap is stated in the durability page, not hidden by a skip.
//! **Three limits, said once here and in the table:** it kills a process at a syscall boundary; it is not power loss (the failpoint rows hold fsync
//! order); and it does not tear a write inside one syscall (the direct restore tests hold that). The kill at syscall *exit* is a mode of the driver and
//! is not swept (K1 measured that it reaches no state the entry sweep does not).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

#[path = "real_kill_sweep/harness.rs"]
mod harness;
#[path = "real_kill_sweep/inventory.rs"]
mod inventory;
#[path = "real_kill_sweep/shapes.rs"]
mod shapes;

use harness::{Ctx, Shape, run_shape};

fn sweep(tag: &str, build: impl FnOnce(&Ctx) -> Shape) {
    let ctx = Ctx::new(tag);
    let shape = build(&ctx);
    run_shape(&ctx, &shape);
    let _ = std::fs::remove_dir_all(&ctx.root);
}

#[test]
fn commit() {
    sweep("commit", shapes::commit);
}

#[test]
fn seal() {
    sweep("seal", shapes::seal);
}

#[test]
fn seal_checkpoint() {
    sweep("seal_checkpoint", shapes::seal_checkpoint);
}

#[test]
fn sync_seal_batch() {
    sweep("sync_seal_batch", shapes::sync_seal);
}

#[test]
fn uncommit() {
    sweep("uncommit", shapes::uncommit);
}

#[test]
fn repair_wal_tail() {
    sweep("repair_wal_tail", shapes::repair_wal_tail);
}

#[test]
fn repair_tails() {
    sweep("repair_tails", shapes::repair_tails);
}

#[test]
fn repair_pointer_index_tail() {
    sweep(
        "repair_pointer_index_tail",
        shapes::repair_pointer_index_tail,
    );
}

#[test]
fn repair_index() {
    sweep("repair_index", shapes::repair_index);
}

#[test]
fn tag_create() {
    sweep("tag_create", shapes::tag_create);
}

#[test]
fn branch_create() {
    sweep("branch_create", shapes::branch_create);
}

#[test]
fn remove_ref_candidates() {
    sweep("remove_ref_candidates", shapes::remove_ref_candidates);
}

#[test]
fn recovery_restore_cut() {
    sweep("recovery_restore_cut", shapes::restore_cut);
}

#[test]
fn recovery_restore_remove() {
    sweep("recovery_restore_remove", shapes::restore_remove);
}

#[test]
fn recovery_clear() {
    sweep("recovery_clear", shapes::recovery_clear);
}

#[test]
fn rebuild_pointer_index() {
    sweep("rebuild_pointer_index", shapes::rebuild_pointer_index);
}

#[test]
fn rebuild_caches() {
    sweep("rebuild_caches", shapes::rebuild_caches);
}

#[test]
fn ref_complete() {
    sweep("ref_complete", shapes::ref_complete);
}

#[test]
fn discard_damaged_commits() {
    sweep("discard_damaged_commits", shapes::discard_damaged_commits);
}

#[test]
fn restore_queue_target() {
    sweep("restore_queue_target", shapes::restore_queue_target);
}

#[test]
fn compact_pointer_index() {
    sweep("compact_pointer_index", shapes::compact_pointer_index);
}

#[test]
fn switch_refused_w2() {
    sweep("switch_refused_w2", shapes::switch_refused);
}

#[test]
fn cancel_import() {
    sweep("cancel_import", shapes::cancel_import);
}

#[test]
fn recovery_restore_of_uncommit() {
    sweep("recovery_restore_of_uncommit", shapes::restore_of_uncommit);
}

#[test]
fn recovery_restore_of_discard() {
    sweep("recovery_restore_of_discard", shapes::restore_of_discard);
}

#[test]
fn recovery_restore_of_rebuild_shortened_slot() {
    sweep(
        "recovery_restore_of_rebuild_shortened_slot",
        shapes::restore_of_rebuild_shortened_slot,
    );
}

#[test]
fn recovery_restore_of_restore_queue_target() {
    sweep(
        "recovery_restore_of_restore_queue_target",
        shapes::restore_of_restore_queue_target,
    );
}

#[test]
fn compact_received_index() {
    sweep("compact_received_index", shapes::compact_received_index);
}

#[test]
fn compact_trust_policy() {
    sweep("compact_trust_policy", shapes::compact_trust_policy);
}

/// The W3 producers' refusals: each of the four, for each shape of held path (a queued new file, an edit, a deletion). An adoption of `n.txt` leaves a
/// queued deletion of `p.txt` as it was (nothing to refuse), so that row is not a refusal and is not swept.
fn w3(tag: &str, build: fn(&Ctx, shapes::Held) -> Shape, skip_deletion: bool) {
    for held in shapes::HELD {
        if skip_deletion && held == shapes::Held::Deletion {
            continue;
        }
        let ctx = Ctx::new(&format!("{tag}-{held:?}"));
        let shape = build(&ctx, held);
        run_shape(&ctx, &shape);
        let _ = std::fs::remove_dir_all(&ctx.root);
    }
}

#[test]
fn w3_merge_refusal() {
    w3("w3-merge", shapes::w3_merge, false);
}

#[test]
fn w3_archive_adopt_refusal() {
    w3("w3-archive", shapes::w3_archive_adopt, true);
}

#[test]
fn w3_bundle_adopt_refusal() {
    w3("w3-bundle", shapes::w3_bundle_adopt, true);
}

#[test]
fn w3_branch_create_refusal() {
    w3("w3-branch", shapes::w3_branch_create, false);
}

/// **A control that can fail.** The sweep, run over a "writer" that is unsound between two of its own syscalls, ends red at exactly those ordinals (the
/// WAL is empty after the first `open(..., "wb")` and until the `write`), and sound at the others. If this test goes green with the harness's checks
/// weakened, the sweep cannot see damage.
#[test]
fn the_sweep_flags_a_writer_that_is_unsound_between_its_syscalls() {
    let ctx = Ctx::new("control");
    let shape = shapes::control_destroyer(&ctx);
    let outcome = harness::sweep(&ctx, &shape);
    eprintln!("{}", harness::summary(&shape, &outcome));
    assert_eq!(outcome.ordinals, 4, "{:?}", outcome.x);
    let unsound: Vec<&str> = outcome
        .x
        .iter()
        .map(|line| line.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(
        unsound,
        ["2", "3"],
        "the two ordinals between the emptying and the write: {:?}",
        outcome.x
    );
    assert_eq!(
        outcome.a + outcome.l + outcome.f + outcome.n,
        2,
        "the other two ordinals are sound"
    );
    let _ = std::fs::remove_dir_all(&ctx.root);
}

/// `init`'s sweep. It was red at ordinal 221 of 228 until 0.59.0 step K4 (K3's finding: a kill between creating `.prikk/FORMAT` and writing the format
/// number left an empty marker that every command refused and none cleared); the marker is now written last, through a temporary and a rename.
#[test]
fn init() {
    sweep("init", shapes::init);
}

/// `format upgrade` over a format-7 repository (the marker's one byte overwritten in place).
#[test]
fn format_upgrade() {
    sweep("format upgrade", shapes::format_upgrade);
}

/// `format upgrade` over a populated repository whose marker is lost (the marker written through a temporary and a rename).
#[test]
fn format_upgrade_lost_marker() {
    sweep(
        "format upgrade (lost marker)",
        shapes::format_upgrade_lost_marker,
    );
}

/// `setup` in an empty directory with no keys (a temporary `HOME`, test keys only). Red at 271 of 273 ordinals until 0.59.0 step K6 (K5's finding: a kill
/// left a `.prikk` that a rerun refused, naming a command that could not work, and a part-written key file in the user's key directory). Now a rerun
/// finishes, or refuses naming the files to remove (which the sweep removes) or the one `trust maintainer add` that adopts the keys.
#[test]
fn setup() {
    sweep("setup", shapes::setup);
}

/// `key generate --out` (a temporary `HOME`; the seed is a test key). Red at 5 of 14 ordinals until K6: the seed and its key-id file are now published whole
/// by a no-replace rename, so a kill leaves a file absent or whole, and the key-id file with no seed beside it names the file to remove.
#[test]
fn key_generate() {
    sweep("key generate --out", shapes::key_generate);
}

/// **The inventory against the CLI's own verb list.** Every verb `prikk --help` lists is in the table, every table verb is listed, every swept test
/// named in the table exists in this file, and every `NotSwept` reason says something. A new verb fails here until it is placed.
#[test]
fn every_verb_of_the_cli_is_swept_or_named_with_its_reason() {
    let ctx = Ctx::new("inventory");
    let help = harness::text(&ctx.out(&ctx.root, &["--help"]));
    let listed: std::collections::BTreeSet<&str> = help
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("  ")?;
            let (verb, description) = rest.split_once(' ')?;
            (!verb.is_empty()
                && !verb.starts_with(' ')
                && description.starts_with(' ')
                && !line.starts_with("   "))
            .then_some(verb)
        })
        .collect();
    let table: std::collections::BTreeSet<&str> =
        inventory::VERBS.iter().map(|(verb, _)| *verb).collect();
    assert!(listed.len() > 30, "the verb list was not read: {help}");
    assert_eq!(
        listed.difference(&table).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "a verb of `prikk --help` is not in the inventory (add it, swept or with its reason)"
    );
    assert_eq!(
        table.difference(&listed).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "the inventory names a verb `prikk --help` does not list"
    );
    let source = include_str!("real_kill_sweep.rs");
    for (verb, status) in inventory::VERBS {
        match status {
            inventory::Status::Swept(tests) | inventory::Status::Partly(tests, _) => {
                if let inventory::Status::Partly(_, rest) = status {
                    assert!(rest.len() > 40, "{verb}: the unswept part needs a reason");
                }
                assert!(!tests.is_empty(), "{verb}: no tests named");
                for test in *tests {
                    assert!(
                        source.contains(&format!("{test} =>"))
                            || source.contains(&format!("fn {test}(")),
                        "{verb}: the swept test `{test}` does not exist in real_kill_sweep.rs"
                    );
                }
            }
            inventory::Status::NotSwept(reason) => {
                assert!(reason.len() > 40, "{verb}: a reason is a sentence");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&ctx.root);
}

/// **The driver's two refusals, each with its reason in the message.** A traced process that creates a task ends the run (the ordinal is counted across
/// tasks in arrival order, deterministic only for a single-threaded writer), and one that issues a syscall the closed table does not classify ends it
/// naming the number and the name.
#[test]
fn the_driver_refuses_a_created_task_and_an_unclassified_syscall() {
    use prikk_test_support::ptrace_sweep::{TraceError, When, trace};
    let ctx = Ctx::new("driver-refusals");
    let sink = ctx.root.join("out.txt");

    let mut forks = std::process::Command::new("sh");
    forks
        .args(["-c", "/bin/true; /bin/true"])
        .current_dir(&ctx.root);
    match trace(&forks, &sink, None, When::Entry) {
        Err(TraceError::TaskCreated { event }) => {
            assert!([1, 2, 3].contains(&event), "clone, fork or vfork: {event}")
        }
        other => panic!("a command that forks must be refused: {other:?}"),
    }

    let mut unclassified = std::process::Command::new("python3");
    unclassified
        .args(["-I", "-c", "import os; os.sync()"])
        .current_dir(&ctx.root);
    match trace(&unclassified, &sink, None, When::Entry) {
        Err(error @ TraceError::Unclassified(_)) => {
            let message = error.to_string();
            assert!(
                message.contains("162 (sync"),
                "the number and the name: {message}"
            );
        }
        other => panic!("`sync` is not in the table and must stop the run: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&ctx.root);
}

/// **The reference (K3 item 8): the reviewer's kit's seven commands, counted by the Rust driver.** The kit (`sctrace.c`, a hand list of mutating
/// syscalls) counts 708 ordinals: 67, 105, 94, 187, 37, 91 and 127. The driver's closed table counts the same, **plus one**: `sync seal` issues one
/// `fchmod` (it materialises a file's mode), which the kit's list does not contain. Seven commands, 709 ordinals; the one extra is a chmod, and it is
/// why the table is closed. **Perturb:** take `fchmod` out of the mutating list and `sync seal` is 187 again.
#[test]
fn the_driver_counts_what_the_kit_counted_and_the_chmod_the_kit_missed() {
    let ctx = Ctx::new("kit-reference");
    let counts: Vec<(&str, u64, u64)> = vec![
        (
            "commit",
            harness::dry_count(&ctx, &shapes::commit(&ctx)),
            67,
        ),
        ("seal", harness::dry_count(&ctx, &shapes::seal(&ctx)), 105),
        (
            "checkpoint seal",
            harness::dry_count(&ctx, &shapes::seal_checkpoint(&ctx)),
            94,
        ),
        (
            "sync seal (the kit: 187; plus one fchmod)",
            harness::dry_count(&ctx, &shapes::sync_seal(&ctx)),
            188,
        ),
        (
            "uncommit",
            harness::dry_count(&ctx, &shapes::uncommit(&ctx)),
            37,
        ),
        (
            "--repair-wal-tail",
            harness::dry_count(&ctx, &shapes::repair_wal_tail(&ctx)),
            91,
        ),
        (
            "--repair-tails",
            harness::dry_count(&ctx, &shapes::repair_tails(&ctx)),
            127,
        ),
    ];
    for (name, got, want) in &counts {
        eprintln!("KIT {name}: {got}");
        assert_eq!(got, want, "{name}");
    }
    assert_eq!(counts.iter().map(|c| c.1).sum::<u64>(), 709);
    let _ = std::fs::remove_dir_all(&ctx.root);
}
