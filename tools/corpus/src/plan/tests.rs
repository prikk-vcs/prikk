#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::profile::{BuilderInputs, OperationKindMix, Provenance, SCHEMA_VERSION, Shape};

fn histogram(pairs: &[(u64, u64)]) -> BTreeMap<String, u64> {
    pairs
        .iter()
        .map(|&(value, count)| (value.to_string(), count))
        .collect()
}

/// A minimal, otherwise-arbitrary profile: every case below overrides exactly the fields it cares
/// about. `path_touches` is fixed generously (200 single-touch paths) so tests need not reason about
/// path-pool exhaustion unless that is specifically what they are testing.
fn fixture_profile(
    commit_count: u64,
    files_changed_per_commit: &[(u64, u64)],
    generator_seed: u64,
) -> Profile {
    Profile {
        schema_version: SCHEMA_VERSION,
        provenance: Provenance {
            source_repository: "test fixture".to_owned(),
            revision: "0".repeat(40),
            extraction_commands: Vec::new(),
            extraction_date: "2026-01-01".to_owned(),
            rename_detection: true,
        },
        shape: Shape {
            commit_count,
            files_changed_per_commit: histogram(files_changed_per_commit),
            operation_kind_mix: OperationKindMix {
                added: 100,
                modified: 0,
                deleted: 0,
                renamed: 0,
                copied: 0,
                type_changed: 0,
            },
            distinct_paths: 200,
            path_touches: histogram(&[(1, 200)]),
            file_sizes: histogram(&[(10, 1)]),
        },
        builder_inputs: BuilderInputs {
            generator_seed,
            author_key_id: "test-author".to_owned(),
            author_seed_hex: "11".repeat(32),
            maintainer_key_id: "test-maintainer".to_owned(),
            maintainer_seed_hex: "22".repeat(32),
            known_nondeterminism_risks: Vec::new(),
        },
    }
}

#[test]
fn zero_commit_count_is_refused() {
    let profile = fixture_profile(0, &[(1, 1)], 1);
    assert_eq!(plan(&profile, 1), Err(PlanError::ZeroProfileCommitCount));
}

#[test]
fn same_profile_same_target_is_byte_identical() {
    let profile = fixture_profile(4, &[(1, 1), (5, 3)], 42);
    let first = plan(&profile, 4).unwrap();
    let second = plan(&profile, 4).unwrap();
    assert_eq!(first, second);
}

/// Handoff §6 control 2: a different seed must actually change the plan, not merely be accepted and
/// ignored. Two distinct seeds with everything else held fixed.
#[test]
fn different_generator_seed_produces_a_different_manifest() {
    let mut profile_a = fixture_profile(1, &[(2, 1)], 1);
    profile_a.shape.file_sizes = histogram(&[(10, 1), (20, 1)]);
    let mut profile_b = profile_a.clone();
    profile_b.builder_inputs.generator_seed = 2;

    let manifest_a = plan(&profile_a, 1).unwrap();
    let manifest_b = plan(&profile_b, 1).unwrap();
    assert_ne!(
        manifest_a, manifest_b,
        "changing only generator_seed must change the planned manifest"
    );
}

/// Handoff §6 control 3: a hand-check, not a plausibility argument. `files_changed_per_commit`'s
/// per-commit counts are produced by exact largest-remainder scaling (module doc) -- fully
/// deterministic arithmetic on the histogram and the target depth, independent of the seed, which
/// only decides *order*. One commit drew "1" and three drew "5" in this profile's own 4-commit
/// sample; targeting the same depth (4) must reproduce that exact multiset.
#[test]
fn per_commit_file_counts_reproduce_the_histogram_exactly() {
    let profile = fixture_profile(4, &[(1, 1), (5, 3)], 7);
    let manifest = plan(&profile, 4).unwrap();
    let mut counts: Vec<usize> = manifest
        .commits
        .iter()
        .map(|commit| commit.actions.len())
        .collect();
    counts.sort_unstable();
    assert_eq!(counts, vec![1, 5, 5, 5]);
}

/// The same exact-scaling arithmetic must hold when the target depth is *not* the profile's own
/// `commit_count` -- this is what lets the build-cost curve go past 600 commits from
/// `profiles/prikk-self.toml`. Scaling `{1: 1, 5: 3}` (source total 4) to a target of 8 doubles every
/// bucket: `{1: 2, 5: 6}`.
#[test]
fn per_commit_file_counts_scale_exactly_to_a_different_target_depth() {
    let profile = fixture_profile(4, &[(1, 1), (5, 3)], 7);
    let manifest = plan(&profile, 8).unwrap();
    let mut counts: Vec<usize> = manifest
        .commits
        .iter()
        .map(|commit| commit.actions.len())
        .collect();
    counts.sort_unstable();
    assert_eq!(counts, vec![1, 1, 5, 5, 5, 5, 5, 5]);
}

#[test]
fn genesis_commit_never_draws_a_kind_that_needs_a_live_path() {
    let mut profile = fixture_profile(1, &[(3, 1)], 3);
    // Every kind gets equal weight; if feasibility masking were missing, the very first action could
    // legally try to modify/delete/rename a path before any path exists.
    profile.shape.operation_kind_mix = OperationKindMix {
        added: 1,
        modified: 1,
        deleted: 1,
        renamed: 1,
        copied: 1,
        type_changed: 1,
    };
    let manifest = plan(&profile, 1).unwrap();
    let first_commit = &manifest.commits[0];
    assert!(!first_commit.actions.is_empty());
    for action in &first_commit.actions {
        assert!(
            matches!(action, PlannedAction::CreateFile { .. }),
            "a path cannot be edited, deleted, or renamed before any path has been created: {action:?}"
        );
    }
}

/// Two commits, each drawing exactly one file-changed unit from a 3-path pool. Whichever commit
/// plans first is forced into all-creates (bootstrap: nothing is live yet) and exhausts the 3-path
/// pool; whichever plans second can only edit, and the same 3-path pool bounds it too. Neither
/// commit's action count reaches its own drawn total once the pool runs out mid-commit -- expected
/// (module doc), not an error -- but within each commit, no path may repeat.
#[test]
fn a_path_is_touched_at_most_once_per_commit() {
    let mut profile = fixture_profile(2, &[(3, 1), (10, 1)], 11);
    profile.shape.operation_kind_mix = OperationKindMix {
        added: 1,
        modified: 5,
        deleted: 0,
        renamed: 0,
        copied: 0,
        type_changed: 0,
    };
    profile.shape.path_touches = histogram(&[(1, 3)]);
    let manifest = plan(&profile, 2).unwrap();
    for commit in &manifest.commits {
        let mut seen = std::collections::BTreeSet::new();
        for action in &commit.actions {
            let path = match action {
                PlannedAction::CreateFile { path, .. }
                | PlannedAction::EditText { path, .. }
                | PlannedAction::DeleteNode { path } => path.clone(),
            };
            assert!(
                seen.insert(path.clone()),
                "path {path} touched twice in one commit"
            );
        }
    }
    let total_actions: usize = manifest
        .commits
        .iter()
        .map(|commit| commit.actions.len())
        .sum();
    assert_eq!(
        total_actions, 6,
        "both commits are bounded by the same 3-path pool"
    );
}

#[test]
fn rename_emits_a_delete_and_a_create_never_a_rename_operation() {
    let mut profile = fixture_profile(6, &[(1, 6)], 5);
    profile.shape.operation_kind_mix = OperationKindMix {
        added: 1,
        modified: 0,
        deleted: 0,
        renamed: 49,
        copied: 0,
        type_changed: 0,
    };
    let manifest = plan(&profile, 6).unwrap();
    let delete_count = manifest
        .commits
        .iter()
        .flat_map(|commit| &commit.actions)
        .filter(|action| matches!(action, PlannedAction::DeleteNode { .. }))
        .count();
    assert!(
        delete_count > 0,
        "renamed has overwhelming weight after the bootstrap create; expected at least one \
         delete+create pair standing in for a rename"
    );
    let has_rename_shaped_action = manifest
        .commits
        .iter()
        .flat_map(|commit| &commit.actions)
        .any(|action| {
            !matches!(
                action,
                PlannedAction::CreateFile { .. } | PlannedAction::DeleteNode { .. }
            )
        });
    assert!(
        !has_rename_shaped_action,
        "PlannedAction has no rename variant; every action must be Create or Delete"
    );
}

#[test]
fn empty_files_changed_histogram_is_refused() {
    let profile = fixture_profile(1, &[], 1);
    assert_eq!(
        plan(&profile, 1),
        Err(PlanError::EmptyHistogram {
            field: "files_changed_per_commit"
        })
    );
}

#[test]
fn expand_and_scale_matches_hand_computed_largest_remainder_rounding() {
    // source_total = 3, target = 10, scale = 10/3. Exact shares: 1 -> 3.333.., 2 -> 6.666...
    // floors: 3 and 6 (assigned 9), one remainder distributed to the larger fractional part (2's
    // 0.666 > 1's 0.333), giving {1: 3, 2: 7}.
    let histogram = [(1_u64, 1_u64), (2, 2)];
    let mut expanded = expand_and_scale(&histogram, 10);
    expanded.sort_unstable();
    let ones = expanded.iter().filter(|&&value| value == 1).count();
    let twos = expanded.iter().filter(|&&value| value == 2).count();
    assert_eq!((ones, twos), (3, 7));
    assert_eq!(expanded.len(), 10);
}
