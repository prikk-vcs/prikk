//! 0.50.0 step 1, Part E (019 §5.7): a compacting container's generation log, emptied or removed
//! *after* a compaction has genuinely switched the live slot, used to resolve silently back to slot
//! A -- the architect's own reproduction, confirmed here end to end against the real binary.
//!
//! **Before this round:** `branch list` printed `heads/main` only, exit 0, no warning -- `heads/
//! topic` (written only into the now-live slot B) read as if it never existed. `verify`/`doctor`
//! caught a *symptom* of this (a ref-publication divergence), but the ordinary read path did not.
//!
//! **After:** every reader and writer of the ref pointer index refuses in this exact state, naming
//! the container and its own way out (`prikk doctor --rebuild-pointer-index`); running that verb
//! resolves it, and `branch list` then shows every branch again.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

#[test]
fn branch_list_refuses_then_the_rebuild_restores_every_branch() {
    let repo = support::unique_repo("part-e-pointer-index-lost-generation-log");
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

    // Before this round's own fix, this printed `heads/main` only, exit 0. Now it refuses.
    let mut list_cmd = support::prikk(&repo);
    let list_output = list_cmd.args(["branch", "list"]).output().unwrap();
    assert!(
        !list_output.status.success(),
        "a lost generation log must refuse, not silently serve the stale slot: {}",
        String::from_utf8_lossy(&list_output.stdout)
    );
    let list_stderr = String::from_utf8_lossy(&list_output.stderr);
    assert!(
        list_stderr.contains("the ref pointer index"),
        "{list_stderr}"
    );
    assert!(
        list_stderr.contains("--rebuild-pointer-index"),
        "{list_stderr}"
    );

    // The named way out: the rebuild does not read either slot as live.
    let mut rebuild_cmd = support::prikk(&repo);
    let rebuild_output = rebuild_cmd
        .args(["doctor", "--rebuild-pointer-index"])
        .output()
        .unwrap();
    assert!(rebuild_output.status.success(), "{rebuild_output:?}");

    // Every branch is back, through the ordinary read path.
    let mut list_again_cmd = support::prikk(&repo);
    let list_again_output = list_again_cmd.args(["branch", "list"]).output().unwrap();
    assert!(list_again_output.status.success(), "{list_again_output:?}");
    let list_again_text = String::from_utf8_lossy(&list_again_output.stdout);
    assert!(list_again_text.contains("heads/main"), "{list_again_text}");
    assert!(list_again_text.contains("heads/topic"), "{list_again_text}");

    let _ = std::fs::remove_dir_all(&repo);
}
