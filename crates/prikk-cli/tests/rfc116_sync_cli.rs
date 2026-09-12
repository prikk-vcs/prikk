//! RFC 116 stage 4 handoff §6 — an end-to-end two-repository sync driven entirely through the
//! `prikk sync` CLI, no direct library calls. This is criterion 1's evidence: two machines can
//! exchange sealed history and both verify it afterward, through the surface a real user actually
//! has (RFC 116 ruling 2: no network, no transport -- the artifact files just move by whatever
//! means the operator already has, simulated here by both repositories sharing a filesystem).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

mod support;

use prikk_store::{
    DEFAULT_HAVE_LIST_MAX_PATCH_COUNT, DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES, ObjectReadSnapshot,
    RepositoryLayout, decode_have_list, order_claims_for_sealing,
};

fn sync_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rfc116-sync-cli-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

/// §6, steps 1-5, driven entirely through the CLI:
/// 1. Repo A seals a patch. Repo B is empty.
/// 2. `sync summary` in A -> file. `sync compare --summary` in B -> reports the ref differs.
/// 3. `sync have` in B -> file. `sync build --have` in A -> artifact.
/// 4. `sync accept` in B -> prints a claim id. `sync seal --claim` in B.
/// 5. B's ref tip now reaches A's patch -- read back via `sync have` in B again, decoded, not
///    inferred from any subcommand's exit code.
#[test]
fn end_to_end_sync_via_the_cli_lands_the_delta_and_is_verified_by_reading_it_back() {
    let repo_a = support::unique_repo("rfc116-sync-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"rfc116 sync cli\n",
        "first",
    );

    let repo_b = support::unique_repo("rfc116-sync-b");
    support::init(&repo_b);

    // Step 2: `sync summary` in A.
    let summary_file = sync_file("summary");
    let summary = support::prikk(&repo_a)
        .args([
            "sync",
            "summary",
            "--output",
            summary_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&summary, "sync summary");
    let summary_stdout = String::from_utf8_lossy(&summary.stdout);
    assert!(
        summary_stdout.contains("refs: 1"),
        "A must report exactly one ref in its summary: {summary_stdout}"
    );

    // Step 2: `sync compare --summary` in B must report the ref differs (B has no `heads/main`
    // at all, so this also exercises the `RemoteOnly`... `LocalOnly` naming from B's perspective:
    // B does not hold the ref, so it must read `remote-only` from B's own point of view).
    let compare = support::prikk(&repo_b)
        .args([
            "sync",
            "compare",
            "--summary",
            summary_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&compare, "sync compare");
    let compare_stdout = String::from_utf8_lossy(&compare.stdout);
    assert!(
        compare_stdout.contains("heads/main remote-only"),
        "B must report heads/main as remote-only (B does not hold it yet): {compare_stdout}"
    );

    // Step 3: `sync have` in B (B holds nothing, so this is an empty have-list) -> file.
    let have_file = sync_file("have");
    let have = support::prikk(&repo_b)
        .args([
            "sync",
            "have",
            "heads/main",
            "--output",
            have_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&have, "sync have");

    // Step 3: `sync build --have` in A -> artifact. A must be able to sign the claim -- adopt and
    // use the same fixed maintainer key `support::seal` already trusted in A during setup.
    let artifact_file = sync_file("artifact");
    let build = support::prikk(&repo_a)
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
        .unwrap();
    support::ok(&build, "sync build");
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    assert!(
        build_stdout.contains("delta patches: 1") && build_stdout.contains("claims: 1"),
        "A must report exactly one delta patch and one claim: {build_stdout}"
    );
    // RFC 116 stage 7 handoff, Part B3: the confidentiality notice appears whenever an artifact is
    // actually written.
    assert!(
        build_stdout.contains("does not encrypt it"),
        "sync build must print the confidentiality notice when it writes an artifact: {build_stdout}"
    );

    // Step 4: `sync accept` in B -> prints a claim id, the load-bearing output this step exists
    // for (handoff §3).
    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let accept_stdout = String::from_utf8_lossy(&accept.stdout);
    assert!(
        accept_stdout.contains("patches: 1") && accept_stdout.contains("claims: 1"),
        "B must report exactly one accepted patch and one claim: {accept_stdout}"
    );
    let claim_line = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .unwrap_or_else(|| panic!("sync accept must print a claim line: {accept_stdout}"));
    let claim_id = claim_line
        .trim_start()
        .strip_prefix("claim ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("could not parse a claim id from: {claim_line}"))
        .trim();

    // `sync pending` in B must show the accepted-but-unsealed patch before sealing -- the only
    // observable evidence an accept did anything, per handoff §3.
    let pending = support::prikk(&repo_b)
        .args(["sync", "pending"])
        .output()
        .unwrap();
    support::ok(&pending, "sync pending");
    let pending_stdout = String::from_utf8_lossy(&pending.stdout);
    assert!(
        pending_stdout.contains("pending (accepted, unsealed) patches: 1"),
        "B must show exactly one pending patch before sealing: {pending_stdout}"
    );

    // Step 4: `sync seal --claim` in B. B needs its own trusted maintainer key to seal under --
    // the same fixed key, adopted here explicitly since B never ran `seal` itself.
    support::trust_maintainer(&repo_b);
    let seal = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/main", "--claim", claim_id])
        .output()
        .unwrap();
    support::ok(&seal, "sync seal");
    let seal_stdout = String::from_utf8_lossy(&seal.stdout);
    assert!(
        seal_stdout.contains("sealed 1 patch(es)"),
        "B must report sealing exactly one patch: {seal_stdout}"
    );

    // Pending must now be empty -- the sealed patch is no longer accepted-but-unsealed.
    let pending_after = support::prikk(&repo_b)
        .args(["sync", "pending"])
        .output()
        .unwrap();
    support::ok(&pending_after, "sync pending after seal");
    assert!(
        String::from_utf8_lossy(&pending_after.stdout)
            .contains("pending (accepted, unsealed) patches: 0"),
        "no patch should remain pending after seal"
    );

    // Step 5, the assertion that matters: read B's own ref tip back through the CLI (`sync have`
    // again), decode it, and confirm the synced patch is reachable -- not inferred from any exit
    // code above.
    let have_after_file = sync_file("have-after");
    let have_after = support::prikk(&repo_b)
        .args([
            "sync",
            "have",
            "heads/main",
            "--output",
            have_after_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&have_after, "sync have after seal");
    let have_after_bytes = std::fs::read(&have_after_file).unwrap();
    let have_after_list = decode_have_list(
        &have_after_bytes,
        DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
        DEFAULT_HAVE_LIST_MAX_PATCH_COUNT,
    )
    .unwrap();
    assert_eq!(
        have_after_list.patch_ids.len(),
        1,
        "B's own heads/main must now reach exactly the one synced patch"
    );

    // And a final `sync compare` from B's own summary against A's must read in-sync.
    let a_summary_again = sync_file("summary-final");
    let summary_again = support::prikk(&repo_a)
        .args([
            "sync",
            "summary",
            "--output",
            a_summary_again.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&summary_again, "sync summary (final)");
    let compare_final = support::prikk(&repo_b)
        .args([
            "sync",
            "compare",
            "--summary",
            a_summary_again.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&compare_final, "sync compare (final)");
    assert!(
        String::from_utf8_lossy(&compare_final.stdout).contains("heads/main in-sync"),
        "after the full loop, B must report heads/main as in-sync with A"
    );

    // Handoff §3/§5: on `AlreadyInSync`, `sync build` must print that and write **no** output
    // file -- not an empty artifact, and not an error. B now genuinely holds the exact same
    // patch object A does (received through the sync it just completed, not independently
    // reconstructed -- `prikk commit`'s own `NodeId` assignment is randomized per invocation, so
    // two separately-committed copies of identical file content would never actually match; the
    // real sync loop is the only way to get two repositories holding the *same* patch object).
    let already_in_sync_output = sync_file("already-in-sync-artifact");
    assert!(
        !already_in_sync_output.exists(),
        "the output path must not exist before the subject call below runs"
    );
    let build_again = support::prikk(&repo_a)
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
            have_after_file.to_str().unwrap(),
            "--output",
            already_in_sync_output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&build_again, "sync build (already in sync)");
    let build_again_stdout = String::from_utf8_lossy(&build_again.stdout);
    assert!(
        build_again_stdout.contains("already in sync"),
        "must report already-in-sync plainly: {build_again_stdout}"
    );
    assert!(
        !already_in_sync_output.exists(),
        "an already-in-sync build must write no output file: {build_again_stdout}"
    );
    // RFC 116 stage 7 handoff, Part B3: the confidentiality notice is absent on the AlreadyInSync
    // path, where no file is created -- the notice belongs to the write, not the subcommand.
    assert!(
        !build_again_stdout.contains("does not encrypt it"),
        "an already-in-sync build must not print the confidentiality notice, since it writes \
         nothing: {build_again_stdout}"
    );

    // RFC 116 stage 6 handoff §1: badge criterion 1's load-bearing clause is "both verify it
    // afterward" -- asserted nowhere until now. The sender is not a formality: it produced the
    // artifacts and its own state must still be sound afterwards, so both sides are checked.
    support::ok(&support::verify(&repo_a), "verify (A, sender)");
    support::ok(&support::verify(&repo_b), "verify (B, receiver)");

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    for file in [
        summary_file,
        have_file,
        artifact_file,
        have_after_file,
        a_summary_again,
    ] {
        let _ = std::fs::remove_file(file);
    }
}

/// RFC 116 stage 5 handoff §4 row 7 / §6 item 3, the case the whole sync arc had never exercised:
/// every prior end-to-end sync test was single-block. A sends **two** sealed blocks where the
/// second genuinely depends on the first -- the same path, edited a second time, so applying the
/// edit before the create would fail outright, not merely land in an unintended order -- and the
/// artifact therefore carries two claims, accepted and sealed as a batch via
/// `sync seal <ref> --claims <file>`. The order comes from `order_claims_for_sealing`, not from
/// the artifact's own claim order (which the handoff itself confirmed is
/// `ancestors_inclusive`'s `BTreeMap` order, sorted by block id, not lineage). The receiver's ref
/// tip must reach both patches afterward.
#[test]
fn row7_multi_block_sync_completes_through_the_cli_alone() {
    let repo_a = support::unique_repo("rfc116-order-row7-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "shared.txt",
        b"first block\n",
        "first",
    );
    support::generation(
        &repo_a,
        "heads/main",
        "shared.txt",
        b"second block, edited\n",
        "second",
    );

    let repo_b = support::unique_repo("rfc116-order-row7-b");
    support::init(&repo_b);

    let have_file = sync_file("row7-have");
    support::ok(
        &support::prikk(&repo_b)
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

    let artifact_file = sync_file("row7-artifact");
    let build = support::prikk(&repo_a)
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
        .unwrap();
    support::ok(&build, "sync build");
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    assert!(
        build_stdout.contains("delta patches: 2") && build_stdout.contains("claims: 2"),
        "the two-block delta must carry exactly two patches and two claims: {build_stdout}"
    );

    let claims_file = sync_file("row7-claims");
    let accept = support::prikk(&repo_b)
        .args([
            "sync",
            "accept",
            artifact_file.to_str().unwrap(),
            "--claims-out",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    assert!(
        String::from_utf8_lossy(&accept.stdout).contains("claims: 2"),
        "B must accept exactly two claims"
    );
    let claim_ids_written = std::fs::read_to_string(&claims_file).unwrap();
    let claim_lines: Vec<&str> = claim_ids_written
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(
        claim_lines.len(),
        2,
        "the claims file must carry exactly two claim ids: {claim_ids_written:?}"
    );
    // `sync accept` writes claims in the artifact's own order, which the handoff itself confirmed
    // is `ancestors_inclusive`'s `BTreeMap` order (sorted by block id) -- not necessarily
    // topological, and possibly *already* topological by chance. A control that removes the sort
    // must not depend on that incidental order agreeing or disagreeing with lineage, or it would
    // pass by luck rather than correctness -- the exact trap that made a control a no-op twice
    // before in this arc. So the correct order is computed independently here (an oracle call to
    // the same production function, over B's own object store) and its *reverse* is written back
    // -- deterministically the wrong order for a two-claim batch, regardless of what `sync accept`
    // happened to emit.
    let claim_ids: Vec<prikk_object::ObjectId> = claim_lines
        .iter()
        .map(|line| line.trim().parse().unwrap())
        .collect();
    let receiver_layout = RepositoryLayout::open(repo_b.clone()).unwrap();
    let receiver_object_store = ObjectReadSnapshot::open(&receiver_layout).unwrap();
    let correct_order = order_claims_for_sealing(&receiver_object_store, &claim_ids).unwrap();
    assert_eq!(correct_order.len(), 2);
    let deliberately_wrong_order: String = correct_order
        .iter()
        .rev()
        .map(|id| format!("{id}\n"))
        .collect();
    std::fs::write(&claims_file, deliberately_wrong_order).unwrap();

    support::trust_maintainer(&repo_b);
    let seal = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "seal",
            "heads/main",
            "--claims",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&seal, "sync seal --claims");
    let seal_stdout = String::from_utf8_lossy(&seal.stdout);
    assert_eq!(
        seal_stdout.matches("sealed 1 patch(es)").count(),
        2,
        "both claims must be sealed, one patch each: {seal_stdout}"
    );

    // The assertion that matters: read B's own ref tip back and confirm both patches are
    // reachable from it.
    let have_after_file = sync_file("row7-have-after");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_after_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have after seal",
    );
    let have_after_bytes = std::fs::read(&have_after_file).unwrap();
    let have_after_list = decode_have_list(
        &have_after_bytes,
        DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
        DEFAULT_HAVE_LIST_MAX_PATCH_COUNT,
    )
    .unwrap();
    assert_eq!(
        have_after_list.patch_ids.len(),
        2,
        "B's own heads/main must reach both of A's patches after the batch seal"
    );

    let pending_after = support::prikk(&repo_b)
        .args(["sync", "pending"])
        .output()
        .unwrap();
    support::ok(&pending_after, "sync pending after batch seal");
    assert!(
        String::from_utf8_lossy(&pending_after.stdout)
            .contains("pending (accepted, unsealed) patches: 0"),
        "no patch should remain pending after the batch seal"
    );

    // RFC 116 stage 6 handoff §1: both sides must verify after a multi-block sync too, not only
    // the single-block case above.
    support::ok(&support::verify(&repo_a), "verify (A, sender)");
    support::ok(&support::verify(&repo_b), "verify (B, receiver)");

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    for file in [have_file, artifact_file, claims_file, have_after_file] {
        let _ = std::fs::remove_file(file);
    }
}

/// RFC 116 stage 5 handoff §4 row 6: a mid-batch failure stops, reports, and leaves the earlier
/// seal intact -- no rollback. Two independent (sibling, not parent/child) single-patch blocks
/// both create the same path with different content; whichever claim's order-tie-break puts it
/// first seals cleanly, and the second hits Stage 4's own divergence refusal when it tries to
/// create the same path again. `sync seal --claims` must report the failure, exit non-zero, and
/// leave the first seal in place -- not roll it back.
#[test]
fn row6_a_mid_batch_seal_failure_stops_reports_and_leaves_the_earlier_seal_intact() {
    let repo_a1 = support::unique_repo("rfc116-order-row6-a1");
    support::init(&repo_a1);
    support::generation(&repo_a1, "heads/main", "collide.txt", b"from a1\n", "a1");

    let repo_a2 = support::unique_repo("rfc116-order-row6-a2");
    support::init(&repo_a2);
    support::generation(&repo_a2, "heads/main", "collide.txt", b"from a2\n", "a2");

    let repo_b = support::unique_repo("rfc116-order-row6-b");
    support::init(&repo_b);

    let claims_file = sync_file("row6-claims");
    let mut combined_claim_ids = String::new();

    for (tag, sender) in [("a1", &repo_a1), ("a2", &repo_a2)] {
        let have_file = sync_file(&format!("row6-have-{tag}"));
        support::ok(
            &support::prikk(&repo_b)
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
        let artifact_file = sync_file(&format!("row6-artifact-{tag}"));
        support::ok(
            &support::prikk(sender)
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
        let claims_out = sync_file(&format!("row6-claims-out-{tag}"));
        support::ok(
            &support::prikk(&repo_b)
                .args([
                    "sync",
                    "accept",
                    artifact_file.to_str().unwrap(),
                    "--claims-out",
                    claims_out.to_str().unwrap(),
                ])
                .output()
                .unwrap(),
            "sync accept",
        );
        combined_claim_ids.push_str(&std::fs::read_to_string(&claims_out).unwrap());
        let _ = std::fs::remove_file(have_file);
        let _ = std::fs::remove_file(artifact_file);
        let _ = std::fs::remove_file(claims_out);
    }
    std::fs::write(&claims_file, &combined_claim_ids).unwrap();

    support::trust_maintainer(&repo_b);
    let seal = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "seal",
            "heads/main",
            "--claims",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !seal.status.success(),
        "a mid-batch seal failure must exit non-zero"
    );
    let seal_stdout = String::from_utf8_lossy(&seal.stdout);
    assert!(
        seal_stdout.contains("sealed 1 patch(es)"),
        "the first claim must still be reported as sealed: {seal_stdout}"
    );
    assert!(
        seal_stdout.contains("FAILED"),
        "the second claim's failure must be reported, not silently swallowed: {seal_stdout}"
    );

    // The assertion that matters: exactly one patch landed, not zero and not two -- the first
    // seal was left in place, not rolled back, and the second was never applied.
    let have_after_file = sync_file("row6-have-after");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_after_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have after partial batch seal",
    );
    let have_after_bytes = std::fs::read(&have_after_file).unwrap();
    let have_after_list = decode_have_list(
        &have_after_bytes,
        DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
        DEFAULT_HAVE_LIST_MAX_PATCH_COUNT,
    )
    .unwrap();
    assert_eq!(
        have_after_list.patch_ids.len(),
        1,
        "exactly one patch must have landed -- the earlier seal intact, not rolled back, and the \
         failed one never applied"
    );

    let pending_after = support::prikk(&repo_b)
        .args(["sync", "pending"])
        .output()
        .unwrap();
    support::ok(&pending_after, "sync pending after partial batch seal");
    assert!(
        String::from_utf8_lossy(&pending_after.stdout)
            .contains("pending (accepted, unsealed) patches: 1"),
        "the failed claim's patch must remain accepted-but-unsealed"
    );

    let _ = std::fs::remove_dir_all(repo_a1);
    let _ = std::fs::remove_dir_all(repo_a2);
    let _ = std::fs::remove_dir_all(repo_b);
    for file in [claims_file, have_after_file] {
        let _ = std::fs::remove_file(file);
    }
}

const UNTRUSTED_MAINTAINER_SEED: &str =
    "222233334444555566667777888899990000aaaabbbbccccddddeeeeffff1111";

/// Trust-gate caller-level coverage (`trust-gate-caller-coverage-handoff-v1.md` §2, `prikk sync
/// build`, `sender.rs:201`). Same opening as the end-to-end test above (A seals a patch, B is
/// empty), through `sync have` in B -- then `sync build` in A is attempted with an
/// untrusted-but-well-formed signer instead of the trusted fixed key.
#[test]
fn sync_build_fails_closed_on_untrusted_signer() {
    let repo_a = support::unique_repo("rfc116-sync-build-untrusted");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"rfc116 sync build trust gate\n",
        "first",
    );

    let repo_b = support::unique_repo("rfc116-sync-build-untrusted-b");
    support::init(&repo_b);

    let have_file = sync_file("have");
    support::ok(
        &support::prikk(&repo_b)
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

    let artifact_file = sync_file("artifact");
    let out = support::prikk(&repo_a)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(UNTRUSTED_MAINTAINER_SEED),
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
        .unwrap();
    assert!(
        !out.status.success(),
        "sync build with an untrusted maintainer signer unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    let _ = std::fs::remove_file(have_file);
}

/// Trust-gate caller-level coverage (`trust-gate-caller-coverage-handoff-v1.md` §2, `prikk sync
/// seal`, `seal_from_accepted.rs:160`). Reuses the full accept scenario (A seals, B accepts a real
/// claim) so `sync seal --claim` has genuine prior state to operate on, then attempts the seal with
/// an untrusted-but-well-formed signer instead of B's own trusted fixed key.
#[test]
fn sync_seal_fails_closed_on_untrusted_signer() {
    let repo_a = support::unique_repo("rfc116-sync-seal-untrusted-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"rfc116 sync seal trust gate\n",
        "first",
    );

    let repo_b = support::unique_repo("rfc116-sync-seal-untrusted-b");
    support::init(&repo_b);

    let have_file = sync_file("have");
    support::ok(
        &support::prikk(&repo_b)
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

    let artifact_file = sync_file("artifact");
    support::ok(
        &support::prikk(&repo_a)
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

    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let accept_stdout = String::from_utf8_lossy(&accept.stdout);
    let claim_line = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .unwrap_or_else(|| panic!("sync accept must print a claim line: {accept_stdout}"));
    let claim_id = claim_line
        .trim_start()
        .strip_prefix("claim ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("could not parse a claim id from: {claim_line}"))
        .trim()
        .to_string();

    // A different key is trusted first, so the policy is non-empty and the refusal below is a
    // policy decision, not a missing-policy accident.
    support::trust_maintainer(&repo_b);
    let out = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(UNTRUSTED_MAINTAINER_SEED),
        )
        .args(["sync", "seal", "heads/main", "--claim", &claim_id])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "sync seal with an untrusted maintainer signer unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    for file in [have_file, artifact_file] {
        let _ = std::fs::remove_file(file);
    }
}
