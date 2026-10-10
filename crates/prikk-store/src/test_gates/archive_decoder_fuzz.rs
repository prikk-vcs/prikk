//! RFC 155 implementation Part V's own fuzz target for the `PREPO001` decoder -- the implementation
//! handoff's own security section: *"never panics, its work is bounded, and damage is reported,
//! never accepted."* Reuses `framed_decoder_fuzz`'s own `Mutation`/`apply` (DC-41's policy:
//! proptest and its minimized `proptest-regressions` files, not cargo-fuzz, since the gate is
//! stable 1.85.0), fed against one real, valid archive built by `export_archive` itself -- never a
//! hand-crafted byte string.
//!
//! **The property.** For any mutation of a genuinely valid archive: `verify_archive` never panics
//! (a panic inside the proptest closure fails the case, the same as every other property here), and
//! if it still decodes to `Ok`, that report must never claim [`ArchiveVerifyReport::
//! is_internally_consistent`] unless the mutated bytes are, byte-for-byte, the original valid
//! archive -- every section and the manifest are checksummed or cross-checked against the
//! manifest's own layout, so a changed byte that still decodes is damage *named*, never damage
//! silently accepted as sound.
//!
//! **V2's own finding, from source** (review `rfc155-impl-V-review-v1`): the property above could
//! not fail on the architect's own manifest-flip repro not because of coverage -- it is "find out
//! which, from source," and the real answer is neither thing the review's own finding guessed.
//! `valid_archive()` used to build its fixture via `archive::tests::seal_one_block`, whose own
//! AUTHOR/MAINTAINER signatures are deliberate placeholders (`test_support`'s own doc comment: "a
//! value, not a signer"), which never resolve. That means the *baseline* `valid_archive()` was
//! already `!is_internally_consistent()` -- confirmed directly: `unresolvable_signatures` carried
//! four permanently-unresolvable entries before any mutation was ever applied. The property's own
//! assertion, `if bytes != valid { assert!(!report.is_internally_consistent()) }`, was therefore
//! true for *every* mutated input whether or not `verify_archive` actually caught the specific
//! damage -- a check that cannot fail is not one, on the nose. This was not a coverage gap in the
//! mutation strategy; it was a fixture that made the property vacuous from the start, for every one
//! of its 20,000 long-run cases. The fix: `valid_archive()` now builds a *genuinely* internally
//! consistent archive (a real `Ed25519MaintainerSigner`, adopted and signing for real, with nothing
//! else to leave unresolved), the same shape `archive::verify::tests::
//! verify_resolves_a_genuinely_signed_maintainer_signature` already proved is_internally_consistent
//! for. Demonstrated red/green in the Part V2 report, by reverting this fixture change.
//!
//! `archive_decoder_catches_every_manifest_and_trailer_byte_flip` is a second, deterministic
//! property using the same corrected fixture: every byte of the manifest+trailer region (where V1's
//! own fix added the only structural checks that did not already exist) is flipped and checked, one
//! at a time -- not relying on `mutation()`'s random `at` to land there at all.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::Cursor;
use std::time::Duration;

use proptest::prelude::*;

use super::framed_decoder_fuzz::{apply, mutation};
use super::hostile_length_support::isolated_with_timeout;
use crate::RepositoryLayout;
use crate::archive::{export_archive, verify_archive};

/// The manifest+trailer region's own starting byte offset, read the same way the real end trailer
/// decoder does (test-only duplication of the two u64 field reads -- this project's own established
/// practice for small decode fragments reused across test-only call sites, rather than a shared
/// constant: see `archive.rs`'s own doc note on `resolve_pointer_index_source`).
fn manifest_region_start(bytes: &[u8]) -> usize {
    let end_trailer_start = bytes.len() - 56;
    u64::from_be_bytes(
        bytes[end_trailer_start + 8..end_trailer_start + 16]
            .try_into()
            .unwrap(),
    ) as usize
}

const DEFAULT_CASES: u32 = 256;
const LONG_RUN_CASES: u32 = 20_000;

/// A genuinely internally-consistent archive: one block, really signed by a really-adopted
/// maintainer key, with no patch, no author signature, and no reference to anything else -- nothing
/// left unresolved. V2's own fix: the previous fixture (`archive::tests::seal_one_block`) carries
/// permanently-unresolvable placeholder signatures by design, which made `is_internally_consistent`
/// false even for the *unmutated* archive -- this module's whole property depends on the baseline
/// being true, or every mutated case passes vacuously (see this module's own doc comment).
fn valid_archive() -> Vec<u8> {
    use crate::maintainer_signing::MaintainerSigner as _;
    use prikk_object::CanonicalEncode;

    let dir = crate::test_gates::test_support::unique_temp_dir("archive-decoder-fuzz");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let signer = crate::Ed25519MaintainerSigner::from_seed("archive-decoder-fuzz", &[0x37; 32])
        .expect("signer");
    let public_key_hex: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(&layout, signer.key_id(), &public_key_hex).expect("adopt");

    let mut store = crate::FileObjectStore::new(layout.clone());
    let payload = prikk_object::BlockPayload {
        parent_block_ids: Vec::new(),
        kind: prikk_object::BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: prikk_object::MerkleRoot([0u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let payload_bytes = payload.to_canonical_bytes().expect("encode block payload");
    let mut envelope =
        prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Block, 2, payload_bytes);
    let block_id = envelope.object_id();
    envelope
        .add_signature(
            crate::maintainer_signature(&signer, prikk_object::ObjectType::Block, block_id)
                .expect("sign for real"),
        )
        .expect("attach signature");
    crate::ObjectWriter::write_object(&mut store, &envelope).expect("write block");

    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    bytes
}

fn run_suite(cases: u32) {
    let valid = valid_archive();
    let config = ProptestConfig {
        cases,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "proptest-regressions/archive_decoder_fuzz.txt",
            ),
        )),
        ..ProptestConfig::default()
    };
    let mut runner = proptest::test_runner::TestRunner::new(config);
    let result = runner.run(&mutation(), |mutation| {
        let bytes = apply(&valid, &mutation);
        let mut cursor = Cursor::new(bytes.clone());
        if let Ok(report) = verify_archive(&mut cursor) {
            if bytes != valid {
                prop_assert!(
                    !report.is_internally_consistent(),
                    "a mutated archive reported itself internally consistent"
                );
            }
        }
        Ok(())
    });
    if let Err(error) = result {
        panic!("archive decoder fuzz target failed: {error}");
    }
}

/// The default-suite run, in a child process (a timeout for termination, the address-space cap) --
/// matching `framed_decoder_fuzz`'s own shape exactly.
#[test]
fn archive_decoder_survives_mutation() {
    if std::env::var("PRIKK_HOSTILE_CHILD").as_deref() == Ok("archive_decoder_survives_mutation") {
        run_suite(DEFAULT_CASES);
        return;
    }
    isolated_with_timeout(
        module_path!(),
        "archive_decoder_survives_mutation",
        Duration::from_secs(120),
        || run_suite(DEFAULT_CASES),
    );
}

/// The long run, deliberately: `cargo test -p prikk-store --lib -- --ignored archive_decoder_long_run`.
#[test]
#[ignore = "a long run: LONG_RUN_CASES mutated decodes, run deliberately"]
fn archive_decoder_long_run() {
    run_suite(LONG_RUN_CASES);
}

/// Confirms the baseline itself: the unmutated archive really is internally consistent, so the
/// properties above (and the exhaustive one below) are checking something that can fail, not
/// vacuously passing because nothing ever starts `true`. V2's own regression guard for the exact
/// gap this part fixes.
#[test]
fn valid_archive_fixture_is_genuinely_internally_consistent() {
    let valid = valid_archive();
    let mut cursor = Cursor::new(valid);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(
        report.is_internally_consistent(),
        "the fuzz fixture's own baseline must be internally consistent, or every mutated-case \
         assertion in this module passes vacuously: {report:?}"
    );
}

/// V2's own second fix: exhaustive, not random. Every single byte of the manifest+trailer region is
/// flipped (one at a time, `XOR 0xFF` so the byte always changes) and checked against the same
/// property the random fuzz above asserts -- no uniform-random draw can miss a byte this loop does
/// not visit, because it visits every one of them, every run. The region is a few hundred bytes for
/// a one-block fixture, so this is cheap: no address-space cap or timeout wrapper is needed, unlike
/// the random suites above.
#[test]
fn archive_decoder_catches_every_manifest_and_trailer_byte_flip() {
    let valid = valid_archive();
    let region_start = manifest_region_start(&valid);
    assert!(
        region_start < valid.len(),
        "the manifest+trailer region must be a real, non-empty suffix of the archive"
    );
    let mut checked = 0usize;
    for offset in region_start..valid.len() {
        let mut bytes = valid.clone();
        bytes[offset] ^= 0xFF;
        checked += 1;
        let mut cursor = Cursor::new(bytes);
        if let Ok(report) = verify_archive(&mut cursor) {
            assert!(
                !report.is_internally_consistent(),
                "flipping manifest/trailer byte at offset {offset} (relative {}) reported itself \
                 internally consistent -- V2's own gap",
                offset - region_start
            );
        }
    }
    assert!(
        checked > 100,
        "sanity: the manifest+trailer region should be a few hundred bytes for this fixture, not \
         suspiciously small (checked {checked})"
    );
}
