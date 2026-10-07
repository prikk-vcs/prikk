//! **The three whole-file/text caches `Format`'s own resync-record shape does not fit** (0.50.0 step
//! 2 Part A; `framed_decoder_fuzz`'s own report disclosed this as not fuzzed in step 5 round 2: "three
//! entries of `FRAMED_FORMAT_CASES` (verified-blocks record, lifecycle cache, commit index) are not in
//! the framed-record family: whole-file or text formats with a different decode shape, so the `Format`
//! harness does not fit them"). Each is one whole-file decode -- a single checksum over the whole body,
//! or (the commit index) a text parse with no length/count field at all -- not an append-only,
//! multi-record container with a resync-on-damage scan. There is no "tail" for a lone whole-file blob
//! to be a prefix of, so `framed_decoder_fuzz`'s property 3 ("a complete frame is never a tail") does
//! not apply here; it is replaced below by a property these three formats' own decode shape actually
//! has: none of them allocates or iterates in proportion to a length/count field's *claimed* value
//! before that value is checked against the bytes actually present (confirmed from source per format,
//! below, and already the subject of a hand-picked extreme-value test each in `hostile_lengths.rs` --
//! this is the same property, fuzzed by mutation instead of a handful of chosen values).
//!
//! Shares its mutation family with `framed_decoder_fuzz` (`Mutation`/`apply`/`mutation`): byte flips,
//! truncations, insertions, length-field rewrites, and wholly random bytes.
//!
//! **The properties, each asserted:**
//! 1. **No panic, and determinism.** Decoding the same (mutated) bytes twice agrees on whether the
//!    bytes are accepted.
//! 2. **The valid file decodes clean.** Each format's own freshly built valid file is accepted.
//! 3. **Bounded work.** One decode of a mutated input (at most a few KiB here) completes within
//!    [`DECODE_TIME_CEILING`] -- generous for input this small, and specifically meant to catch a
//!    regression that reintroduced unchecked claim-proportional work (a huge allocation or a loop
//!    bounded by an attacker's claim rather than the input's own length), which would make a decode
//!    take far longer than this, not merely cost a few more hashed bytes.
//!
//! Termination and panics are additionally covered by the same child-process isolation
//! `framed_decoder_fuzz` uses: a hang trips the timeout, a panic fails the case.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::time::{Duration, Instant};

use proptest::prelude::*;

use super::framed_decoder_fuzz::{apply, mutation};
use super::hostile_length_support::isolated_with_timeout;

/// Cases in the default suite -- the same order of magnitude as `framed_decoder_fuzz`'s own, small
/// enough that the whole set stays well inside the gate's own time budget.
const DEFAULT_CASES: u32 = 256;
/// The long run: `#[ignore]`d, run deliberately.
const LONG_RUN_CASES: u32 = 20_000;
/// How long one decode of a small, mutated input may take before the bounded-work property is
/// considered broken. Ordinary decodes of inputs this size finish in well under a millisecond; this
/// is generous by two orders of magnitude so the property is not flaky, while still being far too
/// tight for a decode that allocates or loops proportional to an unchecked claim (which would run to
/// seconds, not fractions of a millisecond, for a claim near `u32::MAX` or `u64::MAX`).
const DECODE_TIME_CEILING: Duration = Duration::from_millis(50);

struct WholeFileFormat {
    name: &'static str,
    /// Builds this format's own valid file, fresh.
    valid: fn() -> Vec<u8>,
    /// Decodes once; `true` iff the bytes are accepted.
    accepts: fn(&[u8]) -> bool,
}

fn verified_blocks_valid() -> Vec<u8> {
    use prikk_object::ObjectId;
    let set: std::collections::BTreeSet<ObjectId> =
        [ObjectId::from_bytes([7; 32]), ObjectId::from_bytes([9; 32])].into();
    crate::verified_blocks::encode(&set)
}

/// **Bounded work, confirmed from source** (`verified_blocks.rs::decode`): the checksum is hashed over
/// exactly the body's own length (bounded by the input, never a claim), and the block count is
/// checked via `count.checked_mul(32)? == rest.len()` *before* anything is built from it -- a claim
/// that does not match the bytes actually present is refused before any allocation sized by it.
fn verified_blocks_accepts(bytes: &[u8]) -> bool {
    crate::verified_blocks::decode(bytes).is_some()
}

/// A minimal, valid lifecycle-cache body, built directly rather than through `incremental::encode`
/// (whose `IncrementalCache` has no field public outside its own module) -- the same technique
/// `hostile_lengths.rs`'s own `lifecycle_cache` test already uses for its hostile bodies. The four
/// required fields (RFC/`incremental.rs`'s own `decode`: schema version, baseline id, horizon id,
/// steps since reanchor), tags in ascending order, no node records.
fn lifecycle_cache_valid() -> Vec<u8> {
    use prikk_object::{ObjectId, WireType};
    fn push_field(body: &mut Vec<u8>, tag: u16, wire: u8, value: &[u8]) {
        body.extend_from_slice(&tag.to_be_bytes());
        body.push(wire);
        body.extend_from_slice(&(value.len() as u64).to_be_bytes());
        body.extend_from_slice(value);
    }
    let mut body = Vec::new();
    push_field(&mut body, 1, WireType::U32 as u8, &1_u32.to_be_bytes());
    push_field(
        &mut body,
        2,
        WireType::ObjectId as u8,
        ObjectId::from_bytes([1; 32]).as_bytes(),
    );
    push_field(
        &mut body,
        3,
        WireType::ObjectId as u8,
        ObjectId::from_bytes([2; 32]).as_bytes(),
    );
    push_field(&mut body, 4, WireType::U32 as u8, &0_u32.to_be_bytes());
    let mut out = crate::lifecycle_cache::incremental::CACHE_MAGIC.to_vec();
    out.extend_from_slice(&prikk_hash::sha256(&body));
    out.extend_from_slice(&body);
    out
}

/// **Bounded work, confirmed from source** (`lifecycle_cache/incremental.rs::decode`): the checksum
/// is hashed over exactly the body's own length, and each field's own claimed length is consumed via
/// `ByteCursor::read_exact`, which refuses (`None`) rather than allocate when the claim exceeds the
/// bytes actually remaining.
fn lifecycle_cache_accepts(bytes: &[u8]) -> bool {
    crate::lifecycle_cache::incremental::decode(bytes).is_some()
}

fn commit_index_valid() -> Vec<u8> {
    crate::commit_index::serialize(&crate::commit_index::CommitIndex::default())
}

/// **Bounded work, confirmed from source** (`commit_index.rs::parse`): a text format with no length
/// or count field at all -- every claim-sized-allocation risk the other two formats have is
/// structurally absent; `hostile_lengths.rs`'s own `commit_index` test already shows a 100,000-field
/// line is refused, not iterated into unboundedly.
fn commit_index_accepts(bytes: &[u8]) -> bool {
    crate::commit_index::parse(bytes).is_some()
}

fn formats() -> Vec<WholeFileFormat> {
    vec![
        WholeFileFormat {
            name: "verified-blocks record",
            valid: verified_blocks_valid,
            accepts: verified_blocks_accepts,
        },
        WholeFileFormat {
            name: "lifecycle cache",
            valid: lifecycle_cache_valid,
            accepts: lifecycle_cache_accepts,
        },
        WholeFileFormat {
            name: "commit index",
            valid: commit_index_valid,
            accepts: commit_index_accepts,
        },
    ]
}

fn check_decode(format: &WholeFileFormat, bytes: &[u8]) -> Result<(), TestCaseError> {
    let start = Instant::now();
    let first = (format.accepts)(bytes);
    let elapsed = start.elapsed();
    prop_assert!(
        elapsed <= DECODE_TIME_CEILING,
        "{}: one decode of a {}-byte input took {elapsed:?}, past the {DECODE_TIME_CEILING:?} \
         ceiling -- a claim-proportional allocation or loop is the likely cause",
        format.name,
        bytes.len()
    );
    let second = (format.accepts)(bytes);
    prop_assert_eq!(
        first,
        second,
        "{}: two decodes of the same bytes disagree on acceptance",
        format.name
    );
    Ok(())
}

fn fuzz_mutated()
-> impl Fn(usize, super::framed_decoder_fuzz::Mutation) -> Result<(), TestCaseError> {
    move |index, mutation| {
        let formats = formats();
        let format = &formats[index];
        let valid = (format.valid)();
        let bytes = apply(&valid, &mutation);
        check_decode(format, &bytes)
    }
}

fn check_valid_is_clean(format: &WholeFileFormat) -> Result<(), TestCaseError> {
    let valid = (format.valid)();
    prop_assert!(
        (format.accepts)(&valid),
        "{}: the valid file is refused",
        format.name
    );
    Ok(())
}

fn run_suite(cases: u32) {
    let count = formats().len();
    let config = ProptestConfig {
        cases,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "proptest-regressions/whole_file_cache_fuzz.txt",
            ),
        )),
        ..ProptestConfig::default()
    };
    let mut runner = proptest::test_runner::TestRunner::new(config);
    let fuzz = fuzz_mutated();
    let result = runner.run(&(0..count, mutation()), |(index, mutation)| {
        fuzz(index, mutation)
    });
    if let Err(error) = result {
        panic!("whole-file cache fuzz target failed: {error}");
    }
    for format in formats() {
        if let Err(error) = check_valid_is_clean(&format) {
            panic!("{error}");
        }
    }
}

#[test]
fn whole_file_caches_survive_mutation() {
    if std::env::var("PRIKK_HOSTILE_CHILD").as_deref() == Ok("whole_file_caches_survive_mutation") {
        run_suite(DEFAULT_CASES);
        return;
    }
    isolated_with_timeout(
        module_path!(),
        "whole_file_caches_survive_mutation",
        Duration::from_secs(60),
        || run_suite(DEFAULT_CASES),
    );
}

/// The long run, deliberately: `cargo test -p prikk-store --lib -- --ignored whole_file_caches_long_run`.
#[test]
#[ignore = "a long run: LONG_RUN_CASES mutated decodes per format, run deliberately"]
fn whole_file_caches_long_run() {
    run_suite(LONG_RUN_CASES);
}
