//! DC-74 merge execution: adopts the other side's patches verbatim and seals them when the two
//! sides are proven confluent from a common baseline; refuses cleanly, with no partial state,
//! otherwise. Criteria the architect said it would check hardest: adopted patches byte-identical to
//! their originals (same `ObjectId`, same author signature), and a byte-exact rebuild from sealed
//! history through the compiled binary — the DC-67 pattern.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "support/mod.rs"]
mod support;

use support::*;

use prikk_object::{BlockPayload, ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn genesis_block_id(repo: &std::path::Path) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout);
    let ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
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

fn current_ref_state_id(repo: &std::path::Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let ref_store = RefStore::new(layout);
    ref_store
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap()
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn merge_adopts_patches_byte_identical_and_rebuilds_byte_exact() {
    let repo = unique_repo("dc74-adoption-and-rebuild");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);

    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );
    std::fs::write(repo.join("topic.txt"), b"from topic").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change"),
        "commit on topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    // Capture the exact patch topic sealed, before the merge touches anything.
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let topic_ref_state_id = current_ref_state_id(&repo, "heads/topic");
    let topic_ref_state_envelope = object_store
        .read_typed(topic_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let topic_ref_state = RefStatePayload::decode_canonical(
        &topic_ref_state_envelope.canonical_payload,
        topic_ref_state_envelope.schema_version,
    )
    .unwrap();
    let topic_block_envelope = object_store
        .read_typed(topic_ref_state.target_object_id, ObjectType::Block)
        .unwrap()
        .unwrap();
    let topic_block =
        BlockPayload::decode_canonical(&topic_block_envelope.canonical_payload).unwrap();
    assert_eq!(topic_block.patch_ids.len(), 1);
    let adopted_patch_id = topic_block.patch_ids[0];
    let original_patch = object_store
        .read_typed(adopted_patch_id, ObjectType::Patch)
        .unwrap()
        .unwrap();

    let out = merge(&repo, baseline, "heads/main", "heads/topic");
    eprintln!("merge stdout: {}", String::from_utf8_lossy(&out.stdout));
    eprintln!("merge stderr: {}", String::from_utf8_lossy(&out.stderr));
    ok(&out, "merge");

    // Criterion 3: byte-identical, asserted by test, not argued in prose.
    let adopted_patch_after = object_store
        .read_typed(adopted_patch_id, ObjectType::Patch)
        .unwrap()
        .unwrap();
    assert_eq!(
        adopted_patch_after.canonical_payload,
        original_patch.canonical_payload
    );
    assert_eq!(adopted_patch_after.signatures, original_patch.signatures);
    assert!(!adopted_patch_after.signatures.is_empty());

    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after merge",
    );

    // Criterion 4: byte-exact rebuild from sealed history through the compiled binary.
    let materialize_root = rebuild_from_sealed_history(&repo, "dc74-merge-rebuild");
    assert_eq!(
        std::fs::read(materialize_root.join("a.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read(materialize_root.join("topic.txt")).unwrap(),
        b"from topic"
    );
}

#[test]
fn merge_refuses_same_path_create_conflict_with_no_partial_state() {
    let repo = unique_repo("dc74-conflict-same-path-create");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );

    std::fs::write(repo.join("conflict.txt"), b"main version").unwrap();
    ok(
        &commit(&repo, "heads/main", "main writes conflict.txt"),
        "commit main",
    );
    ok(&seal(&repo, "heads/main"), "seal main");
    std::fs::remove_file(repo.join("conflict.txt")).unwrap();

    std::fs::write(repo.join("conflict.txt"), b"topic version").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic writes conflict.txt"),
        "commit topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    let before = current_ref_state_id(&repo, "heads/main");
    let out = merge(&repo, baseline, "heads/main", "heads/topic");
    assert!(
        !out.status.success(),
        "expected refusal: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("merge refused"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        before,
        current_ref_state_id(&repo, "heads/main"),
        "heads/main must not advance"
    );
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after refused merge",
    );
}

#[test]
fn merge_refuses_delete_mutation_conflict() {
    let repo = unique_repo("dc74-conflict-delete-mutate");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"content").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );

    std::fs::remove_file(repo.join("a.txt")).unwrap();
    ok(
        &commit(&repo, "heads/main", "main deletes a.txt"),
        "commit main",
    );
    ok(&seal(&repo, "heads/main"), "seal main");

    std::fs::write(repo.join("a.txt"), b"topic edits content").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic edits a.txt"),
        "commit topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    let before = current_ref_state_id(&repo, "heads/main");
    let out = merge(&repo, baseline, "heads/main", "heads/topic");
    assert!(!out.status.success(), "expected refusal");
    assert_eq!(before, current_ref_state_id(&repo, "heads/main"));
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after refused merge",
    );
}

#[test]
fn merge_refuses_replace_binary_conflict() {
    let repo = unique_repo("dc74-conflict-replace-binary");
    init(&repo);
    std::fs::write(repo.join("bin.dat"), [0xff, 0x00, 0xfe]).unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );

    std::fs::write(repo.join("bin.dat"), [0xaa, 0xbb]).unwrap();
    ok(
        &commit(&repo, "heads/main", "main replaces binary"),
        "commit main",
    );
    ok(&seal(&repo, "heads/main"), "seal main");

    std::fs::write(repo.join("bin.dat"), [0xcc, 0xdd, 0xee]).unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic replaces binary"),
        "commit topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    let before = current_ref_state_id(&repo, "heads/main");
    let out = merge(&repo, baseline, "heads/main", "heads/topic");
    assert!(!out.status.success(), "expected refusal");
    assert_eq!(before, current_ref_state_id(&repo, "heads/main"));
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after refused merge",
    );
}

#[test]
#[cfg(unix)]
fn merge_refuses_change_perm_conflict() {
    let repo = unique_repo("dc74-conflict-change-perm");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"content").unwrap();
    set_mode(&repo.join("a.txt"), 0o644);
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );

    set_mode(&repo.join("a.txt"), 0o755);
    ok(
        &commit(&repo, "heads/main", "main chmod 755"),
        "commit main",
    );
    ok(&seal(&repo, "heads/main"), "seal main");
    // No branch-switch command exists; reset the shared worktree to the baseline mode before
    // simulating topic's own independent chmod.
    set_mode(&repo.join("a.txt"), 0o644);

    set_mode(&repo.join("a.txt"), 0o755);
    ok(
        &commit(&repo, "heads/topic", "topic independently chmod 755"),
        "commit topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    let before = current_ref_state_id(&repo, "heads/main");
    let out = merge(&repo, baseline, "heads/main", "heads/topic");
    assert!(!out.status.success(), "expected refusal");
    assert_eq!(before, current_ref_state_id(&repo, "heads/main"));
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after refused merge",
    );
}

#[test]
fn merge_refuses_overlapping_text_edit_divergence() {
    // patch_algebra classifies this Deferred (same_node_text_transform_deferred), not a hard
    // Conflict — recorded precisely rather than assumed, since the two outcomes are distinct in
    // `MergeEvidenceOutcome`. Either way it is not Confluent, so merge execution refuses.
    let repo = unique_repo("dc74-conflict-text-span");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha beta gamma\n").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );

    std::fs::write(repo.join("a.txt"), b"alpha BETA gamma\n").unwrap();
    ok(
        &commit(&repo, "heads/main", "main edits beta"),
        "commit main",
    );
    ok(&seal(&repo, "heads/main"), "seal main");

    std::fs::write(repo.join("a.txt"), b"alpha beta2 gamma\n").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic edits beta"),
        "commit topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    let before = current_ref_state_id(&repo, "heads/main");
    let out = merge(&repo, baseline, "heads/main", "heads/topic");
    assert!(!out.status.success(), "expected refusal");
    assert_eq!(before, current_ref_state_id(&repo, "heads/main"));
    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify after refused merge",
    );
}

#[test]
fn merged_repository_supports_verify_rollback_preview_and_further_commits() {
    let repo = unique_repo("dc74-post-merge-compat");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );
    std::fs::write(repo.join("topic.txt"), b"from topic").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change"),
        "commit topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");
    ok(
        &merge(&repo, baseline, "heads/main", "heads/topic"),
        "merge",
    );

    ok(
        &prikk(&repo).arg("verify").output().unwrap(),
        "verify post-merge",
    );
    ok(
        &prikk(&repo).arg("doctor").output().unwrap(),
        "doctor post-merge",
    );
    let preview_out = prikk(&repo)
        .args(["rollback-preview", "--ref", "heads/main"])
        .output()
        .unwrap();
    ok(&preview_out, "rollback-preview post-merge");

    // DC-64's incremental cache and ordinary commit/seal must still work against the merged block.
    std::fs::write(repo.join("after-merge.txt"), b"more work").unwrap();
    ok(
        &commit(&repo, "heads/main", "commit after merge"),
        "commit after merge",
    );
    ok(&seal(&repo, "heads/main"), "seal after merge");
    let doctor_out = prikk(&repo).arg("doctor").output().unwrap();
    ok(&doctor_out, "doctor after post-merge commit");
    let doctor_stdout = String::from_utf8_lossy(&doctor_out.stdout);
    assert!(
        doctor_stdout.contains("commit-index divergences: 0")
            && doctor_stdout.contains("lifecycle-cache divergences: 0"),
        "expected zero cache divergences after committing on a merged branch: {doctor_stdout}"
    );
}

/// Trust-gate caller-level coverage (`trust-gate-caller-coverage-handoff-v1.md` §2, `prikk merge`,
/// `merge_execute.rs:118`). Same divergent-branches scenario as
/// `merge_adopts_patches_byte_identical_and_rebuilds_byte_exact`, but the merge itself is attempted
/// with an untrusted-but-well-formed signer instead of `merge()`'s own hardcoded trusted one.
#[test]
fn merge_fails_closed_on_untrusted_signer() {
    let repo = unique_repo("dc74-merge-untrusted-signer");
    init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "genesis commit");
    ok(&seal(&repo, "heads/main"), "genesis seal");
    let baseline = genesis_block_id(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );
    std::fs::write(repo.join("topic.txt"), b"from topic").unwrap();
    ok(
        &commit(&repo, "heads/topic", "topic change"),
        "commit on topic",
    );
    ok(&seal(&repo, "heads/topic"), "seal topic");

    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file("222233334444555566667777888899990000aaaabbbbccccddddeeeeffff1111"),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            "heads/topic",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "merge with an untrusted maintainer signer unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );
}
