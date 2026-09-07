//! RFC 139 increment 2, handoff §6 controls 4, 5, 6: the executor, exercised against a real `prikk`
//! binary and a real, throwaway repository. `#[ignore]`d, matching `dc59_commit_benchmark.rs`'s own
//! precedent: these need a binary and a repository, so they cannot join the ordinary suite the way
//! the planner's own controls (`tests/determinism.rs`) do (RFC 139 §5a.2's whole point). Deliberately
//! separate from `build_cost_curve.rs`: these are shallow, fast correctness controls, not the slow
//! measurement instrument RFC 139 §6 asks for.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use prikk_corpus::execute::{self, BinaryIdentity};
use prikk_corpus::plan::PlannedAction;
use prikk_corpus::{ActionManifest, Profile};

mod support;

fn fixture_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tiny-profile.toml"
    ))
    .expect("reading tiny-profile.toml");
    toml::from_str(&text).expect("parsing tiny-profile.toml")
}

/// The manifest's own prediction of the final live file set and each surviving path's exact bytes,
/// computed purely from the manifest (no execution) by replaying its actions in order. Compared
/// against the real, built repository's worktree in `control4`.
fn expected_final_state(manifest: &ActionManifest) -> BTreeMap<String, Vec<u8>> {
    let mut state = BTreeMap::new();
    for commit in &manifest.commits {
        for action in &commit.actions {
            match action {
                PlannedAction::CreateFile {
                    path,
                    size_bytes,
                    content_seed,
                    ..
                } => {
                    state.insert(
                        path.clone(),
                        prikk_corpus::rng::generate_bytes(*content_seed, *size_bytes),
                    );
                }
                PlannedAction::EditText {
                    path,
                    append_bytes,
                    content_seed,
                    ..
                } => {
                    let appended = prikk_corpus::rng::generate_bytes(*content_seed, *append_bytes);
                    if let Some(existing) = state.get_mut(path) {
                        existing.extend_from_slice(&appended);
                    }
                }
                PlannedAction::DeleteNode { path } => {
                    state.remove(path);
                }
            }
        }
    }
    state
}

/// Every real file under `repo_root`, excluding `.prikk`, keyed by its repository-relative,
/// forward-slash path.
fn actual_worktree_files(repo_root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    walk(repo_root, repo_root, &mut files);
    files
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).expect("reading directory") {
        let entry = entry.expect("reading directory entry");
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(".prikk") {
            continue;
        }
        let file_type = entry.file_type().expect("reading file type");
        if file_type.is_dir() {
            walk(root, &path, out);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("entry is under root")
                .to_str()
                .expect("path is valid UTF-8")
                .replace('\\', "/");
            let bytes = std::fs::read(&path).expect("reading file");
            out.insert(relative, bytes);
        }
    }
}

/// Handoff §6 control 4: the executor builds what the manifest says, at shallow depth -- "the join
/// between the two halves and the place a planner/executor split can silently drift." Checked two
/// ways: the sealed block count (via `prikk log`) matches the planned commit count, and the real
/// worktree's file set and every surviving file's exact bytes match what replaying the manifest alone
/// (no execution) predicts.
#[test]
#[ignore = "drives a real prikk binary and repository; run deliberately"]
fn control4_executor_builds_exactly_what_the_manifest_says() {
    let profile = fixture_profile();
    let manifest = prikk_corpus::plan(&profile, profile.shape.commit_count).expect("planning");
    let binary = support::prikk_binary_path();
    let repo_root = support::unique_dir("control4");

    execute::build(binary, &repo_root, &profile, &manifest).expect("building corpus");

    let block_ids = support::block_ids(
        binary,
        &repo_root,
        execute::REF_NAME,
        manifest.commits.len() + 5,
    );
    assert_eq!(
        block_ids.len(),
        manifest.commits.len(),
        "one seal per planned commit means sealed block count must equal commit count"
    );

    let expected = expected_final_state(&manifest);
    let actual = actual_worktree_files(&repo_root);
    assert_eq!(
        expected, actual,
        "the real worktree's final file set and content must equal what replaying the manifest, \
         with no execution, predicts"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}

/// Handoff §6 control 5: binary identity is recorded, and distinguishable across two different
/// binaries. Building a second, differently-versioned `prikk` release just to prove this would be
/// heavy; the property under test is generic in the binary, so this exercises it against the real
/// `prikk` binary and this test process's own `cargo` binary (via `env!("CARGO")`) as an arbitrary
/// second, genuinely different executable -- not a model of a second prikk release, only a proof that
/// [`execute::binary_identity`] actually varies with its input rather than being a constant.
#[test]
#[ignore = "drives a real prikk binary; run deliberately"]
fn control5_binary_identity_is_recorded_and_distinguishable() {
    let prikk_identity =
        execute::binary_identity(support::prikk_binary_path()).expect("prikk identity");
    let cargo_identity =
        execute::binary_identity(Path::new(env!("CARGO"))).expect("cargo identity");

    assert!(!prikk_identity.sha256.is_empty());
    assert!(!prikk_identity.version_output.is_empty());
    // Checked on `sha256` specifically, not just the whole struct: `path` alone would make any two
    // different input paths compare unequal even if `sha256`/`version_output` were both constants,
    // which would prove nothing about whether this function actually reads its input.
    assert_ne!(
        prikk_identity.sha256, cargo_identity.sha256,
        "two different binaries must hash to two different sha256 identities"
    );
    assert_ne!(
        prikk_identity, cargo_identity,
        "two different binaries must produce two different recorded identities"
    );

    fn assert_identity_is_a_real_identity(identity: &BinaryIdentity) {
        assert_eq!(identity.sha256.len(), 64, "sha256 must be 32 bytes of hex");
        assert!(identity.sha256.chars().all(|ch| ch.is_ascii_hexdigit()));
    }
    assert_identity_is_a_real_identity(&prikk_identity);
    assert_identity_is_a_real_identity(&cargo_identity);
}

/// Handoff §6 control 6, and RFC 139 §5a.2's own ruling made concrete: two shallow builds from the
/// same profile and target depth produce **identical manifests** (already proven at the planner level
/// by `tests/determinism.rs`; re-derived here as this test's own precondition) but **must not**
/// produce identical sealed heads, because `NodeId` is an OS-CSPRNG draw at authoring time
/// (`crates/prikk-store/src/node_id_gen.rs`). Asserting the inequality, not just the equality, is
/// what documents §5a's ruling in the test suite rather than leaving a future reader to assume the
/// heads should match.
#[test]
#[ignore = "drives a real prikk binary and two repositories; run deliberately"]
fn control6_identical_manifests_build_to_different_sealed_heads() {
    let profile = fixture_profile();
    let manifest_a = prikk_corpus::plan(&profile, profile.shape.commit_count).expect("planning a");
    let manifest_b = prikk_corpus::plan(&profile, profile.shape.commit_count).expect("planning b");
    assert_eq!(
        manifest_a, manifest_b,
        "same profile and target depth must plan identically (precondition for this control)"
    );

    let binary = support::prikk_binary_path();
    let repo_a: PathBuf = support::unique_dir("control6-a");
    let repo_b: PathBuf = support::unique_dir("control6-b");
    execute::build(binary, &repo_a, &profile, &manifest_a).expect("building repo a");
    execute::build(binary, &repo_b, &profile, &manifest_b).expect("building repo b");

    let head_a = support::block_ids(binary, &repo_a, execute::REF_NAME, 1);
    let head_b = support::block_ids(binary, &repo_b, execute::REF_NAME, 1);
    assert_eq!(head_a.len(), 1);
    assert_eq!(head_b.len(), 1);
    assert_ne!(
        head_a, head_b,
        "RFC 139 §5a: two CLI-built repositories from the identical manifest must NOT share a \
         sealed head -- NodeId is a fresh OS-CSPRNG draw on every build"
    );

    let _ = std::fs::remove_dir_all(&repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
}
