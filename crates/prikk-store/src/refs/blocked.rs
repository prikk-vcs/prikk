//! 0.59.0 step G2a: **which state the write-path precondition refused** (a read-only classification, for the sentence that names the way out).
//!
//! The precondition ([`super::ensure_publication_precondition_with_replay`]) answers one `IncompletePublication` for several genuine
//! states. A caller that knows the state can name the command that clears it (or say plainly that none does); the upper layer builds those
//! sentences (they name `SOUND_COPY` and `ref complete`), so this module only **classifies**, in the order the precondition checks:
//! a damaged pointer index, a log record carrying a time this version never writes, a pointer naming a RefState the object index lacks,
//! candidate debris. A lead (a pointer that disagrees with the log) and a settled queue are classified by the callers that already own those
//! questions; anything this answers `None` to is the last arm.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RefUpdatePayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReadSnapshot;

/// The sound-copy sequence, in full: the way out of every state no command repairs in place. Defined here (a leaf both `way_out` and `ref_completion` read)
/// so that `ref_completion` names it without an edge into `way_out`, which would close a cycle through `verify`. `way_out::SOUND_COPY` is this value.
pub(crate) const SOUND_COPY: &str = "restore a sound copy of the sealed history: `prikk setup <new-dir>`, then, in \
<new-dir>, `prikk archive verify <backup>` and `prikk archive import <backup> --adopt` (or `prikk bundle \
import --input <bundle>` for one branch); the key that sealed the history must be adopted in <new-dir> \
(`archive verify` reports it). Only sealed history travels in a backup: a commit still waiting for a \
`prikk seal` is made again there";

/// The state the precondition refused, when this module can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublicationBlock {
    /// A pointer-index record failed: `doctor --rebuild-pointer-index` re-derives the index from the ref log.
    PointerIndexDamaged,
    /// A ref-log record carries a non-zero `created_at`: no writer of this version produces one.
    LegacyTimestamp,
    /// The ref log is ahead of the pointer index for `ref_name` (the pointer is an earlier record of that ref's log, or absent while the log has
    /// records): the one divergence `doctor --rebuild-pointer-index` re-derives and accepts.
    LogLeadsPointer {
        /// The ref.
        ref_name: String,
    },
    /// The newest pointer of `ref_name` names a RefState the object index does not hold.
    MissingRefState {
        /// The ref.
        ref_name: String,
        /// The RefState it names.
        ref_state_id: ObjectId,
    },
}

/// Classify the block, `None` when it is none of the four. Read-only; whole reads of the pointer index, the ref log and the object index
/// (the error path of a refusal, not a hot path).
///
/// # Errors
///
/// A container that cannot be read at all.
pub(crate) fn classify_publication_block(
    layout: &RepositoryLayout,
) -> Result<Option<PublicationBlock>> {
    // The same whole-container read the precondition and `mismatched_lead_candidate` make for the same reason (the error path of a refusal, once).
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let pointer_replay = super::replay_pointer_index(layout)?;
    if pointer_replay.has_item_failure() {
        return Ok(Some(PublicationBlock::PointerIndexDamaged));
    }
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let mut chains: std::collections::BTreeMap<[u8; 32], (String, Vec<ObjectId>)> =
        std::collections::BTreeMap::new();
    if let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )? {
        let discovery = super::decode_ref_container_records(&bytes)?;
        if !discovery.has_damage() {
            for record in &discovery.records {
                let update =
                    RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
                if update.created_at != 0 {
                    return Ok(Some(PublicationBlock::LegacyTimestamp));
                }
                chains
                    .entry(record.ref_name_key)
                    .or_insert_with(|| (update.ref_name.clone(), Vec::new()))
                    .1
                    .push(update.new_ref_state_id);
            }
        }
    }
    let newest_pointer: std::collections::BTreeMap<[u8; 32], ObjectId> = pointer_replay
        .entries
        .iter()
        .map(|entry| (entry.ref_name_key, entry.ref_state_id))
        .collect();
    for (key, (ref_name, chain)) in &chains {
        let Some(tip) = chain.last() else { continue };
        let leads = match newest_pointer.get(key) {
            None => true,
            Some(pointer) => pointer != tip && chain.contains(pointer),
        };
        if leads {
            return Ok(Some(PublicationBlock::LogLeadsPointer {
                ref_name: ref_name.clone(),
            }));
        }
    }
    let objects = ObjectReadSnapshot::open(layout)?;
    let mut newest: std::collections::BTreeMap<[u8; 32], (String, ObjectId)> =
        std::collections::BTreeMap::new();
    for entry in &pointer_replay.entries {
        newest.insert(
            entry.ref_name_key,
            (entry.ref_name.clone(), entry.ref_state_id),
        );
    }
    for (ref_name, ref_state_id) in newest.into_values() {
        if !objects.contains_object(ObjectType::RefState, ref_state_id) {
            return Ok(Some(PublicationBlock::MissingRefState {
                ref_name,
                ref_state_id,
            }));
        }
    }
    Ok(None)
}

/// **The one sentence for a RefState the repository does not hold** (0.59.0 step G2c): every reader that follows a ref to its RefState and finds
/// it absent says the same thing, and says plainly what no command can do. The sound-copy sequence is the way out (`verify`'s own way-out line
/// for the state says the same). `ref_name` is the ref the pointer belongs to, when the site knows it.
#[must_use]
pub fn missing_ref_state_sentence(ref_name: Option<&str>, ref_state_id: &ObjectId) -> String {
    match ref_name {
        Some(name) => format!(
            "{name} names RefState {ref_state_id}, which this repository does not hold; no command repairs a missing RefState in place: {SOUND_COPY}"
        ),
        None => format!(
            "RefState {ref_state_id} is not held by this repository; no command repairs a missing RefState in place: {SOUND_COPY}"
        ),
    }
}

/// [`missing_ref_state_sentence`] as the `Integrity` error every store reader returns for it.
#[must_use]
pub(crate) fn missing_ref_state(ref_name: Option<&str>, ref_state_id: ObjectId) -> PrikkError {
    PrikkError::Integrity(missing_ref_state_sentence(ref_name, &ref_state_id))
}

/// The same for a **received** ref (a pointer to what another repository sent, not local history): the way out is `received remove` and receiving it
/// again, which is the sentence `verify` prints for a failed received ref.
#[must_use]
pub(crate) fn missing_received_ref_state(ref_name: &str, ref_state_id: ObjectId) -> PrikkError {
    PrikkError::Integrity(format!(
        "received ref {ref_name} names RefState {ref_state_id}, which this repository does not hold; run `prikk received remove {ref_name} \
         --plan-only`, then `prikk received remove {ref_name}`, and receive it again if you still have it"
    ))
}

/// `PREFPTR1`, a big-endian u64 length, the ref name, the 32-byte RefState id: the file RFC 102 Stage 4 retired (a ref pointer candidate in
/// `refs/tmp`). `None` for any other bytes.
#[must_use]
pub(crate) fn decode_legacy_pointer_candidate(bytes: &[u8]) -> Option<(String, ObjectId)> {
    let rest = bytes.strip_prefix(b"PREFPTR1".as_slice())?;
    let len = usize::try_from(u64::from_be_bytes(rest.get(..8)?.try_into().ok()?)).ok()?;
    let name_end = 8_usize.checked_add(len)?;
    let name = String::from_utf8(rest.get(8..name_end)?.to_vec()).ok()?;
    let id: [u8; 32] = rest
        .get(name_end..name_end.checked_add(32)?)?
        .try_into()
        .ok()?;
    if rest.len() != name_end + 32 {
        return None;
    }
    Some((name, ObjectId::from_bytes(id)))
}

/// **A candidate a publisher could still be using** (0.59.0 step G2d): the only reason the write-path precondition has left to refuse on a file in
/// `refs/tmp`. No writer of this version makes a candidate, so a file there is debris -- unless it decodes as a legacy pointer candidate for a ref
/// whose `RefLock` another running process holds, which is in flight. `None` for everything else (the writers go on; `verify` keeps its warning, and
/// `prikk doctor --remove-ref-candidates` clears it). A non-file entry is damage and refuses, as it always did.
///
/// # Errors
///
/// A non-file entry in `refs/tmp`; an unreadable directory or file.
pub(crate) fn candidate_in_flight(layout: &RepositoryLayout) -> Result<Option<PrikkError>> {
    let dir = layout.refs_dir().join("tmp");
    let relative = layout.repository_relative(&dir)?;
    let root = layout.repository_mutation_root();
    let Some(entries) = crate::foundation::fsutil::list_directory_tolerating_absence(
        root,
        &dir,
        &relative,
        "the ref candidate directory",
    )?
    else {
        return Ok(None);
    };
    for entry in entries {
        if entry.kind != crate::foundation::fsutil::EntryKind::Regular {
            return Err(PrikkError::Integrity(format!(
                "unexpected non-file in ref candidate directory: refs/tmp/{}; no command removes it",
                entry.name.to_string_lossy()
            )));
        }
        let Some(bytes) =
            crate::foundation::fsutil::read_file_if_exists(root, &relative.join(&entry.name))?
        else {
            continue;
        };
        if let Some((ref_name, _)) = decode_legacy_pointer_candidate(&bytes) {
            if super::in_flight::ref_publisher_running(layout, &ref_name)? {
                return Ok(Some(PrikkError::Precondition(format!(
                    "{}; run this command again when it finishes",
                    super::in_flight::in_flight_sentence(&ref_name, None)
                ))));
            }
        }
    }
    Ok(None)
}
