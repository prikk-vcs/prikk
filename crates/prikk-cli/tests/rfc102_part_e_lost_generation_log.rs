//! 0.50.0 step 1, Part E/E2 (019 §5.7): a compacting container's generation log, emptied or removed
//! *after* a compaction has genuinely switched the live slot, used to resolve silently back to slot
//! A -- the architect's own reproduction, confirmed here end to end against the real binary.
//!
//! **Before Part E:** `branch list` printed `heads/main` only, exit 0, no warning -- `heads/topic`
//! (written only into the now-live slot B) read as if it never existed.
//!
//! **Part E2 (the corrected ruling):** content decides it, rather than a refusal every reader and
//! writer must be routed around. Slot B here holds an entry (`heads/topic`) slot A never had, so the
//! deduction resolves to B on its own -- `branch list` keeps working, silently, showing every branch,
//! with no rebuild required. `--rebuild-pointer-index` still runs and still reaches the same state,
//! proven below -- it stays a way out, just no longer the only one.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

#[test]
fn branch_list_keeps_working_through_a_lost_generation_log_by_deducing_the_live_slot() {
    let repo = support::unique_repo("part-e2-pointer-index-lost-generation-log");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    // Compact: slot B now holds heads/main's own entry, and the generation log names it live.
    let compact_output = support::prikk(&repo)
        .args(["compact", "--pointer-index"])
        .output()
        .unwrap();
    assert!(compact_output.status.success(), "{compact_output:?}");

    // A new branch, written only into the now-live slot B -- slot A (the pre-compaction content)
    // never learns about it.
    let branch_output = support::branch_create(&repo, "heads/topic", "heads/main");
    assert!(branch_output.status.success(), "{branch_output:?}");

    let generation_log = repo.join(".prikk/refs/containers/pointer-index-generation.log");
    let slot_b = repo.join(".prikk/refs/containers/pointer-index-b.container");
    assert!(
        std::fs::metadata(&slot_b).unwrap().len() > 0,
        "fixture: slot B must hold real data for this reproduction to mean anything"
    );

    // Lose the record of the switch -- the architect's own reproduction.
    std::fs::write(&generation_log, b"").unwrap();

    // Part E2: slot B holds an entry (heads/topic) slot A never had, so the deduction resolves to B
    // on its own -- no refusal, every branch still reads.
    let mut list_cmd = support::prikk(&repo);
    let list_output = list_cmd.args(["branch", "list"]).output().unwrap();
    assert!(
        list_output.status.success(),
        "the deduction must resolve this silently: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_text = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_text.contains("heads/main"), "{list_text}");
    assert!(list_text.contains("heads/topic"), "{list_text}");

    // `--rebuild-pointer-index` stays a way out, even though nothing required it here: it reaches
    // the identical state through its own, independent path (re-deriving from the ref log).
    let mut rebuild_cmd = support::prikk(&repo);
    let rebuild_output = rebuild_cmd
        .args(["doctor", "--rebuild-pointer-index"])
        .output()
        .unwrap();
    assert!(rebuild_output.status.success(), "{rebuild_output:?}");

    let mut list_again_cmd = support::prikk(&repo);
    let list_again_output = list_again_cmd.args(["branch", "list"]).output().unwrap();
    assert!(list_again_output.status.success(), "{list_again_output:?}");
    let list_again_text = String::from_utf8_lossy(&list_again_output.stdout);
    assert!(list_again_text.contains("heads/main"), "{list_again_text}");
    assert!(list_again_text.contains("heads/topic"), "{list_again_text}");

    let _ = std::fs::remove_dir_all(&repo);
}
