//! **A first fuzz target for the framed decoders** (0.49.0 step 5, round 2, U1; DC-41's policy: proptest
//! and its minimized `proptest-regressions` files, not cargo-fuzz, since the gate is stable 1.85.0).
//!
//! Every format `runaway_guards::all_formats` registers (eight from `hostile_lengths::formats()`, plus the
//! ref container and the pointer index) is fed mutations of its own valid file: byte flips, truncations,
//! insertions, length-field rewrites, and wholly random bytes. Three properties, each asserted:
//!
//! 1. **Bounded and deterministic.** Decoding the same bytes twice gives the same `Result`, and one decode
//!    hashes at most [`BUDGET_MULTIPLE`] times the input plus [`SLACK`] bytes (RFC 167's per-decode budget is
//!    8x, plus the input itself; the measured ref-container decode is exactly 9.00x).
//! 2. **The valid file is clean.** Decoding an unmutated valid file reports no tail and no failed frame.
//! 3. **A complete frame is never a tail.** Flipping the last byte of a valid file corrupts the body of its
//!    last frame, whose full header and full claimed body are both present: a checksum mismatch over a
//!    complete record is damage, never a torn tail (RFC 164 §9). This is the unambiguous case the round-1
//!    rule (`body_end == bytes.len()`) covers; the borrowed-tail case is round 2's U2.
//!
//! Termination and panics are covered by the child process: a hang trips the timeout, a panic fails the case.
//! The suite runs the default case count in-process (a child, for the timeout and the address-space cap);
//! the long run is `#[ignore]`d and states its case count.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::time::Duration;

use proptest::prelude::*;

use super::hostile_length_support::isolated_with_timeout;
use super::hostile_lengths::Format;
use super::runaway_guards::all_formats;
use crate::foundation::frame_resync::hash_tally;

/// RFC 167's per-decode budget (`SCAN_BUDGET_MULTIPLE`, 8x the input) plus the input itself.
const BUDGET_MULTIPLE: u64 = 9;
/// Slack for the fixed-size costs of one decode (a header, a small constant read), in bytes.
const SLACK: u64 = 4096;
/// Cases in the default suite. Each case is one decode of a small (a few hundred bytes to 4 KiB) input,
/// so this stays well inside the gate's 30-second budget for this target.
const DEFAULT_CASES: u32 = 256;
/// The long run: `#[ignore]`d, run deliberately with `cargo test -- --ignored framed_decoders_long_run`.
const LONG_RUN_CASES: u32 = 20_000;

#[derive(Debug, Clone)]
enum Mutation {
    Flip { at: usize, mask: u8 },
    FlipLast { mask: u8 },
    Truncate { keep: usize },
    Insert { at: usize, bytes: Vec<u8> },
    RewriteU64 { at: usize, value: u64 },
    Random(Vec<u8>),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (any::<usize>(), 1_u8..=255).prop_map(|(at, mask)| Mutation::Flip { at, mask }),
        (1_u8..=255).prop_map(|mask| Mutation::FlipLast { mask }),
        any::<usize>().prop_map(|keep| Mutation::Truncate { keep }),
        (any::<usize>(), prop::collection::vec(any::<u8>(), 1..=64))
            .prop_map(|(at, bytes)| Mutation::Insert { at, bytes }),
        (any::<usize>(), any::<u64>()).prop_map(|(at, value)| Mutation::RewriteU64 { at, value }),
        prop::collection::vec(any::<u8>(), 0..=512).prop_map(Mutation::Random),
    ]
}

fn apply(valid: &[u8], mutation: &Mutation) -> Vec<u8> {
    let mut bytes = valid.to_vec();
    match mutation {
        Mutation::Flip { at, mask } => {
            if !bytes.is_empty() {
                let at = at % bytes.len();
                bytes[at] ^= mask;
            }
        }
        Mutation::FlipLast { mask } => {
            if let Some(last) = bytes.last_mut() {
                *last ^= mask;
            }
        }
        Mutation::Truncate { keep } => {
            let keep = keep % (bytes.len() + 1);
            bytes.truncate(keep);
        }
        Mutation::Insert {
            at,
            bytes: inserted,
        } => {
            let at = at % (bytes.len() + 1);
            bytes.splice(at..at, inserted.iter().copied());
        }
        Mutation::RewriteU64 { at, value } => {
            if bytes.len() >= 8 {
                let at = at % (bytes.len() - 7);
                bytes[at..at + 8].copy_from_slice(&value.to_be_bytes());
            }
        }
        Mutation::Random(random) => return random.clone(),
    }
    bytes
}

fn formats_under_test() -> Vec<Format> {
    all_formats()
}

/// The properties, for one format's decode of one byte string.
fn check_decode(format: &Format, bytes: &[u8]) -> Result<(), TestCaseError> {
    hash_tally::reset();
    let first = (format.decode)(bytes);
    let hashed = hash_tally::bytes_hashed();
    let ceiling = BUDGET_MULTIPLE * bytes.len() as u64 + SLACK;
    prop_assert!(
        hashed <= ceiling,
        "{}: one decode hashed {hashed} bytes over a {}-byte input, past the {ceiling}-byte ceiling \
         ({BUDGET_MULTIPLE}x plus {SLACK})",
        format.name,
        bytes.len()
    );
    let second = (format.decode)(bytes);
    prop_assert_eq!(
        &first,
        &second,
        "{}: two decodes of the same bytes disagree",
        format.name
    );
    Ok(())
}

/// **Property 1 and 1's determinism, over mutated inputs of every format.**
fn fuzz_mutated() -> impl Fn(usize, Mutation) -> Result<(), TestCaseError> {
    move |index, mutation| {
        let formats = formats_under_test();
        let format = &formats[index];
        let valid = (format.valid)();
        let bytes = apply(&valid, &mutation);
        check_decode(format, &bytes)?;
        let flips_last = match mutation {
            Mutation::FlipLast { .. } => !valid.is_empty(),
            Mutation::Flip { at, .. } => !valid.is_empty() && at % valid.len() == valid.len() - 1,
            _ => false,
        };
        if flips_last {
            check_last_byte_flip_result(format, &bytes)?;
        }
        Ok(())
    }
}

/// Property 3 over a generated flip: the last byte of a valid file, flipped, is a complete last frame.
fn check_last_byte_flip_result(format: &Format, bytes: &[u8]) -> Result<(), TestCaseError> {
    if last_byte_flip_exemption(format.name).is_some() {
        return Ok(());
    }
    if let Ok(seen) = (format.decode)(bytes) {
        prop_assert_eq!(
            seen.trailing_partial_bytes,
            0,
            "{}: flipping the last byte of a complete record made it a torn tail (RFC 164 §9)",
            format.name
        );
    }
    Ok(())
}

/// **Property 2: the unmutated valid file decodes clean.** Under the round-1 control hook (a complete
/// record made a tail) this is the first assertion to fail, on the identity case.
fn check_valid_is_clean(format: &Format) -> Result<(), TestCaseError> {
    let valid = (format.valid)();
    let seen = (format.decode)(&valid);
    match seen {
        Ok(seen) => {
            prop_assert_eq!(
                seen.trailing_partial_bytes,
                0,
                "{}: the valid file reports a torn tail",
                format.name
            );
            prop_assert_eq!(
                seen.failed,
                0,
                "{}: the valid file reports a failed frame",
                format.name
            );
            Ok(())
        }
        Err(error) => Err(TestCaseError::fail(format!(
            "{}: the valid file fails to decode: {error}",
            format.name
        ))),
    }
}

/// The one format property 3 does not cover, and why. The WAL's own last-frame damage is answered by the
/// commit witness instead (RFC 166 N6: an acknowledged commit damaged after the fact is acknowledged damage,
/// not a tail -- `crates/prikk-cli/tests/torn_tail_is_one_frame.rs`'s
/// `a_witnessed_lone_damaged_record_is_acknowledged_damage_and_the_repair_refuses`). Round 1 left the WAL's
/// decode untouched on purpose (RFC 164 Addendum 2 item 1: "the WAL untouched"), so this property would
/// misstate its design rather than find a defect. Listed here so the exemption is visible, not silent.
fn last_byte_flip_exemption(name: &str) -> Option<&'static str> {
    match name {
        "WAL" => Some("answered by the commit witness, RFC 166 N6 (torn_tail_is_one_frame.rs)"),
        _ => None,
    }
}

/// **Property 3: flipping the last byte of a valid file leaves a complete, checksum-failed last frame, not
/// a torn tail.** A torn tail is a prefix of a frame; this flip changes no length, so nothing is short.
fn check_last_byte_flip_is_never_a_tail(format: &Format) -> Result<(), TestCaseError> {
    if last_byte_flip_exemption(format.name).is_some() {
        return Ok(());
    }
    let valid = (format.valid)();
    if valid.is_empty() {
        return Ok(());
    }
    let mut bytes = valid.clone();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    let seen = (format.decode)(&bytes);
    match seen {
        Ok(seen) => {
            prop_assert_eq!(
                seen.trailing_partial_bytes,
                0,
                "{}: flipping the last byte of a complete record made it a torn tail (RFC 164 §9)",
                format.name
            );
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

fn run_suite(cases: u32) {
    let count = formats_under_test().len();
    let config = ProptestConfig {
        cases,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "proptest-regressions/framed_decoder_fuzz.txt",
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
        panic!("framed-decoder fuzz target failed: {error}");
    }
    for format in formats_under_test() {
        if let Err(error) = check_valid_is_clean(&format) {
            panic!("{error}");
        }
        if let Err(error) = check_last_byte_flip_is_never_a_tail(&format) {
            panic!("{error}");
        }
    }
}

/// The default-suite run, in a child process (a timeout for termination, the address-space cap).
#[test]
fn framed_decoders_survive_mutation() {
    if std::env::var("PRIKK_HOSTILE_CHILD").as_deref() == Ok("framed_decoders_survive_mutation") {
        run_suite(DEFAULT_CASES);
        return;
    }
    isolated_with_timeout(
        module_path!(),
        "framed_decoders_survive_mutation",
        Duration::from_secs(120),
        || run_suite(DEFAULT_CASES),
    );
}

/// The long run, deliberately: `cargo test -p prikk-store -- --ignored framed_decoders_long_run`.
#[test]
#[ignore = "a long run: LONG_RUN_CASES mutated decodes per format, run deliberately"]
fn framed_decoders_long_run() {
    isolated_with_timeout(
        module_path!(),
        "framed_decoders_long_run",
        Duration::from_secs(3600),
        || run_suite(LONG_RUN_CASES),
    );
}
