//! Standing regression test for the `show` lifecycle-replay fix (0.54.0 Part 5c item 3).
//!
//! `prikk show <block-id>` used to fail with `lifecycle replay: blob ... required for a state
//! effect is missing` on any block whose own text edit's preimage was produced by an *earlier*
//! edit and never separately stored -- a legitimate storage optimization: `EditText` never writes
//! a fresh `Blob` object, only `CreateFile` does. The fix lives in `crates/prikk-store/src/show.rs`
//! and `crates/prikk-store/src/lifecycle_cache/replay.rs`.
//!
//! The architect swept every other block-reading CLI command by hand on the broken binary and
//! found `show` was the *only* one affected -- `cat`, `tree`, `checkout --patch-plan`, `extract`,
//! `diff`, `inverse-plan`, `rollback-preview`, `worktree-status`, `verify`, a queued `show`, and a
//! bare `diff` all worked fine throughout. This file encodes that sweep as a standing regression:
//! a chain of consecutive sealed text edits (exactly the shape that broke `show`), with every
//! block-reading command run against every block in the chain, so a recurrence of this *class* of
//! bug in any of them is caught by the test suite, not just by a one-time manual check.
//!
//! This test does not re-validate `show`'s own output content (`crates/prikk-store/src/show/
//! tests.rs` and `rfc142_show.rs` already do that in depth) -- it only asserts that every listed
//! command *exits successfully* on every block in this specific "consecutive unstored text-edit
//! preimages" shape.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::PathBuf;
use std::process::Output;

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// `commit`'s own stdout carries `patch id: <id>` (same idiom `rfc142_show.rs::extract_patch_id`
/// already uses).
fn extract_patch_id(output: &Output) -> String {
    stdout_of(output)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit output must include a patch id line")
        .trim()
        .to_string()
}

/// A fresh, never-before-used directory for one `extract` call. `support::unique_repo` already
/// creates the directory, but `extract` refuses an existing target (`extract.rs`'s own tests),
/// so it is removed immediately -- the same idiom those tests use throughout.
fn fresh_extract_dir(tag: &str) -> PathBuf {
    let dir = support::unique_repo(tag);
    std::fs::remove_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_block_reading_command_succeeds_across_a_chain_of_unstored_text_edit_preimages() {
    let repo = support::unique_repo("show-fix-sweep");
    support::init(&repo);

    let path = "chain.txt";
    // Genesis write: a `CreateFile`, whose own preimage (the empty file) is always a legitimate
    // case -- never part of the bug's class, but included in the per-block sweep below anyway,
    // since the whole point of this test is "the class, kept true" for every listed command.
    support::generation(&repo, "heads/main", path, b"generation 0\n", "genesis");
    // Five further edits: each one's preimage is the *prior* edit's own post-image, which
    // `EditText` never stores as a separate `Blob` object -- exactly the shape that used to break
    // `show`.
    for generation in 1..=5 {
        support::generation(
            &repo,
            "heads/main",
            path,
            format!("generation {generation}\n").as_bytes(),
            &format!("edit {generation}"),
        );
    }

    // Resolve every block's own id, chronological order (genesis first), from `log --format json`
    // in one call, rather than threading a captured `seal` output through six manual write/commit/
    // seal calls -- `support::generation` (used above, per the handoff) does not return one.
    let log_out = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&log_out, "log --format json");
    let log_json = support::json::parse(&stdout_of(&log_out));
    let mut blocks: Vec<String> = log_json
        .get("blocks")
        .as_array()
        .iter()
        .map(|block| block.get("block_id").as_str().to_string())
        .collect();
    assert_eq!(
        blocks.len(),
        6,
        "genesis plus five edits must be six sealed blocks"
    );
    // `log` lists newest-first (confirmed against `rfc146_listing_json.rs`'s own
    // `log_prose_and_json_agree_across_two_blocks`: the Root block is always last); chronological
    // order (genesis first) is what the per-block loop and the parent-lookup below both want.
    blocks.reverse();

    for (index, block_id) in blocks.iter().enumerate() {
        let block_id = block_id.as_str();
        let what = format!("block {index} ({block_id})");

        // show <block-id> / show <block-id> --format json
        support::ok(
            &support::prikk(&repo)
                .args(["show", block_id])
                .output()
                .unwrap(),
            &format!("show: {what}"),
        );
        support::ok(
            &support::prikk(&repo)
                .args(["show", block_id, "--format", "json"])
                .output()
                .unwrap(),
            &format!("show --format json: {what}"),
        );

        // cat --path <path> --ref <block-id>, text and --format json
        support::ok(
            &support::prikk(&repo)
                .args(["cat", "--path", path, "--ref", block_id])
                .output()
                .unwrap(),
            &format!("cat: {what}"),
        );
        support::ok(
            &support::prikk(&repo)
                .args(["cat", "--path", path, "--ref", block_id, "--format", "json"])
                .output()
                .unwrap(),
            &format!("cat --format json: {what}"),
        );

        // tree --ref <block-id>, text and --format json
        support::ok(
            &support::prikk(&repo)
                .args(["tree", "--ref", block_id])
                .output()
                .unwrap(),
            &format!("tree: {what}"),
        );
        support::ok(
            &support::prikk(&repo)
                .args(["tree", "--ref", block_id, "--format", "json"])
                .output()
                .unwrap(),
            &format!("tree --format json: {what}"),
        );

        // checkout --patch-plan --ref <block-id> (prose plan), and the --format json form with
        // --content-path (actual replayed content, per RFC 143 §5 -- "with content!").
        support::ok(
            &support::prikk(&repo)
                .args(["checkout", "--patch-plan", "--ref", block_id])
                .output()
                .unwrap(),
            &format!("checkout --patch-plan: {what}"),
        );
        support::ok(
            &support::prikk(&repo)
                .args([
                    "checkout",
                    "--patch-plan",
                    "--ref",
                    block_id,
                    "--format",
                    "json",
                    "--content-path",
                    path,
                ])
                .output()
                .unwrap(),
            &format!("checkout --patch-plan --format json --content-path: {what}"),
        );

        // extract --ref <block-id> <fresh-dir>, text and --format json -- each into a fresh,
        // never-before-used directory, since extract refuses an existing target.
        let extract_text_dir = fresh_extract_dir(&format!("show-fix-sweep-extract-{index}-text"));
        support::ok(
            &support::prikk(&repo)
                .args([
                    "extract",
                    "--ref",
                    block_id,
                    extract_text_dir.to_str().unwrap(),
                ])
                .output()
                .unwrap(),
            &format!("extract: {what}"),
        );
        let extract_json_dir = fresh_extract_dir(&format!("show-fix-sweep-extract-{index}-json"));
        support::ok(
            &support::prikk(&repo)
                .args([
                    "extract",
                    "--ref",
                    block_id,
                    extract_json_dir.to_str().unwrap(),
                    "--format",
                    "json",
                ])
                .output()
                .unwrap(),
            &format!("extract --format json: {what}"),
        );
        let _ = std::fs::remove_dir_all(&extract_text_dir);
        let _ = std::fs::remove_dir_all(&extract_json_dir);

        // diff --from <parent-block-id> --to <block-id> --path <path>, text and --format json.
        // The genesis block (index 0) has no parent in this chain, so it is skipped here only --
        // it is still covered by every command above.
        if index > 0 {
            let parent = blocks[index - 1].as_str();
            support::ok(
                &support::prikk(&repo)
                    .args(["diff", "--from", parent, "--to", block_id, "--path", path])
                    .output()
                    .unwrap(),
                &format!("diff: {what} against its parent"),
            );
            support::ok(
                &support::prikk(&repo)
                    .args([
                        "diff", "--from", parent, "--to", block_id, "--path", path, "--format",
                        "json",
                    ])
                    .output()
                    .unwrap(),
                &format!("diff --format json: {what} against its parent"),
            );
        }
    }

    // Each of the following exercised once, against the chain's tip. Run while the worktree is
    // still clean (every generation above both committed and sealed) -- `worktree-status`'s own
    // exit-code contract refuses when the worktree has changes, so this must run before the queued
    // commit introduced below.
    support::ok(
        &support::prikk(&repo)
            .args(["inverse-plan"])
            .output()
            .unwrap(),
        "inverse-plan",
    );
    // No --format json here: `inverse-plan`'s own argument parser (`parse_inverse_plan_args`,
    // `crates/prikk-cli/src/args.rs`) accepts only `[path]`/`--ref` -- confirmed by reading it,
    // not assumed -- so there is no JSON form to also exercise.
    support::ok(
        &support::prikk(&repo)
            .args(["rollback-preview"])
            .output()
            .unwrap(),
        "rollback-preview",
    );
    // Same confirmation for `rollback-preview` (`parse_rollback_preview_args`): no --format json.
    support::ok(
        &support::prikk(&repo)
            .args(["worktree-status"])
            .output()
            .unwrap(),
        "worktree-status",
    );
    support::ok(
        &support::prikk(&repo)
            .args(["worktree-status", "--format", "json"])
            .output()
            .unwrap(),
        "worktree-status --format json",
    );
    support::ok(&support::verify(&repo), "verify");
    support::ok(
        &support::prikk(&repo)
            .args(["verify", "--format", "json"])
            .output()
            .unwrap(),
        "verify --format json",
    );

    // A queued `show`: one more edit, committed but never sealed, then `show` on its own patch id
    // (`commit --ref` without a following `seal` is how every other test in this crate builds a
    // queued-but-unsealed patch too, e.g. `rfc142_show.rs::control5_show_on_a_patch_id_works_too`).
    std::fs::write(repo.join(path), b"queued, unsealed\n").unwrap();
    let commit_out = support::commit(&repo, "heads/main", "queued edit");
    support::ok(&commit_out, "commit: queued edit");
    let queued_patch_id = extract_patch_id(&commit_out);
    support::ok(
        &support::prikk(&repo)
            .args(["show", &queued_patch_id])
            .output()
            .unwrap(),
        "show: queued patch",
    );

    // A bare `diff` (no explicit arguments): per `diff.rs`'s own module doc, this compares the
    // current branch's tip -- *with* its queued commit -- against the worktree.
    support::ok(
        &support::prikk(&repo).args(["diff"]).output().unwrap(),
        "bare diff",
    );

    let _ = std::fs::remove_dir_all(&repo);
}
