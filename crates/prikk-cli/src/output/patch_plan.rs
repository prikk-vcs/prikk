//! JSON output for `checkout --patch-plan --format json` (RFC 143): content at a replayed point,
//! for exactly the requested paths.

use prikk_store::{PatchPlanContent, PatchPlanContentReport};

use crate::stdout::println;

use super::verification::escape_json_string;

fn push_content(json: &mut String, content: &PatchPlanContent) {
    match content {
        PatchPlanContent::Text(bytes) => {
            json.push_str("{\"kind\": \"text\", \"text\": ");
            json.push_str(&escape_json_string(&String::from_utf8_lossy(bytes)));
            json.push('}');
        }
        PatchPlanContent::Binary { blob_id, size } => {
            json.push_str("{\"kind\": \"binary\", \"blob_id\": ");
            json.push_str(&escape_json_string(&blob_id.to_string()));
            json.push_str(&format!(", \"size\": {size}}}"));
        }
        PatchPlanContent::Opaque { size } => {
            json.push_str(&format!("{{\"kind\": \"opaque\", \"size\": {size}}}"));
        }
    }
}

/// Print `checkout --patch-plan --format json`: `patch-plan-content-v1`, settling the format for
/// this surface and nothing else (RFC 143 §5, scoped the same way RFC 142 §5 scoped `show`'s own).
pub(crate) fn print_patch_plan_content_json(report: &PatchPlanContentReport) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"patch-plan-content-v1\",\n");
    json.push_str("  \"ref\": ");
    json.push_str(&escape_json_string(&report.ref_name));
    json.push_str(",\n  \"target_block_id\": ");
    json.push_str(&escape_json_string(&report.target_block_id.to_string()));
    json.push_str(",\n  \"coverage\": {\n    \"applied_operation_kinds\": [");
    for (index, kind) in report.coverage.applied_operation_kinds.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&escape_json_string(kind));
    }
    json.push_str("],\n    \"walk\": ");
    json.push_str(&escape_json_string(report.coverage.walk));
    json.push_str("\n  },\n");

    json.push_str("  \"content\": [");
    for (index, entry) in report.entries.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"path\": ");
        json.push_str(&escape_json_string(&entry.path));
        json.push_str(&format!(", \"mode\": {}, \"content\": ", entry.mode));
        push_content(&mut json, &entry.content);
        json.push('}');
    }
    if !report.entries.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("],\n");

    json.push_str("  \"not_found\": [");
    for (index, path) in report.not_found.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&escape_json_string(path));
    }
    json.push_str("]\n");
    json.push('}');
    println!("{json}");
}
