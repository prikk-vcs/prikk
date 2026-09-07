//! Ref-state pointer and ref-log publication primitives.
//!
//! PR-007 introduced the storage mechanics needed before a full seal command exists: a RefState is
//! stored as a normal content-addressed object, the ref file is a durable pointer to that object,
//! and RefUpdate entries are stored inline in an append-only log. The module does not yet perform
//! publication-policy evaluation or patch/block sealing.

mod container;
mod evidence;
mod pointer_index;
mod publication;
mod verify;

// `append_ref_container_record`'s own sole consumer via this re-export is `refs::tests`
// (DC-71-gated to `target_os = "linux"`, real repository mutation) -- gated to match it exactly,
// not the broader `#[cfg(test)]` the other two names here still need for their own cross-platform
// consumers in `verify::tests::ref_cluster`. A cross-target clippy run caught this as unused on
// Windows before it shipped; see `EXECUTION-ORDER.md` §6 rule 9's own cross-target amendment.
// RFC 131 §3/§5: `pub(in crate::refs)`, not `pub(crate)` -- the sole consumer named above is
// inside `refs` itself, and the function's own declaration is `pub(in crate::refs)` now too.
#[cfg(all(test, target_os = "linux"))]
pub(in crate::refs) use container::append_ref_container_record;
#[cfg(test)]
pub(crate) use container::{
    append_torn_ref_log_tail_for_test, encode_ref_container_record_for_test,
};
#[cfg(feature = "test-support")]
pub use pointer_index::{
    force_ref_pointer_to_arbitrary_state_for_test_support,
    remove_ref_pointer_entry_for_test_support,
};
#[cfg(test)]
pub(crate) use pointer_index::{
    remove_pointer_entries_for_test,
    write_ref_pointer_candidate_for_test as write_ref_pointer_candidate,
    write_ref_pointer_entry_with_explicit_key_for_test,
};
// RFC 102 Stage 6 Step 2, design-v1.md §15.6-§15.9: `compact.rs`'s ref-pointer-index compactor is
// outside `refs`, so these need re-exporting here the same way `verify_refs` already is below --
// `pointer_index` itself stays a private submodule; only the specific items a caller outside `refs`
// needs are widened.
pub(crate) use pointer_index::{
    PointerIndexEntry, encode_pointer_index_record, replay_pointer_index,
};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload, RefUpdatePayload, TagPayload,
};

use crate::foundation::layout::RepositoryLayout;
use crate::lock::ActiveLock;
use crate::object_store::{FileObjectStore, ObjectReader, ObjectWriter};

/// Test-only convenience matching the retired `refs/log.rs::append_log_record`'s own 3-argument
/// call shape exactly, for fixtures that need to plant a specific log record directly without going
/// through a real publish. Computes `ref_name_key` itself.
#[cfg(test)]
pub(crate) fn append_log_record_for_signature_test(
    layout: &RepositoryLayout,
    ref_name: &str,
    envelope: &ObjectEnvelope,
) -> Result<()> {
    container::append_ref_container_record(
        layout,
        crate::foundation::layout::ref_name_key_bytes(ref_name),
        envelope,
    )
}

/// Test-only convenience matching the retired `refs/log.rs::encode_log_record_for_test`'s own
/// single-argument call shape: derives `ref_name_key` from the envelope's own decoded
/// `RefUpdatePayload.ref_name` rather than taking it as a separate parameter, since every caller
/// already has an envelope whose payload names its own ref.
#[cfg(test)]
pub(crate) fn encode_log_record_for_test(envelope: &ObjectEnvelope) -> Result<Vec<u8>> {
    let update = RefUpdatePayload::decode_canonical(&envelope.canonical_payload)?;
    container::encode_ref_container_record_for_test(
        crate::foundation::layout::ref_name_key_bytes(&update.ref_name),
        envelope,
    )
}

pub use container::{RefLogRecord, RefLogReplay};
pub use verify::{
    RefFileOutcome, RefFileStatus, RefItemOutcome, RefItemStatus, RefPublicationIssue,
};
pub(crate) use verify::{ensure_ref_target_valid, verify_refs};

/// The two-hop ref-tip resolution `bundle.rs`, `patch_set_digest.rs`, and `patch_exchange.rs` each
/// need: `Branch` names a Block directly; `Tag` names a Tag object one hop away, whose own
/// `target_block_id` is the actual tip. Consolidated here (ref-tip-resolver-consolidation handoff)
/// after the same "just a two-hop resolve" analysis had been re-derived three times across separate
/// handoffs. Returns the Tag envelope too, whenever one was read -- every caller already decodes it
/// to reach `target_block_id`, so returning it costs nothing, and `bundle.rs`'s own accumulator (which
/// needs the envelope itself) can use it instead of re-reading.
///
/// **Not `ensure_ref_target_valid`'s replacement -- see that function's own doc for why they stay
/// separate.** This resolves; it never validates (a `Branch`'s target block existence is never
/// checked here, unlike that function's own `ensure_block_exists`), and it carries no `owner` id for
/// a diagnostic-quality error, because a resolver is never given one to carry.
///
/// **Exhaustive match, no wildcard**: a future `RefKind` variant must fail to compile here rather
/// than silently resolve to nothing and surface as a misleading "missing Block" error the way
/// `export_bundle`'s own pre-consolidation defect did (`bundle-export-tag-ref-gap-v1.md`).
pub(crate) fn resolve_ref_tip_block(
    object_store: &impl ObjectReader,
    ref_state_payload: &RefStatePayload,
) -> Result<(ObjectId, Option<ObjectEnvelope>)> {
    match ref_state_payload.kind {
        RefKind::Branch => Ok((ref_state_payload.target_object_id, None)),
        RefKind::Tag => {
            let tag_id = ref_state_payload.target_object_id;
            let tag_envelope = object_store
                .read_typed(tag_id, ObjectType::Tag)?
                .ok_or_else(|| PrikkError::Integrity(format!("missing Tag object: {tag_id}")))?;
            let tag_payload = TagPayload::decode_canonical(&tag_envelope.canonical_payload)?;
            Ok((tag_payload.target_block_id, Some(tag_envelope)))
        }
    }
}

pub(crate) fn ensure_no_incomplete_publication(layout: &RepositoryLayout) -> Result<()> {
    let verification = verify_refs(layout)?;
    // DC-95 Stage 2 Level 2: item containment means `verify_refs` now returns `Ok` for a single
    // ref's own read/classification failure instead of aborting -- this gate must check for that
    // directly (`has_item_failure`), the same reason `RepositoryVerification::has_stage_failure`
    // alone stopped being sufficient once `verify_objects` gained the same containment.
    if verification.publication_issues.is_empty()
        && !verification.has_item_failure()
        && !evidence::has_incomplete_active_cleanup(layout)?
    {
        return Ok(());
    }
    // RFC 132 part 2: an incomplete publication is a caller precondition, not a lock -- nothing is
    // held and no other writer is racing this one; the fix is running verify/doctor and retrying
    // with the right signer, not waiting.
    Err(PrikkError::Precondition(
        "repository mutation is blocked by incomplete ref publication; run verify/doctor and use signer-backed seal retry"
            .to_string(),
    ))
}

/// Diagnostic ref candidate derived from an append-only format-1 ref log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRecoveryCandidate {
    /// Human-readable ref name.
    pub ref_name: String,
    /// RefState ID selected by the latest valid ref-log record.
    pub ref_state_id: ObjectId,
    /// Target Block ID selected by the RefState.
    pub target_object_id: ObjectId,
    /// Update sequence of the latest ref-log record.
    pub update_seq: u64,
}

/// One enumerated ref pointer, for deterministic listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPointerSummary {
    /// Human-readable ref name recovered from the pointer file body.
    pub ref_name: String,
    /// Current RefState object ID selected by this pointer.
    pub ref_state_id: ObjectId,
}

/// Inputs for a single ref publication primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPublication {
    /// Human-readable ref name, such as `heads/main`.
    pub ref_name: String,
    /// Expected current RefState ID for CAS. Use `None` to create a new ref.
    pub expected_previous_ref_state_id: Option<ObjectId>,
    /// Signed RefState object envelope to persist before publishing the pointer.
    pub ref_state: ObjectEnvelope,
    /// Signed RefUpdate envelope to append after the ref pointer is durable.
    pub ref_update: ObjectEnvelope,
}

/// File-backed ref-state and ref-log store.
#[derive(Debug, Clone)]
pub struct RefStore {
    layout: RepositoryLayout,
}

impl RefStore {
    /// Create a ref store for a repository layout.
    #[must_use]
    pub fn new(layout: RepositoryLayout) -> Self {
        Self { layout }
    }

    /// Return the repository layout.
    #[must_use]
    pub fn layout(&self) -> &RepositoryLayout {
        &self.layout
    }

    /// Publish a signed RefState with ref-specific locking and CAS. Writes through a freshly
    /// decoded `FileObjectStore` -- the safe default for any caller not holding its own session. See
    /// [`Self::publish_with_object_store`] for a caller that already has one (RFC 111 §6.1 Stage 2).
    ///
    /// # Examples
    ///
    /// `publish` validates the `RefState`/`RefUpdate` pair for internal consistency and requires
    /// each envelope to carry at least one signature -- it does not itself dereference the target
    /// block, so this example signs a placeholder id rather than sealing a real one, keeping the
    /// example to what this specific entry point actually checks. `expected_previous_ref_state_id:
    /// None` is genesis CAS: it fails closed if the ref already has a current state, the same
    /// signal `branch create`/`seal`'s own first publish use.
    ///
    /// ```
    /// use prikk_object::{
    ///     CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload,
    ///     RefUpdatePayload,
    /// };
    /// use prikk_store::{
    ///     Ed25519MaintainerSigner, MaintainerSigner as _, RefPublication, RefStore,
    ///     RepositoryLayout, maintainer_signature,
    /// };
    ///
    /// # fn main() -> prikk_error::Result<()> {
    /// let root = std::env::temp_dir().join(format!("prikk-doctest-publish-{}", std::process::id()));
    /// std::fs::create_dir_all(&root)?;
    /// let layout = RepositoryLayout::init(&root)?;
    /// let signer = Ed25519MaintainerSigner::from_seed("doctest-maintainer", &[3u8; 32])?;
    ///
    /// let target_object_id = ObjectId::from_bytes([9u8; 32]);
    /// let ref_state = RefStatePayload {
    ///     ref_name: "heads/main".to_string(),
    ///     kind: RefKind::Branch,
    ///     target_object_id,
    ///     update_seq: 1,
    ///     previous_ref_state_id: None,
    ///     required_attestation_ids: Vec::new(),
    ///     closed: false,
    /// };
    /// let mut ref_state_envelope =
    ///     ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state.to_canonical_bytes()?);
    /// let ref_state_id = ref_state_envelope.object_id();
    /// ref_state_envelope
    ///     .add_signature(maintainer_signature(&signer, ObjectType::RefState, ref_state_id)?)?;
    ///
    /// let ref_update = RefUpdatePayload {
    ///     ref_name: "heads/main".to_string(),
    ///     old_ref_state_id: None,
    ///     new_ref_state_id: ref_state_id,
    ///     new_target_object_id: target_object_id,
    ///     update_seq: 1,
    ///     created_at: 0,
    ///     author_key_id: signer.key_id().to_string(),
    /// };
    /// let mut ref_update_envelope =
    ///     ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update.to_canonical_bytes()?);
    /// let ref_update_id = ref_update_envelope.object_id();
    /// ref_update_envelope
    ///     .add_signature(maintainer_signature(&signer, ObjectType::RefUpdate, ref_update_id)?)?;
    ///
    /// let store = RefStore::new(layout);
    /// let published_id = store.publish(&RefPublication {
    ///     ref_name: "heads/main".to_string(),
    ///     expected_previous_ref_state_id: None,
    ///     ref_state: ref_state_envelope,
    ///     ref_update: ref_update_envelope,
    /// })?;
    /// assert_eq!(published_id, ref_state_id);
    /// assert_eq!(store.read_current_ref_state_id("heads/main")?, Some(ref_state_id));
    ///
    /// # let _ = std::fs::remove_dir_all(&root);
    /// # Ok(())
    /// # }
    /// ```
    pub fn publish(&self, publication: &RefPublication) -> Result<ObjectId> {
        self.publish_with_object_store(&mut FileObjectStore::new(self.layout.clone()), publication)
    }

    /// Same as [`Self::publish`], but writes the RefState through the caller's own object store
    /// instead of constructing a fresh one -- required for any caller holding an `ObjectWriteSession`
    /// (RFC 111 §6.1 Stage 2 addendum, C1: this is the nested-writer site that must be threaded at or
    /// before the first writer migration, not after).
    pub fn publish_with_object_store(
        &self,
        object_store: &mut impl ObjectWriter,
        publication: &RefPublication,
    ) -> Result<ObjectId> {
        self.layout.require_current_format()?;
        crate::format::validate_object_envelope(self.layout.format(), &publication.ref_state)?;
        crate::format::validate_object_envelope(self.layout.format(), &publication.ref_update)?;
        publication::publish(self, object_store, publication)
    }

    /// Finish an exact signer-backed interrupted publication, including a framing-incomplete tail.
    /// Writes through a freshly decoded `FileObjectStore` -- see
    /// [`Self::finish_interrupted_publication_with_object_store`] for a caller that already has one.
    pub fn finish_interrupted_publication(
        &self,
        active_lock: &ActiveLock,
        publication: &RefPublication,
    ) -> Result<ObjectId> {
        self.finish_interrupted_publication_with_object_store(
            &mut FileObjectStore::new(self.layout.clone()),
            active_lock,
            publication,
        )
    }

    /// Same as [`Self::finish_interrupted_publication`], but writes the RefState through the
    /// caller's own object store instead of constructing a fresh one (RFC 111 §6.1 Stage 2 addendum,
    /// C1).
    pub fn finish_interrupted_publication_with_object_store(
        &self,
        object_store: &mut impl ObjectWriter,
        active_lock: &ActiveLock,
        publication: &RefPublication,
    ) -> Result<ObjectId> {
        self.layout.validate_format()?;
        active_lock.require_layout(&self.layout)?;
        crate::format::validate_read_schema(self.layout.format(), &publication.ref_state)?;
        crate::format::validate_read_schema(self.layout.format(), &publication.ref_update)?;
        evidence::validate_signer_backed_recovery(&self.layout, publication)?;
        publication::finish_interrupted(self, object_store, publication)
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn finish_interrupted_publication_for_test(
        &self,
        publication: &RefPublication,
    ) -> Result<ObjectId> {
        publication::finish_interrupted(
            self,
            &mut FileObjectStore::new(self.layout.clone()),
            publication,
        )
    }

    /// Read the current RefState object ID for a ref name.
    pub fn read_current_ref_state_id(&self, ref_name: &str) -> Result<Option<ObjectId>> {
        let key = crate::foundation::layout::ref_name_key_bytes(ref_name);
        let Some(entry) = pointer_index::lookup_ref_pointer(&self.layout, key)? else {
            return Ok(None);
        };
        if entry.ref_name != ref_name {
            return Err(PrikkError::Integrity(format!(
                "ref pointer name mismatch: expected {ref_name}, got {}",
                entry.ref_name
            )));
        }
        Ok(Some(entry.ref_state_id))
    }

    /// Replay the inline ref-update log for a ref name.
    pub fn replay_log(&self, ref_name: &str) -> Result<RefLogReplay> {
        let key = crate::foundation::layout::ref_name_key_bytes(ref_name);
        container::replay_ref_subsequence(&self.layout, key)
    }

    /// Enumerate every published ref pointer, sorted by name. Reads the ref-pointer index's own
    /// last-entry-per-`ref_name_key` view (RFC 102 Stage 4) -- the container-era equivalent of the
    /// old `by-id/` directory listing, which named the complete set of ref pointers directly; the
    /// index now does.
    pub fn list_ref_pointers(&self) -> Result<Vec<RefPointerSummary>> {
        let replay = pointer_index::replay_pointer_index(&self.layout)?;
        if replay.has_item_failure() {
            return Err(PrikkError::Integrity(
                "ref pointer index has a damaged entry; run doctor before listing".to_string(),
            ));
        }
        let mut latest: std::collections::BTreeMap<[u8; 32], pointer_index::PointerIndexEntry> =
            std::collections::BTreeMap::new();
        for entry in replay.entries {
            latest.insert(entry.ref_name_key, entry);
        }
        let mut summaries: Vec<RefPointerSummary> = latest
            .into_values()
            .map(|entry| RefPointerSummary {
                ref_name: entry.ref_name,
                ref_state_id: entry.ref_state_id,
            })
            .collect();
        summaries.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
        Ok(summaries)
    }

    /// Return a diagnostic candidate when the pointer is missing but the format-1 log is valid.
    pub fn recoverable_missing_ref(&self, ref_name: &str) -> Result<Option<RefRecoveryCandidate>> {
        if self.read_current_ref_state_id(ref_name)?.is_some() {
            return Ok(None);
        }
        let replay = self.replay_log(ref_name)?;
        // RFC 102 Stage 2: checked before the emptiness check below -- a log whose only record is
        // damaged would otherwise read as `replay.records.is_empty()`, and this function's whole
        // purpose is detecting exactly this kind of condition, not passing over it.
        if replay.has_item_failure() {
            return Err(PrikkError::Integrity(format!(
                "ref log for {ref_name} has a damaged record"
            )));
        }
        if replay.records.is_empty() {
            return Ok(None);
        }
        if replay.trailing_partial_bytes != 0 {
            return Err(PrikkError::Integrity(format!(
                "ref log for {ref_name} has trailing partial bytes"
            )));
        }
        let object_store = FileObjectStore::new(self.layout.clone());
        let mut previous_ref_state_id = None;
        let mut latest = None;
        for record in &replay.records {
            let update = RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
            if update.ref_name != ref_name {
                return Err(PrikkError::Integrity(format!(
                    "ref-log record name mismatch: expected {ref_name}, got {}",
                    update.ref_name
                )));
            }
            if update.old_ref_state_id != previous_ref_state_id {
                return Err(PrikkError::Integrity(format!(
                    "ref-log chain mismatch for {ref_name} at update {}",
                    update.update_seq
                )));
            }
            let ref_state = verified_ref_state_payload(
                &object_store,
                update.new_ref_state_id,
                ref_name,
                update.new_target_object_id,
            )?;
            if ref_state.previous_ref_state_id != update.old_ref_state_id {
                return Err(PrikkError::Integrity(format!(
                    "RefState previous link disagrees with RefUpdate for {ref_name}"
                )));
            }
            if ref_state.update_seq != update.update_seq {
                return Err(PrikkError::Integrity(format!(
                    "RefState update sequence disagrees with RefUpdate for {ref_name}"
                )));
            }
            previous_ref_state_id = Some(update.new_ref_state_id);
            latest = Some(update);
        }
        let Some(update) = latest else {
            return Ok(None);
        };
        Ok(Some(RefRecoveryCandidate {
            ref_name: ref_name.to_string(),
            ref_state_id: update.new_ref_state_id,
            target_object_id: update.new_target_object_id,
            update_seq: update.update_seq,
        }))
    }

    /// RFC 132 follow-up: this refusal is defence against a lock-discipline regression, not a live
    /// CAS gate on the only path that reaches it today. Its sole production call site is
    /// `publish_locked`'s `PublicationState::Ready` branch (`refs/publication.rs`), which is chosen
    /// only when `classify_state` has already read the same `read_current_ref_state_id` value and
    /// found it equal to `expected` -- and both reads happen under the same `RefPointerIndex`/
    /// `RefLog` container locks, held continuously by `publish_locked` across the whole span between
    /// them, so nothing can write in between. By the time this function re-reads and compares, the
    /// equality is already established; it cannot fail through that path. It stays because it is
    /// exactly what would catch a future change that broke that locking discipline. Exercised
    /// directly (not through `publish`, which cannot reach the failing branch) by the
    /// `ensure_current_matches_refuses_a_mismatched_expectation` test.
    fn ensure_current_matches(&self, ref_name: &str, expected: Option<ObjectId>) -> Result<()> {
        let current = self.read_current_ref_state_id(ref_name)?;
        if current != expected {
            return Err(PrikkError::LockConflict(format!(
                "ref CAS mismatch for {ref_name}: expected {:?}, got {:?}",
                expected, current
            )));
        }
        Ok(())
    }
}

fn verified_ref_state_payload(
    object_store: &FileObjectStore,
    ref_state_id: ObjectId,
    ref_name: &str,
    target_object_id: ObjectId,
) -> Result<RefStatePayload> {
    let Some(envelope) = object_store.read_typed(ref_state_id, ObjectType::RefState)? else {
        return Err(PrikkError::Integrity(format!(
            "missing RefState object for ref recovery: {ref_state_id}"
        )));
    };
    if envelope.signatures.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "RefState {ref_state_id} is unsigned"
        )));
    }
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if payload.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "RefState {ref_state_id} name mismatch: expected {ref_name}, got {}",
            payload.ref_name
        )));
    }
    if payload.target_object_id != target_object_id {
        return Err(PrikkError::Integrity(format!(
            "RefState {ref_state_id} target disagrees with ref log for {ref_name}"
        )));
    }
    let Some(target) = object_store.read_object(target_object_id)? else {
        return Err(PrikkError::Integrity(format!(
            "RefState {ref_state_id} targets missing block {target_object_id}"
        )));
    };
    if target.object_type != ObjectType::Block {
        return Err(PrikkError::Integrity(format!(
            "RefState {ref_state_id} targets {}, expected block",
            target.object_type
        )));
    }
    Ok(payload)
}

pub(crate) fn validate_publication(publication: &RefPublication) -> Result<()> {
    require_signed_type(&publication.ref_state, ObjectType::RefState)?;
    require_signed_type(&publication.ref_update, ObjectType::RefUpdate)?;
    publication.ref_state.validate_strict()?;
    publication.ref_update.validate_strict()?;
    Ok(())
}

pub(crate) fn require_signed_type(
    envelope: &ObjectEnvelope,
    object_type: ObjectType,
) -> Result<()> {
    if envelope.object_type != object_type {
        return Err(PrikkError::ObjectTypeMismatch {
            expected: object_type.to_string(),
            actual: envelope.object_type.to_string(),
        });
    }
    if envelope.signatures.is_empty() {
        return Err(PrikkError::InvalidSignature(format!(
            "{object_type} publication envelope must be signed"
        )));
    }
    envelope.validate()
}

/// Validate a local branch ref name and return its canonical identity string.
pub fn validate_local_branch_ref(ref_name: &str) -> Result<String> {
    if ref_name.is_empty() {
        return Err(PrikkError::InvalidName(
            "ref name must not be empty".to_string(),
        ));
    }
    if ref_name.starts_with("tags/")
        || ref_name.starts_with("remotes/")
        || ref_name.starts_with("rollback/")
    {
        return Err(PrikkError::InvalidName(format!(
            "ref namespace is reserved: {ref_name}"
        )));
    }
    if !ref_name.starts_with("heads/") {
        return Err(PrikkError::InvalidName(format!(
            "ref {ref_name} is not a local branch ref; expected heads/<name>"
        )));
    }
    let branch = &ref_name["heads/".len()..];
    if branch.is_empty() {
        return Err(PrikkError::InvalidName(
            "branch ref must include a name after heads/".to_string(),
        ));
    }
    if ref_name.chars().any(|ch| ch == '\0' || ch.is_control()) {
        return Err(PrikkError::InvalidName(format!(
            "ref {ref_name} contains a forbidden control character"
        )));
    }
    if branch.starts_with('/') || branch.ends_with('/') || branch.contains("//") {
        return Err(PrikkError::InvalidName(format!(
            "branch ref {ref_name} contains an empty path component"
        )));
    }
    if branch
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return Err(PrikkError::InvalidName(format!(
            "branch ref {ref_name} contains a traversal component"
        )));
    }
    Ok(ref_name.to_string())
}

/// Validate a local tag ref name and return its canonical identity string.
///
/// Mirrors `validate_local_branch_ref` with the prefix requirement inverted: `tags/` required,
/// `heads/`/`remotes/`/`rollback/` reserved. Deliberately carries no case-collision rule itself —
/// `validate_local_branch_ref` does not have one either — **but that does not mean `tags/V1` and
/// `tags/v1` coexist as distinct refs.** `validate_no_ref_name_collision` (`refs/publication.rs`)
/// folds every ref name through `ascii_fold` at publication time and refuses a new ref whose folded
/// name collides with an existing one, so an ordinary case-only pair is rejected there, not here.
/// What survives this pair of validators is narrower: a collision that predates the publication
/// check, or one that arrives by a path that never goes through publication; and NFC/NFD or
/// non-ASCII case pairs, which `ascii_fold` cannot see by construction (`refs/publication.rs`'s own
/// doc comment on `validate_no_ref_name_collision` already records this). That residual gap is real
/// but is NFR-SEC-03's, unmet for both namespaces, and tracked separately rather than closed
/// asymmetrically here.
pub fn validate_local_tag_ref(ref_name: &str) -> Result<String> {
    if ref_name.is_empty() {
        return Err(PrikkError::InvalidName(
            "ref name must not be empty".to_string(),
        ));
    }
    if ref_name.starts_with("heads/")
        || ref_name.starts_with("remotes/")
        || ref_name.starts_with("rollback/")
    {
        return Err(PrikkError::InvalidName(format!(
            "ref namespace is reserved: {ref_name}"
        )));
    }
    if !ref_name.starts_with("tags/") {
        return Err(PrikkError::InvalidName(format!(
            "ref {ref_name} is not a local tag ref; expected tags/<name>"
        )));
    }
    let tag = &ref_name["tags/".len()..];
    if tag.is_empty() {
        return Err(PrikkError::InvalidName(
            "tag ref must include a name after tags/".to_string(),
        ));
    }
    if ref_name.chars().any(|ch| ch == '\0' || ch.is_control()) {
        return Err(PrikkError::InvalidName(format!(
            "ref {ref_name} contains a forbidden control character"
        )));
    }
    if tag.starts_with('/') || tag.ends_with('/') || tag.contains("//") {
        return Err(PrikkError::InvalidName(format!(
            "tag ref {ref_name} contains an empty path component"
        )));
    }
    if tag
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return Err(PrikkError::InvalidName(format!(
            "tag ref {ref_name} contains a traversal component"
        )));
    }
    Ok(ref_name.to_string())
}

// DC-71: every test here (including the nested publication_recovery/state_matrix trees) sets up
// its scenario via real repository mutation, which is Linux-only; the module never compiles a
// non-Linux-meaningful test.
#[cfg(all(test, target_os = "linux"))]
mod tests;
