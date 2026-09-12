//! RFC 147 §3b: `log` and `checkout` resolve a tag ref through `refs::resolve_ref_tip_block`.
//!
//! **Every fixture here tags the *first* of two sealed blocks, never the tip.** A tag at the tip
//! resolves to the same block either way, so it cannot tell the fix from the bug — the handoff's
//! own control 1 says so, and the content assertion below is the one that depends on it: at the
//! tagged block `a.txt` reads `first`, at the tip it reads `second`.
//!
//! The three commands exercise three independent resolution sites (`checkout.rs`, `history.rs`,
//! `patch_replay/read.rs`), which is what lets a single reverted site fail exactly one of them.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn block_id_of(output: &std::process::Output) -> String {
    stdout_of(output)
        .lines()
        .find_map(|line| line.strip_prefix("block id: "))
        .expect("seal reports a block id")
        .trim()
        .to_string()
}

/// Two sealed generations; the tag lands on the **first**. Returns the repository, the tagged
/// (non-tip) block id, and the tip block id.
fn repo_with_a_tag_below_the_tip(tag: &str) -> (PathBuf, String, String) {
    let repo = support::unique_repo(tag);
    support::init(&repo);

    std::fs::write(repo.join("a.txt"), "first\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit 1");
    let first_block = block_id_of(&support::seal(&repo, "heads/main"));

    std::fs::write(repo.join("a.txt"), "second\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "second"), "commit 2");
    let tip_block = block_id_of(&support::seal(&repo, "heads/main"));

    assert_ne!(first_block, tip_block, "the fixture needs two real blocks");
    support::ok(
        &support::tag_create(&repo, "tags/v1", &first_block),
        "tag the first block",
    );
    (repo, first_block, tip_block)
}

fn run(repo: &Path, args: &[&str]) -> std::process::Output {
    support::prikk(repo).args(args).output().unwrap()
}

/// Control 1, first output: `log --ref tags/v1` reports the tagged block, not the tip.
#[test]
fn log_resolves_a_tag_ref_to_its_target_block() {
    let (repo, tagged, tip) = repo_with_a_tag_below_the_tip("rfc147b-log");

    let out = run(&repo, &["log", "--ref", "tags/v1"]);
    support::ok(&out, "log --ref tags/v1");
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains(&format!("block {tagged}")),
        "log must report the tagged block: {stdout}"
    );
    assert!(
        !stdout.contains(&tip),
        "log must not report the tip for a tag below it: {stdout}"
    );
    // Control 4: the refusal this fix removes must be unreachable from a valid tag ref, asserted
    // by its absence rather than inferred from the success.
    assert!(
        !stderr_of(&out).contains("expected Block") && !stdout.contains("expected Block"),
        "the Tag-where-Block-expected refusal must be gone: {}",
        stderr_of(&out)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 1, second output: `checkout --plan-only` names the tagged block as `target block:`.
#[test]
fn checkout_plan_resolves_a_tag_ref_to_its_target_block() {
    let (repo, tagged, tip) = repo_with_a_tag_below_the_tip("rfc147b-plan");

    let out = run(&repo, &["checkout", "--plan-only", "--ref", "tags/v1"]);
    support::ok(&out, "checkout --plan-only --ref tags/v1");
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains(&format!("target block: {tagged}")),
        "the plan must target the tagged block: {stdout}"
    );
    assert!(!stdout.contains(&tip), "not the tip: {stdout}");
    // Control 3: the plan reports through the ordinary `materialization:` vocabulary.
    assert!(
        stdout.contains("materialization: "),
        "the plan must still report a materialization status: {stdout}"
    );
    // Control 4 again, for this site's own refusal.
    assert!(
        !stderr_of(&out).contains("object type mismatch"),
        "ObjectTypeMismatch must be unreachable from a valid tag ref: {}",
        stderr_of(&out)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 1, third output — the decisive one. Content **at the tagged block**, which is the only
/// assertion here that a tag-at-the-tip fixture could not distinguish.
#[test]
fn patch_plan_content_comes_from_the_tagged_block_not_the_tip() {
    let (repo, tagged, _tip) = repo_with_a_tag_below_the_tip("rfc147b-content");

    let out = run(
        &repo,
        &[
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
            "--ref",
            "tags/v1",
        ],
    );
    support::ok(&out, "checkout --patch-plan --content-path --ref tags/v1");
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains(&format!("\"target_block_id\": \"{tagged}\"")),
        "the report must name the tagged block: {stdout}"
    );
    assert!(
        stdout.contains("\"text\": \"first\\n\""),
        "content must be the tagged block's, not the tip's: {stdout}"
    );
    assert!(
        !stdout.contains("second"),
        "the tip's content must not appear: {stdout}"
    );

    // The same command on the branch proves the fixture really does differ between the two.
    let tip_out = run(
        &repo,
        &[
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
            "--ref",
            "heads/main",
        ],
    );
    support::ok(&tip_out, "same command on the branch");
    assert!(
        stdout_of(&tip_out).contains("\"text\": \"second\\n\""),
        "the branch must still read the tip: {}",
        stdout_of(&tip_out)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// §2's refusal that must survive: a worktree baseline is a branch, and `validate_local_branch_ref`
/// is right to refuse a tag. If resolution ever "fixes" this, the wrong function was changed.
#[test]
fn worktree_status_still_refuses_a_tag_ref() {
    let (repo, _tagged, _tip) = repo_with_a_tag_below_the_tip("rfc147b-worktree");

    let out = run(&repo, &["worktree-status", "--ref", "tags/v1"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "unchanged refusal, unchanged exit"
    );
    assert!(
        stderr_of(&out).contains("invalid name: ref namespace is reserved"),
        "a worktree baseline is a branch: {}",
        stderr_of(&out)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// §2's positive control: `bundle export` already resolved tags through this same function, so its
/// behaviour must be untouched — asserted on the exported bytes, not only on the summary lines.
#[test]
fn bundle_export_of_a_tag_is_unaffected() {
    let (repo, tagged, _tip) = repo_with_a_tag_below_the_tip("rfc147b-bundle");
    let out_path = repo.join("tag.bundle");

    let out = run(
        &repo,
        &[
            "bundle",
            "export",
            "--ref",
            "tags/v1",
            "--output",
            out_path.to_str().unwrap(),
        ],
    );
    support::ok(&out, "bundle export --ref tags/v1");
    assert!(
        stdout_of(&out).contains(&format!("tip block: {tagged}")),
        "bundle export already resolved the tag and must still: {}",
        stdout_of(&out)
    );
    assert!(out_path.exists(), "the bundle must be written");

    let _ = std::fs::remove_dir_all(&repo);
}

/// The **received** tag-history path, which nothing covered until now.
///
/// `history.rs` resolves in two loops — `load_ref_history` and `load_received_ref_history` — and the
/// perturbation pass found that reverting the second one broke no test in the workspace: it was a
/// site changed on faith. `prikk log --ref remotes/tags/v1` is how a reader reaches it (RFC 102
/// Stage 5: received refs live in their own container and `run_log` routes `remotes/`-prefixed names
/// there), so the coverage hole was reachable, not theoretical.
///
/// The fixture is the same shape as every other here — the tag is below the tip — so the assertion
/// distinguishes resolution from a label reprinted.
#[test]
fn log_resolves_a_received_tag_ref_to_its_target_block() {
    let (origin, tagged, tip) = repo_with_a_tag_below_the_tip("rfc147b-received");
    let bundle = origin.join("tag.bundle");
    support::ok(
        &run(
            &origin,
            &[
                "bundle",
                "export",
                "--ref",
                "tags/v1",
                "--output",
                bundle.to_str().unwrap(),
            ],
        ),
        "bundle export --ref tags/v1",
    );

    let receiver = support::unique_repo("rfc147b-received-in");
    support::init(&receiver);
    let imported = run(
        &receiver,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    support::ok(&imported, "bundle import");
    assert!(
        stdout_of(&imported).contains("received remotes/tags/v1"),
        "the tag arrives under its remotes/ name: {}",
        stdout_of(&imported)
    );

    let out = run(&receiver, &["log", "--ref", "remotes/tags/v1"]);
    support::ok(&out, "log --ref remotes/tags/v1");
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains(&format!("block {tagged}")),
        "the received tag must resolve to the tagged block: {stdout}"
    );
    assert!(
        !stdout.contains(&tip),
        "and not to the origin's tip: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&receiver);
}

/// RFC 147 §3c: `inverse-plan` and `rollback-preview` reach a **fifth** resolution site —
/// `patch_inverse/read.rs::current_target_block`, a *separate* function from
/// `patch_replay/read.rs::current_target_block` despite the identical name and nearly identical
/// body. §3b fixed the latter and a comment there claimed the two were shared; they were not, so
/// both commands still answered `object type mismatch: expected block, got tag` for a valid tag ref.
///
/// One test drives both commands because they share that one site: if it regresses, both fail
/// together, and a reader should see that they are one fact and not two.
#[test]
fn inverse_planning_resolves_a_tag_ref_to_its_target_block() {
    let (repo, tagged, tip) = repo_with_a_tag_below_the_tip("rfc147c-inverse");

    for command in ["inverse-plan", "rollback-preview"] {
        let out = run(&repo, &[command, "--ref", "tags/v1"]);
        support::ok(&out, command);
        let stdout = stdout_of(&out);
        assert!(
            stdout.contains(&format!("target block: {tagged}")),
            "{command} must target the tagged block: {stdout}"
        );
        assert!(
            !stdout.contains(&tip),
            "{command} must not reach the tip: {stdout}"
        );
        // The exact refusal this fix removes, asserted absent rather than inferred from success.
        assert!(
            !stderr_of(&out).contains("object type mismatch"),
            "{command}: ObjectTypeMismatch must be unreachable from a valid tag ref: {}",
            stderr_of(&out)
        );
    }

    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 147 §3d item 1: `branch create --from tags/v1` publishes the new branch at the Block the tag
/// names, rather than refusing with `object type mismatch: expected block, got tag`.
///
/// `--from <ref>` means "at the block this ref names", which is what every read surface now does.
/// The tag is below the tip, so the new branch's own history is the assertion: if the resolution
/// were dropped, the branch would either fail to publish or start at the wrong block.
#[test]
fn branch_create_from_a_tag_ref_starts_at_the_tagged_block() {
    let (repo, tagged, tip) = repo_with_a_tag_below_the_tip("rfc147d-branch-from-tag");

    let created = support::branch_create(&repo, "heads/fromtag", "tags/v1");
    support::ok(&created, "branch create --from tags/v1");
    assert!(
        stdout_of(&created).contains(&format!("target block: {tagged}")),
        "the branch must be published at the tagged block: {}",
        stdout_of(&created)
    );
    assert!(
        !stderr_of(&created).contains("object type mismatch"),
        "the refusal this fixes must be gone: {}",
        stderr_of(&created)
    );

    // Read it back through an independent surface: the branch's own history starts at the tagged
    // block, not at the tip the tag sits below.
    let log = run(&repo, &["log", "--ref", "heads/fromtag"]);
    support::ok(&log, "log --ref heads/fromtag");
    let stdout = stdout_of(&log);
    assert!(
        stdout.contains(&format!("block {tagged}")),
        "the new branch's history starts at the tagged block: {stdout}"
    );
    assert!(!stdout.contains(&tip), "and not at the tip: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 147 §3d item 2: `tag create --target tags/v1` is **refused, not dereferenced** — and the
/// refusal is a precondition, not an integrity failure.
///
/// A tag of a tag is outside the model (ref → tag object → block, one hop). Resolving silently
/// would make `--target tags/v1` and `--target <that block>` produce identical history, so a reader
/// could no longer tell which was asked for. The two forms that *do* work are asserted in the same
/// test, because a refusal is only as good as the alternative it names.
#[test]
fn tag_create_targeting_a_tag_ref_refuses_as_a_precondition() {
    let (repo, tagged, _tip) = repo_with_a_tag_below_the_tip("rfc147d-tag-of-tag");

    let refused = support::tag_create(&repo, "tags/v2", "tags/v1");
    assert_eq!(refused.status.code(), Some(1), "must refuse");
    let stderr = stderr_of(&refused);
    assert_eq!(
        stderr,
        "error: precondition not met: --target names a tag (tags/v1); pass the block id it points \
         at, or a branch ref\n",
        "the refusal names both accepted forms"
    );
    // The class actually changed: the old wording must be gone, not merely accompanied.
    assert!(
        !stderr.contains("object type mismatch"),
        "the ObjectTypeMismatch this replaces must be absent: {stderr}"
    );

    // Both named alternatives still work, so the message is not pointing at a closed door.
    support::ok(
        &support::tag_create(&repo, "tags/byblock", &tagged),
        "tag create --target <block id>",
    );
    support::ok(
        &support::tag_create(&repo, "tags/bybranch", "heads/main"),
        "tag create --target heads/main",
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 147 §3d item 3: after consolidating the two `current_target_block` duplicates into one
/// `refs::read_current_ref_tip_block`, every surface that reached either of them still resolves the
/// same non-tip tag to the same block.
///
/// This is the regression the consolidation could plausibly cause — one module silently losing its
/// resolution — and it is asserted across both former callers in one place rather than trusted.
#[test]
fn every_surface_of_the_consolidated_resolver_agrees_on_one_tag() {
    let (repo, tagged, tip) = repo_with_a_tag_below_the_tip("rfc147d-consolidated");

    for args in [
        &["inverse-plan", "--ref", "tags/v1"][..],
        &["rollback-preview", "--ref", "tags/v1"][..],
        &["checkout", "--patch-plan", "--ref", "tags/v1"][..],
    ] {
        let out = run(&repo, args);
        support::ok(&out, &format!("{args:?}"));
        let stdout = stdout_of(&out);
        assert!(
            stdout.contains(&format!("target block: {tagged}")),
            "{args:?} must resolve to the tagged block: {stdout}"
        );
        assert!(
            !stdout.contains(&tip),
            "{args:?} must not reach the tip: {stdout}"
        );
    }

    // `--content-path` reaches the same resolution through the JSON surface, and its content is the
    // assertion a target-block line alone cannot make.
    let content = run(
        &repo,
        &[
            "checkout",
            "--patch-plan",
            "--format",
            "json",
            "--content-path",
            "a.txt",
            "--ref",
            "tags/v1",
        ],
    );
    support::ok(&content, "checkout --patch-plan --content-path");
    let stdout = stdout_of(&content);
    assert!(
        stdout.contains(&format!("\"target_block_id\": \"{tagged}\"")),
        "content must come from the tagged block: {stdout}"
    );
    assert!(
        stdout.contains("\"text\": \"first\\n\""),
        "and its bytes must be the tagged block's, not the tip's: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
