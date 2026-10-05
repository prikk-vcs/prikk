//! **RFC 160 §9 R3 — every framed reader terminates on arbitrary bytes, and does bounded work doing it.**
//!
//! For each of P4's twelve formats (the ten in `test_gates::hostile_lengths::formats()` plus the two `refs` ones), a deterministic
//! corpus of bounded inputs (arbitrary bytes, and a valid record mutated by bit flips, truncation and a duplicated span, up to 64 KiB)
//! is decoded, and three things are asserted of **every** case: it finishes (R2 makes a hang structurally impossible; this is the
//! outcome-facing proof of it), it reports **no more outcomes than input bytes** (each outcome -- `Evaluated` or `Failed` -- consumes
//! at least one frame header, so this can never be violated by a sound reader), and it does **bounded work**: bytes hashed
//! (`frame_resync::tallied_sha256`) stay within a fixed multiple of the input size.
//!
//! **The external review's M5** found the one case where that last bound did not hold: a buffer packed with real frame headers,
//! each claiming a body reaching **exactly** to the end of the file (so it *fits* the length check), decoded each one as a
//! **complete, checksum-failing record** -- `Invalid`, not `TrailingPartial` -- and RFC 102 Stage 2's own isolate-and-continue rule
//! fully parsed and hashed the next candidate the same way, for ten readers sharing the pattern. **RFC 167 fixes it**: a per-decode
//! work budget (`frame_resync::ScanBudget`, D1), charged at both the place every reader's own ordinary checksum happens and inside
//! the shared scan between candidates, resolves a cut-short scan to damage, never a tail (D2, C3).
//! [`budgeted_scan_is_linear_and_resolves_to_damage`] is the standing guard this round replaces the 1.5x ceilings (and the
//! `#[ignore]`d quadratic measurement they stood in for) with: it is not included in the per-format bound corpus below (that
//! corpus stays at 64 KiB, where the ratio was always small, and asserts the ordinary 8x bound on *ordinary* mutations) because it
//! needs the two deliberately hostile shapes and larger sizes M5 is specifically about.
//! [`verify_hashes_a_hostile_wal_within_k_times_its_size`] is RFC 167 D4's own **command-level** row: the
//! reader-level bound above only sees one reader's own decode call, which is exactly what let the D5
//! regression (a second, independent decode of the same WAL inside `verify`) through unnoticed -- this
//! measures the whole `verify_repository_with_options` call instead.
//!
//! The whole set (T1) runs under the R1 cgroup scope, like every other run in this round.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::time::Duration;

use super::hostile_length_support::isolated_with_timeout;
use super::hostile_lengths::{Format, envelope_bodies, formats, ref_name_length_bodies};
use super::test_support::unique_temp_dir;
use crate::{
    DEFAULT_ACTIVE_NAME, RepositoryLayout, VerifyOptions, Wal, verify_repository_with_options,
};

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

/// [`frame_with_body`]'s own shared layout (`magic(8) version(2) pre length(8) checksum(32) body`), stacked from offset 0: each
/// candidate's header claims a body reaching exactly to the end of `total_len` (so the length check "fits"), with the checksum area
/// left zeroed -- every candidate decodes as a complete, checksum-failing record (`Invalid`), the long-standing RFC 102 Stage 2
/// resync-loop path. `valid`/`pre` come from the format under test ([`Format::valid`]/[`Format::pre`]), so one builder covers every
/// one of the six readers RFC 167 found quadratic.
fn generic_hostile_shape_a(valid: &[u8], pre: usize, total_len: usize) -> Vec<u8> {
    let header_prefix_len = 10 + pre;
    let header_len = header_prefix_len + 8 + 32;
    let header_prefix = &valid[..header_prefix_len];
    let mut bytes = vec![0_u8; total_len];
    let mut at = 0;
    while at + header_len <= total_len {
        bytes[at..at + header_prefix_len].copy_from_slice(header_prefix);
        let claimed = (total_len - at - header_len) as u64;
        bytes[at + header_prefix_len..at + header_prefix_len + 8]
            .copy_from_slice(&claimed.to_be_bytes());
        at += header_len;
    }
    bytes
}

/// Like [`generic_hostile_shape_a`], but with one **leading** header whose claimed body clearly does not fit (a `TrailingPartial`,
/// F3's own partial-frame scan) in front of the same stacked pattern -- RFC 160 §5's second path through the scan.
fn generic_hostile_shape_b(valid: &[u8], pre: usize, total_len: usize) -> Vec<u8> {
    let header_prefix_len = 10 + pre;
    let header_len = header_prefix_len + 8 + 32;
    let header_prefix = &valid[..header_prefix_len];
    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(header_prefix);
    bytes.extend_from_slice(&(1_u64 << 40).to_be_bytes());
    bytes.extend_from_slice(&[0_u8; 32]);
    while bytes.len() + header_len <= total_len {
        let start = bytes.len();
        let claimed = (total_len - start - header_len) as u64;
        bytes.extend_from_slice(header_prefix);
        bytes.extend_from_slice(&claimed.to_be_bytes());
        bytes.extend_from_slice(&[0_u8; 32]);
    }
    bytes.resize(total_len, 0);
    bytes
}

/// **RFC 167 D1/D2/D4 -- the standing guard the 1.5x ceilings are replaced by** (the previous ceiling, and the `#[ignore]`d
/// quadratic measurement it stood in for, are both gone: this is the real bound, not a regression tripwire for an unfixed cost).
/// For each of the six readers RFC 167 found quadratic on `main` (container frame, WAL, trust policy, received index, ref
/// container, pointer index) and both hostile shapes, at 32 KiB, 256 KiB and 2 MiB: the scan never resolves to a tail (**C3**:
/// ambiguity is always damage, confirmed here by `trailing_partial_bytes == 0` and at least one `Failed` outcome), and the bytes
/// hashed stays within [`SCAN_BUDGET_MULTIPLE`] times the input plus one candidate's own worst-case slack (the charge that crosses
/// the limit is itself allowed to finish, `ScanBudget::exceeded`'s own doc) -- linear, not quadratic, confirmed up to 2 MiB per
/// size by checking the *ratio* stays flat rather than only checking an absolute ceiling.
///
/// **Control, run by hand for this round's report, not kept as code**: removing either of RFC 167 D1's two placements (the narrow
/// one inside `sound_frame_after_partial_budgeted`, or the broad one in each reader's own outer loop) leaves exactly the other
/// hostile shape fully quadratic for five of the six readers (container frame, received index, ref container, pointer index: both
/// placements needed; the WAL's own `Invalid` arm always calls the narrow one, so it alone happens to suffice there) -- each
/// placement was shown insufficient alone by direct measurement during the design round, not assumed from the RFC's own framing.
#[test]
fn budgeted_scan_is_linear_and_resolves_to_damage() {
    use crate::foundation::frame_resync::SCAN_BUDGET_MULTIPLE;

    let affected = [
        "container frame",
        "WAL",
        "trust policy",
        "received index",
        "ref container",
        "pointer index",
    ];
    for format in all_formats()
        .into_iter()
        .filter(|format| affected.contains(&format.name))
    {
        let valid = (format.valid)();
        for (shape_name, build) in [
            (
                "A",
                generic_hostile_shape_a as fn(&[u8], usize, usize) -> Vec<u8>,
            ),
            (
                "B",
                generic_hostile_shape_b as fn(&[u8], usize, usize) -> Vec<u8>,
            ),
        ] {
            let mut previous_ratio: Option<f64> = None;
            for size in [32 * 1024, 256 * 1024, 2 * 1024 * 1024] {
                let bytes = build(&valid, format.pre, size);
                crate::foundation::frame_resync::hash_tally::reset();
                let seen = (format.decode)(&bytes).expect("no source of an outer Err here");
                let hashed = crate::foundation::frame_resync::hash_tally::bytes_hashed();
                assert_eq!(
                    seen.trailing_partial_bytes, 0,
                    "{}, shape {shape_name}, {size}: a scan the budget cut short must never resolve to a tail (C3)",
                    format.name
                );
                assert!(
                    seen.failed >= 1,
                    "{}, shape {shape_name}, {size}: the cut-short scan must be reported, not silently dropped",
                    format.name
                );
                // One extra candidate's own worst-case body (up to the whole remaining buffer) may be
                // charged before the next check sees the budget already exceeded -- the ceiling allows
                // exactly one such overshoot, not an unbounded one.
                let ceiling = SCAN_BUDGET_MULTIPLE * size as u64 + size as u64;
                assert!(
                    hashed <= ceiling,
                    "{}, shape {shape_name}, {size}: hashed {hashed} bytes, over its {ceiling}-byte linear ceiling",
                    format.name
                );
                let ratio = hashed as f64 / size as f64;
                if let Some(previous) = previous_ratio {
                    assert!(
                        ratio <= previous * 1.5,
                        "{}, shape {shape_name}, {size}: bytes-hashed/input ratio {ratio:.2} grew past 1.5x the \
                         smaller size's {previous:.2} -- a growing ratio is quadratic, not linear",
                        format.name
                    );
                }
                previous_ratio = Some(ratio);
            }
        }
    }
}

/// **RFC 167 D4 -- the command-level row.** The reader-level bound above (`budgeted_scan_is_linear_
/// and_resolves_to_damage`) calls `wal::decode_records` directly, once. That is exactly the kind of
/// check D5's own regression slipped past: a *second*, independent decode of the same WAL, added
/// inside `verify_repository_with_options` by a later stage (`verify::reachability::
/// compute_reachable_object_ids`, RFC 164 Rule E, before its RFC 167 fix), doubled the real cost
/// without any reader-level ceiling ever seeing it -- each decode, on its own, still stayed within
/// its own bound. This measures the *whole* `verify` call instead, the way a user's own `prikk
/// verify` actually pays for it.
///
/// **Control, run by hand for this round's report, not kept as code**: temporarily restoring the
/// second decode this round's own D5 fix removed doubles the bytes hashed here and pushes it over
/// `k`, exactly the shape the reader-level test cannot see at all (it never calls `verify` itself).
#[test]
fn verify_hashes_a_hostile_wal_within_k_times_its_size() {
    use crate::foundation::frame_resync::SCAN_BUDGET_MULTIPLE;

    let format = formats()
        .into_iter()
        .find(|format| format.name == "WAL")
        .expect("the WAL format is registered");
    let valid = (format.valid)();

    let size = 2 * 1024 * 1024;
    let root = unique_temp_dir("rfc167-verify-command-level-row");
    let layout = RepositoryLayout::init(root).expect("init");
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    std::fs::write(
        wal.path(),
        generic_hostile_shape_a(&valid, format.pre, size),
    )
    .expect("write");

    crate::foundation::frame_resync::hash_tally::reset();
    let report = verify_repository_with_options(
        &layout,
        VerifyOptions {
            stop_on_first_error: false,
        },
    )
    .expect("verify itself does not error even though it finds damage");
    let hashed = crate::foundation::frame_resync::hash_tally::bytes_hashed();

    assert!(
        report.has_item_failure(),
        "a budget-exhausted WAL scan is damage, and a whole verify over it must report that"
    );
    // k: the WAL's own 8x budget, plus slack for one overshoot candidate and the small constant cost
    // of every other file `verify` reads in the same call (all empty or near-empty in this fixture).
    let k = SCAN_BUDGET_MULTIPLE + 2;
    let ceiling = k * size as u64;
    assert!(
        hashed <= ceiling,
        "verify hashed {hashed} bytes over a {size}-byte hostile WAL, over its {ceiling}-byte ({k}x) \
         whole-command ceiling -- a second decode of the same bytes would roughly double this"
    );
}
