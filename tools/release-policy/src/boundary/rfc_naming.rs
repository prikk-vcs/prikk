//! RFC 105: the RFC naming gate. Turns RFC 100's naming rule ("new RFCs are `NNN-slug`, numbered
//! from 100; existing RFCs keep their names permanently") from prose into a checked control --
//! RFC 100 shipped without one, and DC-96 through DC-99 were created after its acceptance, in the
//! scheme it retired, unnoticed for six days and four increments (RFC 105 §0).
//!
//! **The rule.** Every entry directly under the five governed locations must either conform to the
//! pattern or appear in that location's frozen legacy allowlist below:
//! - files under `rfcs/proposed/`, `rfcs/accepted/`, `rfcs/done/`, `rfcs/archive/`:
//!   `^[0-9]{3}-[a-z0-9]+(-[a-z0-9]+)*\.md$`
//! - directories under `rfcs/handoffs/`: `^[0-9]{3}-[a-z0-9]+(-[a-z0-9]+)*$` (no `.md`)
//!
//! **The self-guard (RFC criterion 3).** Every allowlisted name must correspond to an entry that
//! exists **exactly once** across the locations governing its entry kind -- not "at least one", and
//! not "at one specific location" (RFC 105 lifecycle-move-friction handoff §3, amended by its own
//! plan review §1). Zero matches still fails as a pre-authorisation bypass (the cheap "reserve
//! `DC-100`" trick); more than one now fails too, as a duplicate -- this project has shipped one
//! before (RFC 104, withdrawn the day it was accepted as a duplicate of DC-87), and the four file
//! locations (`proposed/`, `accepted/`, `done/`, `archive/`) are checked together for this because a
//! legacy RFC is expected to *move* between them over its lifecycle, never to exist at two of them at
//! once. A file-form entry still does not cover a directory of the same identifier or vice versa --
//! the search never crosses from a file-governed location into `rfcs/handoffs/` or back, only among
//! locations sharing the same governed kind. This does not, and is not meant to, stop a deliberate
//! two-line edit adding both a new legacy entry and its file/directory together -- that is the same
//! standard `unsafe_boundary.rs`'s `UNSAFE_EXEMPT_CRATES` sets: a visible edit to a reviewed constant
//! is the control, invisibility was the problem.
//!
//! **How the five lists below were generated (RFC 105 design-v1.md §3: derive, never
//! transcribe).** At the commit this module first landed:
//! ```sh
//! find rfcs/proposed rfcs/accepted rfcs/done rfcs/archive -maxdepth 1 -mindepth 1 -type f
//! find rfcs/handoffs -maxdepth 1 -mindepth 1 -type d
//! ```
//! each entry checked against the pattern above and kept only if non-conforming. **197 entries
//! total**: 5 `rfcs/proposed/`, 35 `rfcs/accepted/`, 79 `rfcs/done/`, 4 `rfcs/archive/`, 74
//! `rfcs/handoffs/`. `find`, not a shell glob -- a glob silently skips dotfiles, which mattered:
//! three `.gitkeep` housekeeping placeholders live in `rfcs/proposed/`, `rfcs/accepted/`, and
//! `rfcs/archive/` (none in `rfcs/done/` or `rfcs/handoffs/`, since the latter holds directories
//! only). They carry no special exemption of their own
//! (`.git-exclude/reviewed/RFC-105-investigation-ruling-v1.md` §4: this project has no
//! dotfile-exemption category, so they get ordinary allowlist entries like everything else, per
//! `boundary/publication.rs`'s own `collect_files` precedent of filtering by a positive predicate
//! rather than special-casing dotfiles).
//!
//! **One entry is not a legacy RFC name at all.** `consolidation`, under `rfcs/handoffs/`, is a
//! handoff directory (`dead-surface-consolidation-handoff-v1.md`) with **no governing RFC anywhere**
//! in `accepted/`, `done/`, `proposed/`, or `archive/` (`RFC-105-investigation-ruling-v1.md` §2) --
//! work that reached a handoff and merged without ever having had an RFC, a process gap of a
//! different kind than a naming violation and out of this gate's scope to fix. It is allowlisted
//! for exactly that reason, stated here rather than folded silently into "legacy naming" -- an
//! allowlist entry whose comment misdescribes why it exists is how the next reader concludes this
//! rule has exceptions it does not have.
//!
//! ## What this gate cannot see
//! - **Slug quality.** That a slug is lowercase and hyphenated is checkable; that it accurately
//!   describes its RFC is not (RFC 105 §4 non-goal) -- a gate that pretended otherwise would be
//!   worse than none.
//! - **A wrong but well-formed number.** `106` used twice would both pass this check; it verifies
//!   shape, not uniqueness or correctness of the number itself.
//! - **A deliberate two-line bypass.** Adding a new legacy file or directory and its allowlist
//!   entry together in one edit remains possible, by design -- see the self-guard paragraph above.

use super::{BoundaryError, push};

/// `rfcs/proposed/` -- 3 pre-RFC-100 `DC-*` proposals plus `.gitkeep`. Added to this gate's scope
/// by `RFC-105-investigation-ruling-v1.md` §3, correcting an omission in the RFC's first draft:
/// it is where a *new* RFC first appears, so a gate that skipped it would be blind at exactly the
/// moment the naming rule applies.
const RFC_PROPOSED_LEGACY: &[&str] = &[
    ".gitkeep",
    "DC-43-RELEASE-SECURITY-CONTROLS.md",
    "DC-49-PORTABLE-LOGIC-PLATFORM-MATRIX.md",
    "DC-53-REPOSITORY-WIDE-AUTHOR-TRUST-VERIFICATION.md",
];

/// `rfcs/accepted/` -- 34 pre-RFC-100 `DC-*` RFCs plus `.gitkeep`. `100-*.md` through `106-*.md`
/// already conform and need no entry here.
const RFC_ACCEPTED_LEGACY: &[&str] = &[
    ".gitkeep",
    "DC-34-PUBLICATION-IDENTITY-AUTHORITY.md",
    "DC-35-RELEASE-COMPATIBILITY-STATUS-CORRECTION.md",
    "DC-36-EXISTING-OBJECT-PUBLICATION-INTEGRITY.md",
    "DC-37-REQUIRED-FILESYSTEM-DURABILITY.md",
    "DC-38-REF-PUBLICATION-CRASH-RECOVERY.md",
    "DC-41-INTEGRITY-EVIDENCE-CAMPAIGN.md",
    "DC-45-RELEASE-POLICY-TOOLING-CONSOLIDATION.md",
    "DC-50-FIRST-PARTY-SHA256-ROI-DECISION.md",
    "DC-51-PRODUCT-DEPENDENCY-PLACEMENT-GATE.md",
    "DC-54-OPERATION-PATH-VALIDATION-SYMMETRY.md",
    "DC-56-COMMIT-FULL-TREE-SCAN-COMPLIANCE.md",
    "DC-57-ACTIVE-PATCH-THRESHOLDS.md",
    "DC-58-SOURCE-STRUCTURE-AUDIT.md",
    "DC-59-COMMIT-BENCHMARK-HARNESS.md",
    "DC-60-BRANCH-MANAGEMENT-SURFACE.md",
    "DC-61-BRANCH-CLOSURE.md",
    "DC-63-TAG-SURFACE.md",
    "DC-64-BASELINE-RECONSTRUCTION-COST.md",
    "DC-65-TEXT-EDIT-BASELINE-CONTENT.md",
    "DC-66-MULTI-COMMIT-QUEUING.md",
    "DC-67-ORDINARY-USE-CONFORMANCE.md",
    "DC-69-LIFECYCLE-STATE-RETENTION.md",
    "DC-70-PREBUILT-BINARY-DISTRIBUTION.md",
    "DC-71-NON-LINUX-BUILD-CONFORMANCE.md",
    "DC-72-PATH-SAFETY-CONFORMANCE.md",
    "DC-73-NODE-MODEL-APPLY.md",
    "DC-87-WINDOWS-MUTATION.md",
    "DC-93-RELEASE-POLICY-PYTHON-RETIREMENT.md",
    "DC-94-RESPONSIBILITY-MAP-EXECUTABLE-BINDING.md",
    "DC-95-VERIFY-COVERAGE-AND-FINDING-ACCUMULATION.md",
    "DC-96-WINDOWS-ANCHOR-IDENTITY.md",
    "DC-97-WINDOWS-DURABILITY-EVIDENCE.md",
    "DC-98-WINDOWS-CRASH-INJECTION.md",
    "DC-99-WINDOWS-CAPABILITY-PARITY.md",
];

/// `rfcs/done/` -- 50 pre-RFC-100 `DC-*` RFCs and 30 `PR-*` handoffs. `000-rfc-lifecycle-policy.md`
/// already conforms and needs no entry here.
const RFC_DONE_LEGACY: &[&str] = &[
    "DC-10-ROLLBACK-DRAFT-SIGNING.md",
    "DC-11-MAINTAINER-TRUST-STORE.md",
    "DC-12-ARBITRARY-SPAN-TEXT-EDITS.md",
    "DC-13-NONDEFAULT-REF-GENESIS.md",
    "DC-14-ARBITRARY-SPAN-TEXT-INVERSE-ROLLBACK.md",
    "DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md",
    "DC-16-PATCH-ALGEBRA-FOUNDATION.md",
    "DC-17-PATCH-ALGEBRA-EVIDENCE-CONTRACT.md",
    "DC-18-PATCH-ALGEBRA-COMMUTATION-CONFLUENCE.md",
    "DC-19-REPLAY-LIFECYCLE-CRATE-BOUNDARY.md",
    "DC-20-REPLAY-BOUNDARY-STABILIZATION.md",
    "DC-21-MERGE-CONFLICT-EVIDENCE-CONTRACT.md",
    "DC-22-PUBLIC-MERGE-EVIDENCE-UX.md",
    "DC-23-MERGE-EVIDENCE-UX-STABILIZATION.md",
    "DC-24-DATA-MODEL-TRUST-THREAT-DOCS.md",
    "DC-25-MERGE-PLANNING-SURFACE.md",
    "DC-26-DOCUMENTATION-HOME-CORRECTION.md",
    "DC-27-PATCH-ALGEBRA-MERGE-EVIDENCE-CONCEPTS.md",
    "DC-28-DURABILITY-CRASH-RECOVERY-REFERENCE.md",
    "DC-29-VERIFY-DOCTOR-INTEGRITY-RECOVERY-REFERENCE.md",
    "DC-30-KEY-MANAGEMENT-SIGNING-SETUP-GUIDE.md",
    "DC-31-REPOSITORY-LAYOUT-AUTHORITY-REFERENCE.md",
    "DC-32-PATH-WORKTREE-SAFETY-REFERENCE.md",
    "DC-33-CONCURRENCY-LOCKING-REFERENCE.md",
    "DC-39-SIGNATURE-ENVELOPE-AUTHORITY.md",
    "DC-40-STATE-MERKLE-FORMAT-TRANSITION.md",
    "DC-44-MIGRATION-BACKUP-RESTORE-EVIDENCE.md",
    "DC-46-WORKSPACE-RUST-1.85-COMPATIBILITY.md",
    "DC-47-STABLE-CLIPPY-GATE-ALIGNMENT.md",
    "DC-48-LEGACY-CLIPPY-PRODUCTION-RETIREMENT.md",
    "DC-55-FIRST-PARTY-SHA256-REPLACEMENT.md",
    "DC-62-COMMIT-BENCHMARK-MEMORY-AXIS.md",
    "DC-74-MERGE-EXECUTION.md",
    "DC-75-MERGE-BLOCK-LINEAGE.md",
    "DC-76-FILESYSTEM-DURABILITY-CONTRACT.md",
    "DC-77-DOCS-MERMAID-RENDERING.md",
    "DC-78-HISTORY-EXCHANGE.md",
    "DC-79-SHA2-GETRANDOM-UPGRADE.md",
    "DC-80-ED25519-DALEK-UPGRADE.md",
    "DC-81-MACOS-MUTATION.md",
    "DC-82-MUTATION-DISPATCH-COLLAPSE.md",
    "DC-83-TEST-TEMP-DIR-UNIQUENESS.md",
    "DC-84-TEST-HELPER-UNIQUENESS-SWEEP.md",
    "DC-85-MERGE-FROM-RECEIVED-REF.md",
    "DC-86-BUNDLE-DECODER-HARDENING.md",
    "DC-88-DURABILITY-CONTRACT-REQUIREMENT-SHAPE.md",
    "DC-89-PLATFORM-CLAIM-DOCS-ACCURACY.md",
    "DC-90-UNSAFE-CODE-BOUNDARY-GATE.md",
    "DC-91-PUBLICATION-RECORD-SHAPE.md",
    "DC-92-LINEAGE-REPLAY-MEMOIZATION.md",
    "PR-001-IMPLEMENTATION-HANDOFF.md",
    "PR-002-CI-FIX-HANDOFF.md",
    "PR-003-PERSISTENT-STORE-HANDOFF.md",
    "PR-004-WAL-HANDOFF.md",
    "PR-005-CI-FIX-HANDOFF.md",
    "PR-006-VERIFY-HANDOFF.md",
    "PR-007-REF-PUBLICATION-HANDOFF.md",
    "PR-008-COMMIT-SCAFFOLD-HANDOFF.md",
    "PR-009-SEAL-SCAFFOLD-HANDOFF.md",
    "PR-010-VERIFY-HARDENING-HANDOFF.md",
    "PR-011-DOCTOR-HANDOFF.md",
    "PR-012-DOCTOR-REPAIR-HANDOFF.md",
    "PR-013-REF-RECOVERY-HANDOFF.md",
    "PR-014-HISTORY-HANDOFF.md",
    "PR-015-CHECKOUT-PLAN-HANDOFF.md",
    "PR-016-SNAPSHOT-PATH-SAFETY-HANDOFF.md",
    "PR-017-SNAPSHOT-MATERIALIZATION-HANDOFF.md",
    "PR-018-WORKTREE-STATUS-HANDOFF.md",
    "PR-019-WORKTREE-PATCH-HANDOFF.md",
    "PR-020-PATCH-REPLAY-HANDOFF.md",
    "PR-021-PATCH-MATERIALIZATION-HANDOFF.md",
    "PR-022-PATCH-DELETION-HANDOFF.md",
    "PR-023-TEXT-ANCHOR-HANDOFF.md",
    "PR-024-TEXT-REPLAY-HANDOFF.md",
    "PR-025-TEXT-GENERATION-HANDOFF.md",
    "PR-026-INVERSE-PLAN-HANDOFF.md",
    "PR-027-ROLLBACK-PREVIEW-HANDOFF.md",
    "PR-028-ROLLBACK-DRAFT-HANDOFF.md",
    "PR-029-ROLLBACK-DRAFT-VERIFY-HANDOFF.md",
    "PR-030-SEALED-ROLLBACK-HISTORY-HANDOFF.md",
];

/// `rfcs/archive/` -- 3 pre-RFC-100 `DC-*` RFCs plus `.gitkeep`. `101-*.md` and `104-*.md` already
/// conform and need no entry here.
const RFC_ARCHIVE_LEGACY: &[&str] = &[
    ".gitkeep",
    "DC-09-PHASE-4-NODE-MODEL.md",
    "DC-42-PERFORMANCE-MAINTAINABILITY-GATES.md",
    "DC-52-PYTHON-ORACLE-DECOMMISSIONING.md",
];

/// `rfcs/handoffs/` -- 73 pre-RFC-100 `DC-*` handoff directories, plus `consolidation` (not a
/// legacy RFC name; see the module doc). `101-…` through `103-…` and `105-…`/`106-…` already
/// conform and need no entry here.
const RFC_HANDOFFS_LEGACY: &[&str] = &[
    "consolidation",
    "DC-10-rollback-draft-signing",
    "DC-11-maintainer-trust-store",
    "DC-12-arbitrary-span-text-edits",
    "DC-13-nondefault-ref-genesis",
    "DC-14-arbitrary-span-text-inverse-rollback",
    "DC-15-active-session-integrity-hardening",
    "DC-16-patch-algebra-foundation",
    "DC-17-patch-algebra-evidence-contract",
    "DC-18-patch-algebra-commutation-confluence",
    "DC-19-replay-lifecycle-crate-boundary",
    "DC-20-replay-boundary-stabilization",
    "DC-21-merge-conflict-evidence-contract",
    "DC-22-public-merge-evidence-ux",
    "DC-23-merge-evidence-ux-stabilization",
    "DC-24-data-model-trust-threat-docs",
    "DC-25-merge-planning-surface",
    "DC-37-required-filesystem-durability",
    "DC-39-signature-envelope-authority",
    "DC-40-state-merkle-format-transition",
    "DC-41-integrity-evidence-campaign",
    "DC-42-performance-maintainability-gates-superseded",
    "DC-44-migration-backup-restore-evidence",
    "DC-49-portable-logic-platform-matrix",
    "DC-50-first-party-sha256-roi-decision",
    "DC-51-product-dependency-placement-gate",
    "DC-52-python-oracle-decommissioning",
    "DC-53-repository-wide-author-trust-verification",
    "DC-54-operation-path-validation-symmetry",
    "DC-55-first-party-sha256-replacement",
    "DC-56-commit-full-tree-scan-compliance",
    "DC-57-active-patch-thresholds",
    "DC-58-source-structure-audit",
    "DC-59-commit-benchmark-harness",
    "DC-60-branch-management-surface",
    "DC-61-branch-closure",
    "DC-62-commit-benchmark-memory-axis",
    "DC-63-tag-surface",
    "DC-64-baseline-reconstruction-cost",
    "DC-65-text-edit-baseline-content",
    "DC-66-multi-commit-queuing",
    "DC-67-ordinary-use-conformance",
    "DC-69-lifecycle-state-retention",
    "DC-70-prebuilt-binary-distribution",
    "DC-71-non-linux-build-conformance",
    "DC-72-path-safety-conformance",
    "DC-73-node-model-apply",
    "DC-74-merge-execution",
    "DC-75-merge-block-lineage",
    "DC-76-filesystem-durability-contract",
    "DC-77-docs-mermaid-rendering",
    "DC-78-history-exchange",
    "DC-79-sha2-getrandom-upgrade",
    "DC-80-ed25519-dalek-upgrade",
    "DC-81-macos-mutation",
    "DC-82-mutation-dispatch-collapse",
    "DC-83-test-temp-dir-uniqueness",
    "DC-84-test-helper-uniqueness-sweep",
    "DC-85-merge-from-received-ref",
    "DC-86-bundle-decoder-hardening",
    "DC-87-windows-mutation",
    "DC-88-durability-contract-requirement-shape",
    "DC-89-platform-claim-docs-accuracy",
    "DC-90-unsafe-code-boundary-gate",
    "DC-91-publication-record-shape",
    "DC-92-lineage-replay-memoization",
    "DC-93-release-policy-python-retirement",
    "DC-94-responsibility-map-executable-binding",
    "DC-95-verify-coverage-and-finding-accumulation",
    "DC-96-windows-anchor-identity",
    "DC-97-windows-durability-evidence",
    "DC-98-windows-crash-injection",
    "DC-99-windows-capability-parity",
];

/// The four file-governed locations, paired with their own legacy list -- a legacy RFC is expected
/// to *move* among these over its lifecycle (`proposed/` → `accepted/` → `done/`, or → `archive/`),
/// so they are checked together, both for conformance (a name from any of the four lists is accepted
/// at any of the four locations) and for the self-guard (a name declared in exactly one of the four
/// lists must exist exactly once among all four, not just at the location whose list happens to
/// declare it -- see `check`'s own doc for the one exception, entries declared in *more than one*
/// list, which are never movable identities).
const FILE_LOCATIONS: &[(&str, &[&str])] = &[
    ("rfcs/proposed", RFC_PROPOSED_LEGACY),
    ("rfcs/accepted", RFC_ACCEPTED_LEGACY),
    ("rfcs/done", RFC_DONE_LEGACY),
    ("rfcs/archive", RFC_ARCHIVE_LEGACY),
];

pub(super) fn check(root: &std::path::Path, errors: &mut Vec<BoundaryError>) {
    let file_search_locations: Vec<&str> = FILE_LOCATIONS
        .iter()
        .map(|(location, _)| *location)
        .collect();
    let combined_file_legacy: Vec<&str> = FILE_LOCATIONS
        .iter()
        .flat_map(|(_, legacy)| legacy.iter().copied())
        .collect();

    for (location, _) in FILE_LOCATIONS {
        check_entries(
            root,
            location,
            EntryKind::File,
            &combined_file_legacy,
            errors,
        );
    }
    // Self-guard, split by whether a name is a movable RFC identity or an intentionally-repeated
    // placeholder. `.gitkeep` is declared independently in three of the four lists (one entry per
    // directory that needs one) precisely because it is *not* one identity that moved -- three
    // unrelated housekeeping files that happen to share a literal name. Searching for it "exactly
    // once among all four" would report each of its three genuine, correct copies as a duplicate of
    // the others. A name declared in only one list has no such ambiguity: it names one thing, so it
    // is searched across all four locations for that one thing. A name declared in more than one
    // list is checked once per declaring location instead, each independently, matching what the
    // self-guard did before this change -- unaffected by the relaxation, because it was never the
    // "one RFC that might have moved" case the relaxation exists for.
    for (location, legacy) in FILE_LOCATIONS.iter().copied() {
        for name in legacy.iter().copied() {
            let single_name = [name];
            if file_declaration_count(name) > 1 {
                check_self_guard(
                    root,
                    location,
                    &[location],
                    EntryKind::File,
                    &single_name,
                    errors,
                );
            } else {
                check_self_guard(
                    root,
                    location,
                    &file_search_locations,
                    EntryKind::File,
                    &single_name,
                    errors,
                );
            }
        }
    }

    // `rfcs/handoffs/` has no sibling location to move between -- routed through the same two
    // functions as the file locations above rather than a third code path, with its search set
    // simply itself, so there is one mechanism for a future reader to understand rather than two.
    check_entries(
        root,
        "rfcs/handoffs",
        EntryKind::Directory,
        RFC_HANDOFFS_LEGACY,
        errors,
    );
    check_self_guard(
        root,
        "rfcs/handoffs",
        &["rfcs/handoffs"],
        EntryKind::Directory,
        RFC_HANDOFFS_LEGACY,
        errors,
    );
}

/// How many of the four file lists declare `name` -- pulled out of `check` so this decision has its
/// own test against the real, current data, independent of any directory tree. More than one means
/// `name` is an intentionally-repeated placeholder (in practice, only `.gitkeep`, declared in three
/// of the four lists), never a movable RFC identity.
fn file_declaration_count(name: &str) -> usize {
    FILE_LOCATIONS
        .iter()
        .filter(|(_, legacy)| legacy.contains(&name))
        .count()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
}

/// Scan `location`'s directory; flag every entry that neither conforms to the naming pattern nor
/// appears in `allowlist`. `allowlist` may be wider than any single legacy `const` -- for the four
/// file locations it is their union, since a legacy name is allowed at any of them (RFC 105
/// lifecycle-move-friction handoff §3).
fn check_entries(
    root: &std::path::Path,
    location: &str,
    kind: EntryKind,
    allowlist: &[&str],
    errors: &mut Vec<BoundaryError>,
) {
    let directory = root.join(location);
    let Ok(read_dir) = std::fs::read_dir(&directory) else {
        push(
            errors,
            "rfc-naming",
            format!("{location}: directory unreadable"),
        );
        return;
    };
    for entry in read_dir {
        let Ok(entry) = entry else {
            push(
                errors,
                "rfc-naming",
                format!("{location}: entry unreadable"),
            );
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            push(
                errors,
                "rfc-naming",
                format!("{location}: entry type unreadable"),
            );
            continue;
        };
        let is_kind_match = match kind {
            EntryKind::File => file_type.is_file(),
            EntryKind::Directory => file_type.is_dir(),
        };
        if !is_kind_match {
            // Not this location's governed entry kind (e.g. a stray directory under a
            // files-governed location) -- out of this check's scope, not a naming failure.
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let conforms = match kind {
            EntryKind::File => conforms_file(&name),
            EntryKind::Directory => conforms_slug(&name),
        };
        if !conforms && !allowlist.contains(&name.as_ref()) {
            push(
                errors,
                "rfc-naming",
                format!("{location}/{name}: does not conform and is not in the legacy allowlist"),
            );
        }
    }
}

/// The self-guard (RFC criterion 3, amended by RFC 105 lifecycle-move-friction handoff §3's plan
/// review §1): every name in `legacy` must exist as `kind` **exactly once** across
/// `search_locations` -- checked directly against the filesystem, never trusted. Zero matches is the
/// original bypass this guard exists for (pre-authorise a name before it exists). More than one match
/// is a duplicate -- this project has shipped one before (RFC 104, withdrawn the day it was accepted
/// as a duplicate of DC-87) -- and combining the four file locations for conformance above would have
/// silently reopened that hole if this guard still only asked "at least one", so it asks for exactly
/// one instead. `declared_in` is kept only for the error message's provenance (which `const` to go
/// edit), separate from `search_locations` (where existence is actually checked).
fn check_self_guard(
    root: &std::path::Path,
    declared_in: &str,
    search_locations: &[&str],
    kind: EntryKind,
    legacy: &[&str],
    errors: &mut Vec<BoundaryError>,
) {
    for name in legacy {
        let matches: Vec<&str> = search_locations
            .iter()
            .copied()
            .filter(|location| {
                let path = root.join(location).join(name);
                match kind {
                    EntryKind::File => path.is_file(),
                    EntryKind::Directory => path.is_dir(),
                }
            })
            .collect();
        match matches.len() {
            0 => push(
                errors,
                "rfc-naming",
                format!(
                    "{declared_in}/{name}: allowlisted but does not exist under any of {}",
                    search_locations.join(", ")
                ),
            ),
            1 => {}
            _ => push(
                errors,
                "rfc-naming",
                format!(
                    "{declared_in}/{name}: allowlisted but exists more than once, at {}",
                    matches.join(", ")
                ),
            ),
        }
    }
}

/// `^[0-9]{3}-[a-z0-9]+(-[a-z0-9]+)*\.md$` -- a file name.
fn conforms_file(name: &str) -> bool {
    name.strip_suffix(".md").is_some_and(conforms_slug)
}

/// `^[0-9]{3}-[a-z0-9]+(-[a-z0-9]+)*$` -- a bare slug (a directory name, or a file name with its
/// `.md` suffix already stripped). Hand-rolled rather than a `regex` dependency: `tools/release-\
/// policy` has none today, and this pattern is simple enough that adding one would be a new
/// supply-chain entrant for a handful of ASCII-byte comparisons.
fn conforms_slug(value: &str) -> bool {
    let Some((number, rest)) = value.split_once('-') else {
        return false;
    };
    if number.len() != 3 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    !rest.is_empty()
        && rest.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

#[cfg(test)]
#[path = "rfc_naming/tests.rs"]
mod tests;
