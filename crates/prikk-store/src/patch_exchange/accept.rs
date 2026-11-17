//! RFC 115 Stage 3 handoff §4: the accept path -- **streamed since 0.58.0 P3**: two reads of the artifact, one frame in memory at a time.
//!
//! RFC 115 Stage 3 handoff §4: the accept path. Implements Phase A-D in exactly the handoff's own
//! order -- see each phase's comment below for the corresponding numbered item. §4.1's invariant
//! governs the whole shape: **a refused exchange leaves nothing behind.** Every check that can fail
//! runs before any write; Phase D is reached only once nothing earlier could still refuse for a
//! reason attributable to the artifact's content.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Seek, SeekFrom};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, RecognitionClaimPayload};

use crate::author::author_key_index::{
    AuthorKeyEntry, check_author_key_conflict, lookup_author_key_entries,
    record_author_key_material, verify_author_signatures_with,
};
use crate::bundle::{
    AdmissionOutcomeKind, BundleSource, CarriedSignatureAdmitter, PreparedObject,
    dependency_order_indices, prepared_matches_existing,
};
use crate::foundation::file_codec::decode_envelope_file;
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryLayout};
use crate::framed_source::{read_exact_from, read_frame_body_from, stream_position_usize};
use crate::lock::{ActiveLock, acquire_container_locks};
use crate::object_store::{ObjectReadSnapshot, ObjectWriteSession};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations, decode_patch_parent_ids,
};
use crate::patch_set_digest::compute_patch_set_digest;
use crate::recognition_claim::{check_recognition_claim_consistency, verify_claim_signature};
use crate::tag_travel::verify_tag_signature;
use crate::trust::{MaintainerTrustPolicy, load_maintainer_trust_policy_or_empty};
use crate::verify::AuthorSignatureVerification;

pub use crate::recognition_claim::ClaimSignatureVerification;
pub use crate::tag_travel::TagSignatureVerification;

use super::artifact::{
    DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_BYTES, DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT,
    DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES, in_artifact, read_exchange_layout,
};

/// DC-86 resource bound for [`accept_exchange_artifact`], checked before any decode or write --
/// the same shape `BundleImportOptions` gives `import_bundle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptOptions {
    /// Maximum any one of the artifact's four declared counts (patches, blobs, author keys, claims)
    /// may be. Refused before the section it governs is decoded.
    pub max_object_count: usize,
    /// Maximum encoded byte length the artifact may have. Refused before decoding starts at all.
    pub max_total_bytes: usize,
    /// RFC 158 Stage A §3: maximum encoded byte length any *one* object frame (a patch, blob,
    /// claim, or tag) may have. Refused on the length prefix, before that frame is copied or
    /// decoded.
    pub max_object_bytes: usize,
}

impl AcceptOptions {
    /// [`DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT`], [`DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES`],
    /// and [`DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_BYTES`].
    #[must_use]
    pub const fn default_limits() -> Self {
        Self {
            max_object_count: DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT,
            max_total_bytes: DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES,
            max_object_bytes: DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_BYTES,
        }
    }

    /// Override the maximum declared count.
    #[must_use]
    pub const fn with_max_object_count(mut self, max_object_count: usize) -> Self {
        self.max_object_count = max_object_count;
        self
    }

    /// Override the maximum total encoded byte length.
    #[must_use]
    pub const fn with_max_total_bytes(mut self, max_total_bytes: usize) -> Self {
        self.max_total_bytes = max_total_bytes;
        self
    }

    /// Override the maximum per-object encoded byte length.
    #[must_use]
    pub const fn with_max_object_bytes(mut self, max_object_bytes: usize) -> Self {
        self.max_object_bytes = max_object_bytes;
        self
    }
}

/// Summary of an exchange-artifact accept.
///
/// `#[non_exhaustive]` from RFC 156, which added two fields: the next one is then not a break.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptReport {
    /// Patches the artifact carried.
    pub patch_count: usize,
    /// Blobs the artifact carried.
    pub blob_count: usize,
    /// Recognition claims the artifact carried.
    pub claim_count: usize,
    /// Tag objects the artifact carried (RFC 117 stage 3 §2/§3).
    pub tag_count: usize,
    /// Patch, blob, claim, and tag objects that did not already exist in this repository's object store
    /// before this accept -- content-addressed, so a replayed accept (§4.3) reports zero here.
    pub written_object_count: usize,
    /// RFC 156, format 7: objects already held here that gained at least one signature from the
    /// artifact's copy, written as a superseding record.
    pub merged_object_count: usize,
    /// RFC 156 §4: signatures the artifact carried for objects already held here that were not stored.
    pub dropped_signatures: Vec<crate::DroppedSignature>,
    /// AUTHOR key entries the artifact carried and this accept recorded locally. Zero on a replayed
    /// accept, the same continuity-only semantics `BundleImportReport::recorded_author_key_count`
    /// already has.
    pub recorded_author_key_count: usize,
    /// One outcome per carried patch that has an AUTHOR-role signature at all; a patch with none is
    /// simply absent from this list, mirroring `RepositoryVerification`'s own
    /// `author_verification: Option<..>` precedent.
    pub author_signature_outcomes: Vec<(ObjectId, AuthorSignatureVerification)>,
    /// One outcome per carried claim's own MAINTAINER signature.
    pub claim_signature_outcomes: Vec<(ObjectId, ClaimSignatureVerification)>,
    /// One outcome per carried tag's own MAINTAINER signature (RFC 117 stage 3 §3) -- reported,
    /// never gating, the same treatment `claim_signature_outcomes` gets.
    pub tag_signature_outcomes: Vec<(ObjectId, TagSignatureVerification)>,
}

/// Accept a `PEXCH002` exchange artifact held in memory (handoff §4). A thin wrapper over [`accept_exchange_artifact_streaming`] for a caller
/// that already has the bytes (the tests, a library user): it wraps them in the same seekable source a file is, so there is one accept path.
/// The CLI does **not** use it -- it hands the open file over and never holds the artifact.
pub fn accept_exchange_artifact(
    layout: &RepositoryLayout,
    bytes: &[u8],
    options: &AcceptOptions,
) -> Result<AcceptReport> {
    let mut source = BundleSource::Buffered(Cursor::new(bytes.to_vec()));
    accept_exchange_artifact_streaming(layout, &mut source, options)
}

/// What checking one patch's AUTHOR signatures found: the first signer's key id and whether it verified, or `None` for a patch with none.
type AuthorSignatureResult = Result<Option<(String, bool)>>;

/// Which of the artifact's four frame sections a frame came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Patches,
    Blobs,
    Claims,
    Tags,
}

/// Accept a `PEXCH002` exchange artifact (handoff §4). Writes patches, blobs, recognition claims,
/// and Tag objects (RFC 117 stage 3 §3) -- and records AUTHOR key material -- **only** once every
/// fallible check has already passed. Never touches a ref, a Block, or the received namespace: this
/// is patch-level exchange (§0's "the unit is the patch"), and Stage 3 does not extend the accept
/// path into sealing (§1's scope cut) or into tag adoption (RFC 117 T4 -- a received Tag is stored
/// and reportable, never adopted; see `tag_travel::adopt_tag` for the separate, explicit act that
/// does adopt one).
///
/// **0.58.0 P3: two reads, one frame in memory at a time** -- `bundle import`'s shape. The structure is walked first without reading any
/// object body (the author keys sit after the frames they are needed for). Then, under the locks, **pass 1** reads every frame once, decodes
/// it, takes what each later phase needs (ids, the blob ids a patch names, the signature results, admission and the store's own write
/// decision) and drops it; **pass 2** reads each frame again, requires its digest to be pass 1's, and writes it. Every refusal is raised
/// before the first write, in the order the whole-artifact version raised them for an artifact with one fault (and, for the cases the tests
/// pin, with several).
///
/// **Memory follows the largest object, not the artifact:** a patch-exchange artifact of many small objects peaks at a few MB; one 100 MiB
/// file costs about four times its size, the store's own write path (RFC 158), which `bundle import` pays as well.
pub fn accept_exchange_artifact_streaming(
    layout: &RepositoryLayout,
    source: &mut BundleSource,
    options: &AcceptOptions,
) -> Result<AcceptReport> {
    // RFC 136 §10.3b.3: the derivation gate, before any write.
    crate::worktree_marker::ensure_worktree_replay_verified(layout)?;
    // Phase A, item 1: total byte length, before any decoding.
    let total_len = source.seek(SeekFrom::End(0)).map_err(|err| {
        PrikkError::Integrity(format!(
            "patch-exchange artifact: failed to measure the input: {err}"
        ))
    })?;
    if total_len > len_to_u64(options.max_total_bytes)? {
        return Err(PrikkError::MalformedData(format!(
            "patch-exchange artifact is {total_len} bytes, over the configured limit of {} bytes",
            options.max_total_bytes
        )));
    }

    // Phase A item 2 (each declared count against `max_object_count`), the per-frame bound on each length prefix, the author-key section,
    // and the trailing-bytes check: one walk over the structure, no object body read.
    let shape = read_exchange_layout(
        source,
        total_len,
        options.max_object_count,
        options.max_object_bytes,
    )?;

    // Past every structural refusal, before the locks: a concurrent writer acting here is seen by the one locked decision below.
    #[cfg(test)]
    if let Some(change) = BEFORE_ACCEPT_WRITES.with(|slot| slot.borrow_mut().take()) {
        change();
    }

    // The locks, **before pass 1** (0.58.0 P3 ruling 1, as `bundle import`): admission and the store's own write decision are made under
    // them, before the first write, so a refused exchange still changes nothing. `ActiveLock` serialises author-key recording against
    // every other recorder; `ObjectStore` is held across every write (`write_object_under_held_lock`), a leaf declared last in
    // `LockableContainer`'s order. The one author-key conflict decision is made here, under the lock (it used to be made twice: early and
    // read-only, then again under the lock).
    let active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    let object_store_lock = acquire_container_locks(layout, &[LockableContainer::ObjectStore])?;
    let read_snapshot = ObjectReadSnapshot::open(layout)?;
    let mut object_store = ObjectWriteSession::open(layout)?;

    let author_material = |key_id: &str| -> Result<Vec<AuthorKeyEntry>> {
        let mut candidates = lookup_author_key_entries(layout, key_id)?;
        candidates.extend(
            shape
                .author_keys
                .iter()
                .filter(|entry| entry.key_id == key_id)
                .cloned(),
        );
        Ok(candidates)
    };
    // The trust policy and the admitter load it once; a failure to read it is raised where the whole-artifact version raised it (at the
    // claims, and at admission), not before.
    let (policy, policy_error): (Option<MaintainerTrustPolicy>, Option<PrikkError>) =
        match load_maintainer_trust_policy_or_empty(layout) {
            Ok(policy) => (Some(policy), None),
            Err(err) => (None, Some(err)),
        };
    let (admitter, mut admission_error) = match CarriedSignatureAdmitter::new(layout, "sync-accept")
    {
        Ok(admitter) => (Some(admitter), None),
        Err(err) => (None, Some(err)),
    };

    // ---- pass 1: every frame once, nothing written, nothing kept but metadata ----
    let mut content: Vec<PreparedObject> = Vec::new();
    let mut publication: Vec<PreparedObject> = Vec::new();
    let mut index_by_id: BTreeMap<ObjectId, (bool, usize)> = BTreeMap::new();
    let mut patch_ids: Vec<ObjectId> = Vec::new();
    let mut blob_ids: BTreeSet<ObjectId> = BTreeSet::new();
    // What pass 1 decided about each carried patch, kept (instead of the patch) until the phase that reports it, so the refusals surface in
    // the order the whole-artifact version raised them: digest, keys, closure, signatures, claims, tags, then admission.
    let mut patch_closures: Vec<(ObjectId, Result<Vec<ObjectId>>)> = Vec::new();
    let mut patch_signatures: Vec<(ObjectId, AuthorSignatureResult)> = Vec::new();
    let mut claim_checks: Vec<Result<(ObjectId, ClaimSignatureVerification)>> = Vec::new();
    let mut tag_checks: Vec<Result<(ObjectId, TagSignatureVerification)>> = Vec::new();
    let mut duplicate_error: Option<PrikkError> = None;
    let mut write_check_error: Option<PrikkError> = None;
    for (section, span) in [
        (Section::Patches, shape.patches),
        (Section::Blobs, shape.blobs),
        (Section::Claims, shape.claims),
        (Section::Tags, shape.tags),
    ] {
        source.seek(SeekFrom::Start(span.start)).map_err(|err| {
            PrikkError::Integrity(format!(
                "patch-exchange artifact: failed to seek to a section: {err}"
            ))
        })?;
        for _ in 0..span.count {
            let start = in_artifact(stream_position_usize(source))?;
            let (declared_bytes, body) =
                in_artifact(read_frame_body_from(source, options.max_object_bytes))?;
            let end = start
                .checked_add(8)
                .and_then(|value| value.checked_add(body.len()))
                .ok_or_else(|| {
                    PrikkError::Integrity("patch-exchange artifact: frame range overflow".into())
                })?;
            let frame_digest = prikk_hash::sha256_parts(&[&declared_bytes.to_be_bytes(), &body]);
            let envelope = decode_envelope_file(&body)?;
            // The body is the largest buffer this turn holds; `envelope` owns its own copy of the payload, so it is not kept.
            drop(body);
            let object_id = envelope.object_id();
            #[cfg(test)]
            let _live = LiveDecoded::new(envelope.canonical_payload.len());

            // What each section's frames are checked for, kept as a result until its phase (see `PatchCheck`).
            match section {
                Section::Patches => {
                    patch_ids.push(object_id);
                    patch_closures.push((object_id, patch_closure(&envelope, layout)));
                    patch_signatures.push((
                        object_id,
                        verify_author_signatures_with(&envelope, &author_material),
                    ));
                }
                Section::Blobs => {
                    blob_ids.insert(object_id);
                }
                Section::Claims => {
                    if let Some(policy) = &policy {
                        claim_checks.push(check_claim(
                            &envelope,
                            object_id,
                            policy,
                            &read_snapshot,
                        ));
                    }
                }
                Section::Tags => {
                    if let Some(policy) = &policy {
                        tag_checks.push(
                            verify_tag_signature(&envelope, policy)
                                .map(|outcome| (object_id, outcome)),
                        );
                    }
                }
            }

            // One id, one envelope -- an object id does not cover signatures, so the same id can arrive twice with different signature
            // bytes; the store would refuse the second after writing the first. A byte-identical repeat is harmless and admitted once.
            if let Some(&(existing_in_publication, existing_index)) = index_by_id.get(&object_id) {
                let existing = if existing_in_publication {
                    &publication
                } else {
                    &content
                };
                if !prepared_matches_existing(
                    source,
                    existing,
                    existing_index,
                    frame_digest,
                    &envelope,
                )? && duplicate_error.is_none()
                {
                    duplicate_error = Some(PrikkError::MalformedData(format!(
                        "patch-exchange artifact carries object {object_id} twice with different \
                         envelopes -- refusing the whole exchange"
                    )));
                }
                continue;
            }

            // RFC 156 §4 rules 1-3 and the store's own write decision, for this one object, under the lock. A refusal is kept and raised
            // after every earlier phase's (admission first, then the store's check), as the whole-artifact version ordered them.
            let object_type = envelope.object_type;
            let already_present = object_store.contains_object(object_type, object_id)?;
            let mut admission_outcome = AdmissionOutcomeKind::New;
            if admission_error.is_none() {
                if let Some(admitter) = &admitter {
                    match admitter.admit_one(&object_store, envelope, &author_material) {
                        Ok(admitted) => {
                            if write_check_error.is_none() {
                                if let Err(err) = object_store.check_write(&admitted.envelope) {
                                    write_check_error = Some(err);
                                }
                            }
                            admission_outcome = if !already_present {
                                AdmissionOutcomeKind::New
                            } else if admitted.merged {
                                AdmissionOutcomeKind::Merged
                            } else {
                                AdmissionOutcomeKind::AlreadyPresent
                            };
                        }
                        Err(err) => admission_error = Some(err),
                    }
                }
            }
            let prepared = PreparedObject {
                object_id,
                object_type,
                same_kind_dependencies: Vec::new(),
                frame_range: start..end,
                frame_digest,
                admission_outcome,
            };
            if matches!(section, Section::Claims | Section::Tags) {
                index_by_id.insert(object_id, (true, publication.len()));
                publication.push(prepared);
            } else {
                index_by_id.insert(object_id, (false, content.len()));
                content.push(prepared);
            }
        }
    }

    // ---- every refusal, in the order the whole-artifact version raised them; nothing has been written ----
    // Phase B item 4: recompute the patch-set digest over the carried patches; refuse on mismatch.
    let mut decoded_patch_ids = patch_ids;
    decoded_patch_ids.sort_unstable();
    decoded_patch_ids.dedup();
    let recomputed_digest = compute_patch_set_digest(&decoded_patch_ids)?;
    if recomputed_digest != shape.declared_digest {
        return Err(PrikkError::Integrity(
            "patch-exchange artifact's declared patch-set digest does not match its own decoded \
             patches -- refusing before any signature work"
                .to_string(),
        ));
    }

    // Phase B item 5: artifact-internal author-key conflict -- two different public keys for one
    // `key_id` within the artifact itself refuses the whole import (`import_bundle` learned this the
    // hard way; same check, same reason).
    let mut artifact_key_ids: BTreeMap<&str, [u8; 32]> = BTreeMap::new();
    for entry in &shape.author_keys {
        match artifact_key_ids.get(entry.key_id.as_str()) {
            Some(existing) if *existing != entry.public_key => {
                return Err(PrikkError::MalformedData(format!(
                    "patch-exchange artifact's author-key section carries two different public \
                     keys for key_id {} -- refusing the whole exchange",
                    entry.key_id
                )));
            }
            Some(_) => {}
            None => {
                artifact_key_ids.insert(&entry.key_id, entry.public_key);
            }
        }
    }

    // Phase B item 5b and the old item 11, now **one decision under the lock**: an artifact key against a different key this repository
    // already records for the id. The lock is held from before pass 1, so no writer can record one between this and the writes.
    for (&key_id, &public_key) in &artifact_key_ids {
        check_author_key_conflict(layout, key_id, public_key)?;
    }

    // Phase B item 6: closure completeness. Every blob a carried patch's operations reference must
    // be present -- in the artifact or already in this repository -- and `parent_patch_ids` must be empty (the day it stops being
    // empty this is the code that must not silently ignore it). Each patch's own decode and path checks ran in pass 1 and are raised
    // here, in patch order, ahead of the next patch's blob check.
    for (patch_id, closure) in patch_closures {
        for blob_id in closure? {
            if !blob_ids.contains(&blob_id)
                && !read_snapshot.contains_object(ObjectType::Blob, blob_id)
            {
                return Err(PrikkError::Integrity(format!(
                    "patch {} references blob {blob_id}, which is neither carried by this \
                     artifact nor already present in this repository -- refusing the whole \
                     exchange, no partial apply",
                    patch_id
                )));
            }
        }
    }

    // Phase C item 7: every carried patch's AUTHOR signature (RFC 156 Stage 1: every AUTHOR signature on each patch, not only the first).
    let mut author_signature_outcomes = Vec::with_capacity(patch_signatures.len());
    for (patch_id, signature) in patch_signatures {
        let Some((key_id, verifies)) = signature? else {
            continue;
        };
        let outcome = if verifies {
            AuthorSignatureVerification::Sound { key_id }
        } else {
            AuthorSignatureVerification::Unverifiable { key_id }
        };
        author_signature_outcomes.push((patch_id, outcome));
    }

    // Phase C items 8-9: every claim's own MAINTAINER signature, then every claim against blocks
    // this repository already holds.
    if let Some(err) = policy_error {
        return Err(err);
    }
    let mut claim_signature_outcomes = Vec::with_capacity(claim_checks.len());
    for check in claim_checks {
        claim_signature_outcomes.push(check?);
    }

    // RFC 117 stage 3 §3: every carried tag's own MAINTAINER signature, reported and never gating --
    // the same treatment claims get. No consistency check against a held block: a Tag's own identity is its
    // `patch_set_digest`/`patch_count`, which say nothing about which local block (if any) currently matches.
    let mut tag_signature_outcomes = Vec::with_capacity(tag_checks.len());
    for check in tag_checks {
        tag_signature_outcomes.push(check?);
    }

    // Phase D's decisions, before the first write: the same id twice with different envelopes, then RFC 156 §4 admission, then the
    // store's own write check.
    if let Some(err) = duplicate_error {
        return Err(err);
    }
    if let Some(err) = admission_error {
        return Err(err);
    }
    if let Some(err) = write_check_error {
        return Err(err);
    }
    let Some(admitter) = admitter else {
        return Err(PrikkError::Integrity(
            "patch-exchange accept: the admitter was not built, and no refusal said why"
                .to_string(),
        ));
    };

    // 0.49.0 step 0, finding 3: dependency order within each of the two phases below -- never across the content/publication boundary
    // itself, which stays fixed for its own, unrelated reason (design §8.1: no claim or tag may be recorded from an exchange that failed,
    // so claims/tags are deliberately last, under the lock, regardless of object-reference order). For these four object types the
    // order is the artifact's own, except a tag before a claim.
    let content_order = dependency_order_indices(&content)?;
    let publication_order = dependency_order_indices(&publication)?;

    #[cfg(test)]
    if let Some(change) = BETWEEN_ACCEPT_PASSES.with(|slot| slot.borrow_mut().take()) {
        change();
    }

    // ---- pass 2: write. Past this point only I/O (or a file changed under us) can stop the exchange -- never a decision about what it
    // carries, and never a concurrent writer: both locks are held from before pass 1. ----
    let total_objects = content.len() + publication.len();
    let mut writer = PassTwo {
        source,
        admitter: &admitter,
        object_store: &mut object_store,
        object_store_lock: &object_store_lock,
        total_objects,
        written_object_count: 0,
        objects_done: 0,
        steps_done: 0,
        merged_object_count: 0,
        dropped_signatures: Vec::new(),
    };
    // Item 10: patches and blobs. Content-addressed and idempotent -- a replayed accept (§4.3) writes nothing new here.
    for index in content_order {
        writer.write(&content, index, &author_material)?;
    }

    // Item 11: record every entry, each already checked above under this same lock
    // (`multi-key-import-partial-write-v1.md`: never check-then-record one entry at a time).
    let mut recorded_author_key_count = 0_usize;
    for entry in &shape.author_keys {
        writer.step_for_test("key", &entry.key_id)?;
        record_author_key_material(layout, &entry.key_id, entry.public_key, &active_lock)?;
        recorded_author_key_count = recorded_author_key_count.checked_add(1).ok_or_else(|| {
            PrikkError::Integrity("exchange accept recorded-author-key count overflow".to_string())
        })?;
    }

    // Claims and tags last, still under the lock: "no key material, and no claim, may be recorded
    // from an exchange that failed" (design §8.1), and RFC 117 stage 3 §5 row 6 puts a Tag on the
    // same terms -- **a refused exchange records no tag.**
    // 0.58.0 S0 (a P2b follow-up): the claims and tags are what `sync pending` lists, so this write is a visibility point, a member of
    // P2b's class: every directory the hold kept open is checked against its path first, and a directory moved while the command ran
    // refuses here, exit 1, with no claim or tag written. The write order is unchanged (patches and blobs, AUTHOR key material, then
    // claims and tags).
    if !publication.is_empty() {
        #[cfg(test)]
        if let Some(change) = BEFORE_ACCEPT_CLAIMS.with(|slot| slot.borrow_mut().take()) {
            change();
        }
        layout.repository_mutation_root().check_held_directories()?;
    }
    for index in publication_order {
        writer.write(&publication, index, &author_material)?;
    }
    let written_object_count = writer.written_object_count;
    let merged_object_count = writer.merged_object_count;
    let dropped_signatures = writer.dropped_signatures;
    drop(object_store_lock);
    drop(active_lock);

    Ok(AcceptReport {
        patch_count: usize::try_from(shape.patches.count).unwrap_or(usize::MAX),
        blob_count: usize::try_from(shape.blobs.count).unwrap_or(usize::MAX),
        claim_count: usize::try_from(shape.claims.count).unwrap_or(usize::MAX),
        tag_count: usize::try_from(shape.tags.count).unwrap_or(usize::MAX),
        written_object_count,
        merged_object_count,
        dropped_signatures,
        recorded_author_key_count,
        author_signature_outcomes,
        claim_signature_outcomes,
        tag_signature_outcomes,
    })
}

/// A carried patch's own closure result: the blob ids its operations name, or the refusal its decode raised (a non-empty
/// `parent_patch_ids`, an undecodable operation, a path the receiving format cannot hold).
///
/// Deliberately schema-blind about `parent_patch_ids` and deliberately run before the operations are decoded -- Patch schema 2 handoff v2
/// amendment §3: this shadows `decode_patch_operations`'s own schema-2-and-above refusal of a present tag 2, for both schemas, since it
/// refuses a non-empty value at *any* schema, including schema 1 where the field is legal-but-must-be-empty. It is broader, not redundant.
fn patch_closure(envelope: &ObjectEnvelope, layout: &RepositoryLayout) -> Result<Vec<ObjectId>> {
    let parent_patch_ids = decode_patch_parent_ids(&envelope.canonical_payload)?;
    if !parent_patch_ids.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "patch {} carries a non-empty parent_patch_ids -- this field is always empty \
             today and there is nothing defined to walk there yet; refusing rather than \
             silently ignoring it",
            envelope.object_id()
        )));
    }
    let operations = decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?;
    // 0.57.0 U2 (the content half): a receiver that cannot hold a non-ASCII path refuses a patch that carries one,
    // before anything is written -- `sync accept` is an importer like `bundle import` and `archive import`.
    crate::artifact_format::require_receiver_can_hold_paths(
        crate::artifact_format::ArtifactKind::Sync,
        &operations,
        layout.format(),
    )?;
    let mut referenced = Vec::new();
    for operation in operations {
        referenced.extend(referenced_blob_ids(&operation.kind));
    }
    Ok(referenced)
}

/// One carried claim's own MAINTAINER signature outcome, and its consistency against blocks this repository already holds
/// (a claim contradicting a held block refuses the whole exchange).
fn check_claim(
    envelope: &ObjectEnvelope,
    claim_id: ObjectId,
    policy: &MaintainerTrustPolicy,
    snapshot: &ObjectReadSnapshot,
) -> Result<(ObjectId, ClaimSignatureVerification)> {
    let outcome = verify_claim_signature(envelope, policy)?;
    let payload = RecognitionClaimPayload::decode_canonical(&envelope.canonical_payload)?;
    match check_recognition_claim_consistency(snapshot, &payload)? {
        crate::recognition_claim::RecognitionClaimConsistency::Contradicted { .. } => {
            Err(PrikkError::Integrity(format!(
                "recognition claim {claim_id} contradicts a block this repository already \
                 holds -- refusing the whole exchange"
            )))
        }
        crate::recognition_claim::RecognitionClaimConsistency::Consistent
        | crate::recognition_claim::RecognitionClaimConsistency::BlockAbsent => {
            Ok((claim_id, outcome))
        }
    }
}

/// Pass 2's writer: each object is re-read by its recorded range, its frame digest and admission outcome must be pass 1's, and it is
/// written under the held lock. Counts what the report and the refusal messages need.
struct PassTwo<'a> {
    source: &'a mut BundleSource,
    admitter: &'a CarriedSignatureAdmitter,
    object_store: &'a mut ObjectWriteSession,
    object_store_lock: &'a crate::lock::ContainerLockGuard,
    total_objects: usize,
    written_object_count: usize,
    objects_done: usize,
    steps_done: usize,
    merged_object_count: usize,
    dropped_signatures: Vec<crate::DroppedSignature>,
}

impl PassTwo<'_> {
    fn write(
        &mut self,
        objects: &[PreparedObject],
        index: usize,
        author_material: &impl Fn(&str) -> Result<Vec<AuthorKeyEntry>>,
    ) -> Result<()> {
        let Some(prepared) = objects.get(index) else {
            return Err(PrikkError::Integrity(
                "patch-exchange accept: dependency ordering produced an index outside the prepared set"
                    .to_string(),
            ));
        };
        let (done, total) = (self.objects_done, self.total_objects);
        let reread_message = || {
            format!(
                "patch-exchange accept: object {} could not be re-read for its own second-pass check -- \
                 {done} of {total} objects were already written, each validated and admitted before being \
                 written; no author key material, claim or tag was written by a stop before them; re-running \
                 the same accept once the file is stable will complete it",
                prepared.object_id
            )
        };
        if self
            .source
            .seek(SeekFrom::Start(prepared.frame_range.start as u64))
            .is_err()
        {
            return Err(PrikkError::Integrity(reread_message()));
        }
        let frame_len = prepared.frame_range.end - prepared.frame_range.start;
        let Ok(frame) = read_exact_from(self.source, frame_len) else {
            return Err(PrikkError::Integrity(reread_message()));
        };
        // The frame's digest, recomputed from the bytes pass 1 read, before this object is decoded or written: a frame that is not the one
        // pass 1 validated is refused here (row 12), never written.
        if prikk_hash::sha256(&frame) != prepared.frame_digest {
            return Err(PrikkError::Integrity(format!(
                "patch-exchange accept: object {} changed between validation and writing -- \
                 {done} of {total} objects were already written, each validated and admitted before being \
                 written; re-running the same accept once the file is stable will complete it",
                prepared.object_id
            )));
        }
        let Some(frame_body) = frame.get(8..) else {
            return Err(PrikkError::Integrity(
                "patch-exchange accept: a prepared object's own frame is shorter than its length prefix"
                    .to_string(),
            ));
        };
        let envelope = decode_envelope_file(frame_body)?;
        #[cfg(test)]
        let _live = LiveDecoded::new(envelope.canonical_payload.len());
        let already_present = self
            .object_store
            .contains_object(prepared.object_type, prepared.object_id)?;
        let admitted = self
            .admitter
            .admit_one(&*self.object_store, envelope, author_material)?;
        let outcome = if !already_present {
            AdmissionOutcomeKind::New
        } else if admitted.merged {
            AdmissionOutcomeKind::Merged
        } else {
            AdmissionOutcomeKind::AlreadyPresent
        };
        // The store cannot change while the lock is held, so the second pass must reach pass 1's outcome; a mismatch is an internal
        // error, raised before this object's write.
        if outcome != prepared.admission_outcome {
            return Err(PrikkError::Integrity(format!(
                "patch-exchange accept: object {} admitted differently on its second pass ({outcome:?}) than its first ({:?}) -- \
                 the store must not change while the accept's own lock is held; refusing before this object's write",
                prepared.object_id, prepared.admission_outcome
            )));
        }
        if admitted.merged {
            self.merged_object_count =
                self.merged_object_count.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity(
                        "exchange accept merged-object count overflow".to_string(),
                    )
                })?;
        }
        self.dropped_signatures.extend(admitted.dropped);
        if !already_present {
            self.written_object_count =
                self.written_object_count.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity(
                        "exchange accept written-object count overflow".to_string(),
                    )
                })?;
        }
        self.step_for_test(test_kind(prepared.object_type), &prepared.object_id)?;
        // For a `New` object `admitted.envelope` is the frame decoded and handed back unchanged, so the write may skip re-encoding the
        // bytes it already has; a merge changes the content and always re-encodes.
        let known_body = (outcome == AdmissionOutcomeKind::New).then_some(frame_body);
        self.object_store
            .write_object_under_held_lock_with_known_body(
                &admitted.envelope,
                self.object_store_lock,
                known_body,
            )?;
        self.objects_done += 1;
        Ok(())
    }

    /// The `test-support` hooks (in no shipped build): `PRIKK_TEST_ACCEPT_TRACE=<file>` appends one line per durable write, in order
    /// (`patch <id>`, `blob <id>`, `key <id>`, `tag <id>`, `claim <id>`), and `PRIKK_TEST_ACCEPT_FAIL_AT=<n>` makes the n-th durable
    /// write (counting objects and key entries, from 0) fail with an error before it is made. Empty in a shipped build.
    #[allow(clippy::unused_self)]
    fn step_for_test(&mut self, kind: &str, id: &dyn std::fmt::Display) -> Result<()> {
        #[cfg(feature = "test-support")]
        {
            let step = self.steps_done;
            self.steps_done += 1;
            if let Some(fail_at) = std::env::var("PRIKK_TEST_ACCEPT_FAIL_AT")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
            {
                if step == fail_at {
                    return Err(PrikkError::Integrity(
                        "sync accept stopped for test: a simulated write error".to_string(),
                    ));
                }
            }
            if let Some(path) = std::env::var_os("PRIKK_TEST_ACCEPT_TRACE") {
                use std::io::Write as _;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(file, "{kind} {id}");
                }
            }
        }
        #[cfg(not(feature = "test-support"))]
        {
            let _ = (kind, id, self.steps_done);
        }
        Ok(())
    }
}

/// The word a trace line carries for an object type (the four an artifact carries).
#[cfg(any(test, feature = "test-support"))]
fn test_kind(object_type: ObjectType) -> &'static str {
    match object_type {
        ObjectType::Patch => "patch",
        ObjectType::Blob => "blob",
        ObjectType::RecognitionClaim => "claim",
        ObjectType::Tag => "tag",
        _ => "other",
    }
}

#[cfg(not(any(test, feature = "test-support")))]
fn test_kind(_object_type: ObjectType) -> &'static str {
    ""
}

#[cfg(test)]
thread_local! {
    static LIVE_DECODED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PEAK_DECODED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `cfg(test)`: a decoded object's payload bytes while it is alive. The control that holds "the accept never keeps a decoded object past
/// its own turn": the high-water mark of these bytes over a whole accept is one object's, not the artifact's.
#[cfg(test)]
pub(crate) struct LiveDecoded(usize);

#[cfg(test)]
impl LiveDecoded {
    fn new(bytes: usize) -> Self {
        LIVE_DECODED_BYTES.with(|live| {
            live.set(live.get() + bytes);
            PEAK_DECODED_BYTES.with(|peak| peak.set(peak.get().max(live.get())));
        });
        Self(bytes)
    }
}

#[cfg(test)]
impl Drop for LiveDecoded {
    fn drop(&mut self) {
        LIVE_DECODED_BYTES.with(|live| live.set(live.get() - self.0));
    }
}

/// `cfg(test)`: reset the high-water mark, run `body`, and return the highest payload bytes any decoded object of this thread's accepts
/// held at once.
#[cfg(test)]
pub(crate) fn peak_decoded_bytes_for_test<T>(body: impl FnOnce() -> T) -> (T, usize) {
    PEAK_DECODED_BYTES.with(|peak| peak.set(0));
    let result = body();
    (result, PEAK_DECODED_BYTES.with(std::cell::Cell::get))
}

#[cfg(test)]
thread_local! {
    static BETWEEN_ACCEPT_PASSES: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam, unreachable from production: run `change` once, after pass 1 has validated and admitted every object and before pass 2's
/// first write -- where the artifact's own file can be changed under an accept that holds it open (row 12).
#[cfg(test)]
pub(crate) fn between_accept_passes_for_test(change: impl FnOnce() + 'static) {
    BETWEEN_ACCEPT_PASSES.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}
/// Every blob id one decoded operation references -- the same three kinds `export_exchange_artifact`
/// (`artifact.rs`) and `export_bundle` (`bundle.rs`) each scan for, restated here for the accept
/// path's own closure check.
fn referenced_blob_ids(kind: &DecodedOperationKind) -> Vec<ObjectId> {
    match kind {
        DecodedOperationKind::CreateFile { blob_id, .. } => vec![*blob_id],
        DecodedOperationKind::ReplaceBinary {
            old_blob_id,
            new_blob_id,
            ..
        } => vec![*old_blob_id, *new_blob_id],
        DecodedOperationKind::DeleteNode {
            preimage: DecodedDeletePreimage::File { old_blob_id, .. },
            ..
        } => vec![*old_blob_id],
        _ => Vec::new(),
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_ACCEPT_CLAIMS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam, unreachable from production: run `change` once, after the patches, blobs and author keys are written and before the
/// first claim or tag is -- where a held directory can be moved (0.58.0 S0).
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn before_accept_claims_for_test(change: impl FnOnce() + 'static) {
    BEFORE_ACCEPT_CLAIMS.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}

#[cfg(test)]
thread_local! {
    static BEFORE_ACCEPT_WRITES: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam, unreachable from production: run `change` once, after every read-only check and before
/// `accept_exchange_artifact` takes its lock and writes — where a concurrent writer could record an
/// author key the early read-only check did not see.
#[cfg(test)]
pub(crate) fn before_accept_writes_for_test(change: impl FnOnce() + 'static) {
    BEFORE_ACCEPT_WRITES.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}

#[cfg(test)]
mod streaming_tests;
#[cfg(test)]
mod tests;
