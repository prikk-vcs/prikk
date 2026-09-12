//! Ref pointer/log scanning and structural chain validation (RFC 102 Stage 4: rewritten onto the
//! shared ref-pointer-index and ref-log containers; see `refs/container.rs` and
//! `refs/pointer_index.rs` for the storage this now reads).

use std::collections::BTreeMap;
use std::path::PathBuf;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    ObjectEnvelope, ObjectId, RefKind, RefStatePayload, RefUpdatePayload, TagPayload,
};

use crate::foundation::layout::{ContainerSlot, RepositoryLayout, ref_name_key_bytes};
use crate::object_store::ObjectReader;
use crate::refs::container::{
    RefContainerRecordStatus, RefLogRecordStatus, RefLogReplay, decode_ref_container_records,
    replay_ref_subsequence,
};
use crate::refs::pointer_index::{
    PointerIndexEntry, PointerIndexRecordStatus, replay_pointer_index,
};
use prikk_object::ObjectType;

#[derive(Debug, Clone)]
pub(super) struct PointerState {
    pub(super) id: ObjectId,
    pub(super) payload: RefStatePayload,
}

#[derive(Debug, Clone)]
pub(super) struct LogState {
    pub(super) tip: Option<ObjectId>,
    pub(super) previous_tip: Option<ObjectId>,
    pub(super) record_count: usize,
    pub(super) trailing_partial_bytes: usize,
    pub(super) has_legacy_timestamp: bool,
}

pub(super) struct RefLogEnvelope {
    pub(super) ref_name: String,
    pub(super) sequence: u64,
    pub(super) envelope: ObjectEnvelope,
}

/// Outcome of attempting to read one pointer or log entry (DC-95 Stage 2 Level 2). `path` is a
/// display-only locator (the owning container's own path, plus the specific record's byte offset
/// where one exists) -- not a real per-ref filesystem path, the same repurposing Stage 3 already made
/// for `ObjectItemOutcome::path`, since a container holds many refs' records, not one file per ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefFileStatus {
    /// The entry was read and validated successfully.
    Evaluated {
        /// The ref name it resolved to.
        ref_name: String,
    },
    /// Some check for this specific entry failed.
    Failed {
        /// The error the check raised.
        message: String,
    },
}

/// One pointer or log entry's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefFileOutcome {
    /// A display-only locator -- see the type's own doc.
    pub path: PathBuf,
    /// How this entry's own read/validation resolved.
    pub status: RefFileStatus,
}

fn pointer_locator(layout: &RepositoryLayout, offset: usize) -> PathBuf {
    // Display-only. Step 1 (RFC 102 Stage 6, design-v1.md §15.6) never resolves anywhere but `A` --
    // hardcoded here rather than resolver-routed for the same reason `log_locator` hardcodes `A` for
    // the (never-compacting) ref log container.
    layout
        .ref_pointer_index_slot_path(ContainerSlot::A)
        .join(format!("#{offset}"))
}

fn log_locator(layout: &RepositoryLayout, offset: usize) -> PathBuf {
    layout
        .ref_log_container_slot_path(ContainerSlot::A)
        .join(format!("#{offset}"))
}

/// Read every published ref pointer: the ref-pointer index's own last-entry-per-`ref_name_key` view
/// (RFC 102 Stage 4), each cross-validated against its named RefState object exactly as before.
///
/// Returns, alongside the display-only `outcomes`, a `ref_name_key -> failure message` map for
/// internal cross-referencing. `RefFileOutcome`'s own `path` is a display locator (a container
/// offset now, not a per-ref file path — see the type's doc), so it can no longer be used to
/// attribute a failed entry back to the ref name that owns it the way a per-ref filename could;
/// `ref_name_key_bytes(ref_name)` is the only key both sides can compute independently.
#[allow(clippy::type_complexity)]
pub(super) fn read_pointers(
    layout: &RepositoryLayout,
    objects: &impl ObjectReader,
) -> Result<(
    BTreeMap<String, PointerState>,
    BTreeMap<[u8; 32], String>,
    Vec<RefFileOutcome>,
)> {
    let replay = replay_pointer_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "ref pointer index has a damaged entry; run doctor before verify can classify \
             publication state"
                .to_string(),
        ));
    }
    // "Last entry wins" (Step 0 §13.4): iterate in append order, keep overwriting -- the final
    // value per `ref_name_key` after the loop is the same one `lookup_ref_pointer`'s reverse scan
    // would find, and each entry also carries the offset it was found at, for the locator.
    let mut latest: BTreeMap<[u8; 32], (usize, PointerIndexEntry)> = BTreeMap::new();
    for outcome in replay.record_outcomes.iter().zip(replay.entries) {
        let (record_outcome, entry) = outcome;
        if !matches!(record_outcome.status, PointerIndexRecordStatus::Evaluated) {
            continue;
        }
        latest.insert(entry.ref_name_key, (record_outcome.offset, entry));
    }

    let mut pointers = BTreeMap::new();
    let mut failures_by_key = BTreeMap::new();
    let mut outcomes = Vec::new();
    for (key, (offset, entry)) in latest {
        let locator = pointer_locator(layout, offset);
        match read_one_pointer_entry(objects, &entry) {
            Ok((ref_name, state)) => {
                pointers.insert(ref_name.clone(), state);
                outcomes.push(RefFileOutcome {
                    path: locator,
                    status: RefFileStatus::Evaluated { ref_name },
                });
            }
            Err(err) => {
                failures_by_key.insert(key, err.to_string());
                outcomes.push(RefFileOutcome {
                    path: locator,
                    status: RefFileStatus::Failed {
                        message: err.to_string(),
                    },
                });
            }
        }
    }
    Ok((pointers, failures_by_key, outcomes))
}

fn read_one_pointer_entry(
    objects: &impl ObjectReader,
    entry: &PointerIndexEntry,
) -> Result<(String, PointerState)> {
    // The entry's own internal coherence: its claimed `ref_name_key` must actually be
    // `sha256(ref_name)` -- the same defense-in-depth `verify_object_file` already applies by
    // recomputing an object's own id from its decoded bytes.
    if ref_name_key_bytes(&entry.ref_name) != entry.ref_name_key {
        return Err(PrikkError::Integrity(format!(
            "pointer index entry ref_name_key does not match sha256({})",
            entry.ref_name
        )));
    }
    let payload = verified_ref_state_payload(objects, entry.ref_state_id)?;
    if payload.ref_name != entry.ref_name {
        return Err(PrikkError::Integrity(format!(
            "RefState {} name differs from pointer ref {}",
            entry.ref_state_id, entry.ref_name
        )));
    }
    ensure_ref_target_valid(
        objects,
        payload.kind,
        payload.target_object_id,
        entry.ref_state_id,
    )?;
    Ok((
        entry.ref_name.clone(),
        PointerState {
            id: entry.ref_state_id,
            payload,
        },
    ))
}

/// Read every ref's own log subsequence from the shared log container, grouped by `ref_name_key`
/// (Step 0 §13.1/§13.2). One full-container decode discovers which keys exist at all; each key's own
/// subsequence is then replayed through `container::replay_ref_subsequence` -- the same function
/// `RefStore::replay_log` itself uses -- so the ref-scoped `trailing_partial_bytes` attribution
/// (design-v1.md §13.6) is computed exactly once, in one place, not re-derived here.
#[allow(clippy::type_complexity)]
pub(super) fn read_logs(
    layout: &RepositoryLayout,
    objects: &impl ObjectReader,
    _pointers: &BTreeMap<String, PointerState>,
) -> Result<(
    BTreeMap<String, LogState>,
    usize,
    Vec<RefLogEnvelope>,
    BTreeMap<[u8; 32], String>,
    Vec<RefFileOutcome>,
)> {
    let relative =
        layout.repository_relative(&layout.ref_log_container_slot_path(ContainerSlot::A))?;
    let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )?
    else {
        return Ok((BTreeMap::new(), 0, Vec::new(), BTreeMap::new(), Vec::new()));
    };
    let discovery = decode_ref_container_records(&bytes)?;
    let mut keys: std::collections::BTreeSet<[u8; 32]> = discovery
        .records
        .iter()
        .map(|record| record.ref_name_key)
        .collect();
    keys.extend(
        discovery
            .record_outcomes
            .iter()
            .filter_map(|outcome| match &outcome.status {
                RefContainerRecordStatus::Failed {
                    claimed_ref_name_key: Some(key),
                    ..
                } => Some(*key),
                _ => None,
            }),
    );

    let mut logs = BTreeMap::new();
    let mut total = 0_usize;
    let mut envelopes = Vec::new();
    let mut failures_by_key = BTreeMap::new();
    let mut outcomes = Vec::new();
    for key in keys {
        let replay = replay_ref_subsequence(layout, key)?;
        let first_offset = replay
            .record_outcomes
            .first()
            .map_or(0, |outcome| outcome.offset);
        // A key with any damaged record is reported as one failed outcome for that ref -- matching
        // today's file-granularity semantics (a ref's own log failing is one item-level defect, not
        // one per damaged record within it).
        if replay.has_item_failure() {
            let joined = replay
                .record_outcomes
                .iter()
                .filter_map(|outcome| match &outcome.status {
                    RefLogRecordStatus::Failed { message } => {
                        Some(format!("offset {}: {message}", outcome.offset))
                    }
                    RefLogRecordStatus::Evaluated => None,
                })
                .collect::<Vec<_>>()
                .join("; ");
            let message = format!("ref log has damaged record(s): {joined}");
            failures_by_key.insert(key, message.clone());
            outcomes.push(RefFileOutcome {
                path: log_locator(layout, first_offset),
                status: RefFileStatus::Failed { message },
            });
            continue;
        }
        match validate_log_replay(objects, key, &replay) {
            Ok(Some((ref_name, state, record_envelopes))) => {
                total = match total.checked_add(record_envelopes.len()) {
                    Some(value) => value,
                    None => {
                        return Err(PrikkError::Integrity("ref-log count overflow".to_string()));
                    }
                };
                envelopes.extend(record_envelopes);
                logs.insert(ref_name.clone(), state);
                outcomes.push(RefFileOutcome {
                    path: log_locator(layout, first_offset),
                    status: RefFileStatus::Evaluated { ref_name },
                });
            }
            Ok(None) => {}
            Err(err) => {
                failures_by_key.insert(key, err.to_string());
                outcomes.push(RefFileOutcome {
                    path: log_locator(layout, first_offset),
                    status: RefFileStatus::Failed {
                        message: err.to_string(),
                    },
                });
            }
        }
    }
    Ok((logs, total, envelopes, failures_by_key, outcomes))
}

#[allow(clippy::type_complexity)]
fn validate_log_replay(
    objects: &impl ObjectReader,
    ref_name_key: [u8; 32],
    replay: &RefLogReplay,
) -> Result<Option<(String, LogState, Vec<RefLogEnvelope>)>> {
    if replay.records.is_empty() {
        if replay.trailing_partial_bytes == 0 {
            return Ok(None);
        }
        return Err(PrikkError::Integrity(
            "ref log has a trailing partial record with no sound records of its own".to_string(),
        ));
    }
    let mut previous = None;
    let mut previous_tip = None;
    let mut ref_name = None;
    let mut has_legacy_timestamp = false;
    let mut record_envelopes = Vec::with_capacity(replay.records.len());
    for (index, record) in replay.records.iter().enumerate() {
        let update = RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
        // Coherence: this record's own header claimed `ref_name_key`; its decoded payload's own
        // `ref_name` must hash to the same key, the log-side equivalent of `read_one_pointer_entry`'s
        // own check.
        if crate::foundation::layout::ref_name_key_bytes(&update.ref_name) != ref_name_key {
            return Err(PrikkError::Integrity(
                "ref container record header ref_name_key does not match its own envelope"
                    .to_string(),
            ));
        }
        let expected_seq = u64::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| PrikkError::Integrity("ref-log sequence overflow".to_string()))?;
        if ref_name
            .as_ref()
            .is_some_and(|name| name != &update.ref_name)
            || update.old_ref_state_id != previous
            || update.update_seq != expected_seq
        {
            return Err(PrikkError::Integrity(format!(
                "ref-log chain or sequence diverges for {}",
                update.ref_name
            )));
        }
        verify_update(objects, &update)?;
        has_legacy_timestamp |= update.created_at != 0;
        ref_name.get_or_insert_with(|| update.ref_name.clone());
        previous_tip = previous;
        previous = Some(update.new_ref_state_id);
        record_envelopes.push(RefLogEnvelope {
            ref_name: update.ref_name.clone(),
            sequence: update.update_seq,
            envelope: record.envelope.clone(),
        });
    }
    let name = ref_name
        .ok_or_else(|| PrikkError::Integrity("non-empty ref log has no identity".to_string()))?;
    Ok(Some((
        name,
        LogState {
            tip: previous,
            previous_tip,
            record_count: replay.records.len(),
            trailing_partial_bytes: replay.trailing_partial_bytes,
            has_legacy_timestamp,
        },
        record_envelopes,
    )))
}

fn verify_update(objects: &impl ObjectReader, update: &RefUpdatePayload) -> Result<()> {
    let state = verified_ref_state_payload(objects, update.new_ref_state_id)?;
    if state.ref_name != update.ref_name
        || state.previous_ref_state_id != update.old_ref_state_id
        || state.target_object_id != update.new_target_object_id
        || state.update_seq != update.update_seq
    {
        return Err(PrikkError::Integrity(format!(
            "RefState disagrees with RefUpdate for {}",
            update.ref_name
        )));
    }
    ensure_ref_target_valid(
        objects,
        state.kind,
        update.new_target_object_id,
        update.new_ref_state_id,
    )
}

fn verified_ref_state_payload(
    objects: &impl ObjectReader,
    ref_state_id: ObjectId,
) -> Result<RefStatePayload> {
    let envelope = objects
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing RefState object: {ref_state_id}")))?;
    if envelope.signatures.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "RefState {ref_state_id} is unsigned"
        )));
    }
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
}

/// Kind-aware ref-target validation, shared by both the pointer scan (`read_pointers`) and the
/// ref-log scan (`verify_update`), which must agree: `publication.rs`'s coherence check requires
/// `RefUpdatePayload.new_target_object_id == RefStatePayload.target_object_id`, so a log record's
/// target and its pointer's target are the identical value for the identical kind. `RefKind::Branch`
/// must target a `Block` directly; `RefKind::Tag` must target a `Tag` object whose own
/// `target_block_id` is a `Block` — the two-hop indirection §6.6 requires.
///
/// **Not `refs::resolve_ref_tip_block`, on purpose** (ref-tip-resolver-consolidation handoff): this
/// function *validates* and returns `()` — for a `Branch` it confirms the target block actually
/// exists, which a resolver never checks — and its errors carry `owner`, the ref object being
/// verified, which a resolver never has. That `owner` is load-bearing: *"ref object {owner} targets
/// missing tag {target_object_id}"* is the exact message the DC-78 tag-gap ruling turned on (a
/// bundle must ship the Tag object, or `verify` produces this). Folding this into the resolver would
/// either drop that context or thread a message-customisation parameter through code three other
/// callers don't need it in.
pub(crate) fn ensure_ref_target_valid(
    objects: &impl ObjectReader,
    kind: RefKind,
    target_object_id: ObjectId,
    owner: ObjectId,
) -> Result<()> {
    match kind {
        RefKind::Branch => ensure_block_exists(objects, target_object_id, owner),
        RefKind::Tag => {
            let tag_envelope = objects
                .read_typed(target_object_id, ObjectType::Tag)?
                .ok_or_else(|| {
                    PrikkError::Integrity(format!(
                        "ref object {owner} targets missing tag {target_object_id}"
                    ))
                })?;
            let tag_payload = TagPayload::decode_canonical(&tag_envelope.canonical_payload)?;
            ensure_block_exists(objects, tag_payload.target_block_id, owner)
        }
    }
}

fn ensure_block_exists(
    objects: &impl ObjectReader,
    block_id: ObjectId,
    owner: ObjectId,
) -> Result<()> {
    if objects.read_typed(block_id, ObjectType::Block)?.is_some() {
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "ref object {owner} targets missing block {block_id}"
    )))
}
