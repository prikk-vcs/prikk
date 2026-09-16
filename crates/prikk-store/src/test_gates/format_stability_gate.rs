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
//! **Format 7 exists (RFC 156 §5b), and format 6 is covered.** `CURRENT_FORMAT_VERSION_NUMERIC` is 7, so
//! layer 1's range is `[6]`; format 6 is listed; its fixture is a real format-6 repository committed at
//! `tests/fixtures/format-6-migration/repo` (layer 2), written by a prikk binary that still created
//! format 6; and layer 3 carries a copy of it through the documented migration — `prikk format upgrade`,
//! in place, RFC 114 §5.2a — and asserts the result opens at format 7 and verifies.
//!
//! **Before format 7**, `RepositoryFormat` had one variant and every layer passed trivially; the history
//! below records how the tripwire was shown able to fire then.
//!
//! **Observed failing, per `prikk-rfc114-implementation-plan-v1.md` §3's own discipline**: layers 1
//! and 2 were both demonstrated failing in an isolated, discarded probe worktree by temporarily
//! setting `CURRENT_FORMAT_VERSION_NUMERIC` to 7 with no corresponding coverage entry -- see the
//! implementation report for the exact failure text observed. **Layer 3 could not be demonstrated then**:
//! there was no format 7, so nothing could genuinely fail to migrate. **It was, with format 7** (RFC 156
//! §5b): making the migration skip the marker write fails layer 3, because the carried repository still
//! opens at format 6.

// Test-only file: a failed fixture copy or verification is a broken test environment, not a state
// to carry as a `Result` (the same allowance other test modules here take).
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use crate::foundation::layout;

/// First format this contract covers (RFC 114 §5.3: formats 1-5 are not supported, so the contract's
/// obligation starts at format 6). An independent literal, not derived from anything else -- if a
/// future RFC changes which formats are supported, this is the one line to update, and it is reviewed
/// on its own like every other constant in this file.
const FIRST_SUPPORTED_FORMAT: u32 = 6;

/// RFC 114 §4 Gate B, layer 2's list: formats below `CURRENT_FORMAT_VERSION_NUMERIC` whose migration
/// path into the current format has a real, committed byte fixture backing it (checked by
/// `layer_2_every_listed_format_has_a_committed_fixture` below). Format 6, since format 7 (RFC 156 §5b).
/// Adding a number here without adding its fixture is caught by layer 2, distinctly from layer 1's
/// failure, so a reader knows which half is missing.
const FORMATS_WITH_MIGRATION_COVERAGE: &[u32] = &[6];

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
/// The range is `6..7` since format 7 (RFC 156 §5b); format 6 is listed. The `#[expect]` that silenced
/// clippy's empty-range lint while the range was `6..6` retired itself exactly as designed: the lint
/// stopped firing and the expectation had to go.
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
/// directories, checked against the filesystem rather than trusted from the list.
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

/// RFC 114 §4 Gate B, layer 3: the real migration-conformance test. Load each committed fixture, carry it
/// through the documented migration path, and assert the result opens and verifies — the layer a
/// placeholder fixture cannot pass, since garbage bytes fail to decode for real.
///
/// **Format 6 → 7 (RFC 156 §5b; RFC 114 §5.2a):** the migration is `upgrade_repository_format`, the
/// function `prikk format upgrade` runs. A copy of the committed repository is opened at format 6,
/// verified clean, upgraded, and must then open at format 7 and verify clean again with the same object
/// count — nothing was carried because nothing moved, and this proves nothing was lost either.
#[test]
fn layer_3_every_listed_format_migrates_to_a_repository_that_opens_and_verifies() {
    for &format in FORMATS_WITH_MIGRATION_COVERAGE {
        match format {
            6 => format_6_migrates_in_place(),
            other => panic!(
                "format {other} is listed in FORMATS_WITH_MIGRATION_COVERAGE but layer 3 has no \
                 migration-conformance case for it -- extend this test before the bump ships"
            ),
        }
    }
}

/// `prikk verify`'s blocking conditions, read through the same public predicates its declaration
/// (`crates/prikk-cli/src/verify_verdict.rs`) names — a check in a test, not a second authority.
fn verifies_clean(report: &crate::RepositoryVerification) -> std::result::Result<(), String> {
    let conditions: [(&str, bool); 9] = [
        ("stage-failure", report.has_stage_failure()),
        ("item-failure", report.has_item_failure()),
        (
            "active-wal-metadata",
            report.has_active_wal_metadata_integrity_issue(),
        ),
        (
            "ref-publication",
            report.has_blocking_ref_publication_issues(),
        ),
        ("publication-trust", report.has_publication_trust_issues()),
        ("commit-index", report.has_commit_index_divergence()),
        ("lifecycle-cache", report.has_lifecycle_cache_divergence()),
        (
            "active-wal-ordering",
            report.has_active_wal_ordering_issue(),
        ),
        ("merge-baseline", report.has_merge_baseline_divergence()),
    ];
    let failing: Vec<&str> = conditions
        .iter()
        .filter(|(_, failed)| *failed)
        .map(|(id, _)| *id)
        .collect();
    if failing.is_empty() {
        Ok(())
    } else {
        Err(failing.join(", "))
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create fixture copy");
    for entry in std::fs::read_dir(from).expect("read fixture") {
        let entry = entry.expect("fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("entry type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

fn format_6_migrates_in_place() {
    use crate::foundation::layout::{RepositoryFormat, RepositoryLayout};

    let copy = crate::test_gates::test_support::unique_temp_dir("format-6-migration");
    copy_tree(&migration_fixture_dir(6).join("repo"), &copy);
    // Git keeps no empty directory, and a format-6 repository has two (`refs/locks`, `refs/tmp`).
    // Recreated from the layout's own list — empty by definition, so nothing is invented.
    let layout = RepositoryLayout::open(copy.clone()).expect("the fixture opens");
    for dir in layout
        .required_repository_directories()
        .expect("required directories")
    {
        std::fs::create_dir_all(layout.prikk_dir().join(&dir)).expect("recreate directory");
    }
    assert_eq!(
        std::fs::read(copy.join(".prikk").join("FORMAT")).expect("marker"),
        b"6\n",
        "the committed fixture must be a format-6 repository"
    );
    assert_eq!(layout.format(), RepositoryFormat::CurrentV6);

    let before = crate::verify_repository(&layout).expect("verify before");
    verifies_clean(&before).expect("the format-6 fixture verifies clean before the migration");

    let outcome = crate::upgrade_repository_format(&layout, verifies_clean).expect("migration");
    assert_eq!(
        outcome,
        crate::FormatUpgradeOutcome::Upgraded { from: 6, to: 7 }
    );

    let upgraded = RepositoryLayout::open(copy.clone()).expect("reopens after the migration");
    assert_eq!(upgraded.format(), RepositoryFormat::V7);
    let after = crate::verify_repository(&upgraded).expect("verify after");
    verifies_clean(&after).expect("the migrated repository verifies clean");
    assert_eq!(
        after.checked_objects, before.checked_objects,
        "every object is still there"
    );
    let _ = std::fs::remove_dir_all(copy);
}

/// Refinement 1 from `RFC-114-implementation-plan-review-v1.md`: `CURRENT_FORMAT_VERSION`'s byte
/// form and `CURRENT_FORMAT_VERSION_NUMERIC` are pinned with literals on both sides, as two
/// independent assertions -- never one derived from the other, matching the vectors' own "committed
/// literal, never generated at test time" discipline. A format bump must update both `layout.rs`
/// constants *and* both literals in this test, side by side, forcing a human to see both change
/// together rather than one drifting while only the other's own use site notices.
#[test]
fn current_format_version_byte_and_numeric_forms_agree() {
    assert_eq!(layout::CURRENT_FORMAT_VERSION, b"7\n");
    assert_eq!(layout::CURRENT_FORMAT_VERSION_NUMERIC, 7);
}
