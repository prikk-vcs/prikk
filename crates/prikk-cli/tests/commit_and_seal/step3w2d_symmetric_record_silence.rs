//! 0.56.0 step W2d-1, through the compiled binary: `require_concrete_record` is removed entirely
//! (`.git-exclude/reviewed/0.56.0-step-3w2c-review-v1.md`, "W2d-1") -- `node_authoring.rs::
//! path_is_stale`'s comparison is now unconditionally symmetric at every per-path authoring site
//! (the main loop, the rename loop, and the deletion loop, which was already symmetric).
//!
//! **The measured hazard this closes.** `feature` adds `c.txt` (`merged-content`); `merge --into
//! heads/main --from heads/feature` brings it onto `main`'s live tip; the worktree is never told
//! (today's known, intended-to-be-caught materialization gap -- `branch switch <the current
//! branch>` is the resync, exactly as W2's own cases prove for every other producer). The user, who
//! never saw the merge, creates their **own** `c.txt` by hand (different content) and commits.
//! Before this round, `require_concrete_record` exempted a path the worktree's own record was
//! silent about (no concrete entry for `c.txt` at all, since the record predates the merge) from
//! every "would author" site but the deletion loop -- so the user's hand-made file sailed through,
//! and after `seal`, `main`'s own `c.txt` silently read the user's content, not the merge's. The
//! node the merge introduced for `c.txt` already exists in the live baseline by the time this
//! commit runs, so what actually gets authored (before this round) is an `edit-text c.txt` on that
//! same node, not a fresh create -- the record's silence, not a node-identity collision, is the
//! whole hazard.
//!
//! **Why this is not `sequence_06`'s own shape, and `require_concrete_record` was never actually
//! needed.** `dc67_ordinary_use_conformance::sequence_06`'s own alternating-commit idiom (never
//! actually switching branches) was read, at W2c, as proof the asymmetry was needed -- removing it
//! made that test's own `main.txt` round-2 edit refuse. But that idiom was never safe by
//! construction; it merely never collided on a real path, by the test's own deliberate design.
//! Once `sequence_06` is rewritten to `branch switch` for real between rounds (this round, see that
//! test's own doc comment), the record is never silent about a path a *real* branch's own history
//! disagrees with, and the fully symmetric rule no longer refuses it -- there is no longer a case
//! left needing the exemption.
//!
//! **This scenario's active WAL is empty throughout** -- the merge moves only the *sealed* block
//! (`main`'s ref tip), never anything queued. The self-switch resync below therefore takes W2's
//! own original R2 route (the ordinary, always-verified-anchor `replay_for_verified_worktree_write`
//! path, unconditionally, since 0.56.0 step W2d-2 removed the one case that used to branch on a
//! non-empty queue) -- **not** the case 0.56.0 step W2d-2 narrows (a self-switch with a *queued*
//! fold that has moved without its sealed block moving, which refuses). Confirmed explicitly below
//! via `status`'s own "queued patches: 0" at every point in this test.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::Path;
use std::process::Output;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_state_id = RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
        .output()
        .unwrap()
}

fn merge_into_main(repo: &Path, baseline: ObjectId, from_ref: &str) -> Output {
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            from_ref,
        ])
        .output()
        .unwrap()
}

fn commit_from_worktree(repo: &Path, message: &str) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--from-worktree", "-m", message])
        .output()
        .unwrap()
}

fn queued_patch_count_is_zero(repo: &Path) {
    let status = support::prikk(repo).args(["status"]).output().unwrap();
    ok(&status, "status");
    let out = stdout(&status);
    assert!(
        out.contains("queued patches: 0") || !out.contains("queued"),
        "this scenario's active WAL must stay empty throughout -- the hazard and its resync are \
         both about the *sealed* block moving, never a queued fold (that is 0.56.0 step W2d-2's \
         own, separate case): {out}"
    );
}

/// The ruling's own measured case (W2d-1), end to end: refuses, resyncs via a real
/// `branch switch heads/main`, and the user's own edit -- now genuinely of the real merged content
/// -- commits cleanly.
#[test]
fn merge_adds_a_file_the_worktree_never_received_then_a_hand_made_same_path_file_refuses() {
    let repo = support::unique_repo("w2d-case1-merge-introduced-file-overwritten-unseen");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a1\n", "add a.txt");
    let baseline = tip_block_id(&repo, "heads/main");
    ok(
        &support::branch_create(&repo, "heads/feature", "heads/main"),
        "branch create feature",
    );

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    support::generation(
        &repo,
        "heads/feature",
        "c.txt",
        b"merged-content\n",
        "feature adds c.txt",
    );
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    assert!(
        !repo.join("c.txt").exists(),
        "heads/main's worktree has never held c.txt -- it only exists on feature so far"
    );
    queued_patch_count_is_zero(&repo);

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main --from heads/feature",
    );
    assert!(
        !repo.join("c.txt").exists(),
        "the merge must not touch the worktree -- c.txt stays absent on disk even though \
         heads/main's live tip now has it"
    );
    queued_patch_count_is_zero(&repo);

    // The hazard: the user never saw the merge bring c.txt in, so they create their own, with
    // different content, and commit the whole worktree.
    std::fs::write(repo.join("c.txt"), b"my-own\n").unwrap();
    let refused = commit_from_worktree(&repo, "add my own c.txt");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse rather than silently author edit-text c.txt over the merge's own \
         content: {}",
        stdout(&refused)
    );
    let refused_stderr = stderr(&refused);
    assert!(
        refused_stderr.contains("refusing to commit") && refused_stderr.contains("c.txt"),
        "the refusal must name c.txt: {refused_stderr}"
    );
    assert!(
        !refused_stderr.contains("never were"),
        "c.txt is present in the worktree, with stale (the user's own, unrelated) content -- not \
         the deletion-shaped wording: {refused_stderr}"
    );
    queued_patch_count_is_zero(&repo);

    // The refused commit never touched the worktree -- the user's own, never-committed c.txt is
    // still sitting there, untracked relative to main's pre-merge baseline (`switch`'s own refusal
    // 4 will not overwrite a file "not part of the branch being left" without this). A real user
    // discards their own guess before resyncing, the same way they would discard any untracked
    // file in the way of a switch.
    std::fs::remove_file(repo.join("c.txt")).unwrap();

    // The resync: an ordinary self-switch (W2's own original R2 route -- the active WAL is empty,
    // so this is not 0.56.0 step W2d-2's own, separate case). Materializes the merge's real c.txt.
    let resync = switch(&repo, "heads/main");
    ok(&resync, "branch switch heads/main (resync)");
    assert_eq!(
        std::fs::read(repo.join("c.txt")).unwrap(),
        b"merged-content\n",
        "the resync must surface the merge's own real content, not the user's discarded guess"
    );

    // The user's own edit, now genuinely an edit of the real merged content (not a blind
    // overwrite), commits cleanly.
    std::fs::write(repo.join("c.txt"), b"my-own\n").unwrap();
    let clean = commit_from_worktree(&repo, "edit c.txt after seeing the merge");
    ok(&clean, "commit after the resync");
    assert!(
        stdout(&clean).contains("edit-text c.txt"),
        "now a genuine edit of the real, resynced content: {}",
        stdout(&clean)
    );
    ok(&support::seal(&repo, "heads/main"), "seal heads/main");
    assert_eq!(
        std::fs::read(repo.join("c.txt")).unwrap(),
        b"my-own\n",
        "after seal, heads/main's c.txt is the user's own edit -- made with full visibility of \
         the merge's own content, not a silent overwrite of it"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
