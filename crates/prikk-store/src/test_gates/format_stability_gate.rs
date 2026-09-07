//! RFC 114 §4 -- Gate B, the tripwire on `CURRENT_FORMAT_VERSION_NUMERIC`.
//!
//! The property this file protects: **bumping the current repository format must be unable to pass
//! CI without migration coverage for the format being retired.** Three layers, each closing a way to
//! cheat the one before it -- modeled on `tools/release-policy/src/boundary/rfc_naming.rs`'s own
//! self-guard (a frozen list plus a real-filesystem existence check, not just list membership) and
//! `rfc111_index_decode_cost_gate.rs`'s discipline (a gate is only trusted once it has been observed
//! failing). The watcher does not live inside the thing it watches -- this file is separate from
//! `layout.rs`, which is what it checks.
//!
//! **Today there is nothing to migrate.** `RepositoryFormat` has exactly one variant, `CurrentV6`
//! (RFC 114 §0's correction: the contract only ever concerned format 6 and everything after).
//! `FIRST_SUPPORTED_FORMAT == layout::CURRENT_FORMAT_VERSION_NUMERIC == 6`, so layer 1's range is
//! empty and passes trivially, and `FORMATS_WITH_MIGRATION_COVERAGE` starts empty. The tripwire
//! exists for the day `CURRENT_FORMAT_VERSION_NUMERIC` moves to 7: at that instant layer 1's range
//! becomes `[6]`, and CI stays red until format 6 has a real, committed fixture (layer 2) that
//! genuinely decodes and migrates (layer 3).
//!
//! **Observed failing, per `prikk-rfc114-implementation-plan-v1.md` §3's own discipline**: layers 1
//! and 2 were both demonstrated failing in an isolated, discarded probe worktree by temporarily
//! setting `CURRENT_FORMAT_VERSION_NUMERIC` to 7 with no corresponding coverage entry -- see the
//! implementation report for the exact failure text observed. **Layer 3 could not be demonstrated**:
//! there is no format 7 today, so there is nothing for a migration-conformance test to genuinely fail
//! to migrate. Layer 3's test below is a scaffold with zero cases, matching layer 1/2's own "nothing
//! to test yet" state -- it will gain a real case, and a real chance to fail, the day format 7 exists.

use std::path::{Path, PathBuf};

use crate::foundation::layout;

/// First format this contract covers (RFC 114 §5.3: formats 1-5 are not supported, so the contract's
/// obligation starts at format 6). An independent literal, not derived from anything else -- if a
/// future RFC changes which formats are supported, this is the one line to update, and it is reviewed
/// on its own like every other constant in this file.
const FIRST_SUPPORTED_FORMAT: u32 = 6;

/// RFC 114 §4 Gate B, layer 2's list: formats below `CURRENT_FORMAT_VERSION_NUMERIC` whose migration
/// path into the current format has a real, committed byte fixture backing it (checked by
/// `layer_2_every_listed_format_has_a_committed_fixture` below). Starts empty: there is no historical
/// migration into format 6 today. Adding a number here without adding its fixture is caught by layer
/// 2, distinctly from layer 1's failure, so a reader knows which half is missing.
const FORMATS_WITH_MIGRATION_COVERAGE: &[u32] = &[];

/// Layer 2's fixture-location convention: a directory per retired format, named after the format it
/// migrates *from*. Mirrors `dc55_identity_evidence.rs`'s own fixture placement
/// (`crates/prikk-cli/tests/fixtures/dc55_pre_swap_repo`) -- a real byte fixture on disk, not
/// generated at test time.
fn migration_fixture_dir(format: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("format-{format}-migration"))
}

/// RFC 114 §4 Gate B, layer 1: the range check. Fires the instant `CURRENT_FORMAT_VERSION_NUMERIC`
/// moves past `FIRST_SUPPORTED_FORMAT` without a matching entry in `FORMATS_WITH_MIGRATION_COVERAGE`.
/// Today the range `6..6` is empty, so this passes trivially -- exactly the state RFC 114 §0
/// describes: nothing to test yet, not one case satisfied.
// The range below is `6..6`, empty today by construction (RFC 114 §0: nothing to test yet) --
// clippy statically proves this and flags it; that is the correct state to be in until
// `CURRENT_FORMAT_VERSION_NUMERIC` moves past `FIRST_SUPPORTED_FORMAT`, at which point the range
// becomes genuinely non-empty. `#[expect]`, not `#[allow]`: the day that happens, this lint stops
// firing and the suppression itself fails to compile -- a tripwire on the tripwire, so the
// suppression cannot quietly outlive the state it was written for (review's non-blocking
// suggestion, RFC-114-implementation-review-v1.md §6).
#[expect(clippy::reversed_empty_ranges)]
#[test]
fn layer_1_every_retired_format_below_current_has_migration_coverage_listed() {
    for format in FIRST_SUPPORTED_FORMAT..layout::CURRENT_FORMAT_VERSION_NUMERIC {
        assert!(
            FORMATS_WITH_MIGRATION_COVERAGE.contains(&format),
            "format {format} is below the current format ({}) but is not listed in \
             FORMATS_WITH_MIGRATION_COVERAGE -- RFC 114 §4 requires a tested migration path to \
             exist before a format bump ships (§5.2: forward-only, before it ships, not after)",
            layout::CURRENT_FORMAT_VERSION_NUMERIC
        );
    }
}

/// RFC 114 §4 Gate B, layer 2: list membership alone must not be satisfiable by editing the list.
/// Every entry in `FORMATS_WITH_MIGRATION_COVERAGE` must correspond to a real, committed fixture
/// directory on disk -- exactly the discipline `rfc_naming.rs`'s own self-guard applies to lifecycle
/// directories, checked against the filesystem rather than trusted from the list. Passes trivially
/// today because the list is empty.
#[test]
fn layer_2_every_listed_format_has_a_committed_fixture() {
    for &format in FORMATS_WITH_MIGRATION_COVERAGE {
        let fixture_dir = migration_fixture_dir(format);
        assert!(
            fixture_dir.is_dir(),
            "format {format} is listed in FORMATS_WITH_MIGRATION_COVERAGE but has no committed \
             fixture directory at {} -- adding a number to the list is not migration coverage; \
             the fixture must actually exist",
            fixture_dir.display()
        );
    }
}

/// RFC 114 §4 Gate B, layer 3: the real migration-conformance test. Load each committed fixture,
/// carry it through the documented migration path, and assert the result opens and verifies -- the
/// layer a placeholder fixture cannot pass, since garbage bytes fail to decode for real. Zero cases
/// today (`FORMATS_WITH_MIGRATION_COVERAGE` is empty); this gains a real case, and a real chance to
/// fail, the day format 7 exists and format 6 needs a migration path into it.
// This loop panics unconditionally on its first iteration by design: layer 3 has zero real
// migration-conformance cases wired up today (`FORMATS_WITH_MIGRATION_COVERAGE` is empty), and any
// future entry added without a matching case here must fail loudly rather than silently pass.
// `#[expect]`, not `#[allow]`, for the same self-retiring reason as layer 1's suppression above.
#[expect(clippy::never_loop)]
#[test]
fn layer_3_every_listed_format_migrates_to_a_repository_that_opens_and_verifies() {
    for &format in FORMATS_WITH_MIGRATION_COVERAGE {
        panic!(
            "format {format} is listed in FORMATS_WITH_MIGRATION_COVERAGE but layer 3 has no \
             migration-conformance case wired up for it yet -- this test must be extended to load \
             its fixture, run the documented migration path, and assert the result opens and \
             verifies before the format bump that retires it ships"
        );
    }
}

/// Refinement 1 from `RFC-114-implementation-plan-review-v1.md`: `CURRENT_FORMAT_VERSION`'s byte
/// form and `CURRENT_FORMAT_VERSION_NUMERIC` are pinned with literals on both sides, as two
/// independent assertions -- never one derived from the other, matching the vectors' own "committed
/// literal, never generated at test time" discipline. A format bump must update both `layout.rs`
/// constants *and* both literals in this test, side by side, forcing a human to see both change
/// together rather than one drifting while only the other's own use site notices.
#[test]
fn current_format_version_byte_and_numeric_forms_agree() {
    assert_eq!(layout::CURRENT_FORMAT_VERSION, b"6\n");
    assert_eq!(layout::CURRENT_FORMAT_VERSION_NUMERIC, 6);
}
