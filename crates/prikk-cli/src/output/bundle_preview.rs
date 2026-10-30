//! JSON output for `bundle preview --format json` (RFC 144 §4m): what a bundle would do to a
//! repository, machine-branchable per §4m.4 -- *applies cleanly*, *applies with conflicts*, *does
//! not connect* are a field here, never prose to parse or an exit code to infer from.

use prikk_store::{BundlePreviewConflict, BundlePreviewReport};

use crate::stdout::println;

use super::verification::escape_json_string;

/// §4m.4's own honesty limit, stated in the output itself, not only in the docs: until rename
/// authoring lands (RFC 144 increment 3), a bundle containing delete+create for what was really a
/// move previews as delete+create, and this says so rather than let a reader infer intent the
/// repository does not yet record.
const RENAME_HONESTY_NOTE: &str = "a moved path is not yet distinguishable from a delete plus a \
     create in this preview -- until rename authoring lands, do not infer that any delete+create \
     pair shown here was a move";

pub(crate) fn print_bundle_preview_json(report: &BundlePreviewReport) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"bundle-preview-v1\",\n");
    json.push_str("  \"bundle_ref\": ");
    json.push_str(&escape_json_string(&report.bundle_ref_name));
    json.push_str(",\n  \"local_ref\": ");
    json.push_str(&escape_json_string(&report.local_ref_name));
    json.push_str(",\n  \"connectivity\": ");
    json.push_str(&escape_json_string(report.connectivity.as_str()));
    // 0.56.0 step 3a (A2): additive -- "connectivity" above keeps its existing five values
    // unchanged (planeter already reads it); "class" carries the finer six-value split.
    json.push_str(",\n  \"class\": ");
    json.push_str(&escape_json_string(report.class.as_str()));
    json.push_str(",\n  \"ref_state_chain\": ");
    match report.ref_state_chain {
        Some(chain) => json.push_str(&escape_json_string(chain.as_str())),
        None => json.push_str("null"),
    }
    // 0.56.0 step 3c2 (R3): additive, and named so it cannot be confused with "class"/
    // "ref_state_chain" above -- those answer the `--ref` comparison asked for inspection;
    // these answer what `bundle import` of this exact delivery would actually do, against the
    // received pointer at `remotes/<bundle_ref>`, which may be a different name entirely.
    json.push_str(",\n  \"import_class\": ");
    json.push_str(&escape_json_string(report.import_class.as_str()));
    json.push_str(",\n  \"import_ref_state_chain\": ");
    match report.import_ref_state_chain {
        Some(chain) => json.push_str(&escape_json_string(chain.as_str())),
        None => json.push_str("null"),
    }

    json.push_str(",\n  \"conflict\": ");
    match &report.conflict {
        Some(conflict) => json.push_str(&escape_json_string(conflict.as_str())),
        None => json.push_str("null"),
    }
    json.push_str(",\n  \"conflict_detail\": ");
    match &report.conflict {
        Some(BundlePreviewConflict::Conflict { detail }) => {
            json.push_str(&escape_json_string(detail));
        }
        Some(BundlePreviewConflict::Undetermined { reason }) => {
            json.push_str(&escape_json_string(reason));
        }
        _ => json.push_str("null"),
    }

    json.push_str(",\n  \"sealed_by\": [");
    for (index, key_id) in report.sealed_by.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&escape_json_string(key_id));
    }
    json.push_str("],\n  \"sealed_by_note\": ");
    json.push_str(&escape_json_string(
        "recorded signer identity, continuity only -- not a trust decision",
    ));

    json.push_str(",\n  \"effects\": [");
    for (index, effect) in report.effects.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"path\": ");
        json.push_str(&escape_json_string(&effect.path));
        json.push_str(", \"kind\": ");
        json.push_str(&escape_json_string(effect.kind.as_str()));
        json.push_str(", \"current_bytes\": ");
        push_optional_u64(&mut json, effect.current_bytes);
        json.push_str(", \"after_bytes\": ");
        push_optional_u64(&mut json, effect.after_bytes);
        json.push('}');
    }
    if !report.effects.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("],\n  \"rename_note\": ");
    json.push_str(&escape_json_string(RENAME_HONESTY_NOTE));

    json.push_str(",\n  \"manifest\": ");
    match &report.manifest {
        Some(manifest) => {
            json.push_str("{\"repository_format\": ");
            json.push_str(&manifest.repository_format.to_string());
            json.push_str(", \"tool_version\": ");
            json.push_str(&escape_json_string(&manifest.tool_version));
            json.push('}');
        }
        None => json.push_str("null"),
    }

    json.push_str("\n}");
    println!("{json}");
}

fn push_optional_u64(json: &mut String, value: Option<u64>) {
    match value {
        Some(value) => json.push_str(&value.to_string()),
        None => json.push_str("null"),
    }
}

/// Plain-text `bundle preview` output -- the default, human-facing rendering. `--format json` is
/// the machine-branchable surface (§4m.4); this exists for the same reason `bundle export`/
/// `import`/`verify` all have their own plain form.
pub(crate) fn print_bundle_preview_plain(report: &BundlePreviewReport) {
    println!("bundle: {}", report.bundle_ref_name);
    println!("previewed against: {}", report.local_ref_name);
    println!("connectivity: {}", report.connectivity.as_str());
    println!("class: {}", report.class.as_str());
    if let Some(chain) = report.ref_state_chain {
        println!("ref state chain: {}", chain.as_str());
    }
    // 0.56.0 step 3c2 (R3): what `bundle import` of this exact delivery would actually do --
    // against the received pointer at `remotes/<bundle_ref>`, independent of what "class" above
    // just answered against the `--ref` comparison.
    println!(
        "bundle import of this delivery would classify as: {}",
        report.import_class.as_str()
    );
    if let Some(chain) = report.import_ref_state_chain {
        println!("  ref state chain: {}", chain.as_str());
    }
    match &report.conflict {
        Some(BundlePreviewConflict::AppliesCleanly) => println!("conflict: none, applies cleanly"),
        Some(BundlePreviewConflict::Conflict { detail }) => {
            println!("conflict: yes -- {detail}");
        }
        Some(BundlePreviewConflict::Undetermined { reason }) => {
            println!("conflict: undetermined -- {reason}");
        }
        None => {}
    }
    if report.sealed_by.is_empty() {
        println!("sealed by: (no MAINTAINER signature found on the bundle's own RefState)");
    } else {
        println!(
            "sealed by: {} (continuity only, not a trust decision)",
            report.sealed_by.join(", ")
        );
    }
    println!("effects: {}", report.effects.len());
    for effect in &report.effects {
        println!("  {} {}", effect.kind.as_str(), effect.path);
    }
    println!("note: {RENAME_HONESTY_NOTE}");
}
