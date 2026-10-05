//! **Round 2 U3, measure first: a report-only walk of the references the existence checks would cover.**
//!
//! `verify`'s `reachability.rs` module doc names four field groups no check reads today: a RefState's
//! `target_object_id`, `previous_ref_state_id` and `required_attestation_ids`, and a Tag's
//! `target_block_id`. This walk reads the same fields from every ref tip and its whole `previous_ref_state_id`
//! chain, and records, per field, whether the referenced object is present. It never fails anything: it
//! appends one line per checked reference to the file named by `PRIKK_EXISTENCE_REPORT`, so the store's
//! in-process test suite can be measured before any check is allowed to fail a repository (the handoff's
//! rule). Test-only, and called from `verify` only under `cfg(test)`.

use std::collections::BTreeSet;
use std::io::Write;

use prikk_object::{ObjectType, RefStatePayload, TagPayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::refs::RefStore;

pub(crate) fn record(layout: &RepositoryLayout) {
    let Some(path) = std::env::var_os("PRIKK_EXISTENCE_REPORT") else {
        return;
    };
    let Ok(snapshot) = ObjectReadSnapshot::open(layout) else {
        return;
    };
    let Ok(pointers) = RefStore::new(layout.clone()).list_ref_pointers() else {
        return;
    };
    let mut lines = Vec::new();
    for pointer in pointers {
        walk(
            &snapshot,
            &pointer.ref_name,
            pointer.ref_state_id,
            &mut lines,
        );
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let root = std::thread::current().name().unwrap_or("?").to_string();
    for line in lines {
        let _ = writeln!(file, "{root}\t{line}");
    }
}

fn check(
    snapshot: &ObjectReadSnapshot,
    lines: &mut Vec<String>,
    ref_name: &str,
    field: &str,
    id: prikk_object::ObjectId,
) -> bool {
    let present = snapshot.read_object(id).is_ok_and(|found| found.is_some());
    let verdict = if present { "present" } else { "MISSING" };
    lines.push(format!("{verdict}\t{ref_name}\t{field}\t{id}"));
    present
}

fn walk(
    snapshot: &ObjectReadSnapshot,
    ref_name: &str,
    tip: prikk_object::ObjectId,
    lines: &mut Vec<String>,
) {
    let mut seen = BTreeSet::new();
    let mut current = Some(tip);
    while let Some(id) = current {
        if !seen.insert(id) {
            lines.push(format!("CYCLE\t{ref_name}\tprevious_ref_state_id\t{id}"));
            return;
        }
        let Ok(Some(envelope)) = snapshot.read_typed(id, ObjectType::RefState) else {
            lines.push(format!("MISSING\t{ref_name}\tRefState\t{id}"));
            return;
        };
        let Ok(payload) =
            RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        else {
            lines.push(format!("UNDECODABLE\t{ref_name}\tRefState\t{id}"));
            return;
        };
        if check(
            snapshot,
            lines,
            ref_name,
            "target_object_id",
            payload.target_object_id,
        ) {
            if let Ok(Some(target)) = snapshot.read_object(payload.target_object_id) {
                if target.object_type == ObjectType::Tag {
                    if let Ok(tag) = TagPayload::decode_canonical(&target.canonical_payload) {
                        check(
                            snapshot,
                            lines,
                            ref_name,
                            "tag.target_block_id",
                            tag.target_block_id,
                        );
                    }
                }
            }
        }
        for attestation in &payload.required_attestation_ids {
            check(
                snapshot,
                lines,
                ref_name,
                "required_attestation_ids",
                *attestation,
            );
        }
        current = payload.previous_ref_state_id;
        if let Some(previous) = current {
            if !check(snapshot, lines, ref_name, "previous_ref_state_id", previous) {
                return;
            }
        }
    }
}
