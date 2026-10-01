//! RFC 164 Rule E: reachability from committed state, for classifying a stored object's own
//! dangling reference as damage (the object making the reference is itself reachable) or an
//! unreferenced remnant (it is not). Computed fresh on every `verify`/`doctor` run, from committed
//! state only -- never from an object's own claims -- so a remnant that later becomes reachable
//! (a new branch created over it, say) is damage on the very next run; nothing here is cached
//! across runs.
//!
//! **Roots** (RFC 164 §5): every local ref's current RefState, every received ref's current
//! RefState, and every blob a queued (unsealed) patch in any active WAL session references.
//! **Expansion**: a RefState reaches its own `target_object_id` (a Block or a Tag) and its own
//! `previous_ref_state_id` (keeping a ref's whole published lineage reachable, not only its current
//! tip); a Tag reaches its `target_block_id`; a Block reaches `parent_block_ids`, `patch_ids`,
//! `snapshot_blob_ref`, `mainline_parent_id`, and `merge_baseline_block_id` -- this is "a sealed
//! block reached from them" (RFC 164 §5): transitively, through every block's own parents.
//!
//! **Scope, narrower than the general principle**: only `Block`'s own existing missing-reference
//! check (`parent_block_ids`, `patch_ids`, `snapshot_blob_ref` -- `verify_block_payload`) is made
//! reachability-aware this round. `RefState.target_object_id`/`previous_ref_state_id`/
//! `required_attestation_ids`, `Tag.target_block_id`, and `Attestation.target_block_id` are never
//! existence-checked at all today, independent of Rule E -- extending existence-checking to them is
//! separate, larger scope this round does not cover (and `AttestationPayload` has no decoder in
//! `prikk-object` today to even read one back). A `Patch`'s own referenced blobs are not re-derived
//! here either, by the same reasoning: no existing check makes a *sealed* patch's blob references
//! reachability-aware (only a *queued* one's, via the root case above, matching `verify_queued_
//! patch_connectivity`'s own narrow walk). `RecognitionClaim`'s own references are never touched:
//! they are "never trust-conferring and never existence-checked" by standing design (the `ObjectType`
//! doc), and reachability classification must not contradict that.
//!
//! **A reference this walk cannot read is never expanded past.** A missing or undecodable object is
//! simply not added to the reachable set and nothing is pushed from it -- the walk only ever trusts
//! an object's own claims once that very object was itself read and decoded successfully, matching
//! the rule's own security requirement.

use std::collections::BTreeSet;

use prikk_error::Result;
use prikk_object::{BlockPayload, ObjectId, ObjectType, RefStatePayload, TagPayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::received::list_received_pointers;
use crate::refs::RefStore;
use crate::wal::Wal;

pub(super) fn compute_reachable_object_ids(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
) -> Result<BTreeSet<ObjectId>> {
    let mut reached: BTreeSet<ObjectId> = BTreeSet::new();
    let mut frontier: Vec<ObjectId> = Vec::new();

    let ref_store = RefStore::new(layout.clone());
    for pointer in ref_store.list_ref_pointers()? {
        frontier.push(pointer.ref_state_id);
    }
    for pointer in list_received_pointers(layout)? {
        frontier.push(pointer.ref_state_id);
    }
    for name in layout.active_session_names()? {
        let Ok(replay) = Wal::for_layout(layout, &name).replay() else {
            continue;
        };
        for record in &replay.records {
            let Ok(blob_ids) = crate::patch_replay::decode::patch_referenced_blob_ids(
                &record.envelope.canonical_payload,
                record.envelope.schema_version,
            ) else {
                continue;
            };
            reached.extend(blob_ids);
        }
    }

    while let Some(id) = frontier.pop() {
        if reached.contains(&id) {
            continue;
        }
        let Ok(Some(envelope)) = object_store.read_object(id) else {
            // Missing, or unreadable: not reachable, and nothing to expand past it. Its own
            // absence (if this id was itself a dangling reference) is reported by whichever
            // existing check named it; this walk only ever builds forward from what it can read.
            continue;
        };
        reached.insert(id);
        match envelope.object_type {
            ObjectType::RefState => {
                let Ok(payload) = RefStatePayload::decode_canonical(
                    &envelope.canonical_payload,
                    envelope.schema_version,
                ) else {
                    continue;
                };
                frontier.push(payload.target_object_id);
                if let Some(previous) = payload.previous_ref_state_id {
                    frontier.push(previous);
                }
            }
            ObjectType::Tag => {
                let Ok(payload) = TagPayload::decode_canonical(&envelope.canonical_payload) else {
                    continue;
                };
                frontier.push(payload.target_block_id);
            }
            ObjectType::Block => {
                let Ok(payload) = BlockPayload::decode_canonical(&envelope.canonical_payload)
                else {
                    continue;
                };
                frontier.extend(payload.parent_block_ids.iter().copied());
                frontier.extend(payload.patch_ids.iter().copied());
                if let Some(snapshot) = payload.snapshot_blob_ref {
                    frontier.push(snapshot);
                }
                if let Some(mainline) = payload.mainline_parent_id {
                    frontier.push(mainline);
                }
                if let Some(baseline) = payload.merge_baseline_block_id {
                    frontier.push(baseline);
                }
            }
            ObjectType::Patch
            | ObjectType::RefUpdate
            | ObjectType::Blob
            | ObjectType::Attestation
            | ObjectType::BlockSummaryCache
            | ObjectType::RecoveryNote
            | ObjectType::RecognitionClaim => {}
        }
    }
    Ok(reached)
}
