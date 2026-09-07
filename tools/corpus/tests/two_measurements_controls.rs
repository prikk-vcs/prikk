//! RFC 139 increment 3, handoff §5: fast, `#[ignore]`d correctness controls for the two
//! measurements. Deliberately separate from `two_measurements.rs` (the actual, expensive
//! measurement instrument at realistic depth): these use a small, hand-authored profile
//! (`tests/fixtures/shallow-profile.toml`) with a tiny, fixed tree grown deep via edits --
//! DC-92's own isolation technique applied here so "does cost move with depth" is checkable in
//! seconds rather than minutes.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use prikk_corpus::{Profile, execute};

mod support;

fn shallow_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/shallow-profile.toml"
    ))
    .expect("reading shallow-profile.toml");
    toml::from_str(&text).expect("parsing shallow-profile.toml")
}

fn write_divergence_file(repo_root: &Path, path: &str, seed: u64) {
    let bytes = prikk_corpus::rng::generate_bytes(seed, 16);
    let full_path = repo_root.join(path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).expect("creating divergence parent dir");
    }
    std::fs::write(full_path, bytes).expect("writing divergence file");
}

/// Result of producing one two-sided divergence from the current tip of `heads/main`.
struct Divergence {
    baseline_id: String,
    left_id: String,
    right_id: String,
    left_paths: Vec<String>,
    right_paths: Vec<String>,
}

/// Branch from the current tip of `heads/main`, grow `heads/main` by `size` commits (left) and the
/// new branch by `size` commits (right), each one new, branch-unique file per commit -- RFC 139
/// increment 3 §2.3's invented divergence shape (disjoint new files, not overlapping edits; see
/// this crate's own report for why). Restores the worktree to `heads/main`'s own real state
/// afterward (removes the right side's files) so the caller can safely continue committing to
/// `heads/main`.
fn build_divergence(
    binary: &Path,
    repo_root: &Path,
    profile: &Profile,
    branch_name: &str,
    size: u64,
    tag: &str,
) -> Divergence {
    let baseline_id = support::block_ids(binary, repo_root, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .expect("heads/main has a sealed tip");
    execute::branch_create(binary, repo_root, profile, branch_name, execute::REF_NAME)
        .expect("branch create");

    let mut left_paths = Vec::new();
    for index in 0..size {
        let path = format!("divergence/{tag}/left/f{index}.txt");
        write_divergence_file(
            repo_root,
            &path,
            profile.builder_inputs.generator_seed ^ (index + 1),
        );
        execute::run_commit(
            binary,
            repo_root,
            profile,
            execute::REF_NAME,
            &format!("left {tag} {index}"),
        )
        .expect("left commit");
        execute::run_seal(binary, repo_root, profile, execute::REF_NAME).expect("left seal");
        left_paths.push(path);
    }
    let left_id = support::block_ids(binary, repo_root, execute::REF_NAME, 1)
        .into_iter()
        .next()
        .expect("heads/main has a sealed tip after left growth");

    let mut right_paths = Vec::new();
    for index in 0..size {
        let path = format!("divergence/{tag}/right/f{index}.txt");
        write_divergence_file(
            repo_root,
            &path,
            profile.builder_inputs.generator_seed ^ (1000 + index + 1),
        );
        execute::run_commit(
            binary,
            repo_root,
            profile,
            branch_name,
            &format!("right {tag} {index}"),
        )
        .expect("right commit");
        execute::run_seal(binary, repo_root, profile, branch_name).expect("right seal");
        right_paths.push(path);
    }
    let right_id = support::block_ids(binary, repo_root, branch_name, 1)
        .into_iter()
        .next()
        .expect("branch has a sealed tip after right growth");

    // Restore the shared worktree to heads/main's own real current state (shared prefix + left's
    // files) -- the right side's files must not leak into heads/main's next commit.
    for path in &right_paths {
        std::fs::remove_file(repo_root.join(path)).expect("removing right divergence file");
    }

    Divergence {
        baseline_id,
        left_id,
        right_id,
        left_paths,
        right_paths,
    }
}

fn timed_merge_evidence(
    binary: &Path,
    repo_root: &Path,
    baseline: &str,
    left: &str,
    right: &str,
) -> (Duration, Output) {
    let start = Instant::now();
    let output = Command::new(binary)
        .current_dir(repo_root)
        .args([
            "merge-evidence",
            "--baseline-block",
            baseline,
            "--left-block",
            left,
            "--right-block",
            right,
        ])
        .output()
        .expect("running prikk merge-evidence");
    (start.elapsed(), output)
}

fn build_shallow_corpus(binary: &Path, repo_root: &Path, profile: &Profile, depth: u64) {
    let manifest = prikk_corpus::plan(profile, depth).expect("planning");
    execute::build(binary, repo_root, profile, &manifest).expect("building corpus");
}

/// Handoff §5 control 2: the repository under test is the one built from the stated profile at the
/// stated depth -- not a hand-made repository that happens to be deep.
#[test]
#[ignore = "drives a real prikk binary and repository; run deliberately"]
fn control2_corpus_is_built_from_the_stated_profile_at_the_stated_depth() {
    let profile = shallow_profile();
    let depth = 20;
    let manifest = prikk_corpus::plan(&profile, depth).expect("planning");
    let binary = support::prikk_binary_path();
    let repo_root = support::unique_dir("control2");
    execute::build(binary, &repo_root, &profile, &manifest).expect("building corpus");

    let block_ids = support::block_ids(binary, &repo_root, execute::REF_NAME, depth as usize + 5);
    assert_eq!(
        block_ids.len(),
        depth as usize,
        "sealed block count must equal the planned depth"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}

/// Handoff §5 control 4: the divergence is what the report claims -- exact commit counts on each
/// side, and disjoint path sets.
#[test]
#[ignore = "drives a real prikk binary and repository; run deliberately"]
fn control4_divergence_has_the_claimed_shape() {
    let profile = shallow_profile();
    let binary = support::prikk_binary_path();
    let repo_root = support::unique_dir("control4");
    build_shallow_corpus(binary, &repo_root, &profile, 10);

    let divergence = build_divergence(binary, &repo_root, &profile, "heads/divergence", 5, "c4");

    assert_eq!(divergence.left_paths.len(), 5);
    assert_eq!(divergence.right_paths.len(), 5);
    let left_set: std::collections::BTreeSet<_> = divergence.left_paths.iter().collect();
    let right_set: std::collections::BTreeSet<_> = divergence.right_paths.iter().collect();
    assert!(
        left_set.is_disjoint(&right_set),
        "left and right divergence paths must not overlap"
    );

    // Confirm the block counts independently, not just from our own bookkeeping: heads/main is now
    // 10 (shared) + 5 (left) = 15 deep. The branch's own `log` is 1 (the inherited baseline block,
    // which `branch create --from` points the new ref at, per `branch.rs`'s own module doc: "this
    // points at a block that already exists") + 5 (right's own new commits) = 6 -- verified
    // empirically here, not assumed, since the branch's log is not simply "its own new commits".
    let main_blocks = support::block_ids(binary, &repo_root, execute::REF_NAME, 30);
    assert_eq!(
        main_blocks.len(),
        15,
        "heads/main must be 10 + 5 = 15 blocks deep"
    );
    let branch_blocks = support::block_ids(binary, &repo_root, "heads/divergence", 30);
    assert_eq!(
        branch_blocks.len(),
        6,
        "the branch's own log is the inherited baseline block plus its 5 new commits"
    );

    // Confirm the right side's files were actually removed from the worktree (restored state) and
    // the left side's are still present (permanently part of heads/main's own history).
    for path in &divergence.left_paths {
        assert!(repo_root.join(path).exists(), "{path} must still exist");
    }
    for path in &divergence.right_paths {
        assert!(
            !repo_root.join(path).exists(),
            "{path} must have been removed"
        );
    }

    let _ = std::fs::remove_dir_all(&repo_root);
}

/// Handoff §5 control 3 (fast proxy) + control 1 (fast proxy): merge-evidence's cost moves with
/// baseline depth, at a scale cheap enough to assert directly rather than only observe in a table.
/// Uses the tiny-tree shallow profile specifically so tree size stays constant while depth grows --
/// isolating depth as the only thing that could explain a timing difference.
#[test]
#[ignore = "drives a real prikk binary and repository; run deliberately"]
fn control1_and_3_merge_evidence_cost_moves_with_baseline_depth() {
    let profile = shallow_profile();
    let binary = support::prikk_binary_path();

    let shallow_root = support::unique_dir("depth-shallow");
    build_shallow_corpus(binary, &shallow_root, &profile, 10);
    let shallow_divergence = build_divergence(
        binary,
        &shallow_root,
        &profile,
        "heads/divergence",
        5,
        "shallow",
    );
    let (shallow_elapsed, shallow_output) = timed_merge_evidence(
        binary,
        &shallow_root,
        &shallow_divergence.baseline_id,
        &shallow_divergence.left_id,
        &shallow_divergence.right_id,
    );
    assert!(
        shallow_output.status.success(),
        "merge-evidence must succeed (shallow)"
    );

    let deep_root = support::unique_dir("depth-deep");
    build_shallow_corpus(binary, &deep_root, &profile, 400);
    let deep_divergence =
        build_divergence(binary, &deep_root, &profile, "heads/divergence", 5, "deep");
    let (deep_elapsed, deep_output) = timed_merge_evidence(
        binary,
        &deep_root,
        &deep_divergence.baseline_id,
        &deep_divergence.left_id,
        &deep_divergence.right_id,
    );
    assert!(
        deep_output.status.success(),
        "merge-evidence must succeed (deep)"
    );

    eprintln!(
        "merge-evidence: baseline depth 10 -> {:?}, baseline depth 400 -> {:?}",
        shallow_elapsed, deep_elapsed
    );
    assert!(
        deep_elapsed > shallow_elapsed,
        "baseline reconstruction cost must grow with baseline depth (10 -> {shallow_elapsed:?}, \
         400 -> {deep_elapsed:?}); a call that does not reach replay_derived_state's real cost \
         would not show this"
    );

    let _ = std::fs::remove_dir_all(&shallow_root);
    let _ = std::fs::remove_dir_all(&deep_root);
}

/// Handoff §5 control 5: a rebuild at the same depth reproduces the manifest (increment 2's §5a
/// property), so the two measurements are of the same planned history.
#[test]
fn control5_rebuild_at_the_same_depth_reproduces_the_manifest() {
    let profile = shallow_profile();
    let first = prikk_corpus::plan(&profile, 200).expect("planning first");
    let second = prikk_corpus::plan(&profile, 200).expect("planning second");
    assert_eq!(first, second);
}

/// Sanity check for the checkout side of item 1, using the shallow profile: `--patch-plan` and
/// `--patch-materialize` both succeed against a `.prikk`-only copy (no worktree files, the way a
/// fresh clone would look), and materialize actually reconstructs the tracked file count.
#[test]
#[ignore = "drives a real prikk binary and repository; run deliberately"]
fn sanity_checkout_against_a_prikk_only_copy() {
    let profile = shallow_profile();
    let binary = support::prikk_binary_path();
    let repo_root = support::unique_dir("checkout-sanity");
    build_shallow_corpus(binary, &repo_root, &profile, 20);

    let plan_dir = support::unique_dir("checkout-sanity-plan");
    support::copy_prikk_only(&repo_root, &plan_dir);
    let plan_output = execute::checkout_patch_plan(binary, &plan_dir, execute::REF_NAME)
        .expect("checkout --patch-plan");
    assert!(plan_output.status.success());

    let materialize_dir = support::unique_dir("checkout-sanity-materialize");
    support::copy_prikk_only(&repo_root, &materialize_dir);
    let materialize_output =
        execute::checkout_patch_materialize(binary, &materialize_dir, execute::REF_NAME)
            .expect("checkout --patch-materialize");
    assert!(materialize_output.status.success());

    let tree_files = support::count_tree_files(&materialize_dir);
    assert!(
        tree_files > 0,
        "materialize must have written at least one file"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&plan_dir);
    let _ = std::fs::remove_dir_all(&materialize_dir);
}
