//! `prikk sync pending --format json` (0.56.0 step 1 part 1a; planeter letter 2026-10-21 §2):
//! `sync-pending-v1`, the one JSON document this module prints.

use prikk_store::{PendingClaimSignatureOutcome, PendingPatchEntry};

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::stdout::println;

use super::verification::escape_json_string;

/// Print `prikk sync pending --format json`: `sync-pending-v1`. `entries` is already ordered by
/// patch id (`pending_patches_with_claims`'s own order, unchanged from `accepted_but_unsealed_patch_ids`);
/// each entry's own `claims` is already ordered by claim id -- neither is re-sorted here.
pub(crate) fn print_sync_pending_json(entries: &[PendingPatchEntry]) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"sync-pending-v1\",\n");
    json.push_str("  \"patches\": [");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"patch_id\": ");
        json.push_str(&escape_json_string(&entry.patch_id.to_string()));
        json.push_str(", \"claims\": [");
        for (claim_index, claim) in entry.claims.iter().enumerate() {
            if claim_index > 0 {
                json.push(',');
            }
            json.push_str("\n      {\"claim_id\": ");
            json.push_str(&escape_json_string(&claim.claim_id.to_string()));
            json.push_str(", \"target_block_id\": ");
            json.push_str(&escape_json_string(&claim.target_block_id.to_string()));
            json.push_str(", \"parent_block_ids\": [");
            for (parent_index, parent_block_id) in claim.parent_block_ids.iter().enumerate() {
                if parent_index > 0 {
                    json.push_str(", ");
                }
                json.push_str(&escape_json_string(&parent_block_id.to_string()));
            }
            json.push_str("], \"signature\": ");
            push_signature(&mut json, &claim.signature);
            json.push('}');
        }
        if !entry.claims.is_empty() {
            json.push_str("\n    ");
        }
        json.push_str("]}");
    }
    if !entries.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n");
    json.push('}');
    println!("{json}");
}

fn push_signature(json: &mut String, signature: &PendingClaimSignatureOutcome) {
    json.push_str("{\"outcome\": ");
    match signature {
        PendingClaimSignatureOutcome::Sound { key_id } => {
            json.push_str(&escape_json_string("sound"));
            json.push_str(", \"key_id\": ");
            json.push_str(&escape_json_string(key_id));
        }
        PendingClaimSignatureOutcome::Unverifiable { key_id } => {
            json.push_str(&escape_json_string("unverifiable"));
            json.push_str(", \"key_id\": ");
            json.push_str(&escape_json_string(key_id));
        }
        PendingClaimSignatureOutcome::Invalid { key_id, reason } => {
            json.push_str(&escape_json_string("invalid"));
            json.push_str(", \"key_id\": ");
            match key_id {
                Some(key_id) => json.push_str(&escape_json_string(key_id)),
                None => json.push_str("null"),
            }
            json.push_str(", \"reason\": ");
            json.push_str(&escape_json_string(reason));
        }
    }
    json.push('}');
}
