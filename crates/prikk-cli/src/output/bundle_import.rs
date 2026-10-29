//! JSON output for `bundle import --format json` (0.56.0 step 3b, design review v2 ruling A4):
//! `bundle-import-report-v1` carries every field the plain-text form already printed (3a and
//! earlier), additively, plus `pointer_updated` (3a, not yet carried to JSON before this round).
//! The text form itself is unchanged by this round -- see `run_import` in `crate::bundle`.

use prikk_store::BundleImportReport;

use crate::stdout::println;

use super::verification::escape_json_string;

pub(crate) fn print_bundle_import_json(report: &BundleImportReport) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"bundle-import-report-v1\",\n");
    json.push_str("  \"ref_name\": ");
    json.push_str(&escape_json_string(&report.ref_name));
    json.push_str(",\n  \"ref_state_id\": ");
    json.push_str(&escape_json_string(&report.ref_state_id.to_string()));
    json.push_str(",\n  \"object_count\": ");
    json.push_str(&report.object_count.to_string());
    json.push_str(",\n  \"written_object_count\": ");
    json.push_str(&report.written_object_count.to_string());
    json.push_str(",\n  \"merged_object_count\": ");
    json.push_str(&report.merged_object_count.to_string());

    // Same four-field shape `archive import`'s own `prepo-import-report-v1` already uses for the
    // identical `DroppedSignature` type (`print_import_report_json` in `crate::archive`) -- kept
    // consistent across the two sibling schemas rather than inventing a second shape.
    json.push_str(",\n  \"dropped_signatures\": [");
    for (index, dropped) in report.dropped_signatures.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"object_type\": ");
        json.push_str(&escape_json_string(&dropped.object_type.to_string()));
        json.push_str(", \"object_id\": ");
        json.push_str(&escape_json_string(&dropped.object_id.to_string()));
        json.push_str(", \"key_id\": ");
        json.push_str(&escape_json_string(&dropped.key_id));
        json.push_str(", \"reason\": ");
        json.push_str(&escape_json_string(dropped.reason.describe()));
        json.push('}');
    }
    if !report.dropped_signatures.is_empty() {
        json.push_str("\n  ");
    }
    json.push(']');

    json.push_str(",\n  \"recorded_author_key_count\": ");
    json.push_str(&report.recorded_author_key_count.to_string());

    // 0.56.0 step 3a's own fields, carried to JSON for the first time this round -- the same
    // "class"/"ref_state_chain" names `bundle-preview-v1` already shipped (A4: "the pending
    // membership" note doesn't apply here, but the naming carries over verbatim for consistency
    // between the two sibling schemas).
    json.push_str(",\n  \"class\": ");
    json.push_str(&escape_json_string(report.class.as_str()));
    json.push_str(",\n  \"ref_state_chain\": ");
    match report.ref_state_chain {
        Some(chain) => json.push_str(&escape_json_string(chain.as_str())),
        None => json.push_str("null"),
    }
    json.push_str(",\n  \"pointer_updated\": ");
    json.push_str(if report.pointer_updated {
        "true"
    } else {
        "false"
    });

    json.push_str("\n}");
    println!("{json}");
}
