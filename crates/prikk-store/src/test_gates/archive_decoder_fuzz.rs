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

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::Cursor;
use std::time::Duration;

use proptest::prelude::*;

use super::framed_decoder_fuzz::{apply, mutation};
use super::hostile_length_support::isolated_with_timeout;
use crate::RepositoryLayout;
use crate::archive::{export_archive, verify_archive};

const DEFAULT_CASES: u32 = 256;
const LONG_RUN_CASES: u32 = 20_000;

fn valid_archive() -> Vec<u8> {
    let dir = crate::test_gates::test_support::unique_temp_dir("archive-decoder-fuzz");
    let layout = RepositoryLayout::init(&dir).expect("init");
    crate::archive::tests::seal_one_block(&layout).expect("seal");
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
