//! Merge execution (DC-74/DC-75): seals the other side's patches verbatim onto the target ref when
//! the two sides are proven confluent from a common baseline.
//!
//! **A merge authors nothing** — the adopted patches are the exact objects already sealed on the
//! source ref, same canonical bytes, same `ObjectId`, same author signature; this module never
//! decodes, re-derives, or re-signs a patch. Only the new `Block`, `RefState`, and `RefUpdate` are
//! signed here, with the maintainer key, exactly as an ordinary `seal` signs them.
//!
//! Two-parent `BlockKind::Merge` blocks (DC-75): `parent_block_ids` names both `into_ref`'s prior
//! tip (recorded again as `mainline_parent_id`) and `from_ref`'s adopted tip, sorted per the format's
//! uniqueness invariant. `merge_baseline_block_id` records the baseline confluence was proven
//! against — a claim `verify` independently re-derives and cross-checks rather than trusts. State
//! derivation and replay follow the mainline parent only; the secondary parent's own chain is
//! verified independently by the ordinary full-object-store scan. DC-74 sealed merges as
//! indistinguishable `BlockKind::Normal` blocks; this is what discharges its release condition.

use std::collections::BTreeMap;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload,
};

use crate::anchor_trust::AnchorSite;
use crate::block_state::{
    CandidateStateDerivationError, StateAnchoring, derive_candidate, seal_block_at,
};
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::lifecycle_cache::replay::TextCache;
use crate::lock::ActiveLock;
use crate::merge::evidence::{
    MergeEvidenceTarget, candidate_patch_ids, prepare_merge_evidence,
    verify_candidate_blocks_trusted,
};
use crate::patch_replay::ReplayManifest;
use crate::patch_replay::apply::ReplayLiveNode;
use crate::patch_replay::read::{files_to_replay_manifest, read_blob_bytes_with_kind};
use crate::received::validate_received_ref;
use crate::state_root::{StateRootContent, StateRootEntry};
use crate::wal::Wal;
use crate::worktree_materialize::{
    CurrentBranchMaterializeReport, mark_current_branch_switch_pending, materialize_after_publish,
    require_nothing_in_the_way, require_worktree_clean_and_capture,
};
use crate::{
    BlockLineage, GatedOperation, MaintainerSigner, ObjectReader, ObjectWriteSession,
    RefPublication, RefStore, RepositoryLayout, maintainer_signature, validate_local_branch_ref,
    verify_signer_trusted,
};

/// Result of a completed merge execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeExecutionReport {
    /// Ref advanced by the merge (the "into" side).
    pub into_ref: String,
    /// Ref merged in (the "from" side).
    pub from_ref: String,
    /// Baseline block confluence was proven against.
    pub baseline_block_id: ObjectId,
    /// `into_ref`'s target block immediately before the merge (the new block's mainline parent).
    pub parent_block_id: ObjectId,
    /// `from_ref`'s target block the adopted patches were collected up to.
    pub adopted_target_block_id: ObjectId,
    /// Patch IDs adopted verbatim, in sealed order.
    pub adopted_patch_ids: Vec<ObjectId>,
    /// New block ID.
    pub block_id: ObjectId,
    /// New RefState ID.
    pub ref_state_id: ObjectId,
    /// 0.56.0 step W3b (R3): `true` when `into_ref` was the current branch and this merge therefore
    /// materialized its newly published tip into the worktree (clean-worktree precondition met).
    /// `false` when `into_ref` was not the current branch, so the worktree was never this merge's
    /// concern at all -- the same meaning `false` has here as "nothing to report," not "refused."
    pub materialized_into_worktree: bool,
    /// Files created or replaced in the worktree by this merge's own materialization; `0` when
    /// [`materialized_into_worktree`](Self::materialized_into_worktree) is `false`.
    pub materialized_written_files: usize,
    /// Worktree files already present with identical bytes, found while materializing.
    pub materialized_unchanged_files: usize,
    /// Files of the pre-merge worktree the new tip lacks, deleted while materializing.
    pub materialized_deleted_files: usize,
    /// The replay-verified snapshot the materialization could not anchor at, if any (RFC 136
    /// increment 2b) -- `None` whenever `materialized_into_worktree` is `false` too.
    pub materialization_anchor_fallback: Option<crate::patch_replay::SnapshotAnchorFallback>,
}

/// Execute a merge: seal `from_ref`'s patches since `baseline_block_id` verbatim onto `into_ref`.
///
/// Refuses cleanly with no object, WAL, or ref write of any kind unless the two sides are proven
/// confluent from the given baseline — conflict detection is `patch_algebra`'s existing evidence
/// machinery (the same analysis `merge-evidence`/`merge-plan` already report), reused rather than
/// duplicated.
pub fn execute_merge(
    layout: &RepositoryLayout,
    baseline_block_id: ObjectId,
    into_ref: &str,
    from_ref: &str,
    signer: &impl MaintainerSigner,
    current_branch: Option<&str>,
) -> Result<MergeExecutionReport> {
    layout.require_current_format()?;
    let into_ref = validate_local_branch_ref(into_ref)?;
    // 0.56.0 step W3b (R3): serialize with every other writer of the active WAL/worktree for the
    // whole call, exactly the discipline `branch_switch.rs`'s own `switch_branch` already holds for
    // its own writes -- `execute_merge` held no lock of its own before this round because it never
    // wrote the worktree before this round either.
    let _active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    // RFC 165 R3 (C3): refuse while another ref's publication is incomplete, before any write --
    // `into_ref` excluded, since this merge is the thing that would resolve its own state, not
    // something blocked by it. A merge never writes `from_ref`, so it is never excluded.
    crate::refs::ensure_may_publish(layout, &into_ref).map_err(|error| {
        // Part B review carry 2: name `ref complete` only when *another* ref's own refusal is a
        // genuine N3 lead, matched by the error's own variant, never by its words.
        if matches!(&error, PrikkError::IncompletePublication(_)) {
            crate::ref_completion::incomplete_publication_refusal_naming_any_lead(
                layout,
                Some(&into_ref),
            )
            .unwrap_or(error)
        } else {
            error
        }
    })?;
    // RFC 165 R4 item 5: a completable lead on `into_ref` must not be silently built on top of --
    // its own log tip sits one transition behind its pointer, so evidence gathered against the
    // pointer and a publish computed from it would carry the wrong `expected_previous_ref_state_id`
    // and fail deep inside the write, not here where the real cause is nameable. Checked before any
    // evidence gathering or write.
    if crate::ref_completion::plan_ref_completion(layout, &into_ref)?.is_ok() {
        return Err(PrikkError::Precondition(format!(
            "ref {into_ref} has an interrupted publication; run `prikk ref complete {into_ref}` \
             before merging into it"
        )));
    }
    // RFC 136 §10.3b.3: the derivation gate, before any write.
    crate::worktree_marker::ensure_worktree_replay_verified(layout)?;
    // 0.56.0 step W3 (R4): never move a ref the active WAL already holds queued, unsealed work
    // for — those patches were authored against `into_ref`'s *old* tip; folding them onto a tip
    // this merge is about to move it to is not something this check makes safe. `seal` first.
    crate::commit_boundary::active::refuse_if_active_queue_owns_ref(
        layout,
        &into_ref,
        &format!("merging {from_ref} into {into_ref}"),
    )?;
    // DC-85: `from_ref` may be a local branch or a received ref (`remotes/<name>`) — never widen
    // `validate_local_branch_ref` itself to accept `remotes/`, since it also gates `into_ref` here
    // and `branch create --from` elsewhere; a merge source that happens to be received gets its own
    // resolution path instead (§3A.3). `into_ref` is never eligible: `RefStore::publish` only ever
    // writes `refs/by-id/`, so the side being advanced must remain a genuine local branch.
    let from_is_received = from_ref.starts_with("remotes/");
    // Carry the local arm's own canonical string forward for the comparison below, rather than
    // re-deriving `from_ref` from the raw argument a second time — today `validate_local_branch_ref`
    // happens to return its input unchanged, so the two forms agree, but that is not guaranteed to
    // stay true (`refs.rs`'s own comment on `validate_local_tag_ref` already records NFR-SEC-03's
    // case-collision rule as unmet and tracked for later); comparing a validated name against an
    // unvalidated one would silently stop catching `heads/Main` vs `heads/main` the day it lands.
    let (from_target, from_ref) = if from_is_received {
        validate_received_ref(from_ref)?;
        (
            MergeEvidenceTarget::ReceivedRef(from_ref.to_string()),
            from_ref.to_string(),
        )
    } else {
        let canonical = validate_local_branch_ref(from_ref)?;
        (MergeEvidenceTarget::Ref(canonical.clone()), canonical)
    };
    if into_ref == from_ref {
        return Err(PrikkError::InvalidName(
            "merge into_ref and from_ref must differ".to_string(),
        ));
    }
    // Read-only evidence gathering, requiring no signing credential at all — confluence is
    // determinable exactly like `merge-plan`'s, before any question of who may seal is asked.
    let evidence = prepare_merge_evidence(
        layout,
        baseline_block_id,
        MergeEvidenceTarget::Ref(into_ref.clone()),
        from_target,
    )?;
    // A merge the evidence does not prove confluent is refused as a precondition, not damage
    // (DC-75 two-edits handoff §4): the repository verifies clean. Damaged inputs stay `Integrity`.
    if !evidence.is_confluent() {
        return Err(PrikkError::Precondition(format!(
            "merge refused: {from_ref} is not confluent with {into_ref} from baseline \
             {baseline_block_id} (outcome: {}{})",
            evidence.outcome,
            evidence
                .reason
                .map(|reason| format!(", reason: {reason}"))
                .unwrap_or_default(),
        )));
    }

    // Only proceeding to seal needs a trusted signer.
    let policy = verify_signer_trusted(layout, signer, GatedOperation::Merge)?;

    let mut object_store = ObjectWriteSession::open(layout)?;

    // DC-85 §3A.1's mandatory criterion: a received ref's blocks never passed a trust check on the
    // way in (`import_bundle` performs none, deliberately). Checked here, before any write, reusing
    // the signer's already-loaded policy rather than a second load.
    if from_is_received {
        verify_candidate_blocks_trusted(
            &object_store,
            &policy,
            baseline_block_id,
            evidence.right_selector.target_block_id,
        )?;
    }

    let adopted_patch_ids = candidate_patch_ids(
        &object_store,
        baseline_block_id,
        evidence.right_selector.target_block_id,
    )?;
    // Nothing to adopt is the state of the two refs, not damage (DC-75 two-edits handoff §6).
    if adopted_patch_ids.is_empty() {
        return Err(PrikkError::Precondition(format!(
            "{from_ref} has no patches to adopt since baseline {baseline_block_id}"
        )));
    }

    let ref_store = RefStore::new(layout.clone());
    // RFC 164 Rule D: the pointer index's own tail check for `into_ref`, before the merge Block
    // write just below (`seal_block_at`) -- previously this ref's own pointer-index tail was not
    // checked until `publish_locked`'s own, later `Ready`-branch-only call, by which point the new
    // merge Block was already durably written, for nothing (a refused publication left it
    // unreferenced, not undone). `read_current_ref_state_id` below already reads the pointer index
    // too (and so already refuses on damage via `has_item_failure()`); this call adds the tail half.
    ref_store.ensure_pointer_index_has_no_tail(&into_ref)?;
    crate::ref_resolution::require_existing_ref(
        layout,
        &into_ref,
        crate::ref_resolution::ReceivedRefs::LeftToNameValidation,
    )?;
    let into_ref_state_id = ref_store
        .read_current_ref_state_id(&into_ref)?
        .ok_or_else(|| PrikkError::Integrity(format!("ref {into_ref} is not published")))?;
    let into_ref_state_envelope = object_store
        .read_typed(into_ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!("ref {into_ref} points to missing RefState"))
        })?;
    let into_ref_state = RefStatePayload::decode_canonical(
        &into_ref_state_envelope.canonical_payload,
        into_ref_state_envelope.schema_version,
    )?;
    let parent_block_id = into_ref_state.target_object_id;
    ensure_into_ref_unmoved(
        &into_ref,
        parent_block_id,
        evidence.left_selector.target_block_id,
    )?;

    // 0.56.0 step W3c-1 (correcting W3b): when `into_ref` is the current branch, the clean check
    // and the capture of the old baseline's files run *here* -- after every read-only refusal
    // above (RFC 164 Rule D's own pointer-index tail/damage check among them: it must still refuse
    // before this call ever runs, or it would leave a derived lifecycle-cache write behind that a
    // refusal is not supposed to make either) and before `seal_block_at` just below ever builds or
    // writes a single object -- not after it, as W3b had it. Neither step reads anything about
    // this merge's own new content, so nothing about doing this early is unsafe; doing it late is
    // what let a refused merge leave an orphaned Block behind (measured on `c9659ece`, `verify`: 1
    // unreached object). Checked against `into_ref` specifically, never the raw `current_branch`
    // string, so a caller that resolved the pointer for an unrelated ref never trips this.
    //
    // 0.56.0 step W3c-3b: a file in the way of the merge's own candidate state must refuse here
    // too, before any write -- the same "before any write" guarantee `branch create --from`
    // already has (0.56.0 step W3c-3). The confluence proof's own `OracleState` is the wrong
    // source for that candidate state (it is a proof device with its own representation, and a
    // derived check must classify the state the Block will actually commit to, not a different
    // one computed for a different purpose); instead this derives the merge Block's own candidate
    // state directly, with `derive_candidate` (`block_state.rs`) -- the identical call
    // `seal_block_at` just below will make again moments later, from the same mainline parent
    // (`parent_block_id`), the same adopted patch ids, and the same merge anchor site. Deriving it
    // twice is acceptable here (it is only for a merge into the current branch, and it is
    // anchored): changing `seal_block_at`'s own signature to share the derivation was not
    // attempted, since doing so without changing anything it writes was not obviously trivial.
    let pending_materialization = if current_branch == Some(into_ref.as_str()) {
        let active_replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
        let pending = require_worktree_clean_and_capture(
            layout,
            &into_ref,
            &active_replay,
            &format!("merging {from_ref} into {into_ref}"),
        )?;
        // Read-only: `derive_candidate` takes an `ObjectReader`, and performs no write of its own
        // (unlike `seal_block_classified`'s own internal derivation, which runs
        // `store_derived_content_for_candidate` -- a write -- first). This is exactly why it is
        // safe to call here, before any write this merge makes. The DC-65 case (an adopted-side
        // deletion of a file edited by `EditText` on the other side, so its content was never
        // stored as a Blob) is covered by a dedicated test: adopted patches were already sealed on
        // the source ref, so every Blob a `CreateFile`/`ReplaceBinary` among them needs is already
        // present, and a `DeleteNode`'s own preimage check never reads a Blob at all (it compares
        // against the in-memory replayed state) -- confirmed by running that case, not only by this
        // reasoning.
        let candidate = derive_candidate(
            layout,
            &object_store,
            Some(parent_block_id),
            &adopted_patch_ids,
            StateAnchoring::Anchored(AnchorSite::Merge),
        )
        .map_err(|err| match err {
            CandidateStateDerivationError::Lineage(err) => err,
            CandidateStateDerivationError::Patch(err) => err.into(),
        })?;
        let target_manifest =
            candidate_replay_manifest(&object_store, candidate.entries, &candidate.text_cache)?;
        require_nothing_in_the_way(
            layout,
            &pending,
            &target_manifest,
            &format!("merging {from_ref} into {into_ref}"),
        )?;
        Some(pending)
    } else {
        None
    };

    let adopted_target_block_id = evidence.right_selector.target_block_id;
    let block_id = seal_block_at(
        layout,
        &mut object_store,
        BlockLineage::Merge {
            mainline_parent: parent_block_id,
            adopted_parent: adopted_target_block_id,
            baseline: baseline_block_id,
        },
        &adopted_patch_ids,
        signer,
        StateAnchoring::Anchored(AnchorSite::Merge),
    )?;

    let update_seq = into_ref_state.update_seq.checked_add(1).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "merge into {into_ref}: ref-state update_seq overflow"
        ))
    })?;
    let ref_state_payload = RefStatePayload {
        ref_name: into_ref.clone(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq,
        previous_ref_state_id: Some(into_ref_state_id),
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
        signer,
    )?;
    let ref_state_id = ref_state_envelope.object_id();

    let ref_update_payload = RefUpdatePayload {
        ref_name: into_ref.clone(),
        old_ref_state_id: Some(into_ref_state_id),
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
        signer,
    )?;

    let publication = RefPublication {
        ref_name: into_ref.clone(),
        expected_previous_ref_state_id: Some(into_ref_state_id),
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };

    // 0.56.0 step W3c-1: step 2b -- the marker is set immediately before the publish (step 3)
    // below, using the capture already taken just above, after every read-only refusal but still
    // far above every object write this merge makes (`seal_block_at`, the RefState/RefUpdate
    // envelopes). A refusal at any point between there and here (the signer check, `seal_block_at`
    // itself) now leaves nothing written at all -- the standard `verify` holds every other
    // producer to.
    if let Some(pending) = &pending_materialization {
        mark_current_branch_switch_pending(layout, pending)?;
    }

    let published_ref_state_id =
        ref_store.publish_with_object_store(&mut object_store, &publication)?;

    // Step 4: materialize the tip just published, through the ordinary verified path -- a crash
    // between the publish above and this call leaves the marker set and the record stale, both
    // recoverable by `prikk branch switch <into_ref>` (the marker's own named route).
    let materialization = pending_materialization
        .map(|pending| materialize_after_publish(layout, pending))
        .transpose()?;

    let (
        materialized_into_worktree,
        materialized_written_files,
        materialized_unchanged_files,
        materialized_deleted_files,
        materialization_anchor_fallback,
    ) = match materialization {
        Some(CurrentBranchMaterializeReport {
            written_files,
            unchanged_files,
            deleted_files,
            anchor_fallback,
        }) => (
            true,
            written_files,
            unchanged_files,
            deleted_files,
            anchor_fallback,
        ),
        None => (false, 0, 0, 0, None),
    };

    Ok(MergeExecutionReport {
        into_ref,
        from_ref,
        baseline_block_id,
        parent_block_id,
        adopted_target_block_id,
        adopted_patch_ids,
        block_id,
        ref_state_id: published_ref_state_id,
        materialized_into_worktree,
        materialized_written_files,
        materialized_unchanged_files,
        materialized_deleted_files,
        materialization_anchor_fallback,
    })
}

/// Build the `ReplayManifest` [`require_nothing_in_the_way`] needs from a [`derive_candidate`]
/// result's own `entries` plus `text_cache` (0.56.0 step W3c-3b) -- the same per-entry resolution
/// `patch_replay::extract`'s own claim path already uses (resident bytes from the text cache
/// first, the Blob only when absent -- never the other way around, since the text cache, not the
/// node's tracked blob id, holds the truth for a node an `EditText` has touched since its tracked
/// id was last current, DC-65), then `files_to_replay_manifest`, the one existing builder for a
/// `ReplayManifest`, reused rather than copied a third time.
///
/// A symlink entry is refused exactly as `patch_replay::extract`'s claim path and
/// `snapshot::snapshot_files_from_manifest` both already refuse one for the identical reason: the
/// format does not admit a symlink into replay state at all today (`ReplayManifestEntry::kind`'s
/// own doc records why -- both `DeleteNode(symlink)` and `CreateSymlink` are refused by
/// `ensure_apply_supported` before application), so this is unreachable with today's format, not a
/// narrowing of real merge behaviour.
fn candidate_replay_manifest(
    reader: &impl ObjectReader,
    entries: Vec<StateRootEntry>,
    text_cache: &TextCache,
) -> Result<ReplayManifest> {
    let mut files = BTreeMap::new();
    let mut live_nodes = BTreeMap::new();
    for entry in entries {
        let StateRootContent::Blob(blob_id) = entry.content else {
            return Err(PrikkError::UnsupportedObjectType(format!(
                "merge candidate state holds symlink {}; the pre-write in-the-way check supports \
                 the file-level subset only",
                entry.path.as_str()
            )));
        };
        let bytes = match text_cache.get(&entry.node_id) {
            Some(bytes) => bytes.clone(),
            None => read_blob_bytes_with_kind(reader, blob_id)?.1,
        };
        let path = entry.path.as_str().to_string();
        live_nodes.insert(
            entry.node_id,
            ReplayLiveNode {
                path: path.clone(),
                kind: entry.kind,
                mode: entry.mode,
                blob_id,
            },
        );
        files.insert(path, bytes);
    }
    files_to_replay_manifest(files, &live_nodes)
}

/// `into_ref` must still point where the evidence read it. If another writer advanced it in between,
/// that is `LockConflict` (DC-75 two-edits handoff §6): *"another writer may be active"*, and running
/// the same merge again can succeed. Not `Precondition`, whose documented meaning is that *"nothing
/// here is transient and waiting does not help"*.
fn ensure_into_ref_unmoved(
    into_ref: &str,
    current_target: ObjectId,
    evidence_target: ObjectId,
) -> Result<()> {
    if current_target == evidence_target {
        return Ok(());
    }
    Err(PrikkError::LockConflict(format!(
        "ref {into_ref} advanced during merge evidence gathering; retry"
    )))
}

fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> Result<ObjectEnvelope> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, object_type, object_id)?)?;
    Ok(envelope)
}

#[cfg(test)]
mod tests;
