//! DC-84: demonstrates the shared `support::unique_suffix()` helper is genuinely collision-free
//! under thread contention, in the same shape DC-83 used to disprove the naive "process id plus
//! timestamp" pattern. One demonstration for the shared helper is enough — every one of prikk-cli's
//! integration test files that calls into `support::unique_suffix()` (directly or via
//! `support::unique_repo`) inherits this guarantee rather than needing its own copy of this test.

// `support`'s own (pre-existing, unrelated) helpers use `.unwrap()` throughout — matching every
// other prikk-cli integration test file that includes this module.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn unique_suffix_has_no_collisions_under_synchronized_thread_contention() {
    let threads = 64;
    let rounds = 200;
    let mut total_samples = 0usize;
    let mut all_unique = HashSet::new();
    for _ in 0..rounds {
        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    support::unique_suffix()
                })
            })
            .collect();
        for handle in handles {
            let value = match handle.join() {
                Ok(value) => value,
                Err(_) => panic!("stress-test thread panicked"),
            };
            total_samples += 1;
            assert!(
                all_unique.insert(value.clone()),
                "unique_suffix() produced a duplicate value under synchronized contention: {value}"
            );
        }
    }
    assert_eq!(all_unique.len(), total_samples);
}
