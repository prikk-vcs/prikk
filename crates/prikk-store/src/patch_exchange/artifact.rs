//! RFC 115 Stage 3 handoff §3: the `PEXCH002` artifact format.
//!
//! **Representational, not frozen** (RFC 114 §3, restated from the parent module doc): this format
//! carries objects whose identity is already frozen and carries none of its own. `PEXCH002` is
//! emitted on export and accepted on import.
//!
//! **RFC 117 stage 3 §2: `PEXCH001` -> `PEXCH002`, a format revision, and `PEXCH001` is refused, not
//! migrated.** The artifact is transient, in-flight data -- it exists only between one `sync build`
//! and the matching `sync accept`, never stored as repository history -- so under the standing "no
//! production data exists yet" ruling (RFC 117 T5) there is nothing in flight worth preserving across
//! the bump. A `PEXCH001` byte stream simply fails the magic check below and is refused outright, the
//! same as any other malformed input; there is no reader for the old magic and none is added.
//!
//! **Six sections, in order** (handoff §3, extended by stage 3 §2): (1) the declared patch-set
//! digest, 32 fixed bytes, no length prefix; (2) the ordered Patch list, in the sender's own
//! application order (D1) -- order is artifact metadata, never part of any object's identity; (3)
//! blobs -- every blob any carried patch references; (4) author key material, the same
//! `key_id -> public_key` shape `bundle.rs`'s own author-key section carries; (5) recognition claims,
//! may be empty; (6) **Tag objects (new), may be empty** -- every tag whose `target_block_id` lies
//! within the ancestry of the ref being synced (`sender.rs` decides which; this module only carries
//! what it is given, the same separation it already keeps for claims). A carried Tag object is
//! reported on accept, never adopted (RFC 117 T4) -- adoption is a separate, explicit, receiver-signed
//! act (`tag_travel.rs`), not something this format or the accept path performs. Modelled directly on
//! `bundle.rs`'s `encode_bundle`/`decode_bundle` -- the handoff's own instruction was to read that
//! module in full before designing this one, and to inherit its shape rather than re-earn its two
//! past security defects (declared-count bounds checked before the loop that trusts them; an
//! internal author-key conflict refused before anything is recorded).
//!
//! Every declared count (patches, blobs, author keys, claims, tags) is checked against the caller's
//! `max_object_count` ceiling at the moment it is read, before the section's loop runs -- DC-86's
//! rule, applied five times over rather than once, the same way `decode_bundle` applies it to both
//! its own declared counts with one shared ceiling.

use std::collections::BTreeSet;
use std::io::{Seek, SeekFrom};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, Signature, SignerRole};

use crate::author::author_key_index::{AuthorKeyEntry, lookup_author_key_entries};
#[cfg(test)]
use crate::foundation::file_codec::decode_envelope_file;
use crate::foundation::file_codec::{encode_envelope_file, push_bytes_u64, push_u64};
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::layout::RepositoryLayout;
#[cfg(test)]
use crate::framed_source::read_frame_body_from;
use crate::framed_source::{
    BundleSource, read_array_from, read_bytes_u64_from, read_u64_from, skip_object_frames,
    stream_position_usize, u64_to_usize,
};
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations,
};
use crate::patch_set_digest::{PatchSetDigest, compute_patch_set_digest};

/// `PEXCH002`: patch-exchange artifact, format version 2 (RFC 117 stage 3: adds the Tag section).
/// See the module doc. `PEXCH001` is refused, not read -- there is no dual-reader here.
const EXCHANGE_ARTIFACT_MAGIC: &[u8; 8] = b"PEXCH002";

/// DC-86 default hard block on each of the artifact's declared counts (five as of stage 3's Tag
/// section), checked as early as the
/// format allows -- mirroring `DEFAULT_BUNDLE_MAX_OBJECT_COUNT`'s reasoning exactly, restated here
/// because this is a distinct format with its own ceiling, not a reuse of the bundle-specific one.
pub const DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT: usize = 100_000;

/// DC-86 default hard block on the artifact's total encoded byte length, checked before any decoding
/// begins -- the same tight, cheap proxy `DEFAULT_BUNDLE_MAX_TOTAL_BYTES` uses, restated for this
/// format. 256 MiB.
pub const DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// RFC 158 Stage A §3: the default per-object bound, equal to the total default -- the same
/// "inert at the defaults" reasoning [`crate::bundle::DEFAULT_BUNDLE_MAX_OBJECT_BYTES`] states,
/// restated here because this is a distinct format with its own ceiling.
pub const DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_BYTES: usize =
    DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES;

/// Summary of an exchange-artifact export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeExportReport {
    /// Patches carried, in the sender's application order.
    pub patch_count: usize,
    /// Blobs carried -- every blob any carried patch's operations reference.
    pub blob_count: usize,
    /// AUTHOR key entries carried -- one per distinct `key_id` among the carried patches for which
    /// this repository has local material, mirroring `BundleExportReport::author_key_count`.
    pub author_key_count: usize,
    /// Recognition claims carried.
    pub claim_count: usize,
    /// Tag objects carried (RFC 117 stage 3 §2).
    pub tag_count: usize,
}

/// The artifact's decoded sections, before any of the accept path's own checks run. **Test fixture code only** (0.58.0 P3b): the tests take
/// an artifact apart with it to build a mutated one. `accept` never calls it -- it reads the artifact a frame at a time through
/// [`read_exchange_layout`] and never holds the decoded set.
#[cfg(test)]
#[derive(Debug)]
pub(super) struct DecodedExchangeArtifact {
    pub(super) declared_digest: PatchSetDigest,
    pub(super) patches: Vec<ObjectEnvelope>,
    pub(super) blobs: Vec<ObjectEnvelope>,
    pub(super) author_keys: Vec<AuthorKeyEntry>,
    pub(super) claims: Vec<ObjectEnvelope>,
    pub(super) tags: Vec<ObjectEnvelope>,
}

/// Export a `PEXCH002` artifact carrying exactly `patch_ids`, in the given order (the sender's
/// application order, D1 -- this function does not reorder them), plus every blob those patches'
/// operations reference, plus local AUTHOR key material for their signers, plus the
/// already-persisted `RecognitionClaim` objects named by `claim_ids`, plus the already-persisted
/// `Tag` objects named by `tag_ids` (RFC 117 stage 3 §2). `patch_ids` must not contain a duplicate;
/// the caller decides the exact identity and order of what it sends, and a repeated id would make
/// the declared count and the ordered list disagree about how many patches this artifact actually
/// carries. **Which tags belong in `tag_ids` is the caller's decision** (`sender.rs`'s own ancestry
/// filter) -- this function carries whatever it is given, the same separation it already keeps for
/// `claim_ids`.
/// `tip_block_id` is the ref's own tip, used only to derive a deletion preimage the store lacks
/// (DC-78 v2, shared with `bundle.rs` through `patch_replay::derive_deleted_content`). `None` derives
/// nothing, and a missing Blob then fails exactly as it did before.
pub fn export_exchange_artifact(
    layout: &RepositoryLayout,
    patch_ids: &[ObjectId],
    claim_ids: &[ObjectId],
    tag_ids: &[ObjectId],
    tip_block_id: Option<ObjectId>,
) -> Result<(ExchangeExportReport, Vec<u8>)> {
    let mut seen_patch_ids: BTreeSet<ObjectId> = BTreeSet::new();
    for patch_id in patch_ids {
        if !seen_patch_ids.insert(*patch_id) {
            return Err(PrikkError::Integrity(format!(
                "patch id {patch_id} is named more than once in the export list"
            )));
        }
    }

    let object_store = ObjectReadSnapshot::open(layout)?;
    let mut patch_envelopes: Vec<ObjectEnvelope> = Vec::with_capacity(patch_ids.len());
    let mut blob_ids: BTreeSet<ObjectId> = BTreeSet::new();
    for patch_id in patch_ids {
        let envelope = read_required(&object_store, *patch_id, ObjectType::Patch)?;
        // Mirrors `bundle.rs`'s own operation scan (`export_bundle`, DC-78 §D6): a Patch's
        // operations can reference Blobs independently of any Block's `snapshot_blob_ref`, so the
        // closure has to be derived from the operations themselves, not assumed from ancestry --
        // this artifact carries no Block at all to derive it from in the first place.
        for operation in
            decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?
        {
            match operation.kind {
                DecodedOperationKind::CreateFile { blob_id, .. } => {
                    blob_ids.insert(blob_id);
                }
                DecodedOperationKind::ReplaceBinary {
                    old_blob_id,
                    new_blob_id,
                    ..
                } => {
                    blob_ids.insert(old_blob_id);
                    blob_ids.insert(new_blob_id);
                }
                DecodedOperationKind::DeleteNode {
                    preimage: DecodedDeletePreimage::File { old_blob_id, .. },
                    ..
                } => {
                    blob_ids.insert(old_blob_id);
                }
                _ => {}
            }
        }
        patch_envelopes.push(envelope);
    }

    // DC-78 v2: same rule as `bundle.rs` -- carry a deletion's preimage Blob, deriving it by replay
    // when nothing stored it (an `EditText`-only text file, DC-65). Read-only: nothing is written here.
    // `has_object` answers from the index rather than reading each Blob (review v2, finding 3).
    let mut missing: BTreeSet<ObjectId> = BTreeSet::new();
    for blob_id in &blob_ids {
        if !object_store.has_object(*blob_id, ObjectType::Blob)? {
            missing.insert(*blob_id);
        }
    }
    let mut derived = match tip_block_id {
        Some(tip) => crate::patch_replay::derive_deleted_content(&object_store, tip, &missing)?,
        None => crate::patch_replay::DerivedDeletedContent::default(),
    };
    let mut blob_envelopes: Vec<ObjectEnvelope> = Vec::with_capacity(blob_ids.len());
    for blob_id in &blob_ids {
        match derived.found.remove(blob_id) {
            Some((node_kind, bytes)) => {
                blob_envelopes.push(crate::blob_access::blob_envelope_for_kind(
                    bytes, node_kind,
                )?);
            }
            None => blob_envelopes.push(read_required(&object_store, *blob_id, ObjectType::Blob)?),
        }
    }

    // DC-53 Stage 2, D6's own reasoning, restated for this format: the author-key section's scope is
    // exactly the AUTHOR `key_id`s of the patches this artifact carries, never the whole local
    // `author_key_index` container -- exporting everything this repository has ever seen would leak
    // every author it has observed to every recipient, a disclosure the sender did not choose.
    let mut author_key_ids: BTreeSet<String> = BTreeSet::new();
    for envelope in &patch_envelopes {
        // RFC 156 Stage 1: every AUTHOR signer's material, so a receiver can check every signature —
        // carrying only the first signer's would leave the others unverifiable on arrival.
        author_key_ids.extend(
            envelope
                .signatures
                .iter()
                .filter(|signature| signature.signer_role == SignerRole::Author)
                .map(|signature| signature.key_id.clone()),
        );
    }
    let mut author_keys: Vec<AuthorKeyEntry> = Vec::with_capacity(author_key_ids.len());
    for key_id in &author_key_ids {
        let entries = lookup_author_key_entries(layout, key_id)?;
        let mut distinct = entries.iter().map(|entry| entry.public_key);
        let Some(first) = distinct.next() else {
            // No local material for this key_id -- omitted, not an error (material is optional
            // per-author, the same rule `bundle.rs`'s own export follows).
            continue;
        };
        if distinct.any(|public_key| public_key != first) {
            return Err(PrikkError::Integrity(format!(
                "author key_id {key_id} has more than one distinct recorded public key locally; \
                 refusing to export a provenance claim this repository's own material does not \
                 agree on"
            )));
        }
        author_keys.push(AuthorKeyEntry {
            key_id: key_id.clone(),
            public_key: first,
        });
    }

    let mut claim_envelopes: Vec<ObjectEnvelope> = Vec::with_capacity(claim_ids.len());
    for claim_id in claim_ids {
        claim_envelopes.push(read_required(
            &object_store,
            *claim_id,
            ObjectType::RecognitionClaim,
        )?);
    }

    let mut tag_envelopes: Vec<ObjectEnvelope> = Vec::with_capacity(tag_ids.len());
    for tag_id in tag_ids {
        tag_envelopes.push(read_required(&object_store, *tag_id, ObjectType::Tag)?);
    }

    let report = ExchangeExportReport {
        patch_count: patch_envelopes.len(),
        blob_count: blob_envelopes.len(),
        author_key_count: author_keys.len(),
        claim_count: claim_envelopes.len(),
        tag_count: tag_envelopes.len(),
    };
    let bytes = encode_exchange_artifact(
        patch_ids,
        &patch_envelopes,
        &blob_envelopes,
        &author_keys,
        &claim_envelopes,
        &tag_envelopes,
    )?;
    Ok((report, bytes))
}

fn read_required(
    object_store: &impl ObjectReader,
    id: ObjectId,
    object_type: ObjectType,
) -> Result<ObjectEnvelope> {
    object_store
        .read_typed(id, object_type)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing {object_type} object: {id}")))
}

fn encode_exchange_artifact(
    patch_ids: &[ObjectId],
    patch_envelopes: &[ObjectEnvelope],
    blob_envelopes: &[ObjectEnvelope],
    author_keys: &[AuthorKeyEntry],
    claim_envelopes: &[ObjectEnvelope],
    tag_envelopes: &[ObjectEnvelope],
) -> Result<Vec<u8>> {
    let mut sorted_patch_ids: Vec<ObjectId> = patch_ids.to_vec();
    sorted_patch_ids.sort_unstable();
    sorted_patch_ids.dedup();
    let digest = compute_patch_set_digest(&sorted_patch_ids)?;

    let mut out = Vec::new();
    out.extend_from_slice(EXCHANGE_ARTIFACT_MAGIC);
    // The declared digest: 32 fixed bytes, no length prefix -- its length is part of the format, not
    // data the format needs to carry about itself.
    out.extend_from_slice(&digest.0);

    push_u64(&mut out, len_to_u64(patch_envelopes.len())?);
    for envelope in patch_envelopes {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, len_to_u64(blob_envelopes.len())?);
    for envelope in blob_envelopes {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, len_to_u64(author_keys.len())?);
    for entry in author_keys {
        push_bytes_u64(&mut out, entry.key_id.as_bytes())?;
        out.extend_from_slice(&entry.public_key);
    }
    push_u64(&mut out, len_to_u64(claim_envelopes.len())?);
    for envelope in claim_envelopes {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, len_to_u64(tag_envelopes.len())?);
    for envelope in tag_envelopes {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    Ok(out)
}

/// Where one section's frames start in the artifact, and how many there are.
#[derive(Debug, Clone, Copy)]
pub(super) struct SectionSpan {
    pub(super) count: u64,
    /// The offset of the section's first frame (just after its count).
    pub(super) start: u64,
}

/// What a walk over the artifact's *structure* found, without reading a single object's body: the declared digest, where each of the four
/// frame sections starts and how many frames it holds, and the author-key section (which sits **between** the blobs and the claims, but is
/// needed before the first patch's signature can be checked -- the same order problem `bundle import` has, solved the same way:
/// [`skip_object_frames`] seeks over each frame by its length prefix, so this costs one 8-byte read per frame and transfers no content).
#[derive(Debug)]
pub(super) struct ExchangeLayout {
    pub(super) declared_digest: PatchSetDigest,
    pub(super) patches: SectionSpan,
    pub(super) blobs: SectionSpan,
    pub(super) author_keys: Vec<AuthorKeyEntry>,
    pub(super) claims: SectionSpan,
    pub(super) tags: SectionSpan,
}

/// A `bundle`-worded refusal from the shared frame reader, said of this format instead: the reader's messages name the bundle, and a
/// patch-exchange artifact that ends early or declares an impossible field is not one.
pub(super) fn in_artifact<T>(result: Result<T>) -> Result<T> {
    result.map_err(|err| match err {
        PrikkError::MalformedData(message) => PrikkError::MalformedData(
            message
                .replace(
                    "unexpected end of bundle",
                    "unexpected end of patch-exchange artifact",
                )
                .replace("bundle declares", "patch-exchange artifact declares")
                .replace(
                    "the bundle's own declared",
                    "the patch-exchange artifact's own declared",
                ),
        ),
        other => other,
    })
}

/// Walk a `PEXCH002` artifact's structure. Bounds every declared count against `max_object_count` at the moment it is read, before the
/// section it governs is walked (handoff §4.2 Phase A item 2: "a declared count over the limit must not cost more than reading one integer
/// to reject"), and every frame's length against `max_object_bytes` on its length prefix. Performs no cross-section check at all -- no
/// digest, no conflict, no signature, no closure: those are the accept path's own phases, kept out of here as `decode_bundle` keeps them
/// out of `import_bundle`.
pub(super) fn read_exchange_layout(
    source: &mut BundleSource,
    total_len: u64,
    max_object_count: usize,
    max_object_bytes: usize,
) -> Result<ExchangeLayout> {
    let total = u64_to_usize(total_len)?;
    source.seek(SeekFrom::Start(0)).map_err(|err| {
        PrikkError::Integrity(format!(
            "patch-exchange artifact: failed to rewind the input: {err}"
        ))
    })?;
    let magic = in_artifact(read_array_from::<8>(source))?;
    if &magic != EXCHANGE_ARTIFACT_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid patch-exchange artifact magic".to_string(),
        ));
    }
    let declared_digest = PatchSetDigest(in_artifact(read_array_from::<32>(source))?);

    let mut section = |name: &str| -> Result<SectionSpan> {
        let count = in_artifact(read_u64_from(source))?;
        if count > len_to_u64(max_object_count)? {
            return Err(PrikkError::MalformedData(format!(
                "patch-exchange artifact declares {count} {name}, over the configured limit \
                 of {max_object_count}"
            )));
        }
        let start = in_artifact(stream_position_usize(source))? as u64;
        in_artifact(skip_object_frames(source, count, max_object_bytes))?;
        Ok(SectionSpan { count, start })
    };
    let patches = section("patches")?;
    let blobs = section("blobs")?;

    let author_key_count = in_artifact(read_u64_from(source))?;
    if author_key_count > len_to_u64(max_object_count)? {
        return Err(PrikkError::MalformedData(format!(
            "patch-exchange artifact declares {author_key_count} author key entries, over the \
             configured limit of {max_object_count}"
        )));
    }
    let mut author_keys = Vec::new();
    for _ in 0..author_key_count {
        let key_id_bytes = in_artifact(read_bytes_u64_from(source, total))?;
        let key_id = String::from_utf8(key_id_bytes).map_err(|err| {
            PrikkError::MalformedData(format!(
                "invalid patch-exchange artifact author key_id utf-8: {err}"
            ))
        })?;
        Signature::validate_key_id(&key_id)?;
        let public_key = in_artifact(read_array_from::<32>(source))?;
        author_keys.push(AuthorKeyEntry { key_id, public_key });
    }

    let claims = {
        let count = in_artifact(read_u64_from(source))?;
        if count > len_to_u64(max_object_count)? {
            return Err(PrikkError::MalformedData(format!(
                "patch-exchange artifact declares {count} recognition claims, over the configured limit \
                 of {max_object_count}"
            )));
        }
        let start = in_artifact(stream_position_usize(source))? as u64;
        in_artifact(skip_object_frames(source, count, max_object_bytes))?;
        SectionSpan { count, start }
    };
    let tags = {
        let count = in_artifact(read_u64_from(source))?;
        if count > len_to_u64(max_object_count)? {
            return Err(PrikkError::MalformedData(format!(
                "patch-exchange artifact declares {count} tags, over the configured limit \
                 of {max_object_count}"
            )));
        }
        let start = in_artifact(stream_position_usize(source))? as u64;
        in_artifact(skip_object_frames(source, count, max_object_bytes))?;
        SectionSpan { count, start }
    };

    if stream_position_usize(source)? as u64 != total_len {
        return Err(PrikkError::MalformedData(
            "trailing bytes in patch-exchange artifact".to_string(),
        ));
    }
    Ok(ExchangeLayout {
        declared_digest,
        patches,
        blobs,
        author_keys,
        claims,
        tags,
    })
}

#[cfg(test)]
thread_local! {
    static WHOLE_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `cfg(test)`: how many times this thread took an artifact apart whole. `accept` must never be one of them.
#[cfg(test)]
pub(super) fn whole_decodes_for_test() -> usize {
    WHOLE_DECODES.with(std::cell::Cell::get)
}

/// Test fixture code: decode a whole artifact into owned sections, from the same per-frame reader `accept` uses
/// ([`read_exchange_layout`], then [`read_frame_body_from`] and `decode_envelope_file` per frame). **Not reachable from `accept`**, and
/// compiled only for tests.
#[cfg(test)]
pub(super) fn decode_exchange_artifact(
    bytes: &[u8],
    max_object_count: usize,
    max_object_bytes: usize,
) -> Result<DecodedExchangeArtifact> {
    WHOLE_DECODES.with(|calls| calls.set(calls.get() + 1));
    let mut source = BundleSource::Buffered(std::io::Cursor::new(bytes.to_vec()));
    let layout = read_exchange_layout(
        &mut source,
        len_to_u64(bytes.len())?,
        max_object_count,
        max_object_bytes,
    )?;
    let mut read_section = |span: SectionSpan| -> Result<Vec<ObjectEnvelope>> {
        source
            .seek(SeekFrom::Start(span.start))
            .map_err(|err| PrikkError::Integrity(format!("patch-exchange artifact: {err}")))?;
        let mut envelopes = Vec::new();
        for _ in 0..span.count {
            let (_, body) = read_frame_body_from(&mut source, max_object_bytes)?;
            envelopes.push(decode_envelope_file(&body)?);
        }
        Ok(envelopes)
    };
    Ok(DecodedExchangeArtifact {
        declared_digest: layout.declared_digest,
        patches: read_section(layout.patches)?,
        blobs: read_section(layout.blobs)?,
        author_keys: layout.author_keys,
        claims: read_section(layout.claims)?,
        tags: read_section(layout.tags)?,
    })
}

#[cfg(test)]
mod tests;
