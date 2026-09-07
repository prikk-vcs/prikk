//! Pointer-first ref publication and bounded retry state classification.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, RefKind, RefStatePayload, RefUpdatePayload, ascii_fold};

use super::pointer_index::{PointerIndexEntry, append_ref_pointer_entry};
use super::{
    RefPublication, RefStore, container, validate_local_branch_ref, validate_local_tag_ref,
    validate_publication,
};
use crate::foundation::layout::{LockableContainer, RepositoryFormat, ref_name_key_bytes};
use crate::lock::{RefLock, acquire_container_locks};
use crate::object_store::ObjectWriter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationState {
    Ready,
    PointerLeading,
    Complete,
}

pub(super) fn publish(
    store: &RefStore,
    object_store: &mut impl ObjectWriter,
    publication: &RefPublication,
) -> Result<ObjectId> {
    publish_locked(store, object_store, publication, false)
}

pub(super) fn finish_interrupted(
    store: &RefStore,
    object_store: &mut impl ObjectWriter,
    publication: &RefPublication,
) -> Result<ObjectId> {
    publish_locked(store, object_store, publication, true)
}

fn publish_locked(
    store: &RefStore,
    object_store: &mut impl ObjectWriter,
    publication: &RefPublication,
    allow_partial_tail_repair: bool,
) -> Result<ObjectId> {
    let update = validate_coherent_publication(publication)?;
    // DC-72: `expected_previous_ref_state_id: None` is this publication's own CAS signal for "no
    // current state" — the same signal `branch create`/`tag create`/first `seal` already use to mean
    // "this ref does not exist yet" (branch.rs, tag.rs, seal.rs). Checked only here, not on every
    // ordinary pointer-update publication, so an existing ref's routine seal never re-scans every ref.
    if publication.expected_previous_ref_state_id.is_none() {
        validate_no_ref_name_collision(store, &publication.ref_name)?;
    }
    let ref_state_id = publication.ref_state.object_id();
    let ref_name_key = ref_name_key_bytes(&publication.ref_name);
    let ref_lock = RefLock::acquire(&store.layout, &publication.ref_name)?;
    // RFC 102 Stage 6 Step 2, design-v1.md §15.8 (ruled wide): both containers this function writes
    // -- the pointer index and the ref log -- are locked together, for the whole critical section
    // below, before either is read or written. Acquired here, not per-write, so `classify_state`'s
    // own reads (right below) are also excluded from a concurrent compactor, not just the appends --
    // belt and suspenders alongside the readers' own generation-recheck-and-retry (`generation.rs`).
    // `acquire_container_locks` sorts the set itself; this call's own argument order carries no
    // meaning and must not be read as the write order below, which is unchanged and load-bearing
    // (see the `PublicationState::Ready` arm's own comment on why pointer-then-log is not
    // reorderable).
    let container_locks = acquire_container_locks(
        &store.layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
        ],
    )?;
    // Step 0 §13.3, ruled in design-v1.md §13.3: the candidate-write-then-promote mechanism
    // (`refs/pointer.rs`'s old `write_ref_pointer_candidate`/`promote_ref_pointer_candidate`,
    // `remove_candidate_write_temps` here) has no equivalent under an append-only pointer index --
    // an append-only record has no candidate value to stage, the append *is* the publish. `refs/tmp/`
    // is never written by this function again.
    match store.layout.format() {
        RepositoryFormat::CurrentV6 => {
            object_store.write_object(&publication.ref_state)?;
        }
    }

    let (state, trailing_partial_bytes) = classify_state(store, publication, &update)?;
    if trailing_partial_bytes != 0 {
        if !allow_partial_tail_repair
            || state != PublicationState::PointerLeading
            || !container::incomplete_tail_matches(
                &store.layout,
                ref_name_key,
                &publication.ref_update,
            )?
        {
            return Err(PrikkError::Integrity(format!(
                "ref {} has an unauthorized incomplete log tail",
                publication.ref_name
            )));
        }
        container::truncate_incomplete_tail(&store.layout)?;
    }
    match state {
        PublicationState::Ready => {
            // Design-v1.md §13.6: no candidate/promote dance, and the write order is otherwise
            // unchanged from today's pointer-first publication -- the CAS check happens immediately
            // before the pointer-index append, both still inside the same `RefLock` this function
            // already holds for its whole duration, so no other writer for this exact ref name can
            // observe or race between the check and the append.
            //
            // Stage 4 acceptance criterion 4 (handoff §4), probed in a detached worktree and
            // reverted, not left as a toggle: this pointer-then-log order is *why* DC-38's invariant
            // ("format publication never permits an ahead log") holds, not incidental to it.
            // Reversing it (log append first, pointer second) and interrupting between the two with
            // a failpoint left a real log record with no corresponding pointer -- a genuine ahead-log
            // state. `verify_repository` correctly classified it as blocking `PRIKK-VERIFY-REF-
            // DIVERGENCE` ("format-2 ref pointer is missing while committed log history exists"),
            // but `finish_interrupted_publication` refused to recover it (unlike an ordinary
            // `PointerLeading` retry, which completes cleanly) -- confirming this is the one state
            // DC-38's design treats as unrecoverable, not merely diagnosed. The pointer-first order
            // below is what prevents a crash from ever producing it through normal publish.
            store.ensure_current_matches(
                &publication.ref_name,
                publication.expected_previous_ref_state_id,
            )?;
            append_ref_pointer_entry(
                &store.layout,
                &PointerIndexEntry {
                    ref_name_key,
                    ref_name: publication.ref_name.clone(),
                    ref_state_id,
                },
            )?;
            container::append_ref_container_record(
                &store.layout,
                ref_name_key,
                &publication.ref_update,
            )?;
        }
        PublicationState::PointerLeading => {
            container::append_ref_container_record(
                &store.layout,
                ref_name_key,
                &publication.ref_update,
            )?;
        }
        PublicationState::Complete => {
            container::append_ref_container_record(
                &store.layout,
                ref_name_key,
                &publication.ref_update,
            )?;
        }
    }
    ensure_agreement(store, publication, &update)?;
    drop(container_locks);
    drop(ref_lock);
    Ok(ref_state_id)
}

/// Reject a new ref whose ASCII-folded name collides with an existing ref other than itself
/// (DC-72). Branch and tag namespaces never collide with each other here: every valid ref name
/// begins with the exact literal `heads/` or `tags/` (`validate_local_branch_ref`/
/// `validate_local_tag_ref` require the case-sensitive prefix), so folding the full name keeps the
/// two prefixes apart. Folds through `prikk_object::ascii_fold`, the one shared folding definition
/// (DC-72 design ruling, `rfcs/accepted/DC-72-PATH-SAFETY-CONFORMANCE.md` §3.5) — see its doc comment
/// for the recorded NFC/NFD limitation this inherits.
fn validate_no_ref_name_collision(store: &RefStore, ref_name: &str) -> Result<()> {
    let folded = ascii_fold(ref_name);
    for existing in store.list_ref_pointers()? {
        if existing.ref_name != ref_name && ascii_fold(&existing.ref_name) == folded {
            return Err(PrikkError::InvalidName(format!(
                "case-insensitive ref-name collision involving: {}",
                existing.ref_name
            )));
        }
    }
    Ok(())
}

fn validate_coherent_publication(publication: &RefPublication) -> Result<RefUpdatePayload> {
    validate_publication(publication)?;
    let ref_state = RefStatePayload::decode_canonical(
        &publication.ref_state.canonical_payload,
        publication.ref_state.schema_version,
    )?;
    let update = RefUpdatePayload::decode_canonical(&publication.ref_update.canonical_payload)?;
    let ref_state_id = publication.ref_state.object_id();
    if ref_state.ref_name != publication.ref_name || update.ref_name != publication.ref_name {
        return Err(PrikkError::Integrity(
            "publication ref names do not agree".to_string(),
        ));
    }
    // Kind-aware, now that the ref-state payload is decoded and its name is confirmed to agree
    // with the publication. Makes namespace and kind mutually enforcing: a Tag-kind publication
    // for `heads/...` and a Branch-kind publication for `tags/...` are both rejected here, neither
    // of which the name-only check below did on its own.
    match ref_state.kind {
        RefKind::Branch => {
            validate_local_branch_ref(&publication.ref_name)?;
        }
        RefKind::Tag => {
            validate_local_tag_ref(&publication.ref_name)?;
        }
    }
    if ref_state.previous_ref_state_id != publication.expected_previous_ref_state_id
        || update.old_ref_state_id != publication.expected_previous_ref_state_id
        || update.new_ref_state_id != ref_state_id
        || update.new_target_object_id != ref_state.target_object_id
        || update.update_seq != ref_state.update_seq
    {
        return Err(PrikkError::Integrity(
            "RefState and RefUpdate publication fields do not agree".to_string(),
        ));
    }
    if update.created_at != 0 {
        return Err(PrikkError::Integrity(
            "schema-1 RefUpdate mutation requires created_at == 0".to_string(),
        ));
    }
    Ok(update)
}

fn classify_state(
    store: &RefStore,
    publication: &RefPublication,
    update: &RefUpdatePayload,
) -> Result<(PublicationState, usize)> {
    let current = store.read_current_ref_state_id(&publication.ref_name)?;
    let replay = store.replay_log(&publication.ref_name)?;
    // RFC 102 Stage 2: a damaged record silently missing from `replay.records` could make
    // `log_position` below classify a corrupted log as a shorter, sound one -- refuse explicitly
    // rather than let a publication proceed against a chain that isn't what it appears to be.
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(format!(
            "ref log for {} has a damaged record; run doctor before publishing",
            publication.ref_name
        )));
    }
    let (log_tip, exact_last, previous_log_tip) = log_position(&replay, publication)?;
    let expected = publication.expected_previous_ref_state_id;
    let proposed = Some(update.new_ref_state_id);
    let state = match (current, log_tip, exact_last, previous_log_tip) {
        (current, tip, false, _) if current == expected && tip == expected => {
            PublicationState::Ready
        }
        (current, tip, false, _) if current == proposed && tip == expected => {
            PublicationState::PointerLeading
        }
        (current, tip, true, previous)
            if current == proposed && tip == proposed && previous == expected =>
        {
            PublicationState::Complete
        }
        _ => {
            return Err(PrikkError::Integrity(format!(
                "ref {} pointer/log state does not match the expected publication transition",
                publication.ref_name
            )));
        }
    };
    Ok((state, replay.trailing_partial_bytes))
}

fn log_position(
    replay: &super::RefLogReplay,
    publication: &RefPublication,
) -> Result<(Option<ObjectId>, bool, Option<ObjectId>)> {
    let mut previous = None;
    let mut before_last = None;
    for (index, record) in replay.records.iter().enumerate() {
        let update = RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
        let expected_sequence = u64::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| PrikkError::Integrity("ref-log sequence overflow".to_string()))?;
        if update.ref_name != publication.ref_name
            || update.old_ref_state_id != previous
            || update.update_seq != expected_sequence
            || update.created_at != 0
        {
            return Err(PrikkError::Integrity(format!(
                "ref-log chain diverges for {}",
                publication.ref_name
            )));
        }
        before_last = previous;
        previous = Some(update.new_ref_state_id);
    }
    let exact_last = replay
        .records
        .last()
        .is_some_and(|record| record.envelope == publication.ref_update);
    Ok((previous, exact_last, before_last))
}

fn ensure_agreement(
    store: &RefStore,
    publication: &RefPublication,
    update: &RefUpdatePayload,
) -> Result<()> {
    let current = store.read_current_ref_state_id(&publication.ref_name)?;
    let replay = store.replay_log(&publication.ref_name)?;
    let last = replay.records.last().map(|record| &record.envelope);
    // RFC 102 Stage 2: already naturally caught below in practice (a damaged just-appended record
    // makes `last` disagree with `publication.ref_update`), named explicitly rather than left
    // incidental -- this is the final agreement check after a write, and its refusal should not
    // depend on the corrupted record happening to be the very last one.
    if replay.has_item_failure()
        || current != Some(update.new_ref_state_id)
        || replay.trailing_partial_bytes != 0
        || last != Some(&publication.ref_update)
    {
        return Err(PrikkError::Integrity(format!(
            "ref {} pointer/log agreement was not established",
            publication.ref_name
        )));
    }
    Ok(())
}
