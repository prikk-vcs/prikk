#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use serde_json::{Value, json};

use super::{crate_set_mismatch, frozen_corpus_crates, single_reason, value_array};
use crate::oracle::Oracle;

#[test]
fn active_governance_hold_excludes_completion_before_artifact_checks() {
    let snapshot = json!({
        "overall_status": "complete",
        "governance": {
            "record": "public-record",
            "action_or_classification": "authority transaction",
            "old_authority_blob_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "new_authority_blob_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "classification": null,
            "hold_started_at": "2026-07-10T00:00:00Z",
            "hold_ended_at": null,
            "hold_lift": null,
            "transaction_type": "addition",
            "old_authorized_fingerprints": [
                "1111111111111111111111111111111111111111"
            ],
            "new_authorized_fingerprints": [
                "1111111111111111111111111111111111111111",
                "2222222222222222222222222222222222222222"
            ],
            "approvals": [
                {"person":"a","role":"maintainer-administrator","record":"a"},
                {"person":"b","role":"architect-security","record":"b"}
            ],
            "authority_proof": {
                "state": "verified",
                "reason": null,
                "introduced_signers": [{
                    "primary_fingerprint":
                        "2222222222222222222222222222222222222222",
                    "verifier_result": "verified"
                }]
            }
        },
        "tag": Value::Null
    });
    assert_eq!(
        single_reason(&snapshot, &frozen_corpus_crates()),
        Some("governance-review-or-hold")
    );
}

/// The literal that remains is measured against the corpus it describes, not trusted.
///
/// **Measured, not assumed**: the 57 release-evidence cases hold 89 documents (`current`, plus
/// `prior` where there is one). 88 list exactly `FROZEN_CORPUS_CRATES`. One does not, on purpose --
/// `crate-graph-order-mismatch` lists `prikk-hash` twice -- and it must stay an `invalid` case whose
/// reason is the crate check it exists to exercise. A first version of this test asserted all 89
/// matched, and failed on that fixture; the claim was wrong, not the corpus.
///
/// So this pins both halves: every deviation is an invalid, crate-check case, and the deviations are
/// exactly the one known. A rewritten fixture, a literal edited to follow the workspace, or a corpus
/// that grows all fail here by name.
#[test]
fn the_frozen_literal_is_what_the_corpus_lists() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root");
    let oracle = Oracle::load(root).expect("oracle");
    let expected = frozen_corpus_crates();
    let mut documents = 0;
    let mut deviating: Vec<String> = Vec::new();
    for case in oracle
        .manifest
        .cases
        .iter()
        .filter(|case| case.suite_id == "release-evidence")
    {
        let table = crate::json::parse(oracle.input(case, "fixture-table").expect("input"))
            .expect("fixture parses");
        for side in ["current", "prior"] {
            let Some(document) = table.get(side).filter(|value| !value.is_null()) else {
                continue;
            };
            documents += 1;
            let crates = value_array(document, "crates").unwrap_or_default();
            if let Some(mismatch) = crate_set_mismatch(crates, &expected) {
                assert_eq!(
                    case.expected.case_outcome, "invalid",
                    "{}:{side} deviates from the literal yet is expected valid: {mismatch}",
                    case.case_id
                );
                assert_eq!(
                    case.expected.primary_reason, "evidence-tag-or-artifact",
                    "{}:{side} deviates from the literal for a reason other than the crate check: \
                     {mismatch}",
                    case.case_id
                );
                deviating.push(format!("{}:{side}", case.case_id));
            }
        }
    }
    assert_eq!(
        documents, 89,
        "the frozen corpus changed size -- re-measure before trusting this"
    );
    assert_eq!(
        deviating,
        ["crate-graph-order-mismatch:current"],
        "the documents that deviate from the frozen literal changed"
    );
}

/// A mismatch names the crate, whichever way it goes.
#[test]
fn a_crate_set_mismatch_names_the_crate() {
    let rows = |names: &[(&str, u64)]| -> Vec<Value> {
        names
            .iter()
            .map(|(name, level)| json!({"name": name, "publish_level": level}))
            .collect()
    };
    let expected: Vec<(String, u64)> = [("a", 1), ("b", 2)]
        .iter()
        .map(|(name, level)| ((*name).to_owned(), *level))
        .collect();
    assert_eq!(
        crate_set_mismatch(&rows(&[("a", 1), ("b", 2)]), &expected),
        None
    );
    let gained = crate_set_mismatch(&rows(&[("a", 1), ("b", 2), ("c", 3)]), &expected).unwrap();
    assert!(gained.contains("unexpected [\"c\"]"), "{gained}");
    let lost = crate_set_mismatch(&rows(&[("a", 1)]), &expected).unwrap();
    assert!(lost.contains("missing [\"b\"]"), "{lost}");
    let reordered = crate_set_mismatch(&rows(&[("b", 2), ("a", 1)]), &expected).unwrap();
    assert!(reordered.contains("crate row 0: expected a"), "{reordered}");
}
