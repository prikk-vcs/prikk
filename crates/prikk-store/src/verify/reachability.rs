//! RFC 164 Rule E: reachability from committed state, for classifying a stored object's own
//! dangling reference as damage (the object making the reference is itself reachable) or an
//! unreferenced remnant (it is not). Computed fresh on every `verify`/`doctor` run, from committed
//! state only -- never from an object's own claims -- so a remnant that later becomes reachable
//! (a new branch created over it, say) is damage on the very next run; nothing here is cached
//! across runs.
//!
//! **Roots** (RFC 164 §5): every local ref's current RefState, every received ref's current
//! RefState, every blob a queued (unsealed) patch in any active WAL session references, and --
//! 0.56.0 step 3h (#9), widening this set to answer `verify`/`doctor`'s own `Patch`/`Blob`
//! question, not only `Block`/`RefState`'s -- every patch id [`crate::patch_set_digest::
//! patch_ids_pending`] names (3g's own `pending` predicate: a held patch a held claim names and
//! no held Block seals; reused here unmodified, never restated). **Expansion**: a RefState reaches
//! its own `target_object_id` (a Block or a Tag) and its own `previous_ref_state_id` (keeping a
//! ref's whole published lineage reachable, not only its current tip); a Tag reaches its
//! `target_block_id`; a Block reaches `parent_block_ids`, `patch_ids`, `snapshot_blob_ref`,
//! `mainline_parent_id`, and `merge_baseline_block_id` -- this is "a sealed block reached from
//! them" (RFC 164 §5): transitively, through every block's own parents; **and, since 0.56.0 step
//! 3h, a `Patch`** (whether reached as a pending root above, or transitively through a reachable
//! Block's own `patch_ids`) **reaches every Blob its own operations reference** (`CreateFile`'s
//! `blob_id`, `ReplaceBinary`'s `old_blob_id`/`new_blob_id`, a file-kind `DeleteNode`'s own
//! `old_blob_id` -- [`crate::patch_replay::decode::patch_referenced_blob_ids`], the identical
//! decoder the queued-patch root case below already uses). This is "with the blobs they name" from
//! the handoff's own wording: a pending patch's blobs are reached through this one new expansion
//! step, with no separate root needed for them.
//!
//! **Scope, narrower than the general principle**: only `Block`'s own existing missing-reference
//! check (`parent_block_ids`, `patch_ids`, `snapshot_blob_ref` -- `verify_block_payload`) is made
//! reachability-aware. The RefState, Tag and Attestation references are existence-checked by
//! `verify` today, but not through this walk: `RefState.target_object_id` and a Tag's
//! `target_block_id` by `ensure_ref_target_valid` (`refs/verify/scan.rs`), and
//! `required_attestation_ids` *and*, once present, each required attestation's own
//! `target_block_id` by `ensure_required_attestations_present` (0.49.0 step 5, round 2;
//! `AttestationPayload::decode_canonical` and the target-block check added 0.50.0 step 2 Part B).
//! **`previous_ref_state_id`, ruled (0.52.0 step 0, Part C1):** this comment used to call it a
//! disclosed gap, "pending the owner's ruling" -- stale from the moment it was written.
//! `verify_received_refs` (`verify.rs`) already requires it, directly, for a **received** ref's own
//! current tip: one link, checked by reading the previous `RefState` and failing if it is absent.
//! That check landed thirteen minutes after this comment did (0.49.0 step 5, round 2 item 1) and
//! this doc was never updated. For a **local** ref, the same existence requirement holds
//! transitively, through the ref log's own replay (`refs/verify/scan.rs`'s `verify_update`, which
//! decodes every `new_ref_state_id` in sequence and checks each link's own `old_ref_state_id`
//! against the previous record -- so every non-genesis tip's own previous link was itself already
//! read and decoded, not merely compared by id). No other `RefState`'s own previous link is
//! checked anywhere: a superseded received tip from before 0.49.0 may genuinely lack its own chain,
//! and `verify` passes over it, by design. **0.56.0 step 3h adds forward expansion for a sealed or
//! pending `Patch`'s own referenced blobs (above), but still no missing-reference *existence*
//! check for them**: `verify_block_payload`'s own `UnreferencedRemnant`-vs-damage classification
//! stays Block-only (`parent_block_ids`, `patch_ids`, `snapshot_blob_ref`) -- a Patch whose own
//! `blob_id`/`old_blob_id`/`new_blob_id` names a Blob this repository never wrote is not reported
//! as damage by anything today, and this round does not change that; it only widens
//! *reachability*, never *existence checking*. `RecognitionClaim`'s own references are never
//! touched: they are "never trust-conferring and never existence-checked" by standing design (the
//! `ObjectType` doc), and reachability classification must not contradict that.
//!
//! **A reference this walk cannot read is never expanded past.** A missing or undecodable object is
//! simply not added to the reachable set and nothing is pushed from it -- the walk only ever trusts
//! an object's own claims once that very object was itself read and decoded successfully, matching
//! the rule's own security requirement.

use std::collections::BTreeSet;

use prikk_error::Result;
use prikk_object::{
    BlockPayload, ObjectEnvelope, ObjectId, ObjectType, RefStatePayload, TagPayload,
};

use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::object_store::ObjectReader;
use crate::received::list_received_pointers;
use crate::refs::RefStore;
use crate::wal::{Wal, WalReplay};

/// `default_replay`: the `WalReplay` stage's own already-decoded result for
/// [`DEFAULT_ACTIVE_NAME`], when one exists. RFC 167 D5: this walk used to call `Wal::replay()` on
/// `default` a second time, unconditionally -- paying a (possibly hostile) WAL's decode cost twice
/// per `verify`, which is what RFC 167's M5 regression actually was (bisected to this module's own
/// introduction, `03d3be22`). Reusing the caller's own result for `default` closes it, mirroring
/// `verify_queued_patch_connectivity`'s identical fix for the same cost (RFC 162 Addendum 1 fix 4).
/// Every other active session has no shared result to reuse and is still replayed fresh -- the same
/// asymmetry `verify_queued_patch_connectivity` already has, for the same reason.
pub(super) fn compute_reachable_object_ids(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    default_replay: Option<&WalReplay>,
) -> Result<BTreeSet<ObjectId>> {
    compute_reachable_object_ids_with_pending_roots(layout, object_store, default_replay, true)
}

/// 0.56.0 step 3h test-support seam for a red control: [`compute_reachable_object_ids`] with the
/// new pending-patch root case (above) disabled -- reproduces the exact pre-3h gap this round
/// closes (a pending patch, and the blob it names, wrongly counted `unreached`) without a
/// process-global toggle. A parameter, not an env var, for the identical reason 3g's own red
/// control (`patch_ids_pending_disabling_held_block_subtraction_for_test`) gives: `cargo test -p
/// prikk-store --lib` runs this crate's whole suite in one process, in parallel, and
/// `std::env::set_var` would race every other test in the same binary that also calls this
/// function while this one has it set. `#[cfg(test)]`, not `feature = "test-support"`: this
/// function's own type (`pub(super)`, not `pub`) is already invisible outside this crate, so
/// nothing in `prikk-cli`'s own cross-crate integration suite could ever call it regardless of
/// feature gate -- its one real caller is this crate's own `#[cfg(test)] mod tests`, in the same
/// compilation unit, which is exactly what `#[cfg(test)]` (unlike `feature = "test-support"`)
/// guarantees always compiles together, with no risk of the lib-only `--features test-support`
/// build flagging it as dead code (confirmed: `cargo clippy --all-targets --features
/// test-support` did flag it when this was gated by the feature instead, for exactly that
/// reason, matching 3g's own report of the identical trap one level up in `patch_set_digest.rs`).
#[cfg(test)]
pub(super) fn compute_reachable_object_ids_excluding_pending_roots_for_test(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    default_replay: Option<&WalReplay>,
) -> Result<BTreeSet<ObjectId>> {
    compute_reachable_object_ids_with_pending_roots(layout, object_store, default_replay, false)
}

fn compute_reachable_object_ids_with_pending_roots(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    default_replay: Option<&WalReplay>,
    include_pending_roots: bool,
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
    // 0.56.0 step 3h (#9): every pending patch (3g's own predicate, reused unmodified) is a root
    // too -- "reached by no pending patch" is one of the three disjuncts `verify`/`doctor`'s own
    // rule now tests. Pushed onto the same frontier as every other root, so the ordinary walk
    // below both marks the patch itself reached and (via `expand_frontier`'s new `Patch` arm)
    // reaches every blob its own operations name -- "with the blobs they name," the handoff's own
    // wording, falls out of this one addition with no separate blob root needed.
    if include_pending_roots {
        for patch_id in crate::patch_set_digest::patch_ids_pending(layout)? {
            frontier.push(patch_id);
        }
    }
    for name in layout.active_session_names()? {
        let replay_owned;
        let replay = if name == DEFAULT_ACTIVE_NAME {
            match default_replay {
                Some(replay) => replay,
                None => continue,
            }
        } else {
            let Ok(fresh) = Wal::for_layout(layout, &name).replay() else {
                continue;
            };
            replay_owned = fresh;
            &replay_owned
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
        frontier.extend(expand_frontier(&envelope));
    }
    Ok(reached)
}

/// The frontier-expansion step (0.52.0 step 0, Part C1): given one already-decoded, already-reached
/// object, which further ids does reachability extend to through it. A pure function over the
/// envelope alone -- no object-store read, no layout -- so the archive's own closure pass (which
/// builds its own id-to-references map from a streamed, already-decoded envelope, never a live
/// `ObjectReader`) can share this exact step rather than re-deriving an approximation of it. A
/// decode failure here means nothing is pushed, the same outcome the old inline `continue` gave:
/// this object is still marked reached by the caller, but the walk builds no further than it.
pub(crate) fn expand_frontier(envelope: &ObjectEnvelope) -> Vec<ObjectId> {
    let mut next = Vec::new();
    match envelope.object_type {
        ObjectType::RefState => {
            let Ok(payload) = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            ) else {
                return next;
            };
            next.push(payload.target_object_id);
            if let Some(previous) = payload.previous_ref_state_id {
                next.push(previous);
            }
        }
        ObjectType::Tag => {
            let Ok(payload) = TagPayload::decode_canonical(&envelope.canonical_payload) else {
                return next;
            };
            next.push(payload.target_block_id);
        }
        ObjectType::Block => {
            let Ok(payload) = BlockPayload::decode_canonical(&envelope.canonical_payload) else {
                return next;
            };
            next.extend(payload.parent_block_ids.iter().copied());
            next.extend(payload.patch_ids.iter().copied());
            if let Some(snapshot) = payload.snapshot_blob_ref {
                next.push(snapshot);
            }
            if let Some(mainline) = payload.mainline_parent_id {
                next.push(mainline);
            }
            if let Some(baseline) = payload.merge_baseline_block_id {
                next.push(baseline);
            }
        }
        // 0.56.0 step 3h (#9): a reachable Patch (sealed into a reachable Block's own `patch_ids`,
        // or itself a pending root above) reaches every Blob its own operations name. A decode
        // failure here returns the frontier built so far, exactly like every arm above: this
        // Patch stays reached (it was already inserted into `reached` by the caller before
        // `expand_frontier` runs), the walk simply builds no further past it.
        ObjectType::Patch => {
            let Ok(blob_ids) = crate::patch_replay::decode::patch_referenced_blob_ids(
                &envelope.canonical_payload,
                envelope.schema_version,
            ) else {
                return next;
            };
            next.extend(blob_ids);
        }
        // 0.56.0 P4: a reachable Blob of kind `Snapshot` (a checkpoint's manifest, named by a
        // reachable Block's `snapshot_blob_ref`) reaches every content Blob its entries name. An
        // `EditText` names no Blob, so a text file edited step after step has its content Blobs named
        // only by the checkpoint manifests: without this arm every checkpoint's Blobs read as
        // unreached (one per 64th block), with the forgery-leftover warning, on an honest repository.
        // Every other Blob kind names nothing. A manifest that does not decode is treated like a Patch
        // that does not decode (above): the manifest Blob itself stays reached (the caller inserted
        // it before this runs), the walk builds no further past it, and the damage is `verify`'s
        // snapshot check to report, not this walk's.
        ObjectType::Blob => {
            let Ok(blob) = prikk_object::BlobPayload::decode_canonical(&envelope.canonical_payload)
            else {
                return next;
            };
            if blob.blob_kind != prikk_object::BlobKind::Snapshot {
                return next;
            }
            let Ok(manifest) = crate::snapshot::SnapshotManifest::decode(&blob.content) else {
                return next;
            };
            for entry in &manifest.entries {
                if let crate::state_root::StateRootContent::Blob(blob_id) = &entry.content {
                    next.push(*blob_id);
                }
            }
        }
        ObjectType::RefUpdate
        | ObjectType::Attestation
        | ObjectType::BlockSummaryCache
        | ObjectType::RecoveryNote
        | ObjectType::RecognitionClaim => {}
    }
    next
}
