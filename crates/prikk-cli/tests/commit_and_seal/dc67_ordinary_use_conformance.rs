//! DC-67 ordinary-use conformance suite.
//!
//! Three consecutive increments (DC-65, DC-64's incremental step, DC-66's queue fold) each had their
//! decisive defect found by running an ordinary **sequence** — never by inspection, never by an
//! existing gate. All three were the same shape: a path that only misbehaves the *second or later*
//! time it runs against a given thing. This project's assurance is aimed hard at adversarial and
//! structural failure (DC-41) and covers that axis well; this suite is the orthogonal one — ordinary,
//! repeated, sequential use, run through the compiled binary exactly as a user would.
//!
//! **N = 3.** The RFC's own floor (`N >= 3`; two is the boundary DC-65's original defect was missed
//! at, so it proves nothing here). A larger N was considered and rejected: this file drives the
//! compiled binary through nine sequences, each spawning several child processes per generation
//! (`commit`, `seal`, and `trust maintainer add`); N = 3 keeps total suite runtime bounded while still
//! showing the "second-or-later" pattern repeats rather than being a fluke of exactly one repetition.
//!
//! **Sequence 1** ("edit the same text file across N generations") is satisfied by the pre-existing
//! `dc65_text_edit_baseline.rs` — the RFC's own §3 says "keep it," not duplicate it. This file
//! implements sequences 2 through 9 (numbered to match the RFC's §3 list); §3's own item 10 is not a
//! tenth sequence but the ending requirement every sequence here follows: delete the worktree and
//! rebuild it from sealed history, asserting byte-exact content.
//!
//! **Result (criteria 4, 5): the prediction holds — two findings, neither fixed here.**
//!
//! 1. **`checkout --patch-materialize` does not support replaying `ReplaceBinary` or `ChangePerm`**
//!    (sequences 2 and 5 hit this independently: `error: ... patch replay plan does not yet support
//!    ReplaceBinary` / `... ChangePerm`). Pre-existing and partially known —
//!    `patch_replay/decode.rs` already names exactly `CreateFile`, file-`DeleteNode`, and `EditText`
//!    as wired, and DC-65's own store-level binary test worked around the `ReplaceBinary` half by
//!    checking `update_seq` instead of rebuilt content — but this suite is the first place that gap
//!    is shown to block criterion-2-style verification for two of the RFC's ten *named ordinary*
//!    sequences, not an exotic or adversarial one. Not a "second-or-later" defect (it fails
//!    identically at generation 1), so it doesn't confirm the RFC's specific prediction by itself, but
//!    it is a real, reportable gap this suite surfaced. Both sequences verify via `verify` and the
//!    still-committed worktree instead, documented inline at each site.
//! 2. **Historical, now closed by 0.56.0's `branch switch <name>` (RFC 151 §2.3).** At the time this
//!    finding was first written, prikk had no "checkout branch into the worktree" step for active
//!    editing — only `--patch-materialize`, read-only, into a separate directory — so an ordinary
//!    two-branch workflow (commit on one branch, switch, commit on the other) run from one physical
//!    worktree directory was not directly supported: a file created for branch A would be picked up
//!    as a new, untracked create relative to branch B's own baseline too, unless the user manually
//!    removed it first. `sequence_06` worked around this inline. 0.56.0 added a real `branch switch`
//!    command, and step W2d (the measured hazard a merge-then-overwrite case exposed in the
//!    comparison this suite's own `sequence_06` idiom had been read as justifying — see that test's
//!    own doc comment) rewrote `sequence_06` to use it for real, rather than the manual-removal
//!    idiom. This finding is kept here, marked closed, rather than deleted, since it is exactly the
//!    kind of capability gap this suite exists to surface, and its resolution is itself evidence the
//!    suite worked.
//!
//! Per the RFC's own framing (§2: "either outcome is a good one; only not looking is bad"), both are
//! reported here as their own findings, not fixed in this increment (criterion 4) — the fixes, if any
//! are warranted, belong to whichever future increment owns `checkout`'s replay coverage and the
//! multi-branch worktree workflow respectively.
//!
//! **What remains uncovered (criterion 7):** multi-way branch topologies (more than two branches
//! diverging and reconverging via merge — DC-13's merge windows are out of scope for this repository's
//! current increments); rollback-draft interleaved with ordinary commits; symlink authoring (out of
//! scope for the product generally, per `worktree_patch.rs`'s own doc comment); non-Linux platforms
//! (DC-37 restricts all repository mutation to Linux; this suite, like the rest of the test tree, only
//! ever runs there); concurrent/multi-process ordinary use (two `commit` invocations racing, not one
//! process queuing serially — DC-66's chain fold is proven single-process only); cache deletion
//! interleaved *within* a single generation rather than between them (sequence 9 deletes between
//! commits, not mid-`commit`); and content-level rebuild verification for `ReplaceBinary`/`ChangePerm`
//! sequences, blocked by finding 1 above until `checkout`'s replay coverage is extended. This list is
//! deliberately not empty.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

// DC-71: used only by sequence_05's Unix permission-bit assertions, below.
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::support::{
    branch_close, branch_create, commit, generation, init, ok, prikk, rebuild_from_sealed_history,
    seal, tag_create, unique_repo, verify,
};

const N: u32 = 3;

/// `branch switch <target>`, asserting success — the same idiom `dc74_merge_execution.rs` and
/// `dc75_merge_two_edits.rs` already use for the identical purpose (a real switch, not a hand
/// `rm` standing in for one). 0.56.0 step W2d: `sequence_06` below is rewritten to use this.
fn switch(repo: &std::path::Path, target: &str) {
    ok(
        &prikk(repo)
            .args(["branch", "switch", target])
            .output()
            .unwrap(),
        &format!("branch switch {target}"),
    );
}

/// §3.2: edit the same **binary** file across N generations — the `ReplaceBinary` counterpart to
/// DC-65's text-file coverage. Content is deliberately invalid UTF-8 so `classify_new` selects
/// `BlobKind::Binary`/`NodeKind::BinaryFile`, not the text path.
///
/// **Cannot use the criterion-2 rebuild path — reported as its own finding, not fixed here.**
/// `checkout --patch-materialize` (`prepare_patch_replay_plan`) does not support replaying
/// `ReplaceBinary` at all: `error: unsupported object type: patch replay plan does not yet support
/// ReplaceBinary (node-addressed apply pending node model, increment 4.4)`. This is pre-existing and
/// already documented — `patch_replay/decode.rs` names exactly `CreateFile`, file-`DeleteNode`, and
/// `EditText` as wired, and DC-65's own store-level `binary_file_replaced_across_four_sealed_commits_succeeds`
/// worked around the identical gap by checking `update_seq` instead of rebuilt content. It is not a
/// *new* second-or-later defect (it fails identically at generation 1), but it does mean an entire §3
/// sequence the RFC names cannot be verified the way criterion 2 asks — a real gap in checkout's
/// operation-kind coverage that ordinary binary-file use runs straight into. Verified instead via
/// `verify` (structural) and the still-committed worktree content (round-trip through `commit`/`seal`,
/// not through independent replay).
#[test]
fn sequence_02_binary_file_edited_across_generations() {
    let repo = unique_repo("seq02-binary-edits");
    init(&repo);
    for round in 1..=N {
        let content = vec![0xFF, 0xFE, 0x00, round as u8];
        generation(
            &repo,
            "heads/main",
            "data.bin",
            &content,
            &format!("gen {round}"),
        );
    }
    ok(&verify(&repo), "verify after N binary-file generations");
    let expected = vec![0xFF, 0xFE, 0x00, N as u8];
    assert_eq!(std::fs::read(repo.join("data.bin")).unwrap(), expected);
    let _ = std::fs::remove_dir_all(&repo);
}

/// §3.3: create, delete, recreate the **same path** repeatedly. Final state is absent (the last
/// operation in the last cycle is a delete) — the rebuild proves the deletion, not merely a creation,
/// survived sealing correctly.
#[test]
fn sequence_03_create_delete_recreate_same_path_repeatedly() {
    let repo = unique_repo("seq03-create-delete-recreate");
    init(&repo);
    for cycle in 1..=N {
        let content = format!("incarnation {cycle}");
        std::fs::write(repo.join("churn.txt"), &content).unwrap();
        ok(
            &commit(&repo, "heads/main", &format!("create incarnation {cycle}")),
            "create",
        );
        ok(&seal(&repo, "heads/main"), "seal create");

        std::fs::remove_file(repo.join("churn.txt")).unwrap();
        ok(
            &commit(&repo, "heads/main", &format!("delete incarnation {cycle}")),
            "delete",
        );
        ok(&seal(&repo, "heads/main"), "seal delete");
    }
    let materialize_root = rebuild_from_sealed_history(&repo, "seq03");
    assert!(
        !materialize_root.join("churn.txt").exists(),
        "the final operation in the last cycle was a delete"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
}

/// §3.4: create a file, edit it, delete it, then create a **different** file at the same path — the
/// node-identity question DC-66 raised for queuing, here at the ordinary sealed-history level: the
/// second file at that path must be a genuinely new node, not a resurrection of the deleted one.
/// Node-id distinctness is internal; what is externally observable is that authoring the recreation
/// succeeds at all (a resurrection attempt would collide) and that the rebuilt content is the second
/// file's, not the first's.
#[test]
fn sequence_04_create_edit_delete_then_recreate_different_file_same_path() {
    let repo = unique_repo("seq04-recreate-different-file-same-path");
    init(&repo);
    generation(
        &repo,
        "heads/main",
        "slot.txt",
        b"first incarnation v1",
        "create slot",
    );
    generation(
        &repo,
        "heads/main",
        "slot.txt",
        b"first incarnation v2",
        "edit slot",
    );

    std::fs::remove_file(repo.join("slot.txt")).unwrap();
    ok(&commit(&repo, "heads/main", "delete slot"), "delete slot");
    ok(&seal(&repo, "heads/main"), "seal delete slot");

    generation(
        &repo,
        "heads/main",
        "slot.txt",
        b"second incarnation",
        "recreate slot",
    );

    let materialize_root = rebuild_from_sealed_history(&repo, "seq04");
    assert_eq!(
        std::fs::read(materialize_root.join("slot.txt")).unwrap(),
        b"second incarnation"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
}

/// §3.5: change a file's mode across generations, both independent of and combined with content
/// changes.
// DC-71: Windows has no Unix permission-bit model.
#[cfg(unix)]
#[test]
fn sequence_05_mode_changes_with_and_without_content_changes() {
    let repo = unique_repo("seq05-mode-changes");
    init(&repo);
    let path = repo.join("script.sh");
    std::fs::write(&path, "v1").unwrap();
    ok(&commit(&repo, "heads/main", "create"), "create");
    ok(&seal(&repo, "heads/main"), "seal create");

    // Generation 2: mode change only, no content change.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o100_755)).unwrap();
    ok(
        &commit(&repo, "heads/main", "mode only"),
        "mode-only commit",
    );
    ok(&seal(&repo, "heads/main"), "seal mode-only");

    // Generation 3: mode change AND content change together, in the same commit.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o100_644)).unwrap();
    std::fs::write(&path, "v2").unwrap();
    ok(
        &commit(&repo, "heads/main", "mode and content"),
        "mode+content commit",
    );
    ok(&seal(&repo, "heads/main"), "seal mode+content");

    // Cannot use the criterion-2 rebuild path here either — the same pre-existing gap sequence 2
    // documents also covers `ChangePerm`: `error: ... patch replay plan does not yet support
    // ChangePerm`. Verified instead via `verify` and the still-committed worktree.
    ok(&verify(&repo), "verify after mode-change generations");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o111,
        0,
        "final mode must be non-executable, got {mode:o}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// §3.6: branch, commit on both branches, close one, `verify`, keep committing on the other.
///
/// **0.56.0 step W2d: rewritten to `branch switch` for real between rounds, the same fix `dc74`'s
/// divergent-edit tests already got this cycle.** The original version alternated `commit`s
/// between `heads/main` and `heads/topic` from one physical worktree, manually `rm`-ing the other
/// ref's own file before each round's commit rather than ever actually switching — the only way to
/// drive a two-branch workflow before 0.56.0 added `branch switch`. The architect's own W2d review
/// found that idiom was never actually safe by construction: it happened, by this test's own
/// deliberate design, to never collide on a real path, which is why it was wrongly read as a
/// counter-example to a fully symmetric staleness rule (see `node_authoring.rs`'s `path_is_stale`
/// doc for the measured hazard that correction closes). Real switches materialize each branch's
/// own tip directly — `topic.txt` disappears on its own when switching to `heads/main` (topic's
/// tip never had it on main, so it is simply not in what a switch writes), and vice versa — so the
/// manual `std::fs::remove_file` calls are gone, not merely relocated.
#[test]
fn sequence_06_branch_commit_both_close_one_keep_committing_other() {
    let repo = unique_repo("seq06-branch-close-continue");
    init(&repo);
    generation(&repo, "heads/main", "root.txt", b"root v1", "root gen1");
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "create topic",
    );

    for round in 1..=N {
        switch(&repo, "heads/main");
        generation(
            &repo,
            "heads/main",
            "main.txt",
            format!("main {round}").as_bytes(),
            &format!("main gen{round}"),
        );
        switch(&repo, "heads/topic");
        generation(
            &repo,
            "heads/topic",
            "topic.txt",
            format!("topic {round}").as_bytes(),
            &format!("topic gen{round}"),
        );
    }

    // Switch away from topic *before* closing it -- the user's own path. Closing the branch the
    // worktree is currently standing on would leave the next switch's own `from` unresolvable
    // (`current_branch` deliberately refuses to name a closed branch as current -- see
    // `refs.rs::current_branch`'s own doc), which is a real, deliberate, pre-existing refusal, not
    // a defect to route around: a switch with an unresolvable `from` has no old baseline to delete
    // against (this file's own module doc, "an unresolvable pointer... passes from = None"). An
    // ordinary user closes a branch after leaving it, not while standing on it, so this is the
    // realistic order, not a workaround.
    switch(&repo, "heads/main");
    ok(&branch_close(&repo, "heads/topic"), "close topic");
    ok(&verify(&repo), "verify after closing topic");
    for round in (N + 1)..=(N + 2) {
        generation(
            &repo,
            "heads/main",
            "main.txt",
            format!("main {round}").as_bytes(),
            &format!("main gen{round}"),
        );
    }

    // checkout defaults to heads/main.
    let materialize_root = rebuild_from_sealed_history(&repo, "seq06");
    assert_eq!(
        std::fs::read_to_string(materialize_root.join("root.txt")).unwrap(),
        "root v1"
    );
    assert_eq!(
        std::fs::read_to_string(materialize_root.join("main.txt")).unwrap(),
        format!("main {}", N + 2)
    );
    assert!(
        !materialize_root.join("topic.txt").exists(),
        "topic.txt was only ever committed on the closed heads/topic branch"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
}

/// §3.7: tag, keep committing past the tag, tag again.
#[test]
fn sequence_07_tag_then_continue_committing_then_tag_again() {
    let repo = unique_repo("seq07-tag-then-continue");
    init(&repo);
    for round in 1..=N {
        generation(
            &repo,
            "heads/main",
            "doc.txt",
            format!("v{round}").as_bytes(),
            &format!("gen {round}"),
        );
    }
    ok(
        &tag_create(&repo, "tags/v1", "heads/main"),
        "create tags/v1",
    );

    for round in (N + 1)..=(N + 2) {
        generation(
            &repo,
            "heads/main",
            "doc.txt",
            format!("v{round}").as_bytes(),
            &format!("gen {round}"),
        );
    }
    ok(
        &tag_create(&repo, "tags/v2", "heads/main"),
        "create tags/v2",
    );
    ok(&verify(&repo), "verify after tagging twice");

    let materialize_root = rebuild_from_sealed_history(&repo, "seq07");
    assert_eq!(
        std::fs::read_to_string(materialize_root.join("doc.txt")).unwrap(),
        format!("v{}", N + 2)
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
}

/// §3.8 (DC-66): queue N commits, seal as one block; queue N more, seal again.
#[test]
fn sequence_08_queue_n_commits_seal_then_queue_n_more_seal_again() {
    let repo = unique_repo("seq08-queue-then-queue-again");
    init(&repo);
    generation(&repo, "heads/main", "seed.txt", b"seed", "genesis");

    for round in 1..=N {
        std::fs::write(
            repo.join(format!("batch1-{round}.txt")),
            format!("batch1 file {round}"),
        )
        .unwrap();
        ok(
            &commit(&repo, "heads/main", &format!("queue batch1 {round}")),
            "queue batch1",
        );
    }
    ok(&seal(&repo, "heads/main"), "seal batch1");

    for round in 1..=N {
        std::fs::write(
            repo.join(format!("batch2-{round}.txt")),
            format!("batch2 file {round}"),
        )
        .unwrap();
        ok(
            &commit(&repo, "heads/main", &format!("queue batch2 {round}")),
            "queue batch2",
        );
    }
    ok(&seal(&repo, "heads/main"), "seal batch2");
    ok(&verify(&repo), "verify after two sealed batches");

    let materialize_root = rebuild_from_sealed_history(&repo, "seq08");
    for round in 1..=N {
        assert_eq!(
            std::fs::read_to_string(materialize_root.join(format!("batch1-{round}.txt"))).unwrap(),
            format!("batch1 file {round}")
        );
        assert_eq!(
            std::fs::read_to_string(materialize_root.join(format!("batch2-{round}.txt"))).unwrap(),
            format!("batch2 file {round}")
        );
    }

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
}

/// §3.9 (NFR-PERF-04): delete the rebuildable, non-authoritative caches mid-sequence and continue.
#[test]
fn sequence_09_delete_caches_mid_sequence_and_continue() {
    let repo = unique_repo("seq09-delete-caches-mid-sequence");
    init(&repo);
    for round in 1..=N {
        generation(
            &repo,
            "heads/main",
            "doc.txt",
            format!("v{round}").as_bytes(),
            &format!("gen {round}"),
        );
    }

    let commit_index_path = repo.join(".prikk/cache/commit-index.v1");
    let lifecycle_cache_path = repo.join(".prikk/cache/lifecycle-state.v1");
    assert!(
        commit_index_path.exists(),
        "commit-index cache must exist after {N} sealed generations"
    );
    assert!(
        lifecycle_cache_path.exists(),
        "lifecycle-state cache must exist after {N} sealed generations (populated starting at the \
         second commit against a published baseline)"
    );
    std::fs::remove_file(&commit_index_path).unwrap();
    std::fs::remove_file(&lifecycle_cache_path).unwrap();

    for round in (N + 1)..=(N + 2) {
        generation(
            &repo,
            "heads/main",
            "doc.txt",
            format!("v{round}").as_bytes(),
            &format!("gen {round}"),
        );
    }

    ok(&verify(&repo), "verify after mid-sequence cache deletion");
    let materialize_root = rebuild_from_sealed_history(&repo, "seq09");
    assert_eq!(
        std::fs::read_to_string(materialize_root.join("doc.txt")).unwrap(),
        format!("v{}", N + 2)
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
}
