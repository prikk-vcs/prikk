//! RFC 119 track C — Gate G1, pre-1.0 form: a release does not *silently* break what earlier
//! releases wrote.
//!
//! **This file is `format_stability_gate.rs`'s missing sibling** (RFC 119 §10 track C, §2 of the
//! handoff): that gate guards *format version bumps*; `0.23.0`'s `Tag` break went through the hole
//! beside it -- an in-place payload amendment at the *same* schema version, for which no format
//! bump occurs, so Gate B never fires. Same three-layer shape, same discipline ("a gate is only
//! trusted once it has been observed failing," `rfc111_index_decode_cost_gate.rs`), same
//! frozen-fixture precedent (`crates/prikk-cli/tests/fixtures/dc55_pre_swap_repo`).
//!
//! **The guarantee is against silence, not against breaking.** Breaking is permitted when
//! authorized and declared -- `0.23.0`'s own `Tag` amendment was exactly that. The outcome per
//! persisted object type is ternary:
//! - **compatible** -- decodes cleanly under current code;
//! - **breaking, declared** -- does not, and [`DECLARED_BREAKS`] names it, with a reason and a
//!   remedy;
//! - **breaking, undeclared** -- the only failure.
//!
//! **The reverse direction -- an old binary reading data current code wrote -- is declined, not
//! deferred** (RFC 119 `reverse-direction-test-declined` handoff v1). Forward is the direction
//! that protects users, since users upgrade; a schema bump breaks the reverse *by design* (a
//! deliberate format change, not an accident -- [`DeclaredBreak`]'s own doc names the concrete
//! example); and a test built to fail on every deliberate format change would train its own
//! allowlist to grow unexamined -- "recorded, not rejected," the pattern this project already
//! rejects elsewhere. Declining rests on one verified fact: the failure mode is fail-closed.
//! `format.rs::validate_format2_schema` refuses an unadmitted schema with a specific `Integrity`
//! error naming the object type, the schema found, and the accepted set -- an old binary *refuses*
//! unrecognized data, it does not misread it. `CHANGELOG.md`'s per-release reverse-break statement
//! already tells a downgrading user what changed; building and caching an old binary per platform,
//! in CI, for the direction that matters least, is not worth that cost. **What this gives up**: an
//! *accidental* reverse break goes undetected until someone downgrades, surfacing then as a clear,
//! specific refusal -- never silent corruption. Acceptable at pre-1.0, not forever. **Void if**:
//! prikk ever supports downgrade as a documented workflow, or a future format change lets an old
//! binary *misinterpret* new bytes rather than refuse them -- the second voids this outright, since
//! the whole ruling rests on failing closed.
//!
//! **Fixture**: `crates/prikk-cli/tests/fixtures/rfc119_g1_0_27_0_repo`, a real repository written
//! by the real `0.27.0` binary (built from the `0.27.0` git tag in an isolated worktree, never from
//! this working tree). **Do not regenerate it.** See the RFC 119 track C report for the original
//! construction technique, and the G1-fixture-refresh-`0.27.0` report for this specific fixture.
//!
//! **Replaced, not accumulated (RFC 119 track C's own follow-up, `g1-fixture-refresh-0-24-0`):**
//! this gate holds exactly one fixture, from the last release, and each release's own refresh
//! replaces it rather than adding a second baseline to check alongside it. Pre-1.0, no production
//! users, and G1's own form is *declare*, not *prevent* -- one baseline is proportionate. **What
//! this means a reader should not assume**: a future release passing this gate proves it reads the
//! *immediately preceding* release's data, not every retained release's data -- transitivity does
//! not hold (`0.28.0` reading `0.27.0`, and `0.27.0` reading `0.26.0`, does not imply `0.28.0` reads
//! `0.26.0`). The `0.26.0`-vintage fixture this replaced is gone; its own coverage and controls are
//! recorded in the G1-fixture-refresh-`0.26.0` report, not restated here.
//!
//! **Schemas unchanged from `0.26.0`'s own fixture** (`admitted_schemas` has not moved since
//! `0.24.0`): this refresh's schema-version arrays are identical to the ones it replaced --
//! `last_release_fixture_coverage_matches_the_committed_counts`'s committed values did not need to
//! change, only the fixture bytes and this file's own path constant did. **That makes provenance
//! the only evidence this refresh happened at all** -- nothing in this test suite can distinguish a
//! genuinely rebuilt fixture from the old one with its directory renamed, since a schema-array
//! change (like `0.24.0`'s own first coverage of `Patch` schema 2, `PATCH_PARENT_IDS_RETIRED_SCHEMA`)
//! is not available this time to serve as incidental evidence. The G1-fixture-refresh-`0.27.0`
//! report is where that provenance is recorded (worktree commit, `--version` output).
//! **Asserted, not only claimed**: `last_release_fixture_coverage_matches_the_committed_counts`
//! pins every persisted type's observed `schema_version`s, not only a record count -- a future
//! fixture rebuild that silently regressed to all-schema-1 `Patch` records would fail that test.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use prikk_object::{BlobPayload, BlockPayload, ObjectType, RecognitionClaimPayload, TagPayload};

use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, persisted_object_types};
use crate::patch_replay::decode::decode_patch_operations;

/// The release this gate's fixture was built from. [`last_release_fixture_root`] derives its path
/// from this rather than carrying the version as a second, independent literal; every
/// `DeclaredBreak`'s [`DeclaredBreak::older_version`] is checked against this too (RFC 119
/// `g1-declared-break-version-scope` handoff v1 §4) -- the fixture is always *the last release*
/// and the gate always compares it against *current* code, so `<this constant> -> current` is the
/// only version pair a declared break can ever describe here.
const LAST_RELEASE_FIXTURE_VERSION: &str = "0.27.0";

/// One declared **forward-direction** compatibility break between two adjacent releases -- the
/// shape of Gate A's `frozen`/`RFC114_ADMITTED_BUT_UNWRITTEN` pair and
/// `format_stability_gate.rs`'s `FORMATS_WITH_MIGRATION_COVERAGE`: a committed list, never
/// satisfiable by editing alone (the object type must be real, checked below), always carrying a
/// reason and a remedy.
///
/// **Forward direction only, deliberately** (RFC 119 `g1-fixture-refresh-0-24-0` handoff §2): this
/// gate's own mechanism only ever checks *newer code reading an older fixture* -- there is no
/// mechanism here for the reverse (an old binary reading new data), which would need an actual old
/// binary invoked as a subprocess, not this gate's Rust-function-call shape (deferred, see this
/// module's own report). A **reverse** break -- `0.24.0`'s own `Patch` schema 2 is exactly one: it
/// reads `0.23.0` fine, but `0.23.0` cannot read `0.24.0` -- must **not** be added here: doing so
/// would assert a forward break that does not exist, and this gate would then look for a failure
/// its own fixture can never produce. Reverse breaks are recorded in `CHANGELOG.md`'s own "Breaking
/// change" section instead, where every one to date already is.
///
/// **Version-scoped, not just object-type-scoped** (RFC 119 `g1-declared-break-version-scope`
/// handoff v1 §1/§3): an entry only excuses a failure when [`DeclaredBreak::older_version`] equals
/// [`LAST_RELEASE_FIXTURE_VERSION`] *and* the object type matches. The fixture is always the last
/// release and the gate always compares it against current code, so an entry whose older side is
/// any other version cannot apply to anything this gate actually checks -- keeping one anyway
/// would let it silently excuse an unrelated live failure on a matching object type forever, which
/// is the defect this version scoping exists to close. `every_declared_break_applies_to_the_current_fixture`
/// makes a stale entry fail loudly rather than sit inert.
struct DeclaredBreak {
    /// The release the break decodes *from* -- must equal [`LAST_RELEASE_FIXTURE_VERSION`] for
    /// this entry to excuse anything.
    older_version: &'static str,
    /// The release the break was introduced in. Informational only: current code is always the
    /// implicit "newer" side when this gate runs, so this is never compared against anything.
    newer_version: &'static str,
    /// Which persisted object type stops decoding.
    object_type: ObjectType,
    /// Quoted from `CHANGELOG.md`, not re-derived -- the wording is the record.
    reason: &'static str,
    /// A remedy, or an explicit statement that none exists. An entry with an empty remedy is a
    /// documentation defect, not a passing gate (handoff §5) -- there is no way to express "empty"
    /// here other than writing the true sentence, which this one does.
    remedy: &'static str,
}

impl DeclaredBreak {
    /// Rendered for messages -- `older_version`/`newer_version` are kept as separate fields
    /// (checked independently) rather than one pre-joined string (which nothing could check
    /// without parsing it back apart).
    fn version_pair(&self) -> String {
        format!("{} -> {}", self.older_version, self.newer_version)
    }
}

/// **Empty, deliberately** (RFC 119 `g1-declared-break-version-scope` handoff v1 §4 adjudication:
/// retire, don't relocate). `0.23.0`'s own `Tag` break (RFC 119 track C handoff §5) was this list's
/// first and, until this refresh, only entry -- but its `older_version` is `0.22.1`, which no
/// longer equals [`LAST_RELEASE_FIXTURE_VERSION`], so it can no longer apply to anything this gate
/// checks. Keeping it as a second, gate-adjacent historical record -- rather than retiring it --
/// would be exactly the "system reasoning about itself" duplication RFC 119 exists to remove:
/// `CHANGELOG.md`'s own `0.23.0` "Breaking change" section is already the complete, load-bearing
/// record of that break (this entry's own `reason`/`remedy` text was quoted from it verbatim, not
/// independently derived), so nothing is lost by not also carrying it here. **Empty-but-ready**,
/// the same shape `format_stability_gate.rs`'s own `FORMATS_WITH_MIGRATION_COVERAGE` starts in --
/// this becomes non-empty the day a release breaks a persisted type against *this gate's own*
/// fixture version, not on a general "some day" basis.
const DECLARED_BREAKS: &[DeclaredBreak] = &[];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("prikk-store's manifest dir has a workspace root two levels up")
        .to_path_buf()
}

/// The frozen last-release fixture's root (the directory *containing* `.prikk`, matching
/// `RepositoryLayout::open`'s own expectation). Derived from [`LAST_RELEASE_FIXTURE_VERSION`]
/// rather than carrying the version as a second literal inside the path string.
fn last_release_fixture_root() -> PathBuf {
    let directory_version = LAST_RELEASE_FIXTURE_VERSION.replace('.', "_");
    repo_root().join(format!(
        "crates/prikk-cli/tests/fixtures/rfc119_g1_{directory_version}_repo"
    ))
}

/// Read one persisted object type's live (slot A -- compaction never runs on object containers,
/// per `ContainerSlot`'s own doc) container file and decode every record with the *same* decoder
/// each type's real production read path uses -- `Patch` via `decode_patch_operations` (the real
/// general decoder, patch-schema-2 handoff), everything else via its own `XPayload::decode_canonical`.
/// Returns each decoded record's own `schema_version`, in container order, or the first decode
/// failure's message. **The returned `Vec`'s length is the record count** -- callers that only need
/// the count (this file has none left; both former call sites now use the schema list itself, per
/// `g1-fixture-refresh-0-24-0`'s review condition) read `.len()` rather than a separate count.
fn check_type_decodes(
    layout: &RepositoryLayout,
    object_type: ObjectType,
) -> Result<Vec<u32>, String> {
    let container_path = layout.container_slot_path(object_type, ContainerSlot::A);
    let relative = layout
        .repository_relative(&container_path)
        .map_err(|err| err.to_string())?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)
        .map_err(|err| err.to_string())?
    else {
        return Ok(Vec::new());
    };
    let replay = decode_container_records(object_type, &bytes).map_err(|err| err.to_string())?;
    let mut schema_versions = Vec::new();
    for record in &replay.records {
        let envelope = &record.envelope;
        let decode_result: Result<(), prikk_error::PrikkError> = match object_type {
            ObjectType::Block => {
                BlockPayload::decode_canonical(&envelope.canonical_payload).map(|_| ())
            }
            ObjectType::Blob => {
                BlobPayload::decode_canonical(&envelope.canonical_payload).map(|_| ())
            }
            ObjectType::Tag => {
                TagPayload::decode_canonical(&envelope.canonical_payload).map(|_| ())
            }
            ObjectType::RefState => prikk_object::RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )
            .map(|_| ()),
            ObjectType::RecognitionClaim => {
                RecognitionClaimPayload::decode_canonical(&envelope.canonical_payload).map(|_| ())
            }
            ObjectType::Patch => {
                decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)
                    .map(|_| ())
            }
            other => {
                panic!(
                    "G1 has no decode check wired up for persisted type {other} -- \
                     persisted_object_types() grew without this gate growing with it"
                )
            }
        };
        decode_result.map_err(|err| {
            format!(
                "object {} ({object_type}) failed to decode: {err}",
                envelope.object_id()
            )
        })?;
        schema_versions.push(envelope.schema_version);
    }
    Ok(schema_versions)
}

#[test]
fn last_release_fixture_exists() {
    let root = last_release_fixture_root();
    assert!(
        root.join(".prikk").is_dir(),
        "the last-release compatibility fixture is missing at {} -- see this module's doc for how \
         it must be rebuilt (from the actual release tag's binary, never from current code)",
        root.display()
    );
}

/// Layer 1 (mirroring `format_stability_gate.rs`'s own layer 2): every declared break must name a
/// real persisted object type, so a typo or a stale entry cannot silently exempt nothing.
#[test]
fn every_declared_break_names_a_persisted_object_type() {
    for declared in DECLARED_BREAKS {
        assert!(
            persisted_object_types().contains(&declared.object_type),
            "DECLARED_BREAKS entry for {} names {}, which is not a persisted object type",
            declared.version_pair(),
            declared.object_type
        );
        assert!(
            !declared.reason.trim().is_empty(),
            "DECLARED_BREAKS entry for {} ({}) has no reason",
            declared.version_pair(),
            declared.object_type
        );
        assert!(
            !declared.remedy.trim().is_empty(),
            "DECLARED_BREAKS entry for {} ({}) has no remedy",
            declared.version_pair(),
            declared.object_type
        );
    }
}

/// Layer 2 (RFC 119 `g1-declared-break-version-scope` handoff v1 §4): every declared break must
/// apply to *this* fixture, not merely name a real object type. The fixture is always the last
/// release and the gate always compares it against current code, so `older_version` must equal
/// [`LAST_RELEASE_FIXTURE_VERSION`] -- an entry that fails this check cannot excuse anything the
/// gate actually verifies and must be retired or updated, not left to sit inert (§1/§3: sitting
/// inert is how it silently excuses an unrelated future break on a matching object type).
#[test]
fn every_declared_break_applies_to_the_current_fixture() {
    for declared in DECLARED_BREAKS {
        assert_eq!(
            declared.older_version,
            LAST_RELEASE_FIXTURE_VERSION,
            "DECLARED_BREAKS entry for {} ({}) names an older version that does not match the \
             current fixture ({LAST_RELEASE_FIXTURE_VERSION}) -- this entry cannot apply to \
             anything the gate checks and must be retired or updated, not left in place",
            declared.version_pair(),
            declared.object_type
        );
    }
}

/// The real conformance check: every persisted object type in the last-release fixture must either
/// decode cleanly under current code, or have its failure covered by [`DECLARED_BREAKS`] for *this*
/// fixture's own version. This is the test the four controls exercise (RFC 119 track C handoff §7,
/// `g1-fixture-refresh-0-27-0` handoff §7); it currently passes because nothing has changed any
/// persisted type's decode contract since `0.27.0` shipped -- the same "nothing to test yet" state
/// `format_stability_gate.rs`'s own layers 1/2 start in.
#[test]
fn g1_last_release_fixture_is_compatible_or_the_break_is_declared() {
    let root = last_release_fixture_root();
    let layout = RepositoryLayout::open(&root).expect("last-release fixture repository opens");
    for &object_type in &persisted_object_types() {
        if let Err(message) = check_type_decodes(&layout, object_type) {
            let declared = DECLARED_BREAKS.iter().any(|break_| {
                break_.object_type == object_type
                    && break_.older_version == LAST_RELEASE_FIXTURE_VERSION
            });
            assert!(
                declared,
                "undeclared compatibility break: {message}, against the last-release fixture at \
                 {} -- if this break is authorized, add a DECLARED_BREAKS entry (with older_version \
                 == {LAST_RELEASE_FIXTURE_VERSION:?}) naming a reason and a remedy; if not, this is \
                 a live defect",
                root.display()
            );
        }
    }
}

/// Coverage is the gate's real specification (handoff §4/§8): committed literal counts per
/// persisted object type, so removing coverage is caught here, distinctly from a decode failure --
/// the same "committed, never generated at test time" discipline `dc55_identity_evidence.rs`'s own
/// `every_frozen_object_id_matches_its_own_filename` count uses.
///
/// **Pins the observed `schema_version` of every record, not only the count**
/// (`g1-fixture-refresh-0-24-0` review condition): a record count alone cannot tell a schema-1
/// `Patch` from a schema-2 one, so it could not have caught a future fixture rebuild that silently
/// stopped covering schema 2 -- exactly the coverage this fixture exists to add (this module's own
/// top doc). Each slice's length *is* the expected count; there is no separate count to keep in
/// sync. Values derived by running the real decode against the committed fixture and reading back
/// what it reported, not hand-computed -- the same "committed, never generated at test time"
/// discipline applies to the *values* here, not just their presence.
#[test]
fn last_release_fixture_coverage_matches_the_committed_counts() {
    let root = last_release_fixture_root();
    let layout = RepositoryLayout::open(&root).expect("last-release fixture repository opens");
    let expected: &[(ObjectType, &[u32])] = &[
        (ObjectType::Patch, &[2, 2]),
        (ObjectType::Block, &[2]),
        (ObjectType::Blob, &[1, 1]),
        (ObjectType::RefState, &[1, 1]),
        (ObjectType::Tag, &[1]),
        (ObjectType::RecognitionClaim, &[1]),
        (ObjectType::Attestation, &[]),
    ];
    for &(object_type, expected_schemas) in expected {
        let actual = check_type_decodes(&layout, object_type)
            .unwrap_or_else(|err| panic!("{object_type} must decode cleanly here: {err}"));
        assert_eq!(
            actual, expected_schemas,
            "{object_type}: expected schema versions {expected_schemas:?}, found {actual:?} -- \
             update this deliberately if the fixture is ever legitimately rebuilt, never to paper \
             over a coverage regression"
        );
    }
}
