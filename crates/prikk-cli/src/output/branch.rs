//! `prikk branch list --format json` (RFC 146): `branch-list-v1`.

use crate::stdout::println;
use prikk_object::ObjectId;

use super::verification::escape_json_string;

/// One local branch entry as the prose `branch list` loop already computes it, before either
/// output form renders it. `closed` is RFC 146 rule 3's own worked example: the prose marker
/// `(closed)` becomes a real boolean here, not a string a consumer has to parse.
pub(crate) struct BranchListEntry {
    pub(crate) ref_name: String,
    pub(crate) ref_state_id: ObjectId,
    pub(crate) closed: bool,
}

/// One received (remote-tracking) ref entry. Carries no `closed` field: the prose form never
/// decodes a received ref's `RefState` to learn it (`list_received_pointers` returns only the
/// pointer), and RFC 146 forbids computing anything the existing command does not already —
/// adding that decode here would be new work this round is not scoped to do. Reported as its own
/// array rather than a `closed: null` placeholder on a shared shape, since the fact is not merely
/// absent for a received ref, it is a different kind of entry altogether (RFC 146 rule 5: the
/// nesting is the data).
pub(crate) struct ReceivedListEntry {
    pub(crate) ref_name: String,
    pub(crate) ref_state_id: ObjectId,
}

/// Print `prikk branch list --format json`. `branches` must already reflect `--all`'s own
/// filtering (a closed branch omitted unless `--all`) — this function renders what it is given,
/// the same division of labor `print_branch_list`'s prose sibling in `branch.rs` uses.
pub(crate) fn print_branch_list_json(branches: &[BranchListEntry], received: &[ReceivedListEntry]) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"branch-list-v1\",\n");
    json.push_str("  \"branches\": [");
    for (index, branch) in branches.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"ref_name\": ");
        json.push_str(&escape_json_string(&branch.ref_name));
        json.push_str(", \"ref_state_id\": ");
        json.push_str(&escape_json_string(&branch.ref_state_id.to_string()));
        json.push_str(&format!(", \"closed\": {}}}", branch.closed));
    }
    if !branches.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("],\n");
    json.push_str("  \"received\": [");
    for (index, entry) in received.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"ref_name\": ");
        json.push_str(&escape_json_string(&entry.ref_name));
        json.push_str(", \"ref_state_id\": ");
        json.push_str(&escape_json_string(&entry.ref_state_id.to_string()));
        json.push('}');
    }
    if !received.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n");
    json.push('}');
    println!("{json}");
}
