//! DC-75 merge block lineage: a sealed merge structurally records both parents, a mainline pointer,
//! and the baseline confluence was proven against — closing DC-74's release condition. The headline
//! capability this unblocks: merging the same two branches more than once, which addendum-3/5 traced
//! as broken under a naive mainline-only-everywhere design (`PairReplayFailed`, not a clean refusal)
//! and which `reachability-vs-state-derivation-answer-v1.md` / the candidate-set patch-identity
//! exclusion rule fix.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "support/mod.rs"]
mod support;

use support::*;

use prikk_object::{BlockKind, BlockPayload, ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn current_target_block_id(repo: &std::path::Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout);
    let ref_state_id = ref_store
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let ref_state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
            .unwrap();
    ref_state.target_object_id
}

fn read_block(repo: &std::path::Path, block_id: ObjectId) -> BlockPayload {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout);
    let envelope = object_store
        .read_typed(block_id, ObjectType::Block)
        .unwrap()
        .unwrap();
    BlockPayload::decode_canonical(&envelope.canonical_payload).unwrap()
}

fn merge(
    repo: &std::path::Path,
    baseline: ObjectId,
    into_ref: &str,
    from_ref: &str,
) -> std::process::Output {
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            into_ref,
            "--from",
            from_ref,
        ])
        .output()
        .unwrap()
}

/// A single merge records `BlockKind::Merge`, both parents, the mainline pointer, and the recorded
/// baseline — the structural record DC-74's release condition required.
#[test]
fn single_merge_records_two_parents_mainline_and_baseline() {
    let repo = unique_repo("dc75-single-merge-shape");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let m1 = current_target_block_id(&repo, "heads/main");

    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch topic",
    );
    std::fs::write(repo.join("topic.txt"), b"from topic").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change"),
        "commit topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");
    let t1 = current_target_block_id(&repo, "heads/topic");

    ok(&merge(&repo, m1, "heads/main", "heads/topic"), "merge");
    let m2 = current_target_block_id(&repo, "heads/main");
    let m2_payload = read_block(&repo, m2);

    assert_eq!(m2_payload.kind, BlockKind::Merge);
    let mut expected_parents = vec![m1, t1];
    expected_parents.sort();
    assert_eq!(m2_payload.parent_block_ids, expected_parents);
    assert_eq!(m2_payload.mainline_parent_id, Some(m1));
    assert_eq!(m2_payload.merge_baseline_block_id, Some(m1));

    ok(&prikk(&repo).arg("verify").output().unwrap(), "verify");
}

/// The headline DC-75 capability: merging the same two branches more than once. Traced by hand in
/// addendum-3/5 as broken under a naive fix (`PairReplayFailed`); this proves it end to end through
/// the compiled binary, not by construction at the store level.
#[test]
fn repeated_merge_between_the_same_two_branches_succeeds() {
    let repo = unique_repo("dc75-repeated-merge");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let m1 = current_target_block_id(&repo, "heads/main");

    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch topic",
    );
    std::fs::write(repo.join("topic.txt"), b"from topic").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change 1"),
        "commit topic 1",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic 1");
    let t1 = current_target_block_id(&repo, "heads/topic");

    // First merge: baseline M1.
    ok(
        &merge(&repo, m1, "heads/main", "heads/topic"),
        "first merge",
    );
    let m2 = current_target_block_id(&repo, "heads/main");

    // Topic advances again.
    std::fs::write(repo.join("topic2.txt"), b"from topic again").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change 2"),
        "commit topic 2",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic 2");
    let t2 = current_target_block_id(&repo, "heads/topic");

    // Second merge: the correct baseline is T1 -- reachable from M2 only via its secondary parent.
    // This is exactly the case addendum-3 traced as failing (`"baseline is not an ancestor"` under a
    // mainline-only walk) and addendum-5's escape-hatch evaluation as breaking the confluence proof
    // itself (`PairReplayFailed`) under a naive all-parents-follow fix without the patch-identity
    // exclusion rule.
    let merge_again = merge(&repo, t1, "heads/main", "heads/topic");
    ok(&merge_again, "second merge, baseline T1");
    let m3 = current_target_block_id(&repo, "heads/main");
    assert_ne!(m3, m2, "second merge must actually advance heads/main");

    let m3_payload = read_block(&repo, m3);
    assert_eq!(m3_payload.kind, BlockKind::Merge);
    assert_eq!(m3_payload.mainline_parent_id, Some(m2));
    assert_eq!(m3_payload.merge_baseline_block_id, Some(t1));
    let mut expected_parents = vec![m2, t2];
    expected_parents.sort();
    assert_eq!(m3_payload.parent_block_ids, expected_parents);
    // Only T2's own new patch is adopted -- T1's patch is excluded, already reachable from baseline
    // T1 via M2 (verbatim-adopted in the first merge). Confirms the escape-hatch evaluation's finding
    // survived into the real implementation, not just the probe.
    assert_eq!(m3_payload.patch_ids.len(), 1);

    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after repeated merge",
    );
    ok(
        &prikk(&repo).arg("doctor").output().unwrap(),
        "doctor after repeated merge",
    );

    let materialize_root = rebuild_from_sealed_history(&repo, "dc75-repeated-merge-rebuild");
    assert_eq!(
        std::fs::read(materialize_root.join("a.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read(materialize_root.join("topic.txt")).unwrap(),
        b"from topic"
    );
    assert_eq!(
        std::fs::read(materialize_root.join("topic2.txt")).unwrap(),
        b"from topic again"
    );

    // Ordinary work continues on top of the twice-merged history.
    std::fs::write(repo.join("after.txt"), b"more work").unwrap();
    ok(
        &commit(&repo, "heads/main", "after repeated merge"),
        "commit after",
    );
    ok(&seal(&repo, "heads/main"), "seal after");
    let doctor_out = prikk(&repo).arg("doctor").output().unwrap();
    ok(&doctor_out, "doctor after further commit");
    let doctor_stdout = String::from_utf8_lossy(&doctor_out.stdout);
    assert!(
        doctor_stdout.contains("commit-index divergences: 0")
            && doctor_stdout.contains("lifecycle-cache divergences: 0"),
        "expected zero cache divergences after committing on twice-merged history: {doctor_stdout}"
    );
}

/// The DC-74 refusal-diagnostic assertion addendum-5 §4 said §5 owes: attempting a *repeat* merge
/// against the stale pre-first-merge baseline (`M1`, instead of the true merge base `T1`) refuses as
/// a precise `pair_conflict` classification, not a generic or confusing error — the over-old-baseline
/// degeneracy `prerequisite-investigation-v1.md` §5 characterized as a false positive at the
/// operation-classification level (`T1`'s already-adopted patch re-offered against itself), reported
/// as a finding rather than fixed, and unaffected by anything DC-75 changed.
#[test]
fn repeated_merge_with_stale_baseline_refuses_as_pair_conflict() {
    let repo = unique_repo("dc75-repeated-merge-stale-baseline");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let m1 = current_target_block_id(&repo, "heads/main");

    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch topic",
    );
    std::fs::write(repo.join("topic.txt"), b"from topic").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change 1"),
        "commit topic 1",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic 1");

    ok(
        &merge(&repo, m1, "heads/main", "heads/topic"),
        "first merge",
    );
    let m2 = current_target_block_id(&repo, "heads/main");

    std::fs::write(repo.join("topic2.txt"), b"from topic again").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change 2"),
        "commit topic 2",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic 2");

    // Wrong baseline: M1 instead of the true merge base T1.
    let stale_attempt = merge(&repo, m1, "heads/main", "heads/topic");
    assert!(
        !stale_attempt.status.success(),
        "stale-baseline repeat merge unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&stale_attempt.stderr);
    assert!(
        stderr.contains("pair_conflict"),
        "expected a pair_conflict refusal, got: {stderr}"
    );
    assert_eq!(
        current_target_block_id(&repo, "heads/main"),
        m2,
        "refused merge must leave heads/main exactly where it was"
    );
}
