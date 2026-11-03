//! 0.56.0 step W2b: the worktree-baseline guard generalized from deletions alone to every stale
//! path (`.git-exclude/reviewed/0.56.0-step-3w2-review-v1.md`, "W2b. The guard must cover every
//! stale path, not only a missing one").
//!
//! W2 (`step3w2_worktree_baseline.rs`) only caught a path **absent** from the worktree whose
//! absence this worktree's own record never explains. This round generalizes the same mechanism
//! (`node_authoring.rs::path_is_stale`) to every per-path authoring site -- a path is **stale**
//! when its state in the live baseline (what `canonical_ref` currently, really resolves to)
//! disagrees with its state in this worktree's own recorded one, in content, mode, kind, presence,
//! or the node it belongs to -- and refuses whenever a stale path would otherwise have something
//! authored for it (the worktree does not already hold the live state), rather than silently
//! authoring an edit, a mode change, a create, or a rename that would revert a producer's change
//! or record a conflict as an ordinary one.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::Path;
use std::process::Output;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, ObjectReader, RefStore, RepositoryLayout,
    order_claims_for_sealing, seal_from_accepted_claim,
};

/// 0.56.0 step W3c-4: seal every claim named in `claims_file` onto `ref_name`, **in process**, by
/// calling `prikk_store::seal_from_accepted_claim` directly rather than shelling out to `prikk
/// sync seal --claims <file>` -- see `step3w2_worktree_baseline.rs`'s own, identical helper's doc
/// comment for the full grounding: this round's new clean/in-the-way/marker/materialize
/// orchestration lives entirely in `prikk-cli`'s own `run_seal`, never in this store function,
/// whose own contract (seal one claim's patches onto a ref; touch nothing else) stays unchanged,
/// so calling it directly is the one remaining way to reach the exact pre-round staleness this
/// fixture needs.
///
/// **0.56.0 step W3c-4b's correction (same doc, same source):** this stands in for a real,
/// surviving source of exactly this staleness -- an older `prikk` (0.55.0 or earlier) running
/// `sync seal` on the same repository moves the ref without ever materializing the worktree, and
/// so does `archive import --adopt`, until W3c-5 lands its own R3 for it.
fn seal_claims_in_process(repo: &Path, ref_name: &str, claims_file: &Path) {
    let content = std::fs::read_to_string(claims_file).unwrap();
    let claim_ids: Vec<ObjectId> = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.parse::<ObjectId>().unwrap())
        .collect();
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ordered = order_claims_for_sealing(&object_store, &claim_ids).unwrap();
    let signer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();
    for claim_id in ordered {
        seal_from_accepted_claim(&layout, ref_name, claim_id, &signer).unwrap();
    }
}

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

fn worktree_status(repo: &Path) -> Output {
    support::prikk(repo)
        .args(["worktree-status"])
        .output()
        .unwrap()
}

/// Write a brand-new, previously untracked file and commit the whole worktree -- the "innocent,
/// unrelated commit" shape every one of W2's own hazard cases (and this round's) uses to trigger
/// the guard: the new file itself is never stale (neither side names it), so a refusal that names
/// it instead of the real stale path would itself be a bug.
fn commit_unrelated(repo: &Path, path: &str, message: &str) -> Output {
    std::fs::write(repo.join(path), b"unrelated\n").unwrap();
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

/// Change `a.txt`'s content on `repo`'s own `heads/main` to `new_content`, without ever writing
/// `repo`'s worktree -- `heads/main` stays current throughout. `merge --into` itself is excluded
/// (0.56.0 step W3b closed that gap for it); `seal_claims_in_process` (0.56.0 step W3c-4 note,
/// above) reproduces the identical staleness instead, since it calls `seal_from_accepted_claim`
/// directly, bypassing the CLI-level `sync seal` orchestration this round adds. Mechanism: clone
/// `repo`'s exact `heads/main` genesis into a disposable `mirror` (`archive export` + `archive
/// import --adopt`), edit `a.txt` **inside `mirror` only** (so the resulting patch is genuinely
/// new to `repo`'s own object store -- building it in `repo` first, as `setup_main_and_sibling`'s
/// own sibling does, makes sealing read it as "already have this," a no-op), then sync the one
/// new block back by `have`/`build`/`accept --claims-out`/`seal_claims_in_process`, the identical
/// shape `step3w2_worktree_baseline.rs`'s own `make_main_stale_via_sync_seal` uses.
fn make_a_txt_stale_via_sync_seal(repo: &Path, new_content: &[u8]) {
    let archive_file = std::env::temp_dir().join(format!(
        "w2b-stale-archive-{}.bin",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(repo)
            .args(["archive", "export"])
            .arg(&archive_file)
            .output()
            .unwrap(),
        "archive export",
    );

    let mirror = support::unique_repo(&format!("w2b-stale-mirror-{}", support::unique_suffix()));
    support::init(&mirror);
    support::trust_maintainer(&mirror);
    ok(
        &support::prikk(&mirror)
            .args(["archive", "import", "--adopt"])
            .arg(&archive_file)
            .output()
            .unwrap(),
        "archive import --adopt (clone repo's exact genesis objects)",
    );
    ok(
        &switch(&mirror, "heads/main"),
        "branch switch heads/main in mirror (normalize the adopted ref)",
    );
    support::generation(
        &mirror,
        "heads/main",
        "a.txt",
        new_content,
        "a.txt change, in mirror only",
    );

    let have_file =
        std::env::temp_dir().join(format!("w2b-stale-have-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(repo)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact_file = std::env::temp_dir().join(format!(
        "w2b-stale-artifact-{}.bin",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&mirror)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                artifact_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );
    let claims_file =
        std::env::temp_dir().join(format!("w2b-stale-claims-{}.txt", support::unique_suffix()));
    ok(
        &support::prikk(repo)
            .args([
                "sync",
                "accept",
                artifact_file.to_str().unwrap(),
                "--claims-out",
                claims_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync accept",
    );
    support::trust_maintainer(repo);
    seal_claims_in_process(repo, "heads/main", &claims_file);

    let _ = std::fs::remove_dir_all(&mirror);
    let _ = std::fs::remove_file(&archive_file);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);
    let _ = std::fs::remove_file(&claims_file);
}

/// `heads/main` with two sealed files (`a.txt`, `b.txt`), a sealed sibling branch created from the
/// same tip, and a real `branch switch` round trip there and back -- not a bare `branch create`
/// plus a worktree write left lying around (see `step3w2_worktree_baseline.rs`'s own
/// `setup_main_and_sealed_sibling` doc for why that distinction matters: a real round trip is what
/// makes the worktree's record genuinely describe `heads/main`, so a later producer-side change can
/// be reproduced as a *staleness*, not merely as a file the worktree happens to already lack).
/// Returns `heads/main`'s own genesis block id (for `merge --baseline-block`).
fn setup_main_and_sibling(repo: &Path, sibling_name: &str) -> ObjectId {
    support::generation(repo, "heads/main", "a.txt", b"a1\n", "add a.txt");
    support::generation(repo, "heads/main", "b.txt", b"b1\n", "add b.txt");
    let baseline = tip_block_id(repo, "heads/main");
    ok(
        &support::branch_create(repo, sibling_name, "heads/main"),
        "branch create sibling",
    );
    baseline
}

/// Test 1, **closed by 0.56.0 step W3b** for `merge --into` specifically: a merge changing `a.txt`'s
/// **content** on `heads/main` now materializes that change into the worktree immediately, since
/// the worktree was clean before the merge -- there is no window left in which an unrelated commit
/// could silently revert it (the hazard this test used to reproduce for merges; it is still real for
/// the producers W3b has not yet changed).
#[test]
fn case1_merge_changes_content_is_materialized_so_nothing_is_left_to_revert() {
    let repo = support::unique_repo("w2b-case1-edit-reverted");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    support::generation(
        &repo,
        "heads/feature",
        "a.txt",
        b"a2-from-feature\n",
        "feature changes a.txt",
    );
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a1\n",
        "heads/main is unaffected by feature's own change so far"
    );

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a2-from-feature\n",
        "0.56.0 step W3b: the merge itself must materialize its own new content into the worktree"
    );

    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must already be clean right after the merge: {}",
        stdout(&status)
    );

    let clean = commit_unrelated(&repo, "extra1.txt", "unrelated edit after merge");
    ok(&clean, "commit after merge");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("edit-text a.txt"),
        "no stray revert of the merge's own change: {clean_stdout}"
    );
    assert!(
        clean_stdout.contains("create-file extra1.txt"),
        "the only authored operation is the real create: {clean_stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 2, **closed by 0.56.0 step W3b** for `merge --into`: a merge **removing** `a.txt` from
/// `heads/main`'s tip now removes it from the worktree too, immediately -- there is no window left
/// in which an unrelated commit could treat the still-present file as brand new and re-create it
/// under a fresh node id (the hazard this test used to reproduce; still real for the producers
/// W3b has not yet changed).
#[test]
fn case2_merge_deletes_path_is_materialized_so_nothing_is_left_present() {
    let repo = support::unique_repo("w2b-case2-merged-deletion");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "feature deletes a.txt"),
        "commit deletion on feature",
    );
    ok(&support::seal(&repo, "heads/feature"), "seal deletion");
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    assert!(
        repo.join("a.txt").exists(),
        "heads/main is unaffected by feature's own deletion so far"
    );

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    assert!(
        !repo.join("a.txt").exists(),
        "0.56.0 step W3b: the merge itself must delete a.txt from the worktree, matching its own \
         real deletion"
    );

    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must already be clean right after the merge: {}",
        stdout(&status)
    );

    let clean = commit_unrelated(&repo, "extra2.txt", "unrelated edit after merge");
    ok(&clean, "commit after merge");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("create-file a.txt"),
        "no stray re-creation of the merge's own deletion: {clean_stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 3: a merge changes `a.txt`'s **mode** (not its content) on `heads/main`; the worktree still
/// physically holds the old mode. Before this round, the next unrelated commit silently authored
/// `change-perm a.txt`, reverting the merge's own mode change back to the old mode. Now it refuses;
/// the resync materializes the real mode; the following commit is clean.
///
/// DC-71: Windows has no Unix permission-bit model.
/// Test 3, **closed by 0.56.0 step W3b** for `merge --into`: a merge changing `a.txt`'s **mode**
/// (not its content) on `heads/main` now materializes that mode into the worktree immediately --
/// there is no window left in which an unrelated commit could silently revert it (the hazard this
/// test used to reproduce; still real for the producers W3b has not yet changed).
///
/// DC-71: Windows has no Unix permission-bit model.
#[cfg(unix)]
#[test]
fn case3_merge_changes_mode_is_materialized_so_nothing_is_left_to_revert() {
    use std::os::unix::fs::PermissionsExt;

    let repo = support::unique_repo("w2b-case3-mode-reverted");
    support::init(&repo);
    let baseline = setup_main_and_sibling(&repo, "heads/feature");

    ok(&switch(&repo, "heads/feature"), "switch to feature");
    std::fs::set_permissions(
        repo.join("a.txt"),
        std::fs::Permissions::from_mode(0o100_755),
    )
    .unwrap();
    ok(
        &support::commit(&repo, "heads/feature", "feature makes a.txt executable"),
        "commit mode change on feature",
    );
    ok(&support::seal(&repo, "heads/feature"), "seal mode change");
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");
    let mode_on_main = std::fs::symlink_metadata(repo.join("a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode_on_main & 0o777,
        0o644,
        "heads/main is unaffected by feature's own mode change so far"
    );

    ok(
        &merge_into_main(&repo, baseline, "heads/feature"),
        "merge --into heads/main",
    );
    let mode_after_merge = std::fs::symlink_metadata(repo.join("a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode_after_merge & 0o777,
        0o755,
        "0.56.0 step W3b: the merge itself must materialize its own new mode into the worktree"
    );

    let status = worktree_status(&repo);
    assert_eq!(
        status.status.code(),
        Some(0),
        "worktree-status must already be clean right after the merge: {}",
        stdout(&status)
    );

    let clean = commit_unrelated(&repo, "extra3.txt", "unrelated edit after merge");
    ok(&clean, "commit after merge");
    let clean_stdout = stdout(&clean);
    assert!(
        !clean_stdout.contains("change-perm"),
        "no stray revert of the merge's own mode change: {clean_stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 4: the user edits the same stale file themselves -- the worktree holds **neither** the
/// record's old state **nor** the live state, but a third, user-authored one. Refused as a
/// conflict (naming `a.txt`), not silently authored as an edit against the wrong baseline.
#[test]
fn case4_user_edit_of_stale_path_refused_as_conflict() {
    let repo = support::unique_repo("w2b-case4-conflict");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a1\n", "add a.txt");
    support::generation(&repo, "heads/main", "b.txt", b"b1\n", "add b.txt");

    // `merge --into` itself is excluded (0.56.0 step W3b closed that gap for it):
    // `seal_claims_in_process` (0.56.0 step W3c-4 note, above) reproduces the identical
    // staleness instead, since it calls `seal_from_accepted_claim` directly, bypassing the
    // CLI-level `sync seal` orchestration this round adds.
    make_a_txt_stale_via_sync_seal(&repo, b"a2-from-feature\n");

    // The user, not knowing the worktree is already stale, edits a.txt themselves -- a third
    // state, matching neither the record's old bytes ("a1\n") nor the merge's real new ones
    // ("a2-from-feature\n").
    std::fs::write(repo.join("a.txt"), b"a3-user-edit\n").unwrap();

    let refused = commit_unrelated(&repo, "extra4.txt", "unrelated edit after merge");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse the conflict, not author it as an ordinary edit: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit") && stderr(&refused).contains("a.txt"),
        "the refusal must name a.txt: {}",
        stderr(&refused)
    );

    // `branch switch` (the ordinary resync) also refuses here, for the same reason `commit` just
    // did -- a real conflict (a third state matching neither side) is not something a resync may
    // silently discard. This is the expected, doubly-guarded shape: the worktree's own dirty check
    // (unchanged by W2/W2b) still applies on top of the staleness guard.
    let switch_refused = switch(&repo, "heads/main");
    assert_eq!(
        switch_refused.status.code(),
        Some(1),
        "branch switch must also refuse the conflict, not silently discard it: {}",
        stdout(&switch_refused)
    );

    // Discard the user's own conflicting edit by hand, restoring exactly what this worktree's own
    // record says it was last written from ("a1\n") -- the ordinary way out of a dirty-worktree
    // refusal, which `require_clean` here judges against the *recorded* replay, not the live one,
    // because the record is the one genuinely stale against `heads/main` (W2's own rule, §5 of its
    // report). Only then does the resync land on the real, current tip.
    std::fs::write(repo.join("a.txt"), b"a1\n").unwrap();
    let resync = switch(&repo, "heads/main");
    ok(
        &resync,
        "branch switch heads/main (resync, after discarding the conflict)",
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a2-from-feature\n",
        "the resync lands on the merge's own real content: {}",
        stdout(&resync)
    );

    std::fs::remove_file(repo.join("extra4.txt")).unwrap();
    let clean = commit_unrelated(&repo, "extra4.txt", "unrelated edit after resync");
    ok(&clean, "commit after resync");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 5: a declared local rename (`prikk mv`) whose **source** node is itself stale -- a merge
/// changed `a.txt`'s content on `heads/main` (the worktree still holds the old bytes, exactly as
/// test 1), and the user also renames `a.txt` to `renamed.txt` in this same commit, carrying the
/// stale bytes along under the new name. Before a per-path check on the rename's own source path,
/// this would have authored `rename-path a.txt -> renamed.txt` *and* an `edit-text` reverting the
/// merge's change -- a revert disguised as a rename, not merely exempted from the deletion loop by
/// `renamed_away.contains(old_path)`. Refused, naming `a.txt` (the rename's own source), not
/// `renamed.txt`.
#[test]
fn case5_rename_of_a_stale_node_refuses() {
    let repo = support::unique_repo("w2b-case5-stale-rename");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a1\n", "add a.txt");
    support::generation(&repo, "heads/main", "b.txt", b"b1\n", "add b.txt");

    // `merge --into` itself is excluded (0.56.0 step W3b closed that gap for it):
    // `seal_claims_in_process` (0.56.0 step W3c-4 note, above) reproduces the identical
    // staleness instead, since it calls `seal_from_accepted_claim` directly, bypassing the
    // CLI-level `sync seal` orchestration this round adds.
    make_a_txt_stale_via_sync_seal(&repo, b"a2-from-feature\n");
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"a1\n",
        "sync seal must not touch the worktree -- a.txt stays at its old, stale bytes on disk"
    );

    ok(
        &support::prikk(&repo)
            .args(["mv", "a.txt", "renamed.txt"])
            .output()
            .unwrap(),
        "mv a.txt renamed.txt",
    );
    assert!(!repo.join("a.txt").exists());
    assert_eq!(
        std::fs::read(repo.join("renamed.txt")).unwrap(),
        b"a1\n",
        "mv carries the stale bytes along under the new name"
    );

    let refused = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--from-worktree", "-m", "rename the stale file"])
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse: the rename's own source node is stale: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("refusing to commit") && stderr(&refused).contains("a.txt"),
        "the refusal names the rename's own source path, a.txt: {}",
        stderr(&refused)
    );

    let _ = std::fs::remove_dir_all(&repo);
}
