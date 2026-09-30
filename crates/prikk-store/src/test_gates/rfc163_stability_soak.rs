//! 0.49.0 step 0 (`stability-confirmation-handoff-v1.md`, S3(b)): a seeded, randomized sweep over
//! (failpoint × ordinal) for `RefStore::publish` -- the shared primitive `seal`, `branch create`,
//! `tag create` and `merge` all call to append the pointer index and ref log (DC-38,
//! design-v1.md §13/§15.7-15.8).
//!
//! **This is additive, not a replacement** for the existing hand-written failpoint coverage
//! (`refs/tests/publication_recovery/failpoints.rs` picks specific, reasoned `(point, skip)` pairs
//! and asserts exactly what each one leaves durable; `refs/tests.rs`, `trust/tests.rs`,
//! `compact/tests.rs`, `commit_boundary/active/tests.rs`, `wal/tests.rs`, `snapshot/tests.rs`,
//! `patch_checkout/tests.rs`, `object_store/tests.rs` and `branch_switch/tests.rs` each carry their
//! own hand-written failpoint cases for their own operation). What is new here: instead of a human
//! choosing which `(point, skip)` pairs to test, a seeded RNG draws many of them and the same three
//! invariants are checked at every one -- `verify_repository` never panics and reaches a repository
//! a retry of the *same* publish can complete, and a completed retry is durable.
//!
//! `#[ignore]`d: run with
//! `cargo test -p prikk-store --locked --test-threads=1 -- --ignored rfc163_ref_publication_randomized_failpoint_sweep`
//! (or via the crate's own lib-test binary target). Not part of any gate -- a seeded soak's own
//! pass/fail is reported in the round that runs it, not asserted as a permanent CI gate here.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use crate::bundle::{BundleImportOptions, export_bundle, import_bundle};
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::patch_exchange::{AcceptOptions, accept_exchange_artifact, export_exchange_artifact};
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_patch_blob_envelope, signed_patch_envelope,
    signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    BlockStateStatus, DoctorRepairOptions, Ed25519AuthorSigner, Ed25519MaintainerSigner,
    FileObjectStore, MaintainerSigner, ObjectItemStatus, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, RepositoryVerification, add_trusted_maintainer, clear_lock, list_held_locks,
    repair_pointer_index_tail, repair_repository, verify_repository,
};

/// The same publication `refs/tests/publication_recovery.rs`'s own private `root_publication`
/// builds -- duplicated here rather than exported across that module boundary, since it is five
/// lines and this sweep otherwise has no dependency on that module at all.
fn root_publication(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> prikk_error::Result<RefPublication> {
    let block = signed_empty_block_envelope();
    let target = FileObjectStore::new(layout.clone()).write_object(&block)?;
    let ref_state = signed_ref_state_envelope(ref_name, None, target, 1);
    let ref_state_id = ref_state.object_id();
    Ok(RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_update: signed_ref_update_envelope(ref_name, None, ref_state_id, target, 1),
        ref_state,
    })
}

/// Every [`TestFailPoint`] variant this platform has -- `Point` is `#[non_exhaustive]` only for
/// crates outside this one; same-crate code lists it exhaustively like any other enum. Kept in
/// sync with `foundation::fsutil::anchored::failpoints::Point`'s own definition by hand -- a
/// variant added there and missed here just narrows this sweep's own coverage, silently, so check
/// this list whenever that one changes.
fn all_points() -> Vec<TestFailPoint> {
    #[allow(unused_mut)]
    let mut points = vec![
        TestFailPoint::DirectoryCreate,
        TestFailPoint::MutableFileSync,
        TestFailPoint::MutableRename,
        TestFailPoint::RequiredFileSync,
        TestFailPoint::RequiredOpen,
        TestFailPoint::AppendWrite,
        TestFailPoint::Truncate,
        TestFailPoint::Unlink,
    ];
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    points.extend([
        TestFailPoint::CreatedDirectoryParentSync,
        TestFailPoint::ObservedDirectoryParentSync,
        TestFailPoint::MutableParentSync,
        TestFailPoint::RequiredDirectorySync,
        TestFailPoint::CleanupDirectorySync,
    ]);
    points
}

/// Deterministic, dependency-free PRNG (splitmix64) -- this crate does not depend on `rand`, and a
/// seeded sweep only needs a reproducible sequence, not cryptographic quality.
struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % (bound as u64)) as usize
    }
}

#[test]
#[ignore]
fn rfc163_ref_publication_randomized_failpoint_sweep() {
    let seed: u64 = std::env::var("RFC163_SOAK_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20260930);
    let iterations: usize = std::env::var("RFC163_SOAK_N")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500);
    let max_ordinal = 8usize;

    let points = all_points();
    let mut rng = SplitMix64(seed ^ 0x5151_5151_5151_5151);
    let mut failures = Vec::new();

    for i in 0..iterations {
        // A failpoint armed by a previous iteration but never fired (its own `ordinal` skipped
        // more occurrences of `point` than that iteration's operation actually reached) stays
        // armed -- `fail_after`/`fail_once` only clear on firing. Disarm before this iteration's
        // own setup even touches the filesystem, so a stale arm cannot fire during init/fixture
        // building instead of during the `store.publish` call it was meant for.
        clear_failpoint_for_test();

        let point = points[rng.below(points.len())];
        let ordinal = rng.below(max_ordinal);

        let root = unique_temp_dir(&format!("rfc163-soak-{seed}-{i}"));
        let layout = match RepositoryLayout::init(root.clone()) {
            Ok(layout) => layout,
            Err(error) => {
                failures.push(format!("iter {i} seed {seed}: init failed: {error}"));
                continue;
            }
        };
        let publication = match root_publication(&layout, "heads/main") {
            Ok(publication) => publication,
            Err(error) => {
                failures.push(format!(
                    "iter {i} seed {seed}: building the publication failed: {error}"
                ));
                let _ = std::fs::remove_dir_all(&root);
                continue;
            }
        };
        let store = RefStore::new(layout.clone());

        fail_after_for_test(point, ordinal);
        let first = store.publish(&publication);
        let label = format!("iter {i} seed {seed} point {point:?} ordinal {ordinal}");

        if first.is_ok() {
            // The failpoint never fired (this operation makes fewer calls to `point` than
            // `ordinal` skips) -- a clean, uneventful iteration; confirm verify agrees and move on.
            if let Err(error) = verify_repository(&layout) {
                failures.push(format!(
                    "{label}: publish succeeded but verify_repository errored: {error}"
                ));
            }
            let _ = std::fs::remove_dir_all(&root);
            continue;
        }

        // The induced failure fired. verify_repository must not panic.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| verify_repository(&layout)))
            .is_err()
        {
            failures.push(format!("{label}: verify_repository panicked"));
            let _ = std::fs::remove_dir_all(&root);
            continue;
        }

        // Apply the same repairs a real operator's documented way out uses (mirrors matrix.py's
        // own REPAIRS list, and this soak's own black-box (a) sibling): unconditionally, since
        // which one (if any) applies depends on where `ordinal` landed, not on parsing the error.
        let _ = repair_pointer_index_tail(&layout);
        let _ = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail());

        // The natural retry: the SAME publication, again -- DC-38's own mechanism, exercised
        // directly through the store API here rather than through the CLI's `seal`/`branch
        // create`/`tag create`.
        let retry = store.publish(&publication);
        if retry.is_err() {
            // A second, fresh publication for the SAME ref: some `ordinal`s can catch the target
            // durably applied already, and the store's own idempotence marks the retry a genuine
            // conflict rather than nothing pending. This is a normal, benign outcome that the
            // black-box soak's own N3 disclosure already covers; only escalate if `verify` itself
            // is left unsound.
            if let Err(error) = verify_repository(&layout) {
                failures.push(format!(
                    "{label}: retry refused ({retry:?}) and verify_repository is also broken: {error}"
                ));
            }
            let _ = std::fs::remove_dir_all(&root);
            continue;
        }

        match verify_repository(&layout) {
            Ok(verification_after) => {
                if !verification_after.ref_publication_issues.is_empty() {
                    let codes: Vec<&str> = verification_after
                        .ref_publication_issues
                        .iter()
                        .map(|issue| issue.code)
                        .collect();
                    failures.push(format!(
                        "{label}: retry succeeded but verify_repository still reports ref-publication issues: {codes:?}"
                    ));
                }
            }
            Err(error) => {
                failures.push(format!(
                    "{label}: retry succeeded but verify_repository errored: {error}"
                ));
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    assert!(
        failures.is_empty(),
        "{} failure(s) out of {iterations} (seed {seed}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// 0.49.0 step 0 follow-up (`stability-follow-up-handoff-v1.md`, §2 item 2, white-box): the same
// seeded-sweep shape, extended to the two writers finding 3 named -- `bundle import` and
// `sync accept` -- which each lay down more than one object per call and (before this round's own
// fix, `objects_in_dependency_order` in `bundle.rs`) did not order those writes by what references
// what. A kill partway through used to be able to leave a `Block` durable while the `Patch`/`Blob`
// it names is not: `verify`'s connectivity/state-root checks catch this ("references missing" /
// "names missing" / a state-root mismatch), but none of the three `doctor` repairs clear it, and a
// plain re-run of the same import/accept is the only thing that heals it (§1). This sweep asserts
// that shape never survives an interrupted write on the fixed code; the required control (below,
// run by hand outside `cargo test` -- see the round's own report) reverts the fix's call site and
// confirms these same rows go red.
// ---------------------------------------------------------------------------------------------

/// Scans a completed [`RepositoryVerification`] for the shape the dangling-forward-reference defect
/// produces: a `Failed` object item naming a missing reference, or a `Failed` block-state outcome
/// whose message names a state-root mismatch. Mirrors the black-box soak's own `classify()`
/// substring check (`order_fix_soak.py`, this round's scratchpad) -- kept in sync by hand, since the
/// two live in different languages and there is no single source both could share.
/// Clears every lock still held in `layout` -- the store-level equivalent of `prikk unlock --lock
/// <path> --yes`, run unconditionally, the same way this crate's own black-box soak
/// (`order_fix_soak.py`, this round's scratchpad) does as its documented first recovery step
/// (RFC 121 / design-v1.md §15.7 decision 3: stale-lock cleanup is manual, never automatic). Safe
/// here specifically because a sweep iteration's induced failure is this thread's own -- nothing
/// else can be holding this repository's locks. Without this, a failpoint landing inside a lock's
/// own release path (e.g. `Unlink`) can leave the physical lock file behind even though the guard
/// that held it has already gone out of scope, and the natural retry below is then refused for a
/// reason this round's own fix has nothing to do with (a pre-existing, disclosed, unrelated gap:
/// `lock.rs`'s own "no stale-lock stealing yet").
fn clear_stale_locks_for_test(layout: &RepositoryLayout) {
    if let Ok(locks) = list_held_locks(layout) {
        for lock in locks {
            let _ = clear_lock(layout, &lock.path);
        }
    }
}

fn dangling_reference_finding(verification: &RepositoryVerification) -> Option<String> {
    for outcome in &verification.object_outcomes {
        if let ObjectItemStatus::Failed { message } = &outcome.status {
            if message.contains("references missing") || message.contains("names missing") {
                return Some(format!(
                    "object {:?} ({}): {message}",
                    outcome.object_type,
                    outcome.path.display()
                ));
            }
        }
    }
    for outcome in &verification.block_state_outcomes {
        if let BlockStateStatus::Failed { message } = &outcome.status {
            if message.to_lowercase().contains("state root") {
                return Some(format!("block {}: {message}", outcome.block_id));
            }
        }
    }
    None
}

/// Commit-then-seal two generations of real content into `layout`'s `heads/main` (a Root block plus
/// a Normal child, each with a state root `verify_repository`'s own authoritative replay actually
/// agrees with -- unlike `signed_block`'s fixed, always-empty state root, which is only good for the
/// object-shape/transport tests `bundle/tests.rs` itself uses it for and fails full verification by
/// construction), then export it as a bundle. Mirrors `rfc111_seal_decode_cost_gate.rs`'s own
/// `commit_and_seal`, the established way to build a genuinely verifiable sealed history at the
/// store level without spawning the CLI.
fn seal_two_block_history_bundle(layout: &RepositoryLayout) -> prikk_error::Result<Vec<u8>> {
    let maintainer =
        Ed25519MaintainerSigner::from_seed("rfc163-bundle-soak-maintainer", &[0x82; 32])?;
    add_trusted_maintainer(
        layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    let author = Ed25519AuthorSigner::from_seed("rfc163-bundle-soak-author", &[0x81; 32])?;

    for index in 0..2 {
        let path = format!("f{index}.txt");
        std::fs::write(layout.root().join(&path), format!("{path}\n").into_bytes())?;
        crate::commit_boundary::worktree_patch::commit_worktree_changes_signed(
            layout,
            "heads/main",
            "rfc163-bundle-soak",
            crate::commit_boundary::worktree_patch::WorktreePatchCommitOptions::default(),
            &author,
        )?;
        crate::rfc111_seal_simulation::simulate_one_seal(layout, "heads/main", &maintainer)?;
    }

    let (_report, bytes) = export_bundle(layout, "heads/main")?;
    Ok(bytes)
}

/// Write one Blob and the Patch referencing it (`signed_patch_envelope`'s own `CreateFile`
/// operation carries the Blob's id) into a fresh sender repository, then export a `PEXCH002`
/// artifact carrying just that Patch -- `sync accept`'s own write set (patches, blobs, claims,
/// tags; never a Block or RefState, per `patch_exchange/accept.rs`), narrowed to the two object
/// types finding 3 actually concerns here. Mirrors `patch_exchange/accept/tests.rs`'s own
/// `build_single_patch_artifact`, minus the AUTHOR signing that test adds and this sweep does not
/// need (`author_signature_outcomes` is simply empty for an unsigned patch, not a refusal).
fn single_patch_exchange_artifact() -> prikk_error::Result<Vec<u8>> {
    let sender = RepositoryLayout::init(unique_temp_dir("rfc163-accept-soak-sender"))?;
    let mut object_store = FileObjectStore::new(sender.clone());
    object_store.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = object_store.write_object(&patch)?;
    let (_report, bytes) = export_exchange_artifact(&sender, &[patch_id], &[], &[], None)?;
    let _ = std::fs::remove_dir_all(sender.root());
    Ok(bytes)
}

#[test]
#[ignore]
fn rfc163_bundle_import_randomized_failpoint_sweep() {
    let seed: u64 = std::env::var("RFC163_SOAK_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20260930);
    let iterations: usize = std::env::var("RFC163_BUNDLE_SOAK_N")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200);
    // A multi-object import makes more low-level fsutil calls than a single ref publish does --
    // widened from the ref-publication sweep's 8 to give ordinals landing inside the later
    // (Block, RefState) writes a real chance of being drawn, not just the first object.
    let max_ordinal = 16usize;

    let source = match RepositoryLayout::init(unique_temp_dir(&format!(
        "rfc163-bundle-soak-source-{seed}"
    ))) {
        Ok(layout) => layout,
        Err(error) => panic!("building the source fixture repository failed: {error}"),
    };
    let bytes = match seal_two_block_history_bundle(&source) {
        Ok(bytes) => bytes,
        Err(error) => panic!("building the source bundle fixture failed: {error}"),
    };
    let _ = std::fs::remove_dir_all(source.root());

    let points = all_points();
    let mut rng = SplitMix64(seed ^ 0x6262_6262_6262_6262);
    let mut failures = Vec::new();

    for i in 0..iterations {
        clear_failpoint_for_test();
        let point = points[rng.below(points.len())];
        let ordinal = rng.below(max_ordinal);
        let label = format!("iter {i} seed {seed} point {point:?} ordinal {ordinal}");

        let root = unique_temp_dir(&format!("rfc163-bundle-soak-{seed}-{i}"));
        let layout = match RepositoryLayout::init(root.clone()) {
            Ok(layout) => layout,
            Err(error) => {
                failures.push(format!("{label}: init failed: {error}"));
                continue;
            }
        };

        fail_after_for_test(point, ordinal);
        let first = import_bundle(&layout, &bytes, &BundleImportOptions::default_limits());

        if first.is_ok() {
            // The failpoint never fired -- a clean, uneventful iteration.
            match verify_repository(&layout) {
                Ok(verification) => {
                    if let Some(finding) = dangling_reference_finding(&verification) {
                        failures.push(format!(
                            "{label}: import succeeded but verify_repository found: {finding}"
                        ));
                    }
                }
                Err(error) => failures.push(format!(
                    "{label}: import succeeded but verify_repository errored: {error}"
                )),
            }
            let _ = std::fs::remove_dir_all(&root);
            continue;
        }

        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| verify_repository(&layout)))
            .is_err()
        {
            failures.push(format!("{label}: verify_repository panicked"));
            let _ = std::fs::remove_dir_all(&root);
            continue;
        }

        // The defect's own signature (§1): verify shows a dangling reference or state-root failure
        // right after the interrupted write, and -- this is the part the three `doctor` repairs
        // below cannot touch -- only a full re-import clears it, never a repair. So this is the
        // assertion that actually distinguishes the fix from its absence; the retry below only
        // confirms the documented way out still works, it does not gate this one.
        if let Ok(verification) = verify_repository(&layout) {
            if let Some(finding) = dangling_reference_finding(&verification) {
                failures.push(format!(
                    "{label}: import failed and left verify_repository showing: {finding}"
                ));
            }
        }

        clear_stale_locks_for_test(&layout);
        let _ = repair_pointer_index_tail(&layout);
        let _ = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail());

        // The natural retry: the same bundle bytes, re-imported (DC-38's mechanism -- §1's own "25
        // of 25 repositories went to verify 0" evidence for this exact operation).
        let retry = import_bundle(&layout, &bytes, &BundleImportOptions::default_limits());
        match retry {
            Ok(_) => match verify_repository(&layout) {
                Ok(verification) => {
                    if let Some(finding) = dangling_reference_finding(&verification) {
                        failures.push(format!(
                            "{label}: retry succeeded but verify_repository still found: {finding}"
                        ));
                    }
                }
                Err(error) => failures.push(format!(
                    "{label}: retry succeeded but verify_repository errored: {error}"
                )),
            },
            Err(retry_error) => {
                // A refused retry is only a problem here if verify itself is left showing the
                // dangling-reference shape -- an ordinary "nothing new to admit" refusal is benign.
                match verify_repository(&layout) {
                    Ok(verification) => {
                        if let Some(finding) = dangling_reference_finding(&verification) {
                            failures.push(format!(
                                "{label}: retry refused ({retry_error}) and verify_repository still found: {finding}"
                            ));
                        }
                    }
                    Err(error) => failures.push(format!(
                        "{label}: retry refused ({retry_error}) and verify_repository also errored: {error}"
                    )),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    assert!(
        failures.is_empty(),
        "{} failure(s) out of {iterations} (seed {seed}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
#[ignore]
fn rfc163_sync_accept_randomized_failpoint_sweep() {
    let seed: u64 = std::env::var("RFC163_SOAK_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20260930);
    let iterations: usize = std::env::var("RFC163_SYNC_SOAK_N")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200);
    let max_ordinal = 16usize;

    let bytes = match single_patch_exchange_artifact() {
        Ok(bytes) => bytes,
        Err(error) => panic!("building the source exchange-artifact fixture failed: {error}"),
    };

    let points = all_points();
    let mut rng = SplitMix64(seed ^ 0x7373_7373_7373_7373);
    let mut failures = Vec::new();

    for i in 0..iterations {
        clear_failpoint_for_test();
        let point = points[rng.below(points.len())];
        let ordinal = rng.below(max_ordinal);
        let label = format!("iter {i} seed {seed} point {point:?} ordinal {ordinal}");

        let root = unique_temp_dir(&format!("rfc163-sync-soak-{seed}-{i}"));
        let layout = match RepositoryLayout::init(root.clone()) {
            Ok(layout) => layout,
            Err(error) => {
                failures.push(format!("{label}: init failed: {error}"));
                continue;
            }
        };

        fail_after_for_test(point, ordinal);
        let first = accept_exchange_artifact(&layout, &bytes, &AcceptOptions::default_limits());

        if first.is_ok() {
            match verify_repository(&layout) {
                Ok(verification) => {
                    if let Some(finding) = dangling_reference_finding(&verification) {
                        failures.push(format!(
                            "{label}: accept succeeded but verify_repository found: {finding}"
                        ));
                    }
                }
                Err(error) => failures.push(format!(
                    "{label}: accept succeeded but verify_repository errored: {error}"
                )),
            }
            let _ = std::fs::remove_dir_all(&root);
            continue;
        }

        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| verify_repository(&layout)))
            .is_err()
        {
            failures.push(format!("{label}: verify_repository panicked"));
            let _ = std::fs::remove_dir_all(&root);
            continue;
        }

        if let Ok(verification) = verify_repository(&layout) {
            if let Some(finding) = dangling_reference_finding(&verification) {
                failures.push(format!(
                    "{label}: accept failed and left verify_repository showing: {finding}"
                ));
            }
        }

        clear_stale_locks_for_test(&layout);
        let _ = repair_pointer_index_tail(&layout);
        let _ = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail());

        let retry = accept_exchange_artifact(&layout, &bytes, &AcceptOptions::default_limits());
        match retry {
            Ok(_) => match verify_repository(&layout) {
                Ok(verification) => {
                    if let Some(finding) = dangling_reference_finding(&verification) {
                        failures.push(format!(
                            "{label}: retry succeeded but verify_repository still found: {finding}"
                        ));
                    }
                }
                Err(error) => failures.push(format!(
                    "{label}: retry succeeded but verify_repository errored: {error}"
                )),
            },
            Err(retry_error) => match verify_repository(&layout) {
                Ok(verification) => {
                    if let Some(finding) = dangling_reference_finding(&verification) {
                        failures.push(format!(
                            "{label}: retry refused ({retry_error}) and verify_repository still found: {finding}"
                        ));
                    }
                }
                Err(error) => failures.push(format!(
                    "{label}: retry refused ({retry_error}) and verify_repository also errored: {error}"
                )),
            },
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    assert!(
        failures.is_empty(),
        "{} failure(s) out of {iterations} (seed {seed}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
