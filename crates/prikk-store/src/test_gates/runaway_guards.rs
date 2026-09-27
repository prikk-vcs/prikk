//! **RFC 160 §9 R3 — every framed reader terminates on arbitrary bytes, and does bounded work doing it.**
//!
//! For each of P4's twelve formats (the ten in `test_gates::hostile_lengths::formats()` plus the two `refs` ones), a deterministic
//! corpus of bounded inputs (arbitrary bytes, and a valid record mutated by bit flips, truncation and a duplicated span, up to 64 KiB)
//! is decoded, and three things are asserted of **every** case: it finishes (R2 makes a hang structurally impossible; this is the
//! outcome-facing proof of it), it reports **no more outcomes than input bytes** (each outcome -- `Evaluated` or `Failed` -- consumes
//! at least one frame header, so this can never be violated by a sound reader), and it does **bounded work**: bytes hashed
//! (`frame_resync::tallied_sha256`) stay within a fixed multiple of the input size.
//!
//! **The external review's M5** found the one case where that last bound does not hold today: a WAL buffer packed with real frame
//! headers, each claiming a body reaching **exactly** to the end of the file (so it *fits* the length check), decodes each one as a
//! **complete, checksum-failing record** -- `Invalid`, not `TrailingPartial` -- and RFC 102 Stage 2's own isolate-and-continue rule
//! (unchanged by F3: `resync_to_next_magic` from the `Invalid` arm, not `sound_frame_after_partial`, which only ever runs from a
//! `TrailingPartial` arm) fully parses and hashes the next candidate the same way. Since nearly every candidate's claim reaches
//! nearly the whole remaining file, this is quadratic in the file's size -- a defect in the container/WAL/etc. decode loop shared
//! since RFC 102 Stage 2, not something F3's `sound_frame_after_partial` introduced (confirmed by construction: this buffer's only
//! `TrailingPartial` classification is the single genuine short tail at the very end).
//! [`hostile_wal_tail_quadratic_is_measured_and_bounded`] reproduces it, measures it, and documents the ratio; it is **not** included
//! in the per-format bound corpus below (that corpus stays at 64 KiB, where the ratio is still small, and asserts the ordinary 8x
//! bound). No mechanical fix landed this round -- see the round's report for why, and the ruling asked.
//! [`hostile_wal_tail_hashing_stays_within_its_ceiling_at_a_small_size`] is the **standing** guard (RFC 160 §9 Addendum 1): a small,
//! fast case asserting hashed bytes stay at or below 1.5x what it measured when the ceiling was written, so a regression that makes
//! the (still unfixed) quadratic worse is caught even though the quadratic itself is not.
//!
//! The whole set (T1) runs under the R1 cgroup scope, like every other run in this round.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::time::Duration;

use super::hostile_length_support::isolated_with_timeout;
use super::hostile_lengths::{Format, envelope_bodies, formats, ref_name_length_bodies};

/// A small, dependency-free deterministic PRNG (xorshift64*), so the corpus is fixed and a failure reproduces without pulling in
/// `proptest`'s shrinking machinery for what is really a fixed, bounded fuzz sweep.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u64() & 0xff) as u8).collect()
    }
}

/// The longest input a corpus case may be: bounded, per the handoff, so the sweep stays cheap and the work-bound assertion is
/// meaningful (a bound of "8x the input" is only informative for inputs the reader could plausibly meet).
const MAX_CASE_LEN: usize = 64 * 1024;

/// How many cases of each kind, per format. Small enough that the whole set (12 formats, one child process each) finishes well
/// inside T1's budget; large enough that a reader-specific mistake (an off-by-one in `require_progress`'s call site, say) has a real
/// chance of being hit by at least one mutation.
const ARBITRARY_CASES: usize = 200;
const MUTATION_CASES: usize = 200;

/// The multiple of the input size a reader may hash. `sound_frame_after_partial` hashes at most one candidate's claimed body per
/// resync step, and a resync step never revisits bytes already consumed by an accepted record, so on an *ordinary* (non-pathological)
/// input the total hashed is a small multiple of the input -- not 1x, because a rejected candidate's body is hashed once before it is
/// rejected and the next candidate can overlap it. 8x is generous for 64 KiB inputs; see the module doc for the one input shape (a
/// hostile WAL tail) where it does not hold, measured separately.
const WORK_BOUND_MULTIPLE: u64 = 8;

/// One mutation of `valid`: a bit flipped, the buffer truncated, or a byte span duplicated in place -- the three shapes a real
/// interrupted or damaged append can produce, applied at a random position so repeated runs (fixed seed) cover different sites.
fn mutate(rng: &mut Rng, valid: &[u8]) -> Vec<u8> {
    if valid.is_empty() {
        return Vec::new();
    }
    match rng.next_usize(3) {
        0 => {
            let mut bytes = valid.to_vec();
            let at = rng.next_usize(bytes.len());
            let bit = 1_u8 << rng.next_usize(8);
            bytes[at] ^= bit;
            bytes
        }
        1 => {
            let cut = rng.next_usize(valid.len() + 1);
            valid[..cut].to_vec()
        }
        _ => {
            let span_start = rng.next_usize(valid.len());
            let span_len = rng.next_usize(valid.len() - span_start + 1).max(1);
            let span_end = (span_start + span_len).min(valid.len());
            let mut bytes = valid.to_vec();
            bytes.extend_from_slice(&valid[span_start..span_end]);
            bytes
        }
    }
}

/// Run one format's whole corpus (arbitrary bytes, then mutations of its own valid record), asserting termination, the outcome-count
/// bound and the work bound on every case. Called inside the child process [`isolated_with_timeout`] spawns.
fn run_corpus(format: &Format) {
    let valid = (format.valid)();
    let mut rng = Rng::new(0x5EED_0160 ^ format.name.len() as u64);
    let mut checked = 0_usize;
    for len in [0, 1, 8, 50, 200, 1000, MAX_CASE_LEN] {
        for _ in 0..(ARBITRARY_CASES / 7).max(1) {
            check_one(format, &rng.bytes(len), &mut checked);
        }
    }
    for _ in 0..MUTATION_CASES {
        let bytes = mutate(&mut rng, &valid);
        check_one(format, &bytes, &mut checked);
    }
    assert!(
        checked >= ARBITRARY_CASES + MUTATION_CASES - 10,
        "{}: too few cases ran ({checked})",
        format.name
    );
}

fn check_one(format: &Format, bytes: &[u8], checked: &mut usize) {
    crate::foundation::frame_resync::hash_tally::reset();
    let result = (format.decode)(bytes);
    *checked += 1;
    let Ok(seen) = result else {
        return; // a decode-level Err is a termination too, and reports zero outcomes.
    };
    let outcomes = seen.records + seen.failed;
    assert!(
        outcomes <= bytes.len(),
        "{}: {outcomes} outcome(s) from {} byte(s) -- an outcome must consume at least one byte",
        format.name,
        bytes.len()
    );
    let hashed = crate::foundation::frame_resync::hash_tally::bytes_hashed();
    let bound = WORK_BOUND_MULTIPLE * (bytes.len() as u64).max(1);
    assert!(
        hashed <= bound,
        "{}: hashed {hashed} byte(s) for {} byte(s) of input, over the {WORK_BOUND_MULTIPLE}x bound",
        format.name,
        bytes.len()
    );
}

/// The two `refs` formats, built directly here (not reused from `refs::tests::hostile_lengths`, which is `cfg(target_os = "linux")`
/// through its parent module and so would make this module Linux-only too): a valid encoding and a decode that reduces the reply to
/// [`Seen`] via each type's own `counts()` accessor, without naming `PointerIndexRecordStatus` or `RefContainerRecordStatus`
/// (`pub(in crate::refs)`).
fn ref_container_format() -> Format {
    fn valid() -> Vec<u8> {
        let id = prikk_object::ObjectId::from_bytes([8; 32]);
        crate::refs::encode_ref_container_record_for_test(
            [5; 32],
            &crate::test_gates::test_support::signed_ref_update_envelope(
                "heads/main",
                None,
                id,
                id,
                1,
            ),
        )
        .expect("encoding")
    }
    fn decode(bytes: &[u8]) -> prikk_error::Result<super::hostile_length_support::Seen> {
        let (records, failed, trailing_partial_bytes) =
            crate::refs::decode_ref_container_records(bytes)?.counts();
        Ok(super::hostile_length_support::Seen {
            records,
            trailing_partial_bytes,
            failed,
        })
    }
    Format {
        name: "ref container",
        pre: 32,
        valid,
        decode,
        hostile_bodies: envelope_bodies,
    }
}

fn pointer_index_format() -> Format {
    fn valid() -> Vec<u8> {
        crate::refs::encode_pointer_index_record(&crate::refs::PointerIndexEntry {
            ref_name_key: [5; 32],
            ref_name: "heads/main".to_string(),
            ref_state_id: prikk_object::ObjectId::from_bytes([6; 32]),
        })
        .expect("encoding")
    }
    fn decode(bytes: &[u8]) -> prikk_error::Result<super::hostile_length_support::Seen> {
        let (records, failed, trailing_partial_bytes) =
            crate::refs::decode_pointer_index_records(bytes)?.counts();
        Ok(super::hostile_length_support::Seen {
            records,
            trailing_partial_bytes,
            failed,
        })
    }
    Format {
        name: "pointer index",
        pre: 0,
        valid,
        decode,
        hostile_bodies: ref_name_length_bodies,
    }
}

fn all_formats() -> Vec<Format> {
    let mut list = formats();
    list.push(ref_container_format());
    list.push(pointer_index_format());
    list
}

/// One R3 case per format, each in its own child process (an address-space cap, and a wall-clock timeout so a hang -- which R2 should
/// make impossible -- fails the one case instead of the suite). T1's whole set.
macro_rules! runaway_guard_case {
    ($name:ident, $format:expr) => {
        #[test]
        fn $name() {
            if std::env::var("PRIKK_HOSTILE_CHILD").as_deref() == Ok(stringify!($name)) {
                run_corpus(&$format);
                return;
            }
            isolated_with_timeout(
                module_path!(),
                stringify!($name),
                Duration::from_secs(30),
                || run_corpus(&$format),
            );
        }
    };
}

runaway_guard_case!(
    r3_container_frame,
    formats()
        .into_iter()
        .find(|format| format.name == "container frame")
        .unwrap()
);
runaway_guard_case!(
    r3_object_index,
    formats()
        .into_iter()
        .find(|format| format.name == "object index")
        .unwrap()
);
runaway_guard_case!(
    r3_wal,
    formats()
        .into_iter()
        .find(|format| format.name == "WAL")
        .unwrap()
);
runaway_guard_case!(
    r3_trust_key,
    formats()
        .into_iter()
        .find(|format| format.name == "trust key")
        .unwrap()
);
runaway_guard_case!(
    r3_trust_policy,
    formats()
        .into_iter()
        .find(|format| format.name == "trust policy")
        .unwrap()
);
runaway_guard_case!(
    r3_author_key,
    formats()
        .into_iter()
        .find(|format| format.name == "author key")
        .unwrap()
);
runaway_guard_case!(
    r3_received_index,
    formats()
        .into_iter()
        .find(|format| format.name == "received index")
        .unwrap()
);
runaway_guard_case!(
    r3_generation,
    formats()
        .into_iter()
        .find(|format| format.name == "generation")
        .unwrap()
);
runaway_guard_case!(r3_pointer_index, pointer_index_format());
runaway_guard_case!(r3_ref_container, ref_container_format());

/// **The list of formats is exhaustive** (mirrors P4's own `the_suite_covers_every_format`): every format R3 covers is named here,
/// so a thirteenth format joins this list in the round that adds it.
#[test]
fn every_format_has_a_runaway_guard_case() {
    let names: Vec<&str> = all_formats().iter().map(|format| format.name).collect();
    for name in [
        "container frame",
        "object index",
        "WAL",
        "trust key",
        "trust policy",
        "author key",
        "received index",
        "generation",
        "pointer index",
        "ref container",
    ] {
        assert!(names.contains(&name), "{name}: no case in `all_formats()`");
    }
}

/// **The external review's M5, measured.** A torn tail (a frame claiming a body far past what remains) packed with real frame magic
/// bytes every `WAL_HEADER_LEN` bytes, each claiming a body reaching to the end of the file: `sound_frame_after_partial` fully parses
/// -- and hashes -- nearly every one. Reproduces the doubling the external review measured (0.25 / 0.88 / 3.49 / 14.02 s at
/// 256 KiB -> 2 MiB on their machine); this machine's numbers are printed, not asserted, because wall time is not portable across
/// machines (only the doubling shape is the point). **Not fixed this round** -- see the report for why a mechanical fix was not
/// attempted with confidence at this size, and the ruling asked. Run deliberately (`--ignored`); it is a measurement, not a gate.
/// A buffer packed with real WAL frame headers, one every `WAL_HEADER_LEN` bytes, each claiming a body reaching **exactly** to the
/// end of the file (so it "fits" the length check and is fully parsed and hashed, then rejected on its checksum). Shared between
/// the ignored full measurement below and [`hostile_wal_tail_hashing_stays_within_its_ceiling_at_a_small_size`]'s standing guard.
fn hostile_tail(total_len: usize) -> Vec<u8> {
    const WAL_HEADER_LEN: usize = 8 + 2 + 8 + 8 + 32;
    let magic = b"PWALR001";
    let mut bytes = vec![0_u8; total_len];
    let mut at = 0;
    while at + WAL_HEADER_LEN <= total_len {
        bytes[at..at + 8].copy_from_slice(magic);
        bytes[at + 8..at + 10].copy_from_slice(&1_u16.to_be_bytes());
        bytes[at + 10..at + 18].copy_from_slice(&0_u64.to_be_bytes()); // seq
        let claimed = (total_len - at - WAL_HEADER_LEN) as u64; // claims a body reaching exactly to the end of the file: it "fits"
        bytes[at + 18..at + 26].copy_from_slice(&claimed.to_be_bytes());
        at += WAL_HEADER_LEN;
    }
    bytes
}

/// **RFC 160 §9 Addendum 1, item 1 -- M5 gets a standing guard, not only an `#[ignore]`d measurement.** At a size small enough to
/// run in every ordinary `cargo test` (a fraction of a second even in a debug build), the hostile WAL tail's bytes-hashed stays at
/// or below **1.5x what it measured when this ceiling was written** -- the same ceiling shape P2's open rows use (`store_size_
/// independence.rs`), so a regression that makes the quadratic worse is caught even though the quadratic itself is not fixed until
/// 0.49.0 (RFC 160 §9's M5 ruling: the fix, and the real bound this ceiling is replaced by, is a design round, not this one).
/// **Perturb:** hash each candidate twice (call `tallied_sha256` an extra time on the same bytes before comparing): both sizes'
/// hashed counts double, over the ceiling, and this goes red.
#[test]
fn hostile_wal_tail_hashing_stays_within_its_ceiling_at_a_small_size() {
    use crate::wal::decode_records;

    for (size, ceiling) in [(32 * 1024, 13_882_014_u64), (64 * 1024, 55_533_252_u64)] {
        let bytes = hostile_tail(size);
        crate::foundation::frame_resync::hash_tally::reset();
        let replay = decode_records(&bytes).expect("no source of an outer Err here");
        let hashed = crate::foundation::frame_resync::hash_tally::bytes_hashed();
        assert!(
            replay.has_item_failure(),
            "{size}: the packed tail's rejected candidates are reported"
        );
        assert!(
            hashed <= ceiling,
            "{size}: hashed {hashed} bytes, over its {ceiling}-byte ceiling (1.5x the 9,254,676 / 37,022,168 bytes this measured, \
             debug build, when the ceiling was written)"
        );
    }
}

#[test]
#[ignore = "RFC 160 R3/M5 measurement: the hostile-WAL-tail quadratic; run deliberately, prints its own numbers"]
fn hostile_wal_tail_quadratic_is_measured_and_bounded() {
    use crate::wal::decode_records;

    let mut previous: Option<f64> = None;
    for size in [256 * 1024, 512 * 1024, 1024 * 1024, 2 * 1024 * 1024] {
        let bytes = hostile_tail(size);
        crate::foundation::frame_resync::hash_tally::reset();
        let began = std::time::Instant::now();
        let replay = decode_records(&bytes).expect("no source of an outer Err here");
        let elapsed = began.elapsed().as_secs_f64();
        let hashed = crate::foundation::frame_resync::hash_tally::bytes_hashed();
        assert!(
            replay.has_item_failure(),
            "the packed tail is reported as damage"
        );
        let ratio = previous.map(|last| elapsed / last);
        println!(
            "{} KiB: {elapsed:.2} s, {} MB hashed ({:.1}x the input){}",
            size / 1024,
            hashed / 1_000_000,
            hashed as f64 / size as f64,
            ratio.map_or(String::new(), |r| format!(
                ", {r:.2}x the previous size's time"
            )),
        );
        previous = Some(elapsed);
    }
}
