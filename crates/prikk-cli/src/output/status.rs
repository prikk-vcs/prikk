//! `prikk status --format json` (RFC 140): `status-report-v1` -- named for the tool (`status`) and
//! versioned like this repository's other machine-readable schemas. It settles the format for
//! `status` and for nothing else (RFC 140 §6); a third command adopting `--format json` is a third
//! decision.
//!
//! **Carries everything the prose form carries** (RFC 140 §2), plus the queue enumeration the
//! prose form cannot: for each queued patch, in queue order, its patch id, its operations' kinds,
//! and the paths those operations affect -- resolved against the folded worktree baseline for
//! node-addressed operations (`prikk_store::enumerate_queued_patches`), never fatal on an
//! unresolved node id (RFC 140 §4).

use crate::stdout::println;
use prikk_object::ObjectId;
use prikk_store::{
    QueuedOperationContent, QueuedOperationEntry, QueuedPatchEntry, QueuedPathResolution,
    RepositoryLayout,
};

use super::verification::escape_json_string;

/// Which of the two DC-57 warn/hard-limit conditions currently holds, if either -- the same
/// three-way state the prose form's own `if`/`else if` renders as at most one warning line.
pub(crate) enum QueueThresholdStatus {
    /// Below both thresholds: no warning line in the prose form, and no `threshold_status` key.
    None,
    /// At or above the recommended threshold, below the hard limit.
    Warn,
    /// At or above the configured hard limit; `commit` refuses until `seal` runs.
    HardLimit,
}

impl QueueThresholdStatus {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Warn => "warn",
            Self::HardLimit => "hard-limit",
        }
    }
}

/// Which of the three states `ActiveRefMetadata` was in when the queue's owning ref was read, for
/// a non-empty queue -- `None` values below mean "queue is empty," not "unknown."
pub(crate) enum QueueTarget {
    /// The queue's owning ref, read successfully.
    Ref(String),
    /// `ActiveRefMetadata::Missing` -- the prose form prints `<missing metadata>`.
    MissingMetadata,
    /// `ActiveRefMetadata::Invalid` -- the prose form prints `<malformed metadata>`.
    MalformedMetadata,
}

/// Print `prikk status --format json`. `queue_target`/`threshold` are `None` only when the queue
/// is empty (RFC 140 §2: an empty queue is a valid, complete answer, not an absent field -- the
/// document still carries `"queue": {"count": 0, ..., "patches": []}`).
pub(crate) fn print_status_json(
    layout: &RepositoryLayout,
    active_wal_records: usize,
    trailing_partial_wal_bytes: usize,
    heads_main_ref_state: Option<ObjectId>,
    queue_target: Option<&QueueTarget>,
    threshold: Option<(&QueueThresholdStatus, usize, usize)>,
    patches: &[QueuedPatchEntry],
) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"status-report-v1\",\n");
    json.push_str(&format!(
        "  \"repository\": {},\n",
        escape_json_string(&layout.prikk_dir().display().to_string())
    ));
    json.push_str(&format!(
        "  \"active_wal_records\": {active_wal_records},\n"
    ));
    json.push_str(&format!(
        "  \"trailing_partial_wal_bytes\": {trailing_partial_wal_bytes},\n"
    ));
    match heads_main_ref_state {
        Some(id) => json.push_str(&format!(
            "  \"heads_main_ref_state\": {},\n",
            escape_json_string(&id.to_string())
        )),
        None => json.push_str("  \"heads_main_ref_state\": null,\n"),
    }
    json.push_str("  \"queue\": {\n");
    json.push_str(&format!("    \"count\": {},\n", patches.len()));
    match queue_target {
        Some(QueueTarget::Ref(ref_name)) => {
            json.push_str(&format!(
                "    \"target_ref\": {},\n",
                escape_json_string(ref_name)
            ));
            json.push_str("    \"target_ref_status\": null,\n");
        }
        Some(QueueTarget::MissingMetadata) => {
            json.push_str("    \"target_ref\": null,\n");
            json.push_str("    \"target_ref_status\": \"missing-metadata\",\n");
        }
        Some(QueueTarget::MalformedMetadata) => {
            json.push_str("    \"target_ref\": null,\n");
            json.push_str("    \"target_ref_status\": \"malformed-metadata\",\n");
        }
        None => {
            json.push_str("    \"target_ref\": null,\n");
            json.push_str("    \"target_ref_status\": null,\n");
        }
    }
    match threshold {
        Some((status, warn, limit)) => {
            json.push_str(&format!(
                "    \"threshold_status\": {},\n",
                escape_json_string(status.as_str())
            ));
            json.push_str(&format!("    \"warn_threshold\": {warn},\n"));
            json.push_str(&format!("    \"hard_limit\": {limit},\n"));
        }
        None => {
            json.push_str("    \"threshold_status\": null,\n");
            json.push_str("    \"warn_threshold\": null,\n");
            json.push_str("    \"hard_limit\": null,\n");
        }
    }
    json.push_str("    \"patches\": [");
    for (index, patch) in patches.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n      {\"patch_id\": ");
        json.push_str(&escape_json_string(&patch.patch_id.to_string()));
        json.push_str(", \"operations\": [");
        for (op_index, operation) in patch.operations.iter().enumerate() {
            if op_index > 0 {
                json.push(',');
            }
            json.push_str("\n        ");
            push_operation(&mut json, operation);
        }
        if !patch.operations.is_empty() {
            json.push_str("\n      ");
        }
        json.push_str("]}");
    }
    if !patches.is_empty() {
        json.push_str("\n    ");
    }
    json.push_str("]\n");
    json.push_str("  }\n");
    json.push('}');
    println!("{json}");
}

fn push_operation(json: &mut String, operation: &QueuedOperationEntry) {
    json.push_str("{\"kind\": ");
    json.push_str(&escape_json_string(operation.kind));
    json.push_str(", \"paths\": [");
    for (index, path) in operation.paths.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        match path {
            QueuedPathResolution::Path(path) => {
                json.push_str("{\"path\": ");
                json.push_str(&escape_json_string(path));
                json.push('}');
            }
            QueuedPathResolution::Unresolved { node_id } => {
                json.push_str("{\"unresolved_node_id\": ");
                json.push_str(&escape_json_string(node_id));
                json.push('}');
            }
        }
    }
    json.push(']');
    if let QueuedOperationContent::RenamePath { author_key_id } = &operation.content {
        json.push_str(", \"author_key_id\": ");
        json.push_str(&escape_json_string(author_key_id));
    }
    json.push('}');
}
