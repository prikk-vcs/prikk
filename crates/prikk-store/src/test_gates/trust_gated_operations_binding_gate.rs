//! RFC 118 stage 3 — the trust-gated-operations binding gate.
//!
//! `docs/src/reference/trust-threat-model.md` used to state the set of operations gated on
//! [`crate::trust::verify_signer_trusted`] as hand-maintained prose. That prose was derived by hand
//! twice this project's own session -- eight surfaces correctly once, two-instead-of-three wrong
//! the other time. [`GatedOperation`] (`trust.rs`) is now the single declared source; this `#[test]`
//! binds the page to it bidirectionally, the same shape stage 2's join gate and RFC 114's Gate A
//! both already use:
//!
//! - every [`GatedOperation`] variant is named in the page's gated-operations list;
//! - every operation the page's list names is a real variant.
//!
//! **This gate proves the page and the enum agree on which operations gate. It does not, and
//! cannot, prove every operation that *ought* to gate does** -- see the page's own note on that
//! distinction, which this gate does not and must not silently imply away.

#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use crate::trust::GatedOperation;

const TRUST_THREAT_MODEL_PATH: &str = "docs/src/reference/trust-threat-model.md";
const LIST_START_MARKER: &str = "<!-- rfc118-stage3-gated-operations:start -->";
const LIST_END_MARKER: &str = "<!-- rfc118-stage3-gated-operations:end -->";

/// Every `GatedOperation` variant, enumerated by hand once -- adding a variant to the enum without
/// adding it here leaves this list incomplete; `all_gated_operations_is_exhaustive` below catches
/// that the same way `signature_contract_tests::vectors::all_object_types_is_exhaustive` catches
/// its own list going stale.
const ALL_GATED_OPERATIONS: &[GatedOperation] = &[
    GatedOperation::Seal,
    GatedOperation::Merge,
    GatedOperation::SyncBuild,
    GatedOperation::SyncSeal,
    GatedOperation::SyncAdoptTag,
    GatedOperation::TagCreate,
    GatedOperation::BranchCreate,
    GatedOperation::BranchClose,
];

/// The exact backtick-quoted marker each operation is named by in the page's bound list -- an
/// exhaustive match, so a new `GatedOperation` variant with no marker fails to compile rather than
/// silently reading as unnamed.
fn marker(operation: GatedOperation) -> &'static str {
    match operation {
        GatedOperation::Seal => "seal",
        GatedOperation::Merge => "merge",
        GatedOperation::SyncBuild => "sync build",
        GatedOperation::SyncSeal => "sync seal",
        GatedOperation::SyncAdoptTag => "sync adopt-tag",
        GatedOperation::TagCreate => "prikk tag create",
        GatedOperation::BranchCreate => "prikk branch create",
        GatedOperation::BranchClose => "prikk branch close",
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("prikk-store's manifest dir has a workspace root two levels up")
        .to_path_buf()
}

fn read_trust_threat_model() -> String {
    let root = repo_root();
    fs::read_to_string(root.join(TRUST_THREAT_MODEL_PATH))
        .unwrap_or_else(|err| panic!("{TRUST_THREAT_MODEL_PATH} must read: {err}"))
}

/// The text strictly between the two HTML comment markers -- scoping every check to the declared
/// list, not the whole page (which discusses many other, ungated commands, and would produce false
/// positives/negatives if scanned generically).
fn bound_list_text(text: &str) -> &str {
    let after_start = text
        .find(LIST_START_MARKER)
        .map(|start| start + LIST_START_MARKER.len())
        .unwrap_or_else(|| panic!("{TRUST_THREAT_MODEL_PATH} is missing {LIST_START_MARKER}"));
    let end = text[after_start..]
        .find(LIST_END_MARKER)
        .map(|rel| after_start + rel)
        .unwrap_or_else(|| panic!("{TRUST_THREAT_MODEL_PATH} is missing {LIST_END_MARKER}"));
    &text[after_start..end]
}

/// Every backtick-quoted span within `text`, in order.
fn backtick_spans(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    while let Some(rel_start) = text[offset..].find('`') {
        let start = offset + rel_start;
        let Some(rel_end) = text[start + 1..].find('`') else {
            break;
        };
        let end = start + 1 + rel_end;
        spans.push(&text[start + 1..end]);
        offset = end + 1;
    }
    spans
}

#[test]
fn all_gated_operations_is_exhaustive() {
    for operation in ALL_GATED_OPERATIONS {
        match operation {
            GatedOperation::Seal
            | GatedOperation::Merge
            | GatedOperation::SyncBuild
            | GatedOperation::SyncSeal
            | GatedOperation::SyncAdoptTag
            | GatedOperation::TagCreate
            | GatedOperation::BranchCreate
            | GatedOperation::BranchClose => {}
        }
    }
    assert_eq!(ALL_GATED_OPERATIONS.len(), 8);
}

/// Forward direction: every `GatedOperation` variant is named in the page's list.
#[test]
fn every_gated_operation_is_named_in_the_trust_threat_model() {
    let text = read_trust_threat_model();
    let named = backtick_spans(bound_list_text(&text));
    for &operation in ALL_GATED_OPERATIONS {
        assert!(
            named.contains(&marker(operation)),
            "GatedOperation::{operation:?} (marker {:?}) is not named in {TRUST_THREAT_MODEL_PATH}'s \
             gated-operations list",
            marker(operation)
        );
    }
}

/// Reverse direction: every operation the page's list names is a real variant.
#[test]
fn every_named_gated_operation_in_the_trust_threat_model_is_real() {
    let text = read_trust_threat_model();
    for span in backtick_spans(bound_list_text(&text)) {
        assert!(
            ALL_GATED_OPERATIONS
                .iter()
                .any(|&operation| marker(operation) == span),
            "{TRUST_THREAT_MODEL_PATH} names `{span}` as gated, but no GatedOperation variant has \
             that marker"
        );
    }
}
