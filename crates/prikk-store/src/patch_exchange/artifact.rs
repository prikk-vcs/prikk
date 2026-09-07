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

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, Signature, SignerRole};

use crate::author::author_key_index::{AuthorKeyEntry, lookup_author_key_entries};
use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_bytes_u64, push_u64,
};
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::layout::RepositoryLayout;
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

/// The artifact's decoded sections, before any of the accept path's own checks run. Purely
/// structural -- decoding this says nothing about whether the artifact's content is trustworthy or
/// even internally consistent; that is every later phase's job, not this one's.
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
pub fn export_exchange_artifact(
    layout: &RepositoryLayout,
    patch_ids: &[ObjectId],
    claim_ids: &[ObjectId],
    tag_ids: &[ObjectId],
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

    let mut blob_envelopes: Vec<ObjectEnvelope> = Vec::with_capacity(blob_ids.len());
    for blob_id in &blob_ids {
        blob_envelopes.push(read_required(&object_store, *blob_id, ObjectType::Blob)?);
    }

    // DC-53 Stage 2, D6's own reasoning, restated for this format: the author-key section's scope is
    // exactly the AUTHOR `key_id`s of the patches this artifact carries, never the whole local
    // `author_key_index` container -- exporting everything this repository has ever seen would leak
    // every author it has observed to every recipient, a disclosure the sender did not choose.
    let mut author_key_ids: BTreeSet<String> = BTreeSet::new();
    for envelope in &patch_envelopes {
        if let Some(signature) = envelope
            .signatures
            .iter()
            .find(|signature| signature.signer_role == SignerRole::Author)
        {
            author_key_ids.insert(signature.key_id.clone());
        }
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

/// Decode a `PEXCH002` artifact structurally. Bounds every declared count against
/// `max_object_count` at the point it is read, before the section it governs is looped over
/// (handoff §4.2 Phase A item 2: "a declared count over the limit must not cost more than reading
/// one integer to reject"). Performs no cross-section checks at all -- no digest recomputation, no
/// conflict checks, no signature verification, no closure check. Those are the accept path's own
/// Phase B/C, deliberately kept out of this function: decoding a shape and judging its content are
/// different questions, the same separation `decode_bundle` keeps from `import_bundle`.
pub(super) fn decode_exchange_artifact(
    bytes: &[u8],
    max_object_count: usize,
) -> Result<DecodedExchangeArtifact> {
    let mut cursor = ByteCursor::new(bytes);
    let magic = cursor.read_array::<8>()?;
    if &magic != EXCHANGE_ARTIFACT_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid patch-exchange artifact magic".to_string(),
        ));
    }
    let declared_digest = PatchSetDigest(cursor.read_array::<32>()?);

    let patches = decode_envelope_section(&mut cursor, max_object_count, "patches")?;
    let blobs = decode_envelope_section(&mut cursor, max_object_count, "blobs")?;

    let author_key_count = cursor.read_u64()?;
    if author_key_count > len_to_u64(max_object_count)? {
        return Err(PrikkError::MalformedData(format!(
            "patch-exchange artifact declares {author_key_count} author key entries, over the \
             configured limit of {max_object_count}"
        )));
    }
    let mut author_keys = Vec::new();
    for _ in 0..author_key_count {
        let key_id_bytes = cursor.read_bytes_u64()?;
        let key_id = String::from_utf8(key_id_bytes).map_err(|err| {
            PrikkError::MalformedData(format!(
                "invalid patch-exchange artifact author key_id utf-8: {err}"
            ))
        })?;
        Signature::validate_key_id(&key_id)?;
        let public_key = cursor.read_array::<32>()?;
        author_keys.push(AuthorKeyEntry { key_id, public_key });
    }

    let claims = decode_envelope_section(&mut cursor, max_object_count, "recognition claims")?;
    let tags = decode_envelope_section(&mut cursor, max_object_count, "tags")?;

    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in patch-exchange artifact".to_string(),
        ));
    }

    Ok(DecodedExchangeArtifact {
        declared_digest,
        patches,
        blobs,
        author_keys,
        claims,
        tags,
    })
}

fn decode_envelope_section(
    cursor: &mut ByteCursor<'_>,
    max_object_count: usize,
    section_name: &str,
) -> Result<Vec<ObjectEnvelope>> {
    let count = cursor.read_u64()?;
    if count > len_to_u64(max_object_count)? {
        return Err(PrikkError::MalformedData(format!(
            "patch-exchange artifact declares {count} {section_name}, over the configured limit \
             of {max_object_count}"
        )));
    }
    let mut envelopes = Vec::new();
    for _ in 0..count {
        let encoded = cursor.read_bytes_u64()?;
        envelopes.push(decode_envelope_file(&encoded)?);
    }
    Ok(envelopes)
}

#[cfg(test)]
mod tests;
