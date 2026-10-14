//! RFC 155 implementation handoff, Part V: a streaming, read-only verifier for `PREPO001` archives
//! (§9.4). Checks structure (the trailing manifest, found by seeking to the fixed end trailer, never
//! scanning), reference closure (two passes over the object sections, with an id set -- memory
//! bounded by object *count*, never by archive bytes, §9.2), and every signature against the
//! archive's own carried author/maintainer material, held in memory (small key material, not
//! content, §9.4). Reports *"internally consistent"*, never *"trusted"* -- trust is the receiver's
//! own adopted set, checked only after import, never against this archive's carried record.
//!
//! **Writes nothing, on any exit path.** This module never opens a file for writing and never
//! creates a scratch directory; every check is a read against the caller's own `source`.
//!
//! **A section's kind is never a path.** [`ArchiveSectionKind::from_wire_code`] maps a numeric code
//! to one of a fixed, closed set of variants; nothing here ever turns a carried byte into a
//! `Path`/`PathBuf`. An unknown, duplicated, or missing section kind is refused by name, before a
//! single body byte is read -- `validate_manifest_shape` runs immediately after the manifest itself
//! decodes, strictly before any section body is touched.
//!
//! **Why this reuses, rather than re-derives, three existing checks**: `verify_trusted_publication_
//! envelope` (`trust.rs`) and `verify_author_signatures_with` (`author_key_index.rs`) are the exact
//! functions a *local repository's own* `verify_repository` already calls -- the only thing that
//! changes here is which `MaintainerTrustPolicy`/author-material lookup backs them (the archive's own
//! carried sections, decoded in memory, never the local repository's). Reusing them unchanged is what
//! makes this "not a new cryptographic mechanism" (§9.4's own framing), and is also why Part I-A's own
//! stage 0 can reuse this module's own `verify_archive` directly, not a copy.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use prikk_error::{PrikkError, Result};
use prikk_hash::IncrementalSha256;
use prikk_object::{BlockPayload, ObjectEnvelope, ObjectId, ObjectType, SignerRole};

use crate::author::author_key_index::{
    AuthorKeyEntry, decode_author_key_records, verify_author_signatures_with,
};
use crate::bundle::DEFAULT_BUNDLE_MAX_OBJECT_BYTES;
use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::container::{ContainerRecordStatus, decode_frames_with};
use crate::foundation::frame_source::FrameSource;
use crate::foundation::layout::persisted_object_types;
use crate::object_store::ObjectReader;
use crate::received::received_index::{
    ReceivedIndexEntry, decode_received_index_entries_for_resolver, reduce_received_index_entries,
};
use crate::refs::verify::{PointerState, classify_ref_items, read_logs, read_one_pointer_entry};
use crate::refs::{
    DecodedRefLog, PointerIndexEntry, decode_pointer_index_entries_for_resolver,
    decode_ref_container_records, reduce_pointer_index_entries,
};
use crate::trust::{
    AdoptedMaintainerKey, MaintainerTrustPolicy, verify_trusted_publication_envelope,
};
use crate::trust_index::{decode_trust_key_records, decode_trust_policy_records};
use crate::verify::reachability::expand_frontier;
use crate::verify::verify_received_ref_state;

use super::{
    ARCHIVE_MAGIC, ArchiveSectionKind, END_TRAILER_MAGIC, MANIFEST_MAGIC, ManifestEntry,
    SECTION_MAGIC,
};

/// Sanity bound on the manifest's own declared section count, checked before a `Vec` of that
/// capacity is ever requested (DC-86 discipline) -- real archives always declare exactly 13; this
/// guards an adversarial or corrupt manifest from forcing a large allocation on the strength of one
/// `u64` nobody has checked yet.
const MAX_MANIFEST_ENTRIES: u64 = 10_000;
/// Sanity bound on the manifest body's own declared byte length, checked before it is read into
/// memory -- the manifest is bounded by section *count*, never by archive bytes (§9.2), so a real
/// manifest is a few hundred bytes; this is generous headroom, not a working estimate.
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
/// Sanity bound on the three small, carried key-material sections (maintainer keys, the trust
/// policy, author keys), checked before any of them is decoded wholly into memory -- they are
/// bounded by key/policy count, never by repository content size, so a real one is at most a few
/// hundred KiB even for a repository with thousands of distinct keys.
const MAX_SMALL_SECTION_BYTES: u64 = 64 * 1024 * 1024;
/// Magic(8) + manifest_offset(8) + manifest_length(8) + manifest_checksum(32). V2's own fix: the
/// checksum field, so the manifest itself is checked before any of its fields are trusted.
const END_TRAILER_LEN_USIZE: usize = 56;
const SECTION_HEADER_LEN: usize = 18;

/// One maintainer key this archive carries -- listed, never adopted (§9.1/§9.4/§9.8): *"carried, not
/// adopted"*.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CarriedMaintainerKeySummary {
    /// The adopted maintainer key id, as carried.
    pub key_id: String,
    /// The key's own public key, lowercase hex.
    pub public_key_hex: String,
}

/// One `Block`'s own missing, reachability-exempt reference (0.52.0 step 0, Part C1 round 2, finding
/// C1-4): structured, not a pre-formatted string, so a caller can print it with the store's own
/// [`crate::verify`] warning shape instead of inventing a second wording. Harmless by construction --
/// see [`ArchiveVerifyReport::unreferenced_remnants`]'s own doc for why.
/// Field names match the store's own `crate::verify::UnreferencedRemnant` (same RFC 164 Rule E
/// concept, carried by the archive instead of the live repository) so a caller already familiar
/// with one reads the other without translation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnreferencedRemnant {
    /// The `Block` that holds the missing reference.
    pub owner_object_id: ObjectId,
    /// Which of the `Block`'s own three reachability-gated reference kinds is missing: `"parent
    /// block"`, `"block patch"`, or `"snapshot blob"`.
    pub missing_role: String,
    /// The id this `Block` names, that this archive does not carry.
    pub missing_object_id: ObjectId,
}

/// Summary of an archive verify (§9.4). Never claims trust: see [`Self::note`] and
/// [`Self::is_internally_consistent`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArchiveVerifyReport {
    /// The on-disk repository format the archive's own manifest declares.
    pub repository_format: u32,
    /// The exporting build's own version, as carried in the manifest.
    pub tool_version: String,
    /// Every section decoded, every checksum matched, and the fixed set of thirteen section kinds
    /// was present exactly once each.
    pub structurally_sound: bool,
    /// Every reference the store itself would also require resolves to an id this archive carries
    /// (0.52.0 step 0, Part C1, correcting a post-0.51.0 finding: the archive used to require
    /// *every* reference of every object, stricter than the store's own standing design). A `Tag`'s
    /// own target, a `RefState`'s own target and required attestations (and, only for a received
    /// ref's own current tip, its previous state), and an `Attestation`'s own target when some
    /// `RefState` actually requires it: required unconditionally, damage if missing. A `Block`'s own
    /// `parent_block_ids`/`patch_ids`/`snapshot_blob_ref`: required only when the `Block` is itself
    /// reachable from the archive's own carried roots (RFC 164 Rule E) -- otherwise named in [`Self::
    /// unreferenced_remnants`] instead, never here. A `Block`'s own `merge_baseline_block_id`, a
    /// sealed `Patch`'s own blob references, and a `RecognitionClaim`'s own references: never
    /// existence-checked, by the store's own standing design, so never classified here either way.
    pub reference_closure_sound: bool,
    /// Count of signatures that verified against the archive's own carried material (author or
    /// maintainer).
    pub signatures_checked: usize,
    /// Every signature by a key this archive **does carry**, that nonetheless fails to verify --
    /// damage, named, and part of [`Self::is_internally_consistent`]'s own formula (§9.4, amended
    /// 2026-10-12, review `rfc155-impl-IC-review-v1`, finding N1). Before this fix, a signature by a
    /// key the archive simply does not carry (R2: trust is the receiver's, never the archive's own)
    /// was folded into this same list and refused on -- making such an archive permanently
    /// unimportable, by anyone, even into a destination that already trusts the key. That case now
    /// lives in [`Self::not_checked_signatures`] instead.
    pub unresolvable_signatures: Vec<String>,
    /// Every signature by a key this archive does **not** carry, named *"not checked"* -- never
    /// silently skipped (§9.4), but never refused on either: trust is the receiver's own adopted
    /// set, checked only after import (R2), not this archive's own carried record. A received
    /// pointer the source repository itself never trusted is exactly this case.
    pub not_checked_signatures: Vec<String>,
    /// Every structural or reference defect found, named -- a flipped byte, a dangling reference, a
    /// complete container record that fails its own checksum. Damage in an object container travels
    /// verbatim (§9.1); this is where it surfaces. Part of [`Self::is_internally_consistent`]'s own
    /// formula.
    pub damaged: Vec<String>,
    /// A container record this archive's own section declares but never finishes -- a crash tail
    /// (or bytes added since), carried verbatim because the source repository's own `prikk verify`
    /// only ever warns about it (§9.1, amended 2026-10-12). Named, but never part of [`Self::
    /// is_internally_consistent`]'s own formula: an honest backup of a repository whose own `verify`
    /// exits 0 must still restore.
    pub interrupted_appends: Vec<String>,
    /// A `Block`'s own missing `parent_block_ids`/`patch_ids`/`snapshot_blob_ref` reference, when
    /// the `Block` itself is **not** reachable from the archive's own carried roots (0.52.0 step 0,
    /// Part C1; RFC 164 Rule E) -- named, carried verbatim, imported as the store itself would hold
    /// it. Never part of [`Self::is_internally_consistent`]'s own formula: nothing reachable from a
    /// carried ref still needs it, so it is harmless, not damage. A killed `bundle import` is the
    /// standing example (§5): no command removes the remnant it leaves, and an honest export of
    /// that repository carries it forward, same as the store's own `verify` already does.
    pub unreferenced_remnants: Vec<UnreferencedRemnant>,
    /// Every maintainer key this archive carries, listed for inspection -- never adopted by this
    /// call.
    pub maintainer_keys: Vec<CarriedMaintainerKeySummary>,
    /// *"internally consistent"*, never *"trusted"* -- `Some` only when [`Self::
    /// is_internally_consistent`] is true (V2's own fix, finding V3): a damaged or unresolvable
    /// archive gets the damage lines and the error, never a note that contradicts them.
    pub note: Option<&'static str>,
}

impl ArchiveVerifyReport {
    /// The single top-level verdict: every structural, closure, and signature check passed. Reading
    /// this as *"trusted"* would be the exact mistake R2 exists to prevent -- it means only that the
    /// archive agrees with itself and with the key material it itself carries.
    #[must_use]
    pub fn is_internally_consistent(&self) -> bool {
        self.structurally_sound
            && self.reference_closure_sound
            && self.unresolvable_signatures.is_empty()
            && self.damaged.is_empty()
    }
    // Deliberately not consulted above: `not_checked_signatures` (R2: trust is the receiver's, not
    // this archive's own carried record), `interrupted_appends` (§9.1: the source's own `verify`
    // only warns about one of these, so a backup of that repository must still restore), and
    // `unreferenced_remnants` (0.52.0 step 0: RFC 164 Rule E's own harmless case, named, never a
    // reason to refuse -- the store's own `verify` does not fail over one either).
}

/// `pub(super)`, not private: RFC 155 Part I-A's own `archive::import` re-reads the same manifest
/// (over the same already-internally-consistent archive, confirmed by its own call to
/// [`verify_archive`] first) to locate the sections it writes from -- not a second decoder, the
/// same one, shared with its sibling module through their common parent.
pub(super) struct ManifestView {
    pub(super) repository_format: u32,
    pub(super) tool_version: String,
    pub(super) entries: Vec<ManifestEntry>,
}

/// Verify a `PREPO001` archive read from `source`, streamed on both ends (§9.4). Never writes
/// anything, on any exit path. Returns `Err` only for a defect that leaves nothing coherent to
/// report (an unknown/duplicated/missing section kind, a torn trailer, a manifest that does not
/// match its own declared layout); a *content* defect reachable only once the manifest is trusted (a
/// flipped byte, a dangling reference, an unresolvable signature) is named in the returned report
/// instead, with [`ArchiveVerifyReport::is_internally_consistent`] turning `false` -- consistent with
/// §9.1: "damage ... travels verbatim. The archive's verify names it."
pub fn verify_archive<R: Read + Seek>(source: &mut R) -> Result<ArchiveVerifyReport> {
    let (total_len, manifest_offset, manifest_length, manifest_checksum) =
        read_end_trailer(source)?;
    let manifest = read_manifest(source, manifest_offset, manifest_length, manifest_checksum)?;
    validate_manifest_shape(&manifest, manifest_offset, manifest_length, total_len)?;

    let mut damaged = Vec::new();
    let mut interrupted_appends = Vec::new();
    let mut structurally_sound = true;

    // Pass 1: every section's own outer checksum, and -- for the seven object-container sections,
    // in the same single read -- the pass-1 id set (§9.2's "two passes with an id set": this is the
    // first). Each individual object record is decoded only for the duration of this one loop
    // iteration; nothing holds more than one record's own bytes at a time, and the id set holds only
    // 32-byte ids, never an envelope.
    let mut ids: HashSet<ObjectId> = HashSet::new();
    for entry in &manifest.entries {
        if let ArchiveSectionKind::ObjectContainer(object_type) = entry.kind {
            let outcome = for_each_object_record(source, entry, object_type, |envelope| {
                ids.insert(envelope.object_id());
                Ok(())
            })?;
            if !outcome.checksum_ok {
                structurally_sound = false;
                damaged.push(format!(
                    "{} failed its own checksum or is truncated",
                    entry.kind.label()
                ));
            }
            // §9.1, amended 2026-10-12 (review `rfc155-impl-IC-review-v1`): the archive classifies a
            // container's frames exactly as the store does (RFC 162 rule 2). A complete frame that
            // fails is damage; an incomplete one (a crash tail, or bytes added since) is named, never
            // refused on -- the source repository's own `prikk verify` only warns about it.
            for message in &outcome.record_damage {
                structurally_sound = false;
                damaged.push(format!("{}: {message}", entry.kind.label()));
            }
            for message in &outcome.interrupted_appends {
                interrupted_appends.push(format!("{}: {message}", entry.kind.label()));
            }
            if outcome.tail_len > 0 {
                interrupted_appends.push(format!(
                    "{} has {} trailing byte(s) after its last complete record -- an interrupted \
                     append",
                    entry.kind.label(),
                    outcome.tail_len
                ));
            }
        } else if !stream_opaque_section(source, entry)? {
            structurally_sound = false;
            damaged.push(format!(
                "{} failed its own checksum or is truncated",
                entry.kind.label()
            ));
        }
    }

    // The three small, carried trust/author sections, decoded wholly into memory -- bounded by key
    // count, never by repository size (§9.4's own "small key material, not content").
    let maintainer_policy = read_maintainer_policy(source, &manifest)?;
    let author_material = read_author_material(source, &manifest)?;

    // 0.52.0 step 0, Part C1: the archive's own reachability roots, read once before pass 2 -- every
    // local ref's current tip (the carried, resolved pointer index) and every received ref's current
    // tip (the carried, resolved received index), reduced to one entry per ref name exactly the way
    // `archive::import`'s own `compute_landing_plan` already does (`reduce_pointer_index_entries`/
    // `reduce_received_index_entries`, reused, not re-derived). No queued-patch root exists here:
    // export already refuses while any session holds queued, unsealed work (§9.1), and the active
    // WAL itself is never carried at all -- so that root class is always empty in a real archive,
    // confirmed from the spec's own carried-content table, not assumed.
    let pointer_entry = find_entry(&manifest, ArchiveSectionKind::RefPointerIndex)?;
    let pointer_bytes = read_section_body_fully(source, pointer_entry)?;
    let decoded_pointers = decode_pointer_index_entries_for_resolver(&pointer_bytes)?;
    let received_entry = find_entry(&manifest, ArchiveSectionKind::ReceivedIndex)?;
    let received_bytes = read_section_body_fully(source, received_entry)?;
    let decoded_received = decode_received_index_entries_for_resolver(&received_bytes)?;
    let mut reachability_roots: Vec<ObjectId> = Vec::new();
    let mut local_pointer_entries: Vec<PointerIndexEntry> = Vec::new();
    let mut received_pointer_entries: Vec<ReceivedIndexEntry> = Vec::new();
    if !decoded_pointers.damaged {
        local_pointer_entries = reduce_pointer_index_entries(decoded_pointers.entries);
        for entry in &local_pointer_entries {
            reachability_roots.push(entry.ref_state_id);
        }
    }
    if !decoded_received.damaged {
        received_pointer_entries = reduce_received_index_entries(decoded_received.entries);
        for entry in &received_pointer_entries {
            reachability_roots.push(entry.ref_state_id);
        }
    }
    // A damaged resolved index is already named by pass 1's own opaque-section checksum check above
    // (`stream_opaque_section`); its own interior-decode damage is not re-reported here a second
    // time -- reachability simply has no roots from that side, the safe direction (fewer roots never
    // hides damage, it can only turn a reachable object into a remnant it is not).

    // Pass 2: reference closure and every signature, re-streaming each object-container section
    // against the now-complete id set (§9.2's second pass). Also builds `id_to_expansion` (0.52.0
    // step 0, Part C1): one entry per carried object, its own frontier-expansion step (the exact
    // function the store's own `compute_reachable_object_ids` uses, shared, not copied) -- so a
    // Block's own possibly-missing `parent_block_ids`/`patch_ids`/`snapshot_blob_ref` reference can
    // be classified against the archive's own reachability, computed after this pass completes, the
    // same way the store defers `ensure_object_exists`'s own classification to after its own walk.
    let mut reference_closure_sound = true;
    let mut signatures_checked = 0usize;
    let mut unresolvable_signatures = Vec::new();
    let mut not_checked_signatures = Vec::new();
    let mut id_to_expansion: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
    // 0.52.0 step 0, Part C1 round 3 (finding C1-8): restricted to the four types the shared ref
    // checks actually read (`reader_holds_type`'s own doc) -- round 2 held every non-`Blob` envelope,
    // including `Patch`, whose own body is not small (an `EditText` carries its own before/after span
    // text, DC-65), so a text-heavy history's `archive verify` memory used to scale with the
    // archive's own total patch volume, not the largest single object (§9.2).
    let mut id_to_envelope: HashMap<ObjectId, ObjectEnvelope> = HashMap::new();
    let mut block_candidates: Vec<(ObjectId, &'static str, ObjectId)> = Vec::new();
    for entry in &manifest.entries {
        let ArchiveSectionKind::ObjectContainer(object_type) = entry.kind else {
            continue;
        };
        for_each_object_record(source, entry, object_type, |envelope| {
            id_to_expansion.insert(envelope.object_id(), expand_frontier(envelope));
            classify_references(envelope, &ids, &mut block_candidates)?;
            if reader_holds_type(envelope.object_type) {
                id_to_envelope.insert(envelope.object_id(), envelope.clone());
            }
            check_signatures(
                envelope,
                &maintainer_policy,
                &author_material,
                &mut signatures_checked,
                &mut unresolvable_signatures,
                &mut not_checked_signatures,
            );
            Ok(())
        })?;
    }

    // Reachability, computed once, after pass 2's own id-to-expansion map is complete (0.52.0 step
    // 0, Part C1) -- in-memory only, no further archive bytes read, O(objects) memory, the same
    // order as the id set §9.2 already bounds.
    let reachable = compute_archive_reachability(&reachability_roots, &id_to_expansion);
    let mut unreferenced_remnants = Vec::new();
    for (owner_object_id, missing_role, missing_object_id) in block_candidates {
        if reachable.contains(&owner_object_id) {
            reference_closure_sound = false;
            damaged.push(format!(
                "block {owner_object_id} references {missing_object_id}, which this archive does \
                 not carry"
            ));
        } else {
            unreferenced_remnants.push(UnreferencedRemnant {
                owner_object_id,
                missing_role: missing_role.to_string(),
                missing_object_id,
            });
        }
    }

    // 0.52.0 step 0, Part C1 round 3: the store's own pointer-entry check, the real ref-log decode
    // and replay (`refs::verify::read_logs`, over a `DecodedRefLog` built from the archive's own
    // carried `RefLog` section bytes -- the identical decoder a live container read already uses,
    // confirmed from source: `ArchiveSectionKind::RefLog` is that container's own slot `a` bytes,
    // verbatim), and the pointer<->log tie (`refs::verify::classify_ref_items`) -- run against the
    // archive's own carried content through `ArchiveObjectReader`, shared, not re-derived. Round 2's
    // own reimplemented ref-log walk (sorted by `update_seq`, grouped by the decoded name rather than
    // the frame's own header key, and silent about trailing partial bytes) is gone: it was a second
    // copy of exactly the security-bearing logic this round shares instead (finding C1-7).
    let object_reader = ArchiveObjectReader {
        envelopes: &id_to_envelope,
    };
    let mut pointers: BTreeMap<String, PointerState> = BTreeMap::new();
    for entry in &local_pointer_entries {
        match read_one_pointer_entry(&object_reader, entry) {
            Ok((ref_name, state)) => {
                pointers.insert(ref_name, state);
            }
            Err(error) => {
                reference_closure_sound = false;
                damaged.push(format!("local ref {}: {error}", entry.ref_name));
            }
        }
    }
    for entry in &received_pointer_entries {
        if let Err(error) =
            verify_received_ref_state(&object_reader, &entry.ref_name, entry.ref_state_id)
        {
            reference_closure_sound = false;
            damaged.push(format!("received ref {}: {error}", entry.ref_name));
        }
    }
    let ref_log_entry = find_entry(&manifest, ArchiveSectionKind::RefLog)?;
    let ref_log_bytes = read_section_body_fully(source, ref_log_entry)?;
    let ref_log_decoded: Result<Option<DecodedRefLog>> =
        decode_ref_container_records(&ref_log_bytes).map(|replay| {
            Some(DecodedRefLog {
                bytes: ref_log_bytes,
                replay,
            })
        });
    let (logs, _log_record_count, _ref_log_envelopes, log_failures_by_key, _log_outcomes) =
        read_logs(
            &|offset| PathBuf::from(format!("archive:RefLog#{offset}")),
            &object_reader,
            &pointers,
            &ref_log_decoded,
        )?;
    // Every damaged ref-log record (a frame whose checksum fails, whose payload does not decode, or
    // whose own header `ref_name_key` disagrees with its decoded `ref_name`) is damage, regardless of
    // whether the ref it belongs to is also named by a carried pointer -- `read_logs` only inserts a
    // ref's own `LogState` once its full subsequence validates cleanly, so a damaged key is otherwise
    // silent here.
    for message in log_failures_by_key.values() {
        reference_closure_sound = false;
        damaged.push(format!("ref log: {message}"));
    }
    // The pointer<->log tie: the exact classification `verify_refs_with` runs over a live
    // repository's own `pointers`/`logs` maps, shared through `classify_ref_items`. A local pointer
    // naming an uncarried previous RefState, with no log record at all for that ref, falls through
    // every named arm to `classify_ref_state`'s own catch-all `Err` -- the same source shape this
    // archive used to pass before this round (finding C1-6/C1-1).
    // 0.52.0 step 0, Part C2, N1 (review v3): no read-failure skip here -- any read failure this
    // pass already hit (a damaged pointer entry, a damaged ref-log record) was already folded into
    // `damaged` above, before `pointers`/`logs` were even built, so every name in the union is
    // classified, same as before this closure parameter existed.
    let (ref_items, publication_issues) = classify_ref_items(&pointers, &logs, |_| None);
    for (ref_name, result) in ref_items {
        if let Err(error) = result {
            reference_closure_sound = false;
            damaged.push(format!("ref {ref_name}: {error}"));
        }
    }
    // `classify_ref_state` returns `Ok` for `POINTER-LEADS-LOG`/`DIVERGENCE` (named, non-fatal
    // conditions to a live repository's own ref-log scan), but a live repository's own overall
    // `verify` still fails over either one (`VerificationReport::has_blocking_ref_publication_
    // issues`) -- `archive export` never refuses on either state (neither is a queued-unsealed-work
    // nor a slotted-container-ambiguity condition, confirmed from `archive.rs`'s own `refuse_on_
    // queued_unsealed_work`/`resolve_pointer_index_source`), so an honest archive can carry this
    // state verbatim, and this closure pass must classify it exactly as the source's own `verify`
    // would, not silently pass it because `classify_ref_state` itself did not `Err`.
    for issue in publication_issues {
        if issue.blocking {
            reference_closure_sound = false;
            let ref_name = issue.ref_name.as_deref().unwrap_or("(repository-wide)");
            damaged.push(format!(
                "ref {ref_name}: {} ({})",
                issue.message, issue.code
            ));
        }
    }
    let maintainer_keys = maintainer_policy
        .keys
        .iter()
        .map(|key| CarriedMaintainerKeySummary {
            key_id: key.key_id.clone(),
            public_key_hex: prikk_hash::to_hex(&key.public_key),
        })
        .collect();

    let mut report = ArchiveVerifyReport {
        repository_format: manifest.repository_format,
        tool_version: manifest.tool_version,
        structurally_sound,
        reference_closure_sound,
        signatures_checked,
        unresolvable_signatures,
        not_checked_signatures,
        damaged,
        interrupted_appends,
        unreferenced_remnants,
        maintainer_keys,
        note: None,
    };
    // V3's own fix (review `rfc155-impl-V-review-v1` finding V3): the note claimed "internally
    // consistent" even beside damage and an error exit -- printed only when it is actually true,
    // computed from the report already built so this can never drift from
    // `is_internally_consistent`'s own formula.
    if report.is_internally_consistent() {
        report.note = Some(
            "internally consistent -- not trusted; trust comes only from a receiver's own adopted \
             set, checked after import, never from this archive's own carried record",
        );
    }
    Ok(report)
}

/// §9.4's amended rule (N1): a signature by a key this archive does not carry is named *"not
/// checked"* and never refused on (trust is the receiver's, R2); a signature by a key this archive
/// **does** carry, that nonetheless fails to verify, is damage. Applies identically to MAINTAINER
/// and AUTHOR signatures -- both buckets below are symmetric for the two roles, not a special case
/// for one.
fn check_signatures(
    envelope: &ObjectEnvelope,
    maintainer_policy: &MaintainerTrustPolicy,
    author_material: &HashMap<String, Vec<AuthorKeyEntry>>,
    signatures_checked: &mut usize,
    unresolvable: &mut Vec<String>,
    not_checked: &mut Vec<String>,
) {
    if envelope
        .signatures
        .iter()
        .any(|signature| signature.signer_role == SignerRole::Maintainer)
    {
        match verify_trusted_publication_envelope(maintainer_policy, envelope) {
            Ok(_) => *signatures_checked += 1,
            // "PRIKK-TRUST-PUBLICATION-UNTRUSTED": every MAINTAINER signature on this envelope is by
            // a key `maintainer_policy` does not carry -- not checked, not damage (R2).
            Err(issue) if issue.code == "PRIKK-TRUST-PUBLICATION-UNTRUSTED" => {
                not_checked.push(format!(
                    "{} {}: not checked: signed by a key this archive does not carry",
                    envelope.object_type,
                    envelope.object_id()
                ));
            }
            // "PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE": a key the policy *does* carry, whose
            // signature does not verify -- damage.
            Err(issue) => unresolvable.push(format!(
                "{} {}: {}",
                envelope.object_type,
                envelope.object_id(),
                issue.message
            )),
        }
    }
    match verify_author_signatures_with(envelope, |key_id| {
        Ok(author_material.get(key_id).cloned().unwrap_or_default())
    }) {
        Ok(Some((_, true))) => *signatures_checked += 1,
        // No recorded material at all for this key id in the archive -- not carried, not checked,
        // not damage (the same N1 rule, for AUTHOR signatures).
        Ok(Some((key_id, false))) => not_checked.push(format!(
            "{} {}: not checked: AUTHOR key {key_id} is not carried by this archive",
            envelope.object_type,
            envelope.object_id()
        )),
        Ok(None) => {}
        // Material *is* recorded for this key id, but the signature itself does not verify against
        // it (or the key id's own recorded material disagrees with itself, DC-53 Stage 2 D8) --
        // damage.
        Err(err) => unresolvable.push(format!(
            "{} {}: AUTHOR signature does not verify: {err}",
            envelope.object_type,
            envelope.object_id()
        )),
    }
}

/// 0.52.0 step 0, Part C1 round 3 (finding C1-8): the one predicate deciding whether
/// [`ArchiveObjectReader`]'s own `id_to_envelope` map holds an envelope -- factored out so a test can
/// call the exact production predicate, not a restated copy of it, to prove a `Patch` id reads as
/// absent. `RefState`, `Tag`, `Attestation` and `Block` are the only types the shared checks below
/// ever read (`verified_ref_state_payload`, `ensure_ref_target_valid`,
/// `ensure_required_attestations_present`, `verify_received_ref_state`'s own previous-link read) --
/// never `Patch` (DC-65: an `EditText`'s own before/after span text can be large) and never `Blob`
/// (the one thing ever large, up to the per-object bound).
const fn reader_holds_type(object_type: ObjectType) -> bool {
    matches!(
        object_type,
        ObjectType::RefState | ObjectType::Tag | ObjectType::Attestation | ObjectType::Block
    )
}

/// 0.52.0 step 0, Part C1 round 2 (restricted further in round 3, see [`reader_holds_type`]): a
/// random-access [`ObjectReader`] over pass 2's own `id_to_envelope` map, so the store's own
/// ref-checking functions (`refs::verify::{read_one_pointer_entry, read_logs}`, `crate::verify::
/// verify_received_ref_state`) can run against the archive's own carried content directly, unmodified
/// -- the ruling's own item 1 ("an object reader over the archive"). The ruling's own wording
/// describes an id -> (type, section, offset) map with a seeking, per-read decode; this holds the
/// already-decoded envelope instead (built once, during pass 2, which was already decoding every one
/// of them to compute `id_to_expansion`). Still O(objects) memory, never O(archive bytes) -- the one
/// thing ever large, a `Blob`'s own body, is never held (see `reader_holds_type`'s own doc note above)
/// -- and avoids a second `R: Read + Seek` borrow living across this struct's own lifetime, which the
/// seeking design would require threading through every one of pass 2's own closures. Declared here,
/// not guessed past: if this simplification is wrong for this use, say so in review.
struct ArchiveObjectReader<'a> {
    envelopes: &'a HashMap<ObjectId, ObjectEnvelope>,
}

impl ObjectReader for ArchiveObjectReader<'_> {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        Ok(self.envelopes.get(&id).cloned())
    }
}

/// 0.52.0 step 0, Part C1 round 2: the archive's own closure rule now classifies only `Block`'s own
/// three reachability-gated references (RFC 164 Rule E) -- deferred into `block_candidates` for
/// classification once the archive's own reachability is computed, after this pass completes.
/// `merge_baseline_block_id` is never existence-checked at all (only used in a content/divergence
/// comparison, `verify.rs`'s own `MergeBaselineDivergence`), so it is not classified here either way.
///
/// Every other reference this module used to classify here -- `Tag.target_block_id`,
/// `RefState.target_object_id`/`required_attestation_ids`/`previous_ref_state_id`,
/// `Attestation.target_block_id` -- is no longer classified in this pass at all. Those are exactly
/// the references the store's own shared functions (`read_one_pointer_entry`, `read_logs`'s own
/// `validate_log_replay`/`classify_ref_state` tie, `verify_received_ref_state`, called below through
/// [`ArchiveObjectReader`]) already check, at the
/// places the store itself checks them: a pointer entry's own tip, a ref-log record, a received ref's
/// own tip. Checking them here too, by this pass's own re-derived approximation, was exactly the
/// post-0.51.0 and round-1 defect class this part exists to fix (the archive required *every*
/// reference unconditionally, and round 1's re-derivation disagreed with the store on three more
/// rows) -- so this pass now checks only what no shared function already covers.
/// `RecognitionClaim`'s own references, and a sealed `Patch`'s own blob references: never
/// existence-checked, by standing design (R2-adjacent: a claim is "never trust-conferring and never
/// existence-checked"; a sealed patch's blobs are not re-derived, DC-65) -- not classified at all.
fn classify_references(
    envelope: &ObjectEnvelope,
    ids: &HashSet<ObjectId>,
    block_candidates: &mut Vec<(ObjectId, &'static str, ObjectId)>,
) -> Result<()> {
    if envelope.object_type == ObjectType::Block {
        let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
        let block_id = envelope.object_id();
        for parent in payload.parent_block_ids {
            if !ids.contains(&parent) {
                block_candidates.push((block_id, "parent block", parent));
            }
        }
        for patch in payload.patch_ids {
            if !ids.contains(&patch) {
                block_candidates.push((block_id, "block patch", patch));
            }
        }
        if let Some(blob) = payload.snapshot_blob_ref {
            if !ids.contains(&blob) {
                block_candidates.push((block_id, "snapshot blob", blob));
            }
        }
    }
    Ok(())
}

/// The archive's own reachability walk (0.52.0 step 0, Part C1): in-memory only, over
/// `id_to_expansion` (one entry per carried object, built during pass 2), using the exact same
/// frontier-expansion step the store's own `compute_reachable_object_ids` uses
/// (`crate::verify::reachability::expand_frontier`, shared, not copied). `roots` are the archive's
/// own carried pointer-index and received-index tips -- an honest export carries every ref, so the
/// archive's own roots equal the source repository's own roots, and the classification that follows
/// is the source's own classification.
fn compute_archive_reachability(
    roots: &[ObjectId],
    id_to_expansion: &HashMap<ObjectId, Vec<ObjectId>>,
) -> HashSet<ObjectId> {
    let mut reached: HashSet<ObjectId> = HashSet::new();
    let mut frontier: Vec<ObjectId> = roots.to_vec();
    while let Some(id) = frontier.pop() {
        if reached.contains(&id) {
            continue;
        }
        let Some(expansion) = id_to_expansion.get(&id) else {
            // Not carried, or never decoded: not reachable, nothing to expand past it -- the same
            // "missing or unreadable" outcome the store's own live walk gives.
            continue;
        };
        reached.insert(id);
        frontier.extend(expansion.iter().copied());
    }
    reached
}

/// [`for_each_object_record`]'s own outcome: the outer checksum's own pass/fail; every **complete**
/// frame that failed (RFC 162 rule 2's own `complete: true` -- a full header and a full claimed
/// body were actually present, but the record failed its own checksum, or decoded to the wrong
/// envelope type, or the resync scan itself exhausted its budget), named by byte offset -- damage,
/// in [`SectionReadOutcome::record_damage`]; and every **incomplete** frame (rule 2's `complete:
/// false`) -- a crash tail, or bytes added since, including one later overrun by a sound frame --
/// named in [`SectionReadOutcome::interrupted_appends`], never damage on its own. **0.52.0 step 2
/// (RFC 155 §9.4):** the archive now classifies a section's frames by calling the identical shared
/// primitive the store's own `foundation::container::decode_container_records` calls
/// (`foundation::container::decode_frames_with`), over a [`SectionFrameSource`] scoped to this one
/// section's window rather than a whole-container in-memory slice -- one function decides every
/// frame's own classification for both readers, not two hand-maintained copies of the identical
/// rule held equal only by a differential test. `decode_frames_with`'s own per-record callback is
/// what keeps memory bounded by one record's own body at a time: it is called inline, per sound
/// frame, and never collects a `Vec` of them the way the store's own `Vec`-returning
/// `decode_container_records` does for its own, already-whole-container-resident callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SectionReadOutcome {
    pub(super) checksum_ok: bool,
    /// One message per complete frame that failed -- a checksum mismatch over a fully-present
    /// claimed body that no later sound frame overran, a body that decodes to the wrong envelope
    /// type, or a resync scan that exhausted its own budget. Damage, named by byte offset within
    /// the section.
    pub(super) record_damage: Vec<String>,
    /// One message per incomplete frame -- a crash tail (or bytes added since) that no later write
    /// overran, or a torn frame a later, genuine write did overrun (the store's own overrun rule:
    /// `checksum_mismatch_is_complete`). Named, but never damage on its own: the same bytes a live
    /// repository's own crash tail would leave, which that repository's own `prikk verify` only
    /// ever warns about.
    pub(super) interrupted_appends: Vec<String>,
    /// The genuinely final tail -- a short or long read that reaches the section's own declared
    /// end with no sound frame anywhere in the remainder. Confirmed from source,
    /// `foundation::container::decode_container_records`'s own `TrailingPartial` arm's
    /// `BoundedScan::NoSoundFrame` case pushes **no** `record_outcomes` entry at all for this one
    /// case -- only this separate `trailing_partial_bytes`-shaped field -- unlike every other
    /// non-sound case (`Sound(next)`, `Undetermined`, `Invalid`, `ChecksumMismatch`), which all do
    /// push one. Tracked separately so [`Self::record_damage`]/[`Self::interrupted_appends`] can be
    /// held equal to `record_outcomes` exactly, offset for offset, by the differential test.
    pub(super) tail_len: u64,
}

/// One archive-specific framing for every [`SectionReadOutcome::interrupted_appends`] line,
/// regardless of which of the several call sites inside [`for_each_object_record`] produces it.
/// External review 024's own finding: one of those sites used to push
/// `foundation::frame_resync::partial_before_sound_frame_message`'s own text unchanged, whose
/// prose literally says *"damage, not a torn tail"* -- correct for the several *live-repository*
/// readers that helper serves (several of which do treat this shape as damage in their own
/// context), but wrong for the archive, where this exact shape is an interrupted append the
/// source repository's own `prikk verify` only ever warns about (§9.1). This helper is the one
/// place that wording is written, so no call site can silently reintroduce "damage" by calling a
/// store-side message function directly.
fn interrupted_append_message(detail: impl std::fmt::Display) -> String {
    format!("interrupted append: {detail}; not imported")
}

/// Read and validate one object-container section: its own outer header and checksum against
/// `entry` (read in one streamed, chunked pass -- [`section_header_and_checksum`]), then every
/// inner container record in turn, feeding `on_record` only the records whose own checksum matches
/// ([`classify_section_records`]). §9.1 (amended 2026-10-12): the archive classifies a section's
/// frames exactly as a live repository's own container read would classify the identical bytes --
/// including the resync-and-continue behavior after any unsound frame, and the overrun rule for a
/// torn frame a later write genuinely finished past. Memory is bounded by one record's own body at a
/// time, plus a small, fixed scan window the resync itself uses -- never the whole section.
///
/// `on_record` takes `&ObjectEnvelope`, not raw bytes (0.52.0 step 2): the shared primitive
/// (`foundation::container::decode_frames_with`) already decodes the envelope once, to classify the
/// frame at all, so every caller re-decoding the same raw bytes a second time was pure waste -- all
/// three call sites already did exactly that as their own first step, confirmed from source before
/// this change.
///
/// `pub(super)`: RFC 155 Part I-A's own `archive::import` needs the identical decode -- not a
/// second copy of it -- for its own stage 1, over an archive its own stage 0 already confirmed
/// internally consistent via [`verify_archive`] (see [`ManifestView`]'s own doc note).
pub(super) fn for_each_object_record<R: Read + Seek>(
    source: &mut R,
    entry: &ManifestEntry,
    object_type: ObjectType,
    on_record: impl FnMut(&ObjectEnvelope) -> Result<()>,
) -> Result<SectionReadOutcome> {
    let Some(checksum_ok) = section_header_and_checksum(source, entry)? else {
        // The section's own outer header (magic, wire code, declared length) does not match --
        // nothing about its own record area can be trusted enough to classify; matches the
        // pre-resync behavior exactly.
        return Ok(SectionReadOutcome {
            checksum_ok: false,
            record_damage: Vec::new(),
            interrupted_appends: Vec::new(),
            tail_len: 0,
        });
    };
    // The outer checksum is a separate, independent signal from record-level classification (it
    // covers every raw byte in the section, verbatim) -- classification always runs, checksum_ok
    // or not, the same as before this part.
    let (record_damage, interrupted_appends, tail_len) =
        classify_section_records(source, entry, object_type, on_record)?;
    Ok(SectionReadOutcome {
        checksum_ok,
        record_damage,
        interrupted_appends,
        tail_len,
    })
}

/// The section's own outer header and checksum, read in one streamed, chunked pass -- `None` when
/// the header itself (magic, wire code, declared length) does not match `entry`; `Some(matches)`
/// once it does, where `matches` is whether the trailing 32-byte checksum, computed over the header
/// plus every byte of the record area, agrees with what is stored **and** with the manifest's own
/// recorded entry for this section (RFC 155 §9.4's own amendment, I-E's own aside: the exporter
/// always writes the identical checksum to both places, `archive.rs`'s own `write_section`
/// confirms it, so a section's own on-disk trailing checksum disagreeing with what the
/// checksummed, tamper-evident manifest recorded for it -- e.g. two sections swapped, each still
/// internally self-consistent -- is damage, the same way a torn or flipped section already is).
fn section_header_and_checksum<R: Read + Seek>(
    source: &mut R,
    entry: &ManifestEntry,
) -> Result<Option<bool>> {
    source.seek(SeekFrom::Start(entry.offset))?;
    let mut header = [0u8; SECTION_HEADER_LEN];
    source.read_exact(&mut header)?;
    let header_ok = &header[0..8] == SECTION_MAGIC
        && u16::from_be_bytes([header[8], header[9]]) == entry.kind.wire_code()
        && u64::from_be_bytes(header[10..18].try_into().unwrap_or([0u8; 8])) == entry.length;
    if !header_ok {
        return Ok(None);
    }
    let mut hasher = IncrementalSha256::new();
    hasher.update(&header);
    let mut remaining = entry.length;
    let mut buffer = vec![0u8; super::STREAM_CHUNK_BYTES];
    while remaining > 0 {
        let take = remaining.min(super::STREAM_CHUNK_BYTES as u64) as usize;
        let Some(slice) = buffer.get_mut(..take) else {
            return Err(PrikkError::Integrity(
                "archive section checksum buffer slice out of bounds".to_string(),
            ));
        };
        source.read_exact(slice)?;
        hasher.update(slice);
        remaining -= take as u64;
    }
    let mut section_checksum = [0u8; 32];
    source.read_exact(&mut section_checksum)?;
    let computed_matches_stored = hasher.finalize() == section_checksum;
    let stored_matches_manifest_entry = section_checksum == entry.checksum;
    Ok(Some(
        computed_matches_stored && stored_matches_manifest_entry,
    ))
}

/// A [`FrameSource`] scoped to one archive section's own record area in a seekable source --
/// 0.52.0 step 2's one streamed implementation of the primitive (RFC 155 §9.4), replacing the
/// archive's own former hand-written `parse_frame_at_streamed`/`resync_to_next_magic_streamed`/
/// `sound_frame_after_partial_streamed`/`checksum_mismatch_is_complete_streamed` twins.
///
/// `read_at` seeks and reads into `scratch`, a buffer this struct owns and reuses across calls --
/// never allocating more than the one candidate frame actually being read at a time, the same "never
/// allocates `len` bytes before confirming they exist" discipline [`FrameSource::read_at`]'s own doc
/// requires (the DC-86 bound below is always checked, by the shared primitive, before a claim this
/// large is ever read -- see `parse_frame_at_reporting`'s own doc).
///
/// **Does not override [`FrameSource::scan_for_magic`]**: its default, windowed body already does,
/// through `read_at`, exactly what the archive's old hand-written `resync_to_next_magic_streamed`
/// did by hand (bounded, overlapping 1 MiB windows, each handed to the real
/// `frame_resync::resync_to_next_magic` unchanged) -- only [`SliceFrameSource`]
/// (`foundation/frame_source.rs`) has a reason to override it, to skip windowing entirely on the
/// store's own zero-copy commit-path reads.
struct SectionFrameSource<'a, R: Read + Seek> {
    source: &'a mut R,
    record_area_start: u64,
    section_len: usize,
    scratch: Vec<u8>,
}

impl<'a, R: Read + Seek> SectionFrameSource<'a, R> {
    fn new(source: &'a mut R, record_area_start: u64, section_len: usize) -> Self {
        Self {
            source,
            record_area_start,
            section_len,
            scratch: Vec::new(),
        }
    }
}

impl<R: Read + Seek> FrameSource for SectionFrameSource<'_, R> {
    fn total_len(&self) -> usize {
        self.section_len
    }

    // RFC 160 §3.3 (P3): `read_len` is never sized by an unvalidated declared length on its own --
    // this trait method has exactly two callers, both outside this file: `FrameSource::
    // scan_for_magic`'s own default windowed body (`foundation/frame_source.rs`) always clamps it
    // to the fixed `DEFAULT_SCAN_WINDOW_BYTES` constant first; `foundation::container::
    // parse_frame_at_reporting`'s own body read only reaches this call once `max_body_len` (here,
    // `Some(DEFAULT_BUNDLE_MAX_OBJECT_BYTES)`, DC-86) has already refused a claim over it. See
    // `allocation_bound_scan.rs`'s own `ALLOWED` entry for this exact site.
    fn read_at(&mut self, offset: usize, read_len: usize) -> Result<Option<&[u8]>> {
        let Some(end) = offset.checked_add(read_len) else {
            return Ok(None);
        };
        if end > self.section_len {
            return Ok(None);
        }
        self.source
            .seek(SeekFrom::Start(self.record_area_start + offset as u64))?;
        self.scratch.resize(read_len, 0);
        self.source.read_exact(&mut self.scratch)?;
        Ok(Some(&self.scratch[..read_len]))
    }
}

/// Classify every record in one section's own record area, calling `on_record` for each sound one.
/// 0.52.0 step 2 (RFC 155 §9.4): this is now a thin translation over the shared primitive
/// (`foundation::container::decode_frames_with`), over a [`SectionFrameSource`] scoped to this
/// section's own window -- the same decode loop the store's `decode_container_records` calls, not a
/// second, hand-maintained copy of its classification rule. `max_body_len` is
/// `Some(DEFAULT_BUNDLE_MAX_OBJECT_BYTES)` (DC-86): the archive is untrusted input and must never
/// allocate past its own per-object bound, a parameter the store's own commit-path reader always
/// passes `None` for instead (`parse_frame_at_reporting`'s own doc, `foundation/container.rs`).
///
/// **The wording is now one wording** (0.52.0 step 2 item 2): every `Failed` outcome's own
/// `message` comes from the primitive, unchanged. The archive's own added context is the frame's
/// own byte offset (the store's own rendering already carries this separately, in the locator path
/// `verify/objects.rs` builds, `container_path.join(format!("#{}", outcome.offset))` -- the archive
/// has no equivalent locator field, so this is where that same fact has to live for it) and, for an
/// incomplete frame, [`interrupted_append_message`]'s own wrapper. Not every one of the primitive's
/// own messages already names its offset in its own text (a bad magic/version, a malformed length,
/// or an envelope decode failure do not; a checksum mismatch or an object-type mismatch already do)
/// -- prefixing it unconditionally here, rather than only where it is missing, keeps the archive's
/// own invariant simple: every line it reports names its own offset, first, every time. Before this
/// part, one branch (a checksum mismatch later overrun by a sound frame) built its own independent
/// sentence instead of reusing the primitive's own text at all -- the two readers now say the
/// identical thing about the identical shape, pinned by a test.
fn classify_section_records<R: Read + Seek>(
    source: &mut R,
    entry: &ManifestEntry,
    object_type: ObjectType,
    mut on_record: impl FnMut(&ObjectEnvelope) -> Result<()>,
) -> Result<(Vec<String>, Vec<String>, u64)> {
    let record_area_start = entry.offset + SECTION_HEADER_LEN as u64;
    let section_len = usize::try_from(entry.length)
        .map_err(|_| PrikkError::Integrity("archive section length overflow".to_string()))?;
    let mut frame_source = SectionFrameSource::new(source, record_area_start, section_len);
    let summary = decode_frames_with(
        object_type,
        &mut frame_source,
        Some(DEFAULT_BUNDLE_MAX_OBJECT_BYTES as u64),
        |record| on_record(&record.envelope),
    )?;
    let mut record_damage = Vec::new();
    let mut interrupted_appends = Vec::new();
    for outcome in &summary.record_outcomes {
        if let ContainerRecordStatus::Failed { message, complete } = &outcome.status {
            let located = format!("byte offset {}: {message}", outcome.offset);
            if *complete {
                record_damage.push(located);
            } else {
                interrupted_appends.push(interrupted_append_message(located));
            }
        }
    }
    let tail_len = summary.trailing_partial_bytes as u64;
    Ok((record_damage, interrupted_appends, tail_len))
}

/// As [`for_each_object_record`], for a non-object section: stream the body in fixed chunks (never
/// buffered whole), confirming only the outer checksum -- these five families (the ref log, the
/// resolved ref pointer index/received index/trust policy, and whichever of the two key-material
/// sections is not read via [`read_section_body_fully`] here) carry no per-record reference closure
/// of their own.
fn stream_opaque_section<R: Read + Seek>(source: &mut R, entry: &ManifestEntry) -> Result<bool> {
    const CHUNK: usize = 64 * 1024;
    source.seek(SeekFrom::Start(entry.offset))?;
    let mut header = [0u8; SECTION_HEADER_LEN];
    source.read_exact(&mut header)?;
    let header_ok = &header[0..8] == SECTION_MAGIC
        && u16::from_be_bytes([header[8], header[9]]) == entry.kind.wire_code()
        && u64::from_be_bytes(header[10..18].try_into().unwrap_or([0u8; 8])) == entry.length;
    if !header_ok {
        return Ok(false);
    }
    let mut hasher = IncrementalSha256::new();
    hasher.update(&header);
    let mut remaining = entry.length;
    let mut buffer = vec![0u8; CHUNK];
    while remaining > 0 {
        let take = remaining.min(CHUNK as u64) as usize;
        let Some(slice) = buffer.get_mut(..take) else {
            return Err(PrikkError::Integrity(
                "archive verify: chunk exceeds buffer".to_string(),
            ));
        };
        source.read_exact(slice)?;
        hasher.update(slice);
        remaining -= take as u64;
    }
    let mut checksum = [0u8; 32];
    source.read_exact(&mut checksum)?;
    // I-E's own aside (RFC 155 §9.4): the section's own on-disk trailing checksum must also equal
    // what the manifest itself recorded for this entry -- see `section_header_and_checksum`'s own
    // doc comment for why (the exporter always writes the identical value to both places).
    Ok(hasher.finalize() == checksum && checksum == entry.checksum)
}

/// Read one section's full body into memory -- only ever called for the three small, carried
/// key-material sections, bounded by [`MAX_SMALL_SECTION_BYTES`], never for an object container.
///
/// `pub(super)`: RFC 155 Part I-A's own `archive::import` needs this for the two resolved-ref-index
/// sections (to compute its landing plan, §9.5) as well, which this module never decodes past their
/// own outer checksum (`verify_archive` only cares that each resolved family's checksum matches --
/// it never needs the ref names inside, so [`stream_opaque_section`] is all it ever runs against
/// them). See [`ManifestView`]'s own doc note.
pub(super) fn read_section_body_fully<R: Read + Seek>(
    source: &mut R,
    entry: &ManifestEntry,
) -> Result<Vec<u8>> {
    if entry.length > MAX_SMALL_SECTION_BYTES {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes: entry.length,
            bound_bytes: MAX_SMALL_SECTION_BYTES,
        });
    }
    source.seek(SeekFrom::Start(entry.offset + SECTION_HEADER_LEN as u64))?;
    let len = usize::try_from(entry.length)
        .map_err(|_| PrikkError::Integrity("archive section length overflow".to_string()))?;
    let mut body = vec![0u8; len];
    source.read_exact(&mut body)?;
    Ok(body)
}

/// `pub(super)`: `archive::import`'s own reuse, see [`ManifestView`]'s own doc note.
pub(super) fn find_entry(
    manifest: &ManifestView,
    kind: ArchiveSectionKind,
) -> Result<&ManifestEntry> {
    manifest
        .entries
        .iter()
        .find(|entry| entry.kind.wire_code() == kind.wire_code())
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "archive manifest's own {} section vanished after validation",
                kind.label()
            ))
        })
}

fn read_maintainer_policy<R: Read + Seek>(
    source: &mut R,
    manifest: &ManifestView,
) -> Result<MaintainerTrustPolicy> {
    let keys_bytes = read_section_body_fully(
        source,
        find_entry(manifest, ArchiveSectionKind::MaintainerKeys)?,
    )?;
    let policy_bytes = read_section_body_fully(
        source,
        find_entry(manifest, ArchiveSectionKind::TrustPolicy)?,
    )?;
    let key_replay = decode_trust_key_records(&keys_bytes)?;
    let mut key_map: HashMap<String, [u8; 32]> = HashMap::new();
    for entry in key_replay.entries {
        key_map.insert(entry.key_id, entry.public_key);
    }
    let policy_replay = decode_trust_policy_records(&policy_bytes)?;
    let current_ids = policy_replay
        .entries
        .last()
        .map(|entry| entry.key_ids.clone())
        .unwrap_or_default();
    let keys = current_ids
        .into_iter()
        .filter_map(|key_id| {
            key_map.get(&key_id).map(|public_key| AdoptedMaintainerKey {
                key_id,
                public_key: *public_key,
            })
        })
        .collect();
    Ok(MaintainerTrustPolicy { keys })
}

/// `pub(super)`: RFC 155 Part I-A's own stage 2 needs the identical carried author-key material
/// this module's own signature checks already decode -- see [`ManifestView`]'s own doc note.
pub(super) fn read_author_material<R: Read + Seek>(
    source: &mut R,
    manifest: &ManifestView,
) -> Result<HashMap<String, Vec<AuthorKeyEntry>>> {
    let bytes = read_section_body_fully(
        source,
        find_entry(manifest, ArchiveSectionKind::AuthorKeys)?,
    )?;
    let replay = decode_author_key_records(&bytes)?;
    let mut map: HashMap<String, Vec<AuthorKeyEntry>> = HashMap::new();
    for entry in replay.entries {
        map.entry(entry.key_id.clone()).or_default().push(entry);
    }
    Ok(map)
}

/// `pub(super)`: RFC 155 Part I-A's own `archive::import` re-locates the manifest the same way
/// [`verify_archive`] already did, over the same reader -- see [`ManifestView`]'s own doc note.
pub(super) fn read_end_trailer<R: Read + Seek>(
    source: &mut R,
) -> Result<(u64, u64, u64, [u8; 32])> {
    let total_len = source.seek(SeekFrom::End(0))?;
    if total_len < END_TRAILER_LEN_USIZE as u64 + 8 {
        return Err(PrikkError::MalformedData(
            "archive is too short to contain an end trailer".to_string(),
        ));
    }
    source.seek(SeekFrom::Start(0))?;
    let mut archive_magic = [0u8; 8];
    source.read_exact(&mut archive_magic)?;
    if &archive_magic != ARCHIVE_MAGIC {
        return Err(PrikkError::MalformedData(
            "not a PREPO001 archive: wrong magic".to_string(),
        ));
    }
    source.seek(SeekFrom::End(-(END_TRAILER_LEN_USIZE as i64)))?;
    let mut trailer = [0u8; END_TRAILER_LEN_USIZE];
    source.read_exact(&mut trailer)?;
    if &trailer[0..8] != END_TRAILER_MAGIC {
        return Err(PrikkError::MalformedData(
            "archive end trailer has the wrong magic -- truncated, or not an archive".to_string(),
        ));
    }
    let manifest_offset = u64::from_be_bytes(trailer[8..16].try_into().unwrap_or([0u8; 8]));
    let manifest_length = u64::from_be_bytes(trailer[16..24].try_into().unwrap_or([0u8; 8]));
    let manifest_checksum: [u8; 32] = trailer[24..56].try_into().unwrap_or([0u8; 32]);
    Ok((
        total_len,
        manifest_offset,
        manifest_length,
        manifest_checksum,
    ))
}

/// V2's own fix (review `rfc155-impl-V-review-v1` finding V1a): `manifest_checksum` (from the end
/// trailer) is checked against the manifest's own real bytes **before** `decode_manifest_bytes`
/// turns any of them into a field this call trusts -- a mismatch refuses by name, never silently
/// decoding a manifest that does not match what the trailer says it should be.
/// `pub(super)`: RFC 155 Part I-A's own reuse -- see [`ManifestView`]'s own doc note.
pub(super) fn read_manifest<R: Read + Seek>(
    source: &mut R,
    manifest_offset: u64,
    manifest_length: u64,
    manifest_checksum: [u8; 32],
) -> Result<ManifestView> {
    if manifest_length > MAX_MANIFEST_BYTES {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes: manifest_length,
            bound_bytes: MAX_MANIFEST_BYTES,
        });
    }
    source.seek(SeekFrom::Start(manifest_offset))?;
    let len = usize::try_from(manifest_length)
        .map_err(|_| PrikkError::Integrity("archive manifest length overflow".to_string()))?;
    let mut body = vec![0u8; len];
    source.read_exact(&mut body)?;
    let actual_checksum = prikk_hash::sha256(&body);
    if actual_checksum != manifest_checksum {
        return Err(PrikkError::MalformedData(
            "archive manifest failed its own checksum -- refusing the whole archive".to_string(),
        ));
    }
    decode_manifest_bytes(&body)
}

fn decode_manifest_bytes(body: &[u8]) -> Result<ManifestView> {
    let mut cursor = ByteCursor::new(body);
    let magic = cursor.read_array::<8>()?;
    if &magic != MANIFEST_MAGIC {
        return Err(PrikkError::MalformedData(
            "archive manifest has the wrong magic".to_string(),
        ));
    }
    let repository_format = cursor.read_u32()?;
    let tool_version = cursor.read_string_u16()?;
    let count = cursor.read_u64()?;
    if count > MAX_MANIFEST_ENTRIES {
        // External review 023: this bounds a *count* of entries, not a byte length --
        // `CountOverBound`, not `ObjectOverBound`, so the refusal names what is actually bounded.
        return Err(PrikkError::CountOverBound {
            declared_what: "the archive manifest",
            unit: "sections",
            declared_count: count,
            bound_count: MAX_MANIFEST_ENTRIES,
        });
    }
    let mut entries = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for _ in 0..count {
        let code = cursor.read_u16()?;
        let offset = cursor.read_u64()?;
        let length = cursor.read_u64()?;
        let checksum = cursor.read_array::<32>()?;
        let kind = ArchiveSectionKind::from_wire_code(code).ok_or_else(|| {
            PrikkError::MalformedData(format!(
                "archive manifest names unknown section kind {code} -- refusing the whole archive"
            ))
        })?;
        entries.push(ManifestEntry {
            kind,
            offset,
            length,
            checksum,
        });
    }
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in archive manifest".to_string(),
        ));
    }
    Ok(ManifestView {
        repository_format,
        tool_version,
        entries,
    })
}

/// The handoff's own security section: *"An unknown, duplicated or missing section is a refusal
/// naming it."* Checked in full before any section body is read.
/// `total_len` (the real file length, from a plain `seek(End(0))` -- no allocation) must equal the
/// manifest's own declared end plus the fixed trailer. This is what makes every entry's own
/// `length` a bounded quantity rather than an arbitrary `u64` from the wire: entries are checked
/// below to be contiguous from byte 8 up to `manifest_offset`, so no entry's `length` can exceed
/// `total_len` once this holds -- the bound every `vec![0u8; n]` allocation sized by a section's own
/// `length` elsewhere in this module relies on (DC-86 discipline, checked once, here, rather than
/// re-derived at each allocation site).
fn validate_manifest_shape(
    manifest: &ManifestView,
    manifest_offset: u64,
    manifest_length: u64,
    total_len: u64,
) -> Result<()> {
    // V2's own fix (review `rfc155-impl-V-review-v1` finding V1b): the manifest checksum (checked
    // just before this call, in `read_manifest`) only proves the bytes are the ones a real export
    // wrote -- not that *this build* knows how to read the format number carried inside them. A
    // future build may legitimately write a format this one predates.
    if !super::known_repository_format_numbers().contains(&manifest.repository_format) {
        return Err(PrikkError::MalformedData(format!(
            "archive declares repository format {}, which this build does not know how to read \
             -- refusing the whole archive",
            manifest.repository_format
        )));
    }
    let claimed_end = manifest_offset
        .checked_add(manifest_length)
        .and_then(|sum| sum.checked_add(END_TRAILER_LEN_USIZE as u64))
        .ok_or_else(|| PrikkError::Integrity("archive manifest end overflow".to_string()))?;
    if claimed_end != total_len {
        return Err(PrikkError::MalformedData(format!(
            "archive manifest's own declared end ({claimed_end} bytes) does not match the \
             archive's real length ({total_len} bytes) -- refusing the whole archive"
        )));
    }
    let required: Vec<ArchiveSectionKind> = persisted_object_types()
        .into_iter()
        .map(ArchiveSectionKind::ObjectContainer)
        .chain([
            ArchiveSectionKind::RefLog,
            ArchiveSectionKind::RefPointerIndex,
            ArchiveSectionKind::ReceivedIndex,
            ArchiveSectionKind::TrustPolicy,
            ArchiveSectionKind::MaintainerKeys,
            ArchiveSectionKind::AuthorKeys,
        ])
        .collect();

    let mut seen: HashSet<u16> = HashSet::new();
    for entry in &manifest.entries {
        let code = entry.kind.wire_code();
        if !seen.insert(code) {
            return Err(PrikkError::MalformedData(format!(
                "archive manifest carries its {} section twice -- refusing the whole archive",
                entry.kind.label()
            )));
        }
    }
    for kind in &required {
        if !seen.contains(&kind.wire_code()) {
            return Err(PrikkError::MalformedData(format!(
                "archive manifest is missing its {} section -- refusing the whole archive",
                kind.label()
            )));
        }
    }
    if seen.len() != required.len() {
        return Err(PrikkError::MalformedData(
            "archive manifest carries a section kind outside the fixed allow-list -- refusing the \
             whole archive"
                .to_string(),
        ));
    }

    let mut expected_offset = 8u64; // the archive magic's own length
    for entry in &manifest.entries {
        if entry.offset != expected_offset {
            return Err(PrikkError::MalformedData(format!(
                "archive manifest's section layout disagrees with itself at byte offset \
                 {expected_offset} -- refusing the whole archive"
            )));
        }
        expected_offset = expected_offset
            .checked_add(super::section_total_len(entry.length)?)
            .ok_or_else(|| PrikkError::Integrity("archive section layout overflow".to_string()))?;
    }
    if expected_offset != manifest_offset {
        return Err(PrikkError::MalformedData(
            "archive manifest's own section layout does not reach the manifest's own declared \
             offset -- refusing the whole archive"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
