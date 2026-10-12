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

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

use prikk_error::{PrikkError, Result};
use prikk_hash::IncrementalSha256;
use prikk_object::{
    AttestationPayload, BlockPayload, ObjectEnvelope, ObjectId, ObjectType,
    RecognitionClaimPayload, RefStatePayload, SignerRole, TagPayload,
};

use crate::author::author_key_index::{
    AuthorKeyEntry, decode_author_key_records, verify_author_signatures_with,
};
use crate::bundle::DEFAULT_BUNDLE_MAX_OBJECT_BYTES;
use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::container::{
    CONTAINER_HEADER_LEN, CONTAINER_VERSION, container_magic, record_checksum,
};
use crate::foundation::file_codec::decode_envelope_file;
use crate::foundation::frame_resync::{
    BoundedScan, ScanBudget, resync_to_next_magic, scan_budget_exceeded_message,
};
use crate::foundation::layout::persisted_object_types;
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations,
};
use crate::trust::{
    AdoptedMaintainerKey, MaintainerTrustPolicy, verify_trusted_publication_envelope,
};
use crate::trust_index::{decode_trust_key_records, decode_trust_policy_records};

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
    /// Every object reference (a `Block`'s `patch_ids`/`parent_block_ids`/`snapshot_blob_ref`/
    /// `merge_baseline_block_id`, a `Patch`'s own blob references, a `Tag`/`Attestation`'s target, a
    /// `RefState`'s target/previous/required attestations, a `RecognitionClaim`'s own block/patch
    /// references) resolves to an id this archive itself carries.
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
    // this archive's own carried record) and `interrupted_appends` (§9.1: the source's own `verify`
    // only warns about one of these, so a backup of that repository must still restore).
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
            let outcome = for_each_object_record(source, entry, object_type, |body| {
                let envelope = decode_envelope_file(body)?;
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

    // Pass 2: reference closure and every signature, re-streaming each object-container section
    // against the now-complete id set (§9.2's second pass).
    let mut reference_closure_sound = true;
    let mut signatures_checked = 0usize;
    let mut unresolvable_signatures = Vec::new();
    let mut not_checked_signatures = Vec::new();
    for entry in &manifest.entries {
        let ArchiveSectionKind::ObjectContainer(object_type) = entry.kind else {
            continue;
        };
        for_each_object_record(source, entry, object_type, |body| {
            let Ok(envelope) = decode_envelope_file(body) else {
                // Already named by pass 1's own checksum/decode failure above; nothing new to add.
                return Ok(());
            };
            for referenced in extract_references(&envelope)? {
                if !ids.contains(&referenced) {
                    reference_closure_sound = false;
                    damaged.push(format!(
                        "{} {} references {referenced}, which this archive does not carry",
                        envelope.object_type,
                        envelope.object_id()
                    ));
                }
            }
            check_signatures(
                &envelope,
                &maintainer_policy,
                &author_material,
                &mut signatures_checked,
                &mut unresolvable_signatures,
                &mut not_checked_signatures,
            );
            Ok(())
        })?;
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

/// Every id one object references, by type -- the same fields `bundle.rs::export_bundle`'s own
/// closure walk already reads, just checked against an id set here instead of a live repository.
fn extract_references(envelope: &ObjectEnvelope) -> Result<Vec<ObjectId>> {
    let mut refs = Vec::new();
    match envelope.object_type {
        ObjectType::Block => {
            let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
            refs.extend(payload.parent_block_ids);
            refs.extend(payload.patch_ids);
            if let Some(blob) = payload.snapshot_blob_ref {
                refs.push(blob);
            }
            if let Some(baseline) = payload.merge_baseline_block_id {
                refs.push(baseline);
            }
        }
        ObjectType::Tag => {
            let payload = TagPayload::decode_canonical(&envelope.canonical_payload)?;
            refs.push(payload.target_block_id);
        }
        ObjectType::Attestation => {
            let payload = AttestationPayload::decode_canonical(&envelope.canonical_payload)?;
            refs.push(payload.target_block_id);
        }
        ObjectType::RefState => {
            let payload = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            refs.push(payload.target_object_id);
            if let Some(previous) = payload.previous_ref_state_id {
                refs.push(previous);
            }
            refs.extend(payload.required_attestation_ids);
        }
        ObjectType::RecognitionClaim => {
            let payload = RecognitionClaimPayload::decode_canonical(&envelope.canonical_payload)?;
            refs.push(payload.block_id);
            refs.extend(payload.patch_ids);
            refs.extend(payload.parent_block_ids);
        }
        ObjectType::Patch => {
            for operation in
                decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?
            {
                match operation.kind {
                    DecodedOperationKind::CreateFile { blob_id, .. } => refs.push(blob_id),
                    DecodedOperationKind::ReplaceBinary {
                        old_blob_id,
                        new_blob_id,
                        ..
                    } => {
                        refs.push(old_blob_id);
                        refs.push(new_blob_id);
                    }
                    DecodedOperationKind::DeleteNode {
                        preimage: DecodedDeletePreimage::File { old_blob_id, .. },
                        ..
                    } => refs.push(old_blob_id),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Ok(refs)
}

/// [`for_each_object_record`]'s own outcome: the outer checksum's own pass/fail; every **complete**
/// frame that failed (RFC 162 rule 2's own `complete: true` -- a full header and a full claimed
/// body were actually present, but the record failed its own checksum, or decoded to the wrong
/// envelope type, or the resync scan itself exhausted its budget), named by byte offset -- damage,
/// in [`SectionReadOutcome::record_damage`]; and every **incomplete** frame (rule 2's `complete:
/// false`) -- a crash tail, or bytes added since, including one later overrun by a sound frame --
/// named in [`SectionReadOutcome::interrupted_appends`], never damage on its own. The archive
/// classifies a section's frames by the same rule the store applies to a live container's own
/// bytes, held equal to it by a differential test (`section_frame_classification` test module)
/// rather than by a shared function call: `foundation::container`'s own decoder takes the whole
/// container as one in-memory slice, and its own resync scan's look-ahead is sized from that whole
/// slice's length -- calling it here would mean holding one section wholly in memory, which this
/// reader must never do. One shared, random-access-based primitive both paths could call literally
/// is future work; what is reused today, unchanged, is the primitive pieces that do not require
/// whole-buffer access: `ScanBudget`'s own accounting, `BoundedScan`'s own three outcomes,
/// `resync_to_next_magic` itself (called on bounded, overlapping windows rather than the whole
/// remainder), and `require_progress`'s own invariant.
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
/// `pub(super)`: RFC 155 Part I-A's own `archive::import` needs the identical decode -- not a
/// second copy of it -- for its own stage 1, over an archive its own stage 0 already confirmed
/// internally consistent via [`verify_archive`] (see [`ManifestView`]'s own doc note).
pub(super) fn for_each_object_record<R: Read + Seek>(
    source: &mut R,
    entry: &ManifestEntry,
    object_type: ObjectType,
    on_record: impl FnMut(&[u8]) -> Result<()>,
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

/// One streamed container-frame attempt's outcome -- the archive's own twin of
/// `foundation::container`'s private `FrameAttempt`, over a seekable source instead of an in-memory
/// slice, so a candidate's bytes are read only when actually needed, never pre-loaded.
enum StreamedFrameAttempt {
    Record { body: Vec<u8>, frame_len: usize },
    TrailingPartial,
    Invalid { message: String, complete: bool },
    ChecksumMismatch { message: String, body_end: usize },
}

/// Attempt to parse one container frame at `offset` (relative to the section's own record area,
/// which starts at `record_area_start` in `source`), mirroring `foundation::container::
/// parse_frame_at`'s own decision order exactly: magic/version, then the claimed body's own fit
/// within the section, then the DC-86 per-object bound, then the checksum, then the envelope decode
/// and its own type. Charges `budget` the same way, at the same point (right before hashing a
/// candidate's claimed body), so a candidate rejected before that point costs nothing, matching the
/// store's own accounting.
fn parse_frame_at_streamed<R: Read + Seek>(
    source: &mut R,
    magic: &[u8; 8],
    object_type: ObjectType,
    record_area_start: u64,
    section_len: usize,
    offset: usize,
    budget: &mut ScanBudget,
) -> Result<StreamedFrameAttempt> {
    let remaining = section_len.saturating_sub(offset);
    if remaining < CONTAINER_HEADER_LEN {
        return Ok(StreamedFrameAttempt::TrailingPartial);
    }
    source.seek(SeekFrom::Start(record_area_start + offset as u64))?;
    let mut header = [0u8; CONTAINER_HEADER_LEN];
    source.read_exact(&mut header)?;
    let record_magic: [u8; 8] = header[0..8].try_into().unwrap_or([0u8; 8]);
    let record_version = u16::from_be_bytes([header[8], header[9]]);
    if record_magic != *magic || record_version != CONTAINER_VERSION {
        return Ok(StreamedFrameAttempt::Invalid {
            message: format!("invalid container record magic or version at byte offset {offset}"),
            complete: false,
        });
    }
    let body_len_u64 = u64::from_be_bytes(header[10..18].try_into().unwrap_or([0u8; 8]));
    let checksum_declared: [u8; 32] = header[18..50].try_into().unwrap_or([0u8; 32]);
    // Past-the-end comes first, exactly as `parse_frame_at_reporting` orders it -- the DC-86 bound
    // below only ever applies to a claim already known to fit entirely within real, present
    // section bytes (I-E item 0(a): the pre-fix order checked the bound first, so a corrupted
    // length field read as a huge number resynced to the next *magic* and was named damage,
    // instead of resyncing to the next *sound* frame the way the store's own short-read path
    // already does). The two overflow arms below mirror `parse_frame_at_reporting`'s own
    // classification exactly (`Invalid`, not `TrailingPartial` -- a length that cannot even be
    // represented is not "a well-formed header claiming more than is left," the one shape
    // `TrailingPartial` is for).
    let Ok(body_len) = usize::try_from(body_len_u64) else {
        return Ok(StreamedFrameAttempt::Invalid {
            message: format!("container record length overflow at byte offset {offset}"),
            complete: false,
        });
    };
    let Some(body_end) = offset
        .checked_add(CONTAINER_HEADER_LEN)
        .and_then(|header_end| header_end.checked_add(body_len))
    else {
        return Ok(StreamedFrameAttempt::Invalid {
            message: format!("container body end overflow at byte offset {offset}"),
            complete: false,
        });
    };
    if body_end > section_len {
        // A well-formed header claiming more body than this section has left -- the hallmark of a
        // genuine crash tail (RFC 162 rule 2's own `TrailingPartial`). Incomplete, not damage.
        return Ok(StreamedFrameAttempt::TrailingPartial);
    }
    // The claim is now known to fit entirely within real, present section bytes -- no longer a
    // magic-byte coincidence pointing past what is actually there. DC-86: the archive is untrusted
    // input and must never allocate past its own per-object bound, so a claim this large, even
    // though it is genuinely in range, is refused outright (the same whole-operation refusal
    // `read_section_body_fully`/`read_manifest` already give an over-bound but structurally sound
    // declared length) -- never silently skipped as "maybe not a real record," because by this
    // point it demonstrably is one.
    if body_len_u64 > DEFAULT_BUNDLE_MAX_OBJECT_BYTES as u64 {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes: body_len_u64,
            bound_bytes: DEFAULT_BUNDLE_MAX_OBJECT_BYTES as u64,
        });
    }
    let mut body = vec![0u8; body_len];
    source.read_exact(&mut body)?;
    budget.charge((body.len() + CONTAINER_HEADER_LEN) as u64);
    let expected = record_checksum(magic, body_len_u64, &body);
    if expected != checksum_declared {
        return Ok(StreamedFrameAttempt::ChecksumMismatch {
            message: format!("container record at byte offset {offset} failed its own checksum"),
            body_end,
        });
    }
    let envelope = match decode_envelope_file(&body) {
        Ok(envelope) => envelope,
        Err(err) => {
            return Ok(StreamedFrameAttempt::Invalid {
                message: format!("container record at byte offset {offset}: {err}"),
                complete: true,
            });
        }
    };
    if envelope.object_type != object_type {
        return Ok(StreamedFrameAttempt::Invalid {
            message: format!(
                "container record at byte offset {offset} is under type {object_type} but \
                 envelope type is {}",
                envelope.object_type
            ),
            complete: true,
        });
    }
    Ok(StreamedFrameAttempt::Record {
        body,
        frame_len: CONTAINER_HEADER_LEN + body_len,
    })
}

/// The streamed twin of `foundation::frame_resync::resync_to_next_magic`, over a seekable source:
/// finds the next occurrence of `magic` at or after `start` (relative to the section's own record
/// area), reading bounded, overlapping windows rather than the whole remainder -- each window is
/// handed to the real `resync_to_next_magic` unchanged, so the actual byte-search algorithm is
/// reused, not reimplemented; only the I/O around it is streamed.
fn resync_to_next_magic_streamed<R: Read + Seek>(
    source: &mut R,
    record_area_start: u64,
    section_len: usize,
    start: usize,
    magic: &[u8; 8],
) -> Result<Option<usize>> {
    const WINDOW_BYTES: usize = 1024 * 1024;
    let overlap = magic.len().saturating_sub(1);
    let mut window_start = start;
    loop {
        if window_start >= section_len {
            return Ok(None);
        }
        let remaining = section_len - window_start;
        let read_len = remaining.min(WINDOW_BYTES);
        if read_len < magic.len() {
            return Ok(None);
        }
        source.seek(SeekFrom::Start(record_area_start + window_start as u64))?;
        let mut window = vec![0u8; read_len];
        source.read_exact(&mut window)?;
        if let Some(found) = resync_to_next_magic(&window, 0, magic.as_slice()) {
            return Ok(Some(window_start + found));
        }
        let advance = read_len.saturating_sub(overlap);
        if advance == 0 {
            return Ok(None);
        }
        window_start += advance;
    }
}

/// The streamed twin of `foundation::frame_resync::sound_frame_after_partial_budgeted`: scans
/// forward from `offset + 1` for a byte-offset where a genuinely sound frame starts, using the
/// streamed resync above to find each magic candidate and a full [`parse_frame_at_streamed`] to
/// confirm it. Reuses [`BoundedScan`] and [`ScanBudget`] unchanged, so a budget-exceeded candidate
/// resolves to [`BoundedScan::Undetermined`] by the identical rule the store's own version uses.
fn sound_frame_after_partial_streamed<R: Read + Seek>(
    source: &mut R,
    magic: &[u8; 8],
    object_type: ObjectType,
    record_area_start: u64,
    section_len: usize,
    offset: usize,
    budget: &mut ScanBudget,
) -> Result<BoundedScan> {
    let mut from = offset.saturating_add(1);
    loop {
        let Some(candidate) =
            resync_to_next_magic_streamed(source, record_area_start, section_len, from, magic)?
        else {
            return Ok(BoundedScan::NoSoundFrame);
        };
        if budget.exceeded() {
            return Ok(BoundedScan::Undetermined);
        }
        let sound = matches!(
            parse_frame_at_streamed(
                source,
                magic,
                object_type,
                record_area_start,
                section_len,
                candidate,
                budget
            )?,
            StreamedFrameAttempt::Record { .. }
        );
        if sound {
            return Ok(BoundedScan::Sound(candidate));
        }
        from = candidate.saturating_add(1);
    }
}

/// The streamed twin of `foundation::container::checksum_mismatch_is_complete` (RFC 162 rule 2, the
/// overrun rule): a complete frame whose checksum fails is damage, **unless** a later, sound frame
/// starts inside the range its own header claimed -- then the frame was torn, and a later write
/// genuinely finished past it (an interrupted append, not damage). `body_end == section_len` means
/// nothing can come after it in this section at all, so there is no overrun to check for.
#[allow(clippy::too_many_arguments)]
fn checksum_mismatch_is_complete_streamed<R: Read + Seek>(
    source: &mut R,
    magic: &[u8; 8],
    object_type: ObjectType,
    record_area_start: u64,
    section_len: usize,
    offset: usize,
    body_end: usize,
    budget: &mut ScanBudget,
) -> Result<bool> {
    if body_end == section_len {
        return Ok(true);
    }
    match sound_frame_after_partial_streamed(
        source,
        magic,
        object_type,
        record_area_start,
        section_len,
        offset,
        budget,
    )? {
        BoundedScan::Sound(next) => Ok(next >= body_end),
        BoundedScan::NoSoundFrame | BoundedScan::Undetermined => Ok(true),
    }
}

/// Classify every record in one section's own record area, calling `on_record` for each sound one.
/// After any frame that is not sound -- a short read, a bad magic/version, or a checksum mismatch --
/// scans forward for the next sound frame (mirroring `foundation::container::
/// decode_container_records`'s own loop exactly) and resumes decoding from there, so damage or an
/// interrupted append in one record never hides a sound record after it. §9.1's own rule: a
/// budget-exhausting resync scan is damage (RFC 167 C3), never a tail.
fn classify_section_records<R: Read + Seek>(
    source: &mut R,
    entry: &ManifestEntry,
    object_type: ObjectType,
    mut on_record: impl FnMut(&[u8]) -> Result<()>,
) -> Result<(Vec<String>, Vec<String>, u64)> {
    let magic = container_magic(object_type)?;
    let record_area_start = entry.offset + SECTION_HEADER_LEN as u64;
    let section_len = usize::try_from(entry.length)
        .map_err(|_| PrikkError::Integrity("archive section length overflow".to_string()))?;
    let mut budget = ScanBudget::for_input(section_len);
    let mut record_damage = Vec::new();
    let mut interrupted_appends = Vec::new();
    let mut tail_len: u64 = 0;
    let mut offset: usize = 0;
    while offset < section_len {
        if budget.exceeded() {
            // `decode_container_records`'s own top-of-loop budget check pushes
            // `Failed{complete: false}` (confirmed from source) -- an interrupted append here, not
            // damage, matching every other budget-exceeded case outside `checksum_mismatch_is_
            // complete`'s own, which is the one place `Undetermined` maps to `complete: true`.
            interrupted_appends.push(interrupted_append_message(scan_budget_exceeded_message(
                offset,
            )));
            break;
        }
        match parse_frame_at_streamed(
            source,
            magic,
            object_type,
            record_area_start,
            section_len,
            offset,
            &mut budget,
        )? {
            StreamedFrameAttempt::Record { body, frame_len } => {
                on_record(&body)?;
                let next = offset.checked_add(frame_len).ok_or_else(|| {
                    PrikkError::Integrity("archive section offset overflow".to_string())
                })?;
                if next <= offset {
                    return Err(PrikkError::Integrity(format!(
                        "archive section decode did not advance past byte offset {offset}"
                    )));
                }
                offset = next;
            }
            StreamedFrameAttempt::TrailingPartial => {
                match sound_frame_after_partial_streamed(
                    source,
                    magic,
                    object_type,
                    record_area_start,
                    section_len,
                    offset,
                    &mut budget,
                )? {
                    BoundedScan::NoSoundFrame => {
                        // Confirmed from source: `decode_container_records`'s own `TrailingPartial`
                        // arm's `NoSoundFrame` case returns *without* pushing any `record_outcomes`
                        // entry at all -- only `ContainerReplay::trailing_partial_bytes` is set.
                        // Tracked separately here (`tail_len`), never as a differential-test-facing
                        // `record_damage`/`interrupted_appends` entry, so those two stay held equal
                        // to `record_outcomes` exactly.
                        tail_len = (section_len - offset) as u64;
                        break;
                    }
                    BoundedScan::Undetermined => {
                        // Confirmed from source: `decode_container_records`'s own `TrailingPartial`
                        // arm pushes `complete: false` for its own `Undetermined` case too -- unlike
                        // `checksum_mismatch_is_complete`'s own, this is the one place `Undetermined`
                        // does *not* become damage. An interrupted append.
                        interrupted_appends.push(interrupted_append_message(
                            scan_budget_exceeded_message(offset),
                        ));
                        break;
                    }
                    BoundedScan::Sound(next) => {
                        // `foundation::container`'s own `ContainerRecordStatus::Failed{complete:
                        // false}` for this exact branch (confirmed from source,
                        // `decode_container_records`'s own `TrailingPartial` arm): the structured
                        // `complete` flag this format's own downstream classifier actually reads
                        // (`verify/objects.rs`: `if *complete { Failed } else { InterruptedAppend }`)
                        // is false here -- object containers report this as a warning, per RFC 164
                        // Rule E, and a live repository's own `prikk verify` agrees. External review
                        // 024's own finding: `frame_resync.rs`'s own `partial_before_sound_frame_
                        // message` is written for every isolate-and-continue *reader* this helper
                        // serves, several of which legitimately call this shape damage in their own
                        // context -- its own prose literally says "damage, not a torn tail", which
                        // leaked into the archive's own report even though the structured
                        // classification was already correct. The wording below never uses that
                        // word for an outcome a healthy backup's own source repository only warns
                        // about.
                        interrupted_appends.push(interrupted_append_message(format!(
                            "the frame at byte offset {offset} claims more bytes than remain, \
                             and a sound frame starts at byte offset {next}"
                        )));
                        if next <= offset {
                            return Err(PrikkError::Integrity(format!(
                                "archive section decode did not advance past byte offset {offset}"
                            )));
                        }
                        offset = next;
                    }
                }
            }
            StreamedFrameAttempt::Invalid { message, complete } => {
                // Confirmed from source: `decode_container_records`'s own `Invalid` arm always
                // pushes exactly this one `Failed{message, complete}` entry (at this frame's own
                // narrow offset), regardless of what resync finds afterward -- unlike
                // `TrailingPartial`'s own `NoSoundFrame` case, it never widens into a whole-span
                // "trailing bytes" claim, and `trailing_partial_bytes` stays 0 when resync finds
                // nothing further (confirmed: the `None` arm there returns `trailing_partial_
                // bytes: 0` explicitly, not `section_len - offset`).
                if complete {
                    record_damage.push(message);
                } else {
                    interrupted_appends.push(interrupted_append_message(message));
                }
                match resync_to_next_magic_streamed(
                    source,
                    record_area_start,
                    section_len,
                    offset + 1,
                    magic,
                )? {
                    Some(next) => offset = next,
                    None => break,
                }
            }
            StreamedFrameAttempt::ChecksumMismatch { message, body_end } => {
                let complete = checksum_mismatch_is_complete_streamed(
                    source,
                    magic,
                    object_type,
                    record_area_start,
                    section_len,
                    offset,
                    body_end,
                    &mut budget,
                )?;
                // Confirmed from source: `decode_container_records`'s own `ChecksumMismatch` arm
                // always pushes exactly this one `Failed{message, complete}` entry, the same as
                // `Invalid` above -- never widened into a whole-span claim when resync finds
                // nothing further (`trailing_partial_bytes` stays 0 there too).
                if complete {
                    record_damage.push(message);
                } else {
                    // I-E item 0(f): the checksum genuinely does not match this frame's own claimed
                    // bytes, but `checksum_mismatch_is_complete_streamed` has already confirmed a
                    // later, sound frame starts inside its own claimed range -- a real write
                    // finished past a torn one, exactly what the source repository's own `prikk
                    // verify` calls an interrupted append, never a checksum failure. Reusing
                    // `message`'s own "failed its own checksum" wording here would alarm a reader
                    // of a healthy backup's own report with language the source itself does not
                    // use for this outcome.
                    interrupted_appends.push(format!(
                        "interrupted append at byte offset {offset} -- a later write already \
                         completed past this frame's own claimed end; not imported"
                    ));
                }
                match resync_to_next_magic_streamed(
                    source,
                    record_area_start,
                    section_len,
                    offset + 1,
                    magic,
                )? {
                    Some(next) => offset = next,
                    None => break,
                }
            }
        }
    }
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
