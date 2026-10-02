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
    append_torn_ref_log_tail_for_test, decode_ref_container_records,
    encode_ref_container_record_for_test,
};
#[cfg(feature = "test-support")]
pub use pointer_index::{
    force_ref_pointer_to_arbitrary_state_for_test_support,
    remove_ref_pointer_entry_for_test_support,
};
#[cfg(test)]
pub(crate) use pointer_index::{
    remove_pointer_entries_for_test, write_ref_pointer_candidate_for_test,
    write_ref_pointer_entry_with_explicit_key_for_test,
};
// RFC 102 Stage 6 Step 2, design-v1.md §15.6-§15.9: `compact.rs`'s ref-pointer-index compactor is
// outside `refs`, so these need re-exporting here the same way `verify_refs` already is below --
// `pointer_index` itself stays a private submodule; only the specific items a caller outside `refs`
// needs are widened.
pub use pointer_index::PointerIndexRepair;
#[cfg(test)]
pub(crate) use pointer_index::decode_pointer_index_records;
pub(crate) use pointer_index::{
    PointerIndexEntry, encode_pointer_index_record, replay_pointer_index,
    truncate_pointer_index_trailing_partial,
};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload, RefUpdatePayload, TagPayload,
};

use crate::foundation::layout::RepositoryLayout;
use crate::lock::ActiveLock;
use crate::object_store::{FileObjectStore, ObjectReadSnapshot, ObjectReader, ObjectWriter};

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
/// **`pub` since RFC 147 §3d**, so `prikk branch create --from tags/<t>` can dereference a tag ref
/// through this function rather than becoming a seventh hand-rolled copy of the same two hops in
/// `prikk-cli`. That is the whole reason the visibility widened: the alternative was another copy,
/// and copies of this exact walk are what §3b and §3c spent two rounds repairing.
pub fn resolve_ref_tip_block(
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

/// Read a published ref by name and resolve it to the Block its tip names.
///
/// RFC 147 §3d: `patch_replay/read.rs` and `patch_inverse/read.rs` each carried a `pub(super) fn
/// current_target_block` with the same signature and a byte-identical body. That duplication is
/// exactly how §3b's fix reached one and not the other, leaving `inverse-plan`, `rollback-preview`
/// and `rollback-draft-verify` refusing a valid tag ref while `log` and `checkout` accepted it --
/// and a comment in one of them asserted, wrongly, that they were shared. One function now, here,
/// because both callers already depend on this module and neither depends on the other: putting it
/// in either of them would invent a lateral `patch_replay <-> patch_inverse` edge for nothing.
///
/// This is [`resolve_ref_tip_block`] plus the read that gets you a [`RefStatePayload`] in the first
/// place; that function stays separate because several callers already hold a decoded payload and
/// must not re-read it. Like it, this **resolves and never validates** -- the returned Block id is
/// not checked to exist, which is each caller's own job (see `ensure_ref_target_valid`).
pub(crate) fn read_current_ref_tip_block(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
) -> Result<ObjectId> {
    let ref_store = RefStore::new(layout.clone());
    let ref_state_id = ref_store
        .read_current_ref_state_id(ref_name)?
        .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} is not published")))?;
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {ref_name} points to missing RefState {ref_state_id}"
            ))
        })?;
    let ref_state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if ref_state.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "RefState name mismatch: expected {ref_name}, got {}",
            ref_state.ref_name
        )));
    }
    let (block_id, _tag_envelope) = resolve_ref_tip_block(object_store, &ref_state)?;
    Ok(block_id)
}

/// Refuse to proceed while any ref is mid-publication or fails its own verification.
///
/// The precondition every write path checks before touching refs: a publication interrupted between
/// its pointer write and its ref-state write leaves a ref that reads as neither old nor new, and a
/// second writer stepping onto it would make the damage permanent rather than resumable.
///
/// RFC 165 R2 (M8): answers exactly this question, in one pass over the pointer index and one over
/// the ref log container, instead of `verify_refs`'s full per-ref `replay_ref_subsequence` loop
/// (refs × log size) -- `verify_refs` is no longer called from here at all. `verify` and `doctor`
/// keep the fuller check this precondition never needs (chain continuity, signature-envelope
/// structure, missing-object detection): those answer "is this repository's ref state fully sound,"
/// a broader question than "is a write safe to proceed." But two of `verify_refs`'s checks are kept,
/// explicit and separate, because the existing test suite ties them to this precondition specifically
/// -- removing them during this round's own implementation turned six tests red, each naming a
/// mutation entry point (`append_patch`, `add_trusted_maintainer`, `repair_repository`) that depends
/// on this exact gate catching it, not `verify`/`doctor` catching it later:
/// - **candidate debris** (`candidate_issues`, below) -- a `refs/tmp/` leftover from an era before
///   RFC 102 Stage 4's append-only pointer index, kept reachable for an older-format repository and
///   for any writer outside `prikk` that still produces one; a cheap directory listing, not a
///   refs-or-log-sized read.
/// - **a legacy (`created_at != 0`) ref-log record** -- free to check here: this function's own log
///   scan already decodes every record's `RefUpdatePayload` to read `new_ref_state_id`; checking
///   `created_at` costs nothing further.
///   `has_incomplete_active_cleanup` stays for the same reason, and because review v2 §2 item 1
///   already named it explicitly: not incidental coverage, half of what this precondition protects
///   (a settled publication whose queue has not drained).
///
/// **A fourth check, beyond R2's own literal "two passes," found the same way**: a ref whose pointer
/// and log agree on a `RefState` id, but whose `RefState` object is itself missing or unreadable, is
/// still an unsafe state for a second writer to build on -- `add_trusted_maintainer` and
/// `repair_repository` do not always read the ref content their own write touches, so this is not
/// always caught downstream the way a content-reading command like `commit` or `seal` mostly would
/// be. One `ObjectReadSnapshot::open` (one object-index decode, the same cost shape `verify`'s own
/// object stage already pays once) followed by an O(1) `contains_object` lookup per distinct
/// `RefState` id the pointer index names keeps this a bounded, one-more-read cost, not a return to
/// refs × anything -- measured alongside R2's own release timing, not assumed.
pub(crate) fn ensure_no_incomplete_publication(layout: &RepositoryLayout) -> Result<()> {
    ensure_no_incomplete_publication_except(layout, None)
}

/// RFC 165 R3 (C3): the same precondition, but blind to one ref's own pointer/log disagreement and
/// missing-object state -- `exclude_ref_name`. `commit` and every other non-publishing writer call
/// this (unexcluded, via `ensure_no_incomplete_publication`); the six publications call
/// [`ensure_may_publish`] instead, which adds the ref-log tail check below -- this function alone never
/// refuses merely because the ref log container has a lead-free tail (RFC 165 Addendum 1 §1): `commit`
/// never appends to the ref log, so Rule D does not apply to it. `exclude_ref_name: None` is
/// `ensure_no_incomplete_publication` itself: every other check here (damaged records anywhere,
/// a legacy record anywhere, candidate debris, pending active cleanup) stays global regardless of
/// which ref is excluded -- none of them are attributed to one ref in the first place (candidate
/// debris carries no ref name at all; a crash cannot produce a legacy record under current write-time
/// enforcement, so one found anywhere is already an anomaly, not this ref's own business).
///
/// **Known interim state (R3, not R4): excluding a ref from this check does not give it a way to
/// *finish* an interrupted publication** -- only `seal`'s own pre-existing DC-38 retry mechanism
/// actually completes one. `branch create`/`branch close`/`tag create`/`sync adopt-tag`/`merge`
/// still answer "already exists"/"not confluent" once past this check, exactly as before R3; `sync
/// seal` is deliberately **not** changed to call this excluding form at all (its own two existing,
/// unscoped `ensure_no_incomplete_publication` calls stay as they are) -- review v2 and the round 1
/// handoff both name `sync seal`'s self-lockout as a known gap this round does not work around.
pub fn ensure_no_incomplete_publication_except(
    layout: &RepositoryLayout,
    exclude_ref_name: Option<&str>,
) -> Result<()> {
    ensure_publication_precondition(layout, exclude_ref_name, false)
}

/// RFC 165 Addendum 1 §1: [`ensure_no_incomplete_publication_except`], plus RFC 164 Rule D's "a writer
/// refuses over a tail in a file it appends to," applied to the ref log container specifically -- every
/// publication appends to it; `commit` and every other caller of the function above never does. One
/// read of the ref log container total: the tail check reuses the same `discovery`/bytes the
/// precondition above already read, not a second whole read.
///
/// **Why this is not the same question as an incomplete publication**: `publish_locked` writes the
/// pointer before the log, so a genuine crash mid-publication always leaves a pointer lead, which the
/// precondition's own agreement check (above) catches regardless of whether the ref log also has a
/// tail. A tail with **no** pointer lead -- zeros, random bytes, or a torn prefix the crash left behind
/// for reasons unrelated to any pending write -- is not an interrupted publication at all (RFC 165 R5
/// names exactly this shape), and must not block `commit` or any other writer that does not append to
/// the ref log; `ensure_no_incomplete_publication_except` alone (no tail check) is what those callers
/// use. But a *publishing* command is about to append past that tail, which Rule D forbids regardless
/// of why the tail is there -- hence this separate, additive check, named and worded differently (the
/// tail's own offset and byte count, and that its repair arrives with R5 -- never "incomplete
/// publication," never "seal retry," since neither applies to a lead-free tail). `ref_name`'s own
/// attributable tail is let through, the same as the precondition's own exclusion: a retry of `ref_name`
/// itself is DC-38's business, not Rule D's.
pub fn ensure_may_publish(layout: &RepositoryLayout, ref_name: &str) -> Result<()> {
    ensure_publication_precondition(layout, Some(ref_name), true)
}

fn ensure_publication_precondition(
    layout: &RepositoryLayout,
    exclude_ref_name: Option<&str>,
    check_ref_log_tail: bool,
) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};

    let excluded_key = exclude_ref_name.map(crate::foundation::layout::ref_name_key_bytes);

    let pointer_replay = replay_pointer_index(layout)?;
    if pointer_replay.has_item_failure() {
        return Err(incomplete_publication_refusal());
    }
    let mut newest_pointer: BTreeMap<[u8; 32], ObjectId> = BTreeMap::new();
    for entry in &pointer_replay.entries {
        newest_pointer.insert(entry.ref_name_key, entry.ref_state_id);
    }

    // RFC 165 R2: this is the one whole-container read this precondition now does, in total --
    // the `ref-log-replay` open finding (`whole_read_guard.rs`'s own `SCOPES` table), reached here
    // once per precondition check instead of once per ref.
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let mut newest_log: BTreeMap<[u8; 32], ObjectId> = BTreeMap::new();
    if let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )? {
        let discovery = container::decode_ref_container_records(&bytes)?;
        // RFC 165 R5 (§9.2): `has_damage()` is true for a complete, fully-written record whose later
        // validation failed (damage, wherever it sits) or a `Failed` frame with something sound or
        // failed after it (interior damage) -- never for the container's own genuine, lead-free tail
        // (represented purely by `trailing_partial_bytes`, checked separately by `ensure_may_publish`
        // for the publications that append to this container; see its own doc for why a lead-free
        // tail is a different question from this one). Not attributable to one ref until it decodes,
        // so this refuses for the whole repository rather than naming which ref, unlike `verify`'s own
        // per-ref attribution.
        if discovery.has_damage() {
            return Err(ref_log_damage_refusal(&discovery));
        }
        for record in &discovery.records {
            let update = RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
            if update.created_at != 0 {
                return Err(incomplete_publication_refusal());
            }
            newest_log.insert(record.ref_name_key, update.new_ref_state_id);
        }
        if check_ref_log_tail {
            if let Some(tail) = container::ref_log_container_tail(&bytes, &discovery) {
                // RFC 165 Addendum 1 §1 fix: a torn tail too short to carry a readable
                // `ref_name_key` (as little as a handful of bytes -- shorter even than the header's
                // own `ref_name_key` field) is "unattributable" by header inspection alone, but the
                // excluded ref's *own* retry still must not be blocked by it. The earlier version of
                // this check used `tail.attributed_ref_name_key == excluded_key`, which refused a
                // seal retrying its own first-ever, very-short torn write (nothing to read a name
                // from) -- `seal_truncates_only_partial_tail_before_completion` caught this. The
                // right test is not "whose name does the tail's header claim" but "does the excluded
                // ref itself currently have a pointer lead" (`newest_pointer` vs. `newest_log`
                // disagree for it): the agreement loop below already refuses for every *other* ref
                // that leads, before this point is ever reached (publish_locked writes the pointer
                // before the log, so a genuine crash always produces a lead for its own ref, not just
                // a tail) -- so if control reaches here, either the excluded ref itself leads (its
                // own business, DC-38 completes it regardless of whether the physical tail can be
                // attributed to it by header inspection), or no ref leads at all, in which case
                // whatever tail is present is lead-free by elimination and Rule D applies.
                let excluded_ref_leads = match excluded_key {
                    Some(key) => newest_pointer.get(&key) != newest_log.get(&key),
                    None => false,
                };
                if !excluded_ref_leads {
                    crate::foundation::tail_guard::require_no_unclean_tail(
                        "the ref log",
                        tail.len,
                        tail.offset,
                        "a repair arrives with RFC 165 R5",
                    )?;
                }
            }
        }
    }

    let mut keys: BTreeSet<[u8; 32]> = BTreeSet::new();
    keys.extend(newest_pointer.keys().copied());
    keys.extend(newest_log.keys().copied());
    for key in keys {
        if Some(key) == excluded_key {
            continue;
        }
        if newest_pointer.get(&key) != newest_log.get(&key) {
            return Err(incomplete_publication_refusal());
        }
    }

    // One object-index decode, then an O(1) lookup per distinct pointer-named RefState -- the
    // fourth check named above.
    let objects = ObjectReadSnapshot::open(layout)?;
    for (key, ref_state_id) in &newest_pointer {
        if Some(*key) == excluded_key {
            continue;
        }
        if !objects.contains_object(ObjectType::RefState, *ref_state_id) {
            return Err(incomplete_publication_refusal());
        }
    }

    if !verify::candidate_issues(layout)?.is_empty() {
        return Err(incomplete_publication_refusal());
    }
    if evidence::has_incomplete_active_cleanup(layout, exclude_ref_name)? {
        return Err(incomplete_publication_refusal());
    }
    Ok(())
}

/// RFC 165 R5 (§9.2, Rule B extended to the ref log): the container's own tail/damage status, for
/// `verify`'s `AppendedFileTailStatus` reporting (RFC 164 Rule B) and `doctor --repair-tails`, matching
/// how both already read each Rule-A file directly and independently rather than through whatever else
/// touches it first. Returns `(trailing_partial_bytes, tail_offset, interior_damage_message)` -- plain
/// primitives, since `AppendedFileTailStatus` itself lives in `crate::verify`, a sibling module that
/// cannot see `container`'s own `pub(in crate::refs)` internals.
pub(crate) fn ref_log_tail_status(
    layout: &RepositoryLayout,
) -> Result<(usize, usize, Option<String>)> {
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )?
    else {
        return Ok((0, 0, None));
    };
    let discovery = container::decode_ref_container_records(&bytes)?;
    if let Some(tail) = container::ref_log_container_tail(&bytes, &discovery) {
        return Ok((tail.len, tail.offset, None));
    }
    let interior_damage =
        discovery
            .record_outcomes
            .iter()
            .find_map(|outcome| match &outcome.status {
                container::RefContainerRecordStatus::Failed { message, .. } => {
                    Some(message.clone())
                }
                container::RefContainerRecordStatus::Evaluated => None,
            });
    Ok((0, 0, interior_damage))
}

fn incomplete_publication_refusal() -> PrikkError {
    // RFC 132 part 2: an incomplete publication is a caller precondition, not a lock -- nothing is
    // held and no other writer is racing this one; the fix is running verify/doctor and retrying
    // with the right signer, not waiting.
    PrikkError::Precondition(
        "repository mutation is blocked by incomplete ref publication; run verify/doctor and use signer-backed seal retry"
            .to_string(),
    )
}

/// RFC 165 R5 (§9.2): found live against the architect's own `rfc165_ref_log_tail_shapes_probe.sh`
/// (the `flip-last-body` shape) -- `discovery.has_damage()` was routed through
/// `incomplete_publication_refusal()`, the same text a genuine pointer lead gets ("... use
/// signer-backed seal retry"), which is wrong here: a complete, fully-written record whose checksum
/// or envelope fails is damage, not an interrupted publication, and no seal retry resolves it. The
/// handoff's own classification table names the real way out: "a complete damaged record, anywhere:
/// damage ... The way out is a copy." This names the first damaged offset `has_damage()` found, not
/// just a generic refusal.
fn ref_log_damage_refusal(discovery: &container::RefContainerReplay) -> PrikkError {
    let last_index = discovery.record_outcomes.len().checked_sub(1);
    let detail = discovery
        .record_outcomes
        .iter()
        .enumerate()
        .find_map(|(index, outcome)| match &outcome.status {
            container::RefContainerRecordStatus::Failed {
                message,
                never_a_tail,
                ..
            } if *never_a_tail || Some(index) != last_index => {
                Some(format!(" at byte offset {}: {message}", outcome.offset))
            }
            _ => None,
        })
        .unwrap_or_default();
    PrikkError::Integrity(format!(
        "the ref log has a damaged record{detail}; this is not an incomplete publication and no seal \
         retry resolves it -- the way out is a copy of a sound repository, not a repair"
    ))
}

/// RFC 165 R2: `ensure_no_incomplete_publication`'s own pre-R2 implementation, kept as a test oracle
/// (handoff §2: "equivalence with the old function, kept as a test oracle"). Not called from any
/// production path -- `ensure_no_incomplete_publication` itself no longer calls `verify_refs` at all.
/// RFC 165 Addendum 1 §2: its sole consumer is the equivalence sweep in
/// `refs::tests::no_refs_times_log_precondition`, which lives under `refs::tests`
/// (DC-71-gated to `target_os = "linux"`, real repository mutation) -- gated to match it exactly,
/// the same reasoning `container::append_ref_container_record`'s own re-export already carries,
/// rather than the broader `#[cfg(test)]` this had before: that left it (and `RefVerification::
/// has_item_failure`, its own sole remaining caller) dead code on Windows and macOS, where
/// `refs::tests` does not compile at all.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn ensure_no_incomplete_publication_via_verify_refs_for_test(
    layout: &RepositoryLayout,
) -> Result<()> {
    let verification = verify_refs(layout)?;
    if verification.publication_issues.is_empty()
        && !verification.has_item_failure()
        && !evidence::has_incomplete_active_cleanup(layout, None)?
    {
        return Ok(());
    }
    Err(incomplete_publication_refusal())
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

    /// Read the current RefState object ID for a ref name. A reader: never refuses on the pointer
    /// index's own trailing-partial tail (rule 1), the same as every other reader of a file RFC 163
    /// guards on the write side.
    pub fn read_current_ref_state_id(&self, ref_name: &str) -> Result<Option<ObjectId>> {
        let key = crate::foundation::layout::ref_name_key_bytes(ref_name);
        let (entry, _tail) = pointer_index::lookup_ref_pointer(&self.layout, key)?;
        let Some(entry) = entry else {
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

    /// RFC 164 Rule D: the pointer index's own tail/damage check, standalone from the CAS comparison
    /// `ensure_current_matches` also makes. **Call this at the very start of a publishing command**
    /// (`seal`, `branch create`, `tag create`, `merge`), before any content-object write of its own
    /// (a queued patch's objects, a sealed Block) -- not only inside `publish_locked`'s own later,
    /// redundant-but-harmless re-check, which exists only to catch a lock-discipline regression, not
    /// as the primary guard. `lookup_ref_pointer` itself already refuses on interior damage
    /// (`has_item_failure()`); `require_no_unclean_tail` adds the tail check on top, the same pair
    /// every other guarded-file writer performs under its own replay.
    pub fn ensure_pointer_index_has_no_tail(&self, ref_name: &str) -> Result<()> {
        let key = crate::foundation::layout::ref_name_key_bytes(ref_name);
        let (_entry, tail) = pointer_index::lookup_ref_pointer(&self.layout, key)?;
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the ref pointer index",
            tail.trailing_partial_bytes,
            tail.tail_offset,
            "run `prikk doctor --repair-pointer-index-tail`, then retry",
        )
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
    /// RFC 163 §2: also the pointer index's own write-side tail guard. `publish_locked`'s `Ready`
    /// branch calls this immediately before its own `append_ref_pointer_entry`, so the refusal below
    /// runs at the one whole read that branch already performs -- no second read is added.
    fn ensure_current_matches(&self, ref_name: &str, expected: Option<ObjectId>) -> Result<()> {
        let key = crate::foundation::layout::ref_name_key_bytes(ref_name);
        let (entry, tail) = pointer_index::lookup_ref_pointer(&self.layout, key)?;
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the ref pointer index",
            tail.trailing_partial_bytes,
            tail.tail_offset,
            "run `prikk doctor --repair-pointer-index-tail`, then retry",
        )?;
        let current = match entry {
            Some(entry) if entry.ref_name == ref_name => Some(entry.ref_state_id),
            Some(entry) => {
                return Err(PrikkError::Integrity(format!(
                    "ref pointer name mismatch: expected {ref_name}, got {}",
                    entry.ref_name
                )));
            }
            None => None,
        };
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

/// The branch a repository with no current-branch pointer is on: every repository initialized before
/// RFC 151, and the unborn default of a fresh one.
const UNBORN_DEFAULT_BRANCH: &str = "heads/main";

/// How the pointer file is named in a refusal -- repository-relative, the path a user edits.
const CURRENT_BRANCH_DISPLAY: &str = ".prikk/current-branch";

/// RFC 151 §2.1: the branch `--ref` defaults to, read from `.prikk/current-branch`.
///
/// **A default, never an authority.** Local, mutable and unsigned, so nothing that decides trust
/// reads it: not `verify`, not trust or signing, not `bundle` or `sync`, not any object. Only the
/// CLI's default resolution and `doctor` call this, and a test over both production trees holds
/// that.
///
/// - no file: `heads/main` (a repository created before RFC 151);
/// - not exactly one valid local branch ref name followed by a newline: `Precondition` naming the
///   file;
/// - a branch that does not exist or is closed: `Precondition` naming the branch and the two ways
///   out. `heads/main` not existing yet is the unborn default of a fresh repository, not a refusal.
///
/// Reads only; it never writes the file, so a pre-RFC repository stays exactly as it was.
pub fn current_branch(layout: &RepositoryLayout) -> Result<String> {
    let relative = layout.repository_relative(&layout.current_branch_path())?;
    let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )?
    else {
        return Ok(UNBORN_DEFAULT_BRANCH.to_string());
    };
    let malformed = |detail: String| {
        PrikkError::Precondition(format!(
            "{CURRENT_BRANCH_DISPLAY} is malformed ({detail}); it must hold one local branch ref \
             name followed by a newline, such as heads/main"
        ))
    };
    let text = std::str::from_utf8(&bytes).map_err(|err| malformed(format!("not UTF-8: {err}")))?;
    let name = text
        .strip_suffix('\n')
        .ok_or_else(|| malformed("no trailing newline".to_string()))?;
    let canonical = validate_local_branch_ref(name).map_err(|err| malformed(err.to_string()))?;
    let routes = "run `prikk branch switch heads/<name>` to a branch that exists and is open, or \
                  `prikk branch create` it";
    let Some(ref_state_id) = RefStore::new(layout.clone()).read_current_ref_state_id(&canonical)?
    else {
        if canonical == UNBORN_DEFAULT_BRANCH {
            return Ok(canonical);
        }
        return Err(PrikkError::Precondition(format!(
            "{CURRENT_BRANCH_DISPLAY} names {canonical}, which does not exist; {routes}"
        )));
    };
    let envelope = FileObjectStore::new(layout.clone())
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "missing RefState object for {canonical}: {ref_state_id}"
            ))
        })?;
    let state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if state.closed {
        return Err(PrikkError::Precondition(format!(
            "{CURRENT_BRANCH_DISPLAY} names {canonical}, which is closed; {routes}"
        )));
    }
    Ok(canonical)
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
