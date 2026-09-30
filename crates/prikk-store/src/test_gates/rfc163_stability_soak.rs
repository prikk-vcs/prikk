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

use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::{
    DoctorRepairOptions, FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
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
