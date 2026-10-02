//! RFC 165 R4 (N3): completing an interrupted publication.
//!
//! A pointer lead -- the pointer names a RefState the ref log has not yet confirmed -- is completable
//! when the rule below holds. This module is the one place that rule lives: `verify`'s own reporting
//! (`PRIKK-VERIFY-REF-POINTER-LEADS-LOG` vs `PRIKK-VERIFY-REF-DIVERGENCE`) and `prikk ref complete`'s
//! own precondition both call [`plan_ref_completion`], so a lead is never reported completable by one
//! and refused by the other for a reason this rule itself should have caught.
//!
//! **The rule, every condition evaluated before any write (K2: fail closed on any doubt):**
//! - (a) the leading RefState verifies under the current trust policy, signed by an adopted
//!   maintainer key;
//! - (b) it chains: its ref name is the ref, its previous state is the log's own tip, and its
//!   sequence is the next one -- folded into the leading-state classification below, not a separate
//!   check: a pointer that leads by anything other than exactly one coherent transition is not a
//!   completable lead at all;
//! - (c) its target exists, with the kind the ref requires ([`ensure_ref_target_valid`]);
//! - (d) for a publication that consumed the active WAL (a `seal`/`sync seal` in progress for this
//!   exact ref, by the same classification `verify`'s own `ActiveWalMetadataStatus` uses), the
//!   retained WAL evidence still matches. Skipped entirely for every other publication kind (`branch
//!   create`, `tag create`, `sync adopt-tag`, `merge`): there is no WAL evidence for them to check in
//!   the first place, so a check gated on its presence would always refuse them -- the exact DC-38
//!   "seal only" limitation RFC 165 R4 exists to end;
//! - (e) no complete damage anywhere in the ref log or the pointer index -- checked first, globally,
//!   not attributed to one ref until it decodes (matching `refs::ensure_no_incomplete_publication`'s
//!   own reasoning for the same ordering).
//!
//! Never a new code path for the append itself: completing a plan still goes through
//! `publication::finish_interrupted` -> `publish_locked`, the same primitive DC-38's own `seal` retry
//! uses -- reached here via `RefStore::finish_interrupted_publication_for_ref_complete`, a sibling
//! entry point that skips `finish_interrupted_publication_with_object_store`'s own
//! `validate_signer_backed_recovery` (which unconditionally requires matching retained WAL evidence,
//! correct for `seal`'s own retry but wrong for the four publication kinds this rule's condition (d)
//! has no WAL evidence for at all) because the rule above has already decided, more generally, whether
//! this exact write is safe. This module only decides *whether* to call it and *what*
//! `RefPublication` to build, signed by the completing key -- the append itself is untouched.

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlockPayload, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload, RefUpdatePayload,
};

use crate::commit_boundary::active::ActiveRefMetadata;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::maintainer_signing::{MaintainerSigner, maintainer_signature};
use crate::object_store::{FileObjectStore, ObjectReader, ObjectWriter};
use crate::refs::{
    RefPublication, RefStore, ensure_ref_target_valid, ref_log_tail_status, replay_pointer_index,
};
use crate::trust::{load_maintainer_trust_policy, verify_trusted_publication_envelope};
use crate::wal::Wal;

/// Everything `prikk ref complete <ref>` needs to print before it writes, and to build the
/// publication from once a signer is supplied (K1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompletionPlan {
    /// The ref this plan completes.
    pub ref_name: String,
    /// The pointer's own current (leading) RefState id.
    pub leading_ref_state_id: ObjectId,
    /// The adopted key that signed the leading RefState (condition (a)'s own witness).
    pub original_signer_key_id: String,
    /// The leading RefState's own target.
    pub target_object_id: ObjectId,
    /// The leading RefState's own kind.
    pub kind: RefKind,
    /// The ref log's own current tip for this ref -- what the completing record will chain to.
    pub log_tip: Option<ObjectId>,
    /// The sequence number the completing record will carry.
    pub next_sequence: u64,
    /// Trailing bytes this ref's own log subsequence currently carries, attributable to this ref,
    /// that completion's own write removes first (`publish_locked`'s existing partial-tail-repair
    /// path) -- `0` when there is nothing to remove.
    pub removes_partial_tail_bytes: usize,
}

/// Why a lead is not completable. One variant per rule condition, each carrying enough to explain
/// itself without the caller re-deriving which check failed (RFC 165 R4 K3's own "each condition gets
/// a constructed case" -- the per-condition text here is what those tests assert against).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompletionRefusal {
    /// The ref has no pointer at all.
    NoPointer,
    /// The pointer does not lead the log by exactly one coherent transition (also covers condition
    /// (b): a chain that does not fit is not a lead this rule recognizes).
    NotALead,
    /// Condition (e): the pointer index has a damaged record, anywhere.
    PointerIndexDamaged,
    /// Condition (e): the ref log has a damaged record, anywhere.
    RefLogDamaged(String),
    /// Condition (e): this ref's own log subsequence has a damaged record.
    OwnLogDamaged,
    /// Condition (a): no adopted key's signature on the leading RefState verifies.
    UntrustedSigner(String),
    /// Condition (c): the leading RefState's own target does not exist, or is the wrong kind.
    InvalidTarget(String),
    /// Condition (d): a WAL-consuming publication's retained evidence does not match.
    WalEvidenceMismatch(String),
}

impl std::fmt::Display for CompletionRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPointer => write!(formatter, "has no pointer; nothing to complete"),
            Self::NotALead => write!(
                formatter,
                "pointer does not lead the log by exactly one coherent transition; not a pending completion"
            ),
            Self::PointerIndexDamaged => {
                write!(formatter, "the ref pointer index has a damaged record")
            }
            Self::RefLogDamaged(message) => {
                write!(formatter, "the ref log has a damaged record: {message}")
            }
            Self::OwnLogDamaged => write!(
                formatter,
                "this ref's own log subsequence has a damaged record"
            ),
            Self::UntrustedSigner(message) => write!(
                formatter,
                "the leading RefState's signature does not verify under the current trust policy: {message}"
            ),
            Self::InvalidTarget(message) => {
                write!(
                    formatter,
                    "the leading RefState's own target is invalid: {message}"
                )
            }
            Self::WalEvidenceMismatch(message) => write!(
                formatter,
                "a signer-backed completion requires matching retained active-WAL evidence: {message}"
            ),
        }
    }
}

/// RFC 165 R4: evaluate the completion rule for `ref_name`, read-only. `Ok(Ok(plan))` when every
/// condition holds; `Ok(Err(refusal))` when the ref has a pointer lead that fails one of them (still
/// not an error -- a refused plan is a normal, reportable outcome, not a propagated failure); a hard
/// `Err` only for an I/O or decode failure this rule cannot itself classify.
pub fn plan_ref_completion(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<std::result::Result<CompletionPlan, CompletionRefusal>> {
    // Condition (e), first and global: no complete damage anywhere, checked before this ref's own
    // state is even read (K2 -- a damaged container's own bytes are not trustworthy enough to use
    // for classifying anything, including whether this ref is a lead at all). This pointer-index
    // half is `ref complete`'s own, not shared with RFC 165 R5's rebuild: a rebuild's whole purpose
    // is to fix a damaged pointer-index record, so that same damage must never block it from
    // evaluating an unrelated ref's own lead (`pointer_rebuild.rs` calls [`evaluate_known_lead`]
    // directly, with its own damage-tolerant pointer read, bypassing this gate on purpose).
    if replay_pointer_index(layout)?.has_item_failure() {
        return Ok(Err(CompletionRefusal::PointerIndexDamaged));
    }
    let store = RefStore::new(layout.clone());
    let Some(leading_id) = store.read_current_ref_state_id(ref_name)? else {
        return Ok(Err(CompletionRefusal::NoPointer));
    };
    evaluate_known_lead(layout, ref_name, leading_id)
}

/// RFC 165 R4's own rule, conditions (b)-(e)'s ref-log half plus (a)/(c)/(d), given a `leading_id`
/// the caller has already resolved (by whatever means -- [`plan_ref_completion`]'s own damage-gated
/// pointer lookup, or RFC 165 R5's rebuild, which tolerates pointer-index damage elsewhere). Shared
/// so the two verbs' own classification can never drift on what counts as a completable lead.
pub(crate) fn evaluate_known_lead(
    layout: &RepositoryLayout,
    ref_name: &str,
    leading_id: ObjectId,
) -> Result<std::result::Result<CompletionPlan, CompletionRefusal>> {
    let (_, _, ref_log_interior_damage) = ref_log_tail_status(layout)?;
    if let Some(message) = ref_log_interior_damage {
        return Ok(Err(CompletionRefusal::RefLogDamaged(message)));
    }

    let store = RefStore::new(layout.clone());
    let replay = store.replay_log(ref_name)?;
    if replay.has_item_failure() {
        return Ok(Err(CompletionRefusal::OwnLogDamaged));
    }
    let log_tip = replay
        .records
        .last()
        .map(|record| {
            RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)
                .map(|update| update.new_ref_state_id)
        })
        .transpose()?;
    let next_sequence = u64::try_from(replay.records.len())
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| PrikkError::Integrity("ref-log sequence overflow".to_string()))?;

    let objects = FileObjectStore::new(layout.clone());
    let state_envelope = objects
        .read_typed(leading_id, ObjectType::RefState)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing RefState object: {leading_id}")))?;
    let state = RefStatePayload::decode_canonical(
        &state_envelope.canonical_payload,
        state_envelope.schema_version,
    )?;

    // Condition (b): a pointer lead is, by definition, a pointer naming a RefState the log has not
    // yet confirmed, exactly one transition ahead -- the same test `refs::publication::classify_state`
    // uses to recognize `PublicationState::PointerLeading` in the first place.
    if state.ref_name != ref_name
        || state.previous_ref_state_id != log_tip
        || state.update_seq != next_sequence
    {
        return Ok(Err(CompletionRefusal::NotALead));
    }

    // Condition (a).
    let policy = load_maintainer_trust_policy(layout)?;
    let original_signer_key_id = match verify_trusted_publication_envelope(&policy, &state_envelope)
    {
        Ok(key_id) => key_id,
        Err(issue) => return Ok(Err(CompletionRefusal::UntrustedSigner(issue.message))),
    };

    // Condition (c).
    if let Err(error) =
        ensure_ref_target_valid(&objects, state.kind, state.target_object_id, leading_id)
    {
        return Ok(Err(CompletionRefusal::InvalidTarget(error.to_string())));
    }

    // Condition (d): only when the active WAL's own retained metadata claims *this* ref -- the same
    // classification `verify`'s own `classify_active_wal_metadata` uses, reused directly so the two
    // never drift on what counts as "a seal/sync seal in progress for this ref".
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let wal_replay = wal.replay()?;
    let active_metadata = crate::commit_boundary::active::read_active_ref_metadata(layout)?;
    let wal_claims_this_ref = !wal_replay.records.is_empty()
        && matches!(active_metadata, ActiveRefMetadata::Valid(active) if active == ref_name);
    if wal_claims_this_ref {
        if wal_replay.has_item_failure() {
            return Ok(Err(CompletionRefusal::WalEvidenceMismatch(
                "the active WAL has a damaged record".to_string(),
            )));
        }
        if wal_replay.trailing_partial_bytes != 0 {
            return Ok(Err(CompletionRefusal::WalEvidenceMismatch(
                "the active WAL has an incomplete trailing record".to_string(),
            )));
        }
        let block = objects
            .read_typed(state.target_object_id, ObjectType::Block)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!("missing Block object: {}", state.target_object_id))
            })?;
        let block_payload = BlockPayload::decode_canonical(&block.canonical_payload)?;
        let wal_patch_ids: Vec<ObjectId> = wal_replay
            .records
            .iter()
            .map(|record| record.envelope.object_id())
            .collect();
        if block_payload.patch_ids != wal_patch_ids {
            return Ok(Err(CompletionRefusal::WalEvidenceMismatch(
                "the retained active WAL does not prove the leading target Block".to_string(),
            )));
        }
    }

    Ok(Ok(CompletionPlan {
        ref_name: ref_name.to_string(),
        leading_ref_state_id: leading_id,
        original_signer_key_id,
        target_object_id: state.target_object_id,
        kind: state.kind,
        log_tip,
        next_sequence,
        removes_partial_tail_bytes: replay.trailing_partial_bytes,
    }))
}

/// RFC 165 R4: carry out a plan [`plan_ref_completion`] already found completable, signed by
/// `signer` (any adopted maintainer key -- owner decision 2, RFC 165 §6: the state being endorsed is
/// already signed, the completer adds only the log record, which names its own key). Gates `signer`
/// itself against the trust policy first (`GatedOperation::RefComplete`) -- a separate question from
/// condition (a)'s check of the *original* signer inside `plan_ref_completion`, and the one this
/// module would otherwise be the only publishing verb to skip. Builds the one new object this verb
/// produces (a signed `RefUpdate` envelope chaining to `plan.log_tip` at `plan.next_sequence`, naming
/// `plan.leading_ref_state_id`) and reuses the already-durable, already-signed `RefState` object
/// verbatim -- completion never re-signs or re-derives it.
///
/// Routes through `RefStore::finish_interrupted_publication_for_ref_complete` -- see the module
/// doc comment for why this, and not DC-38's own `finish_interrupted_publication_with_object_store`,
/// is the right entry point; both still terminate in the same `publish_locked` write.
pub fn complete_ref_publication(
    layout: &RepositoryLayout,
    object_store: &mut impl ObjectWriter,
    active_lock: &crate::lock::ActiveLock,
    plan: &CompletionPlan,
    signer: &impl MaintainerSigner,
) -> Result<ObjectId> {
    crate::trust::verify_signer_trusted(layout, signer, crate::trust::GatedOperation::RefComplete)?;
    let objects = FileObjectStore::new(layout.clone());
    let ref_state = objects
        .read_typed(plan.leading_ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "missing RefState object: {}",
                plan.leading_ref_state_id
            ))
        })?;

    let update = RefUpdatePayload {
        ref_name: plan.ref_name.clone(),
        old_ref_state_id: plan.log_tip,
        new_ref_state_id: plan.leading_ref_state_id,
        new_target_object_id: plan.target_object_id,
        update_seq: plan.next_sequence,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut update_envelope = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        prikk_object::CanonicalEncode::to_canonical_bytes(&update)?,
    );
    let update_id = update_envelope.object_id();
    update_envelope.add_signature(maintainer_signature(
        signer,
        ObjectType::RefUpdate,
        update_id,
    )?)?;

    let publication = RefPublication {
        ref_name: plan.ref_name.clone(),
        expected_previous_ref_state_id: plan.log_tip,
        ref_state,
        ref_update: update_envelope,
    };
    let store = RefStore::new(layout.clone());
    store.finish_interrupted_publication_for_ref_complete(object_store, active_lock, &publication)
}

#[cfg(test)]
mod tests;
