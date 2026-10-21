#![allow(clippy::expect_used, clippy::indexing_slicing)]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::reason;

fn snapshot(sequence: &str, status: &str) -> Value {
    json!({
        "schema_version": 1,
        "sequence": sequence,
        "version": "0.19.0",
        "overall_status": status,
        "prior_snapshot": null,
        "tag": {
            "name": "0.19.0",
            "object_id": "0000000000000000000000000000000000000000",
            "peeled_commit": "0000000000000000000000000000000000000000",
            "release_tag_verification": {
                "status": "not-observed",
                "signer_primary_fingerprint": null,
                "authority_path": null,
                "authority_blob_id": null,
                "verifier_result": null
            }
        },
        "archive": {
            "name": "prikk-v0.19.0.tar.gz",
            "checksum_name": "prikk-v0.19.0.tar.gz.sha256",
            "archive_sha256": null,
            "checksum_sha256": null,
            "archive_attached": false,
            "checksum_attached": false
        },
        "crates": [{
            "name": "prikk",
            "version": "0.19.0",
            "publish_level": 5,
            "staged_sha256": null,
            "registry_checksum": null,
            "fetched_sha256": null,
            "published": false,
            "registry_visible": false
        }],
        "release_page": {"status": "absent"},
        "pages": {
            "status": "pending",
            "deployed_commit": null,
            "inapplicable_ruling": null
        },
        "governance": null,
        "attempts": []
    })
}

fn pair(old_status: &str, new_status: &str) -> (Value, Value, Vec<u8>, Vec<u8>) {
    let old = snapshot("001", old_status);
    let old_bytes = serde_json::to_vec(&old).expect("old bytes");
    let mut new = snapshot("002", new_status);
    new["prior_snapshot"] = json!({
        "name": "prikk-0.19.0-release-evidence-001.json",
        "sha256": Sha256::digest(&old_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    });
    new["attempts"] = json!([{"attempt": "next"}]);
    let new_bytes = serde_json::to_vec(&new).expect("new bytes");
    (old, new, old_bytes, new_bytes)
}

#[test]
fn bounded_transition_matrix_matches_dc35_relation() {
    let statuses = ["pending", "partial", "complete", "superseded"];
    let allowed = [
        ("pending", "pending"),
        ("pending", "partial"),
        ("pending", "complete"),
        ("pending", "superseded"),
        ("partial", "partial"),
        ("partial", "complete"),
        ("partial", "superseded"),
        ("complete", "superseded"),
    ];
    for old_status in statuses {
        for new_status in statuses {
            let (old, new, old_bytes, new_bytes) = pair(old_status, new_status);
            let actual = reason(&old, &new, &old_bytes, &new_bytes);
            assert_eq!(
                actual.is_none(),
                allowed.contains(&(old_status, new_status)),
                "{old_status} -> {new_status}"
            );
        }
    }
}

#[test]
fn attempt_history_requires_strict_prefix_growth() {
    let (mut old, mut new, _, _) = pair("pending", "partial");
    old["attempts"] = json!([{"attempt": "old"}]);
    new["attempts"] = json!([{"attempt": "changed"}]);
    let old_bytes = serde_json::to_vec(&old).expect("old bytes");
    new["prior_snapshot"]["sha256"] = json!(
        Sha256::digest(&old_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let new_bytes = serde_json::to_vec(&new).expect("new bytes");
    assert_eq!(
        reason(&old, &new, &old_bytes, &new_bytes),
        Some("evidence-transition-or-attempt-prefix")
    );
}

#[test]
fn observed_pages_object_is_fully_immutable() {
    for field in ["status", "deployed_commit"] {
        let (mut old, mut new, _, _) = pair("pending", "partial");
        old["pages"] = json!({
            "status": "published",
            "deployed_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "inapplicable_ruling": null
        });
        new["pages"] = old["pages"].clone();
        new["pages"][field] = if field == "status" {
            json!("failed")
        } else {
            json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        };
        let old_bytes = serde_json::to_vec(&old).expect("old bytes");
        new["prior_snapshot"]["sha256"] = json!(
            Sha256::digest(&old_bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let new_bytes = serde_json::to_vec(&new).expect("new bytes");
        assert_eq!(
            reason(&old, &new, &old_bytes, &new_bytes),
            Some("evidence-byte-identity-or-link")
        );
    }
}

#[test]
fn observed_publication_fields_are_monotonic() {
    for mutation in 0..4 {
        let (mut old, mut new, _, _) = pair("partial", "complete");
        match mutation {
            0 => {
                old["archive"]["archive_attached"] = json!(true);
                new["archive"]["archive_attached"] = json!(false);
            }
            1 => {
                old["crates"][0]["published"] = json!(true);
                new["crates"][0]["published"] = json!(false);
            }
            2 => {
                old["release_page"]["status"] = json!("published");
                new["release_page"]["status"] = json!("absent");
            }
            _ => {
                old["crates"][0]["staged_sha256"] = json!("a".repeat(64));
                new["crates"][0]["staged_sha256"] = json!("b".repeat(64));
            }
        }
        let old_bytes = serde_json::to_vec(&old).expect("old bytes");
        new["prior_snapshot"]["sha256"] = json!(
            Sha256::digest(&old_bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let new_bytes = serde_json::to_vec(&new).expect("new bytes");
        assert_eq!(
            reason(&old, &new, &old_bytes, &new_bytes),
            Some("evidence-byte-identity-or-link")
        );
    }
}

#[test]
fn byte_identity_precedes_transition_reason() {
    let (old, new, old_bytes, _) = pair("superseded", "pending");
    let new_bytes = serde_json::to_vec(&snapshot("002", "complete")).expect("different bytes");
    assert_eq!(
        reason(&old, &new, &old_bytes, &new_bytes),
        Some("evidence-byte-identity-or-link")
    );
}
