//! The one decision `bundle import` and `bundle preview` share (0.58.0 B′b).
//!
//! **One walk, one function that decides.** [`decide`] is `bundle import`'s pass 1 and everything after it up to the write order: the
//! structure, every object's admission and the store's own write decision, the closure, the author-key conflict, the class against the
//! received pointer, the forged-signature check, and whether `--adopt` can land the branch. It returns an [`ImportDecision`]: the plan
//! (per-object metadata, the write order, the adoption) and **every refusal, in the import's own order**. `bundle import` takes the
//! locks, calls it, raises the first refusal with the same error it always raised, and applies the plan; `bundle preview` calls the same
//! function with no lock and reports every refusal.
//!
//! **It holds no handle that can write.** Its store is an [`ObjectReadSnapshot`] (lookups, an index refresh and `check_write`, which
//! appends nothing); it names no `ObjectWriteSession`, no lock guard and no signer (`--adopt` arrives as an [`AdoptionRequest`]: a key
//! id and, if the caller vouches for it, a public key). The *witness* that it writes nothing is behavioural -- the repository is
//! byte-identical after it, for every refusal kind and the clean path (`bundle/tests/decision.rs`); a source scan over this file for
//! the names of the repository's writers is a secondary check that matches names only, so a writer it does not list would pass it.
//! `RepositoryLayout` (which this takes, to read) can reach every writer in the crate, so "fails to compile" is not on offer.
//!
//! **The lock is the caller's, and what that means for a preview.** The import holds the locks around this call, so its facts are
//! consistent. A preview holds none and reads each fact as of the moment it reads it: a writer acting between a preview and an import
//! (or between two of the preview's reads) can change the answer. The preview does not run the second pass's per-object checks (a frame
//! that changed since, an I/O failure while writing).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Seek, SeekFrom};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, Signature};

use super::adopt::{self, AdoptionRequest};
use super::adopt_local_ref;
use super::{
    AdmissionOutcomeKind, BUNDLE_MAGIC, BundleAndLocalReader, BundleConnectivityClass,
    BundleImportOptions, BundleScope, BundleSource, CarriedSignatureAdmitter, ClosureMetadata,
    DecodedManifest, PreparedObject, RETIRED_BUNDLE_MAGIC_V1, RETIRED_BUNDLE_MAGIC_V2,
    RefStateChainRelation, classify_import_connectivity, closure_metadata_for,
    dependency_order_indices, find_forged_publication_signature, non_fast_forward_refusal,
    prepared_matches_existing, validate_bundle_closure,
};
#[cfg(test)]
use super::{
    corrupted_admission_outcome_for_test, corrupted_frame_digest_for_test,
    force_connectivity_class_for_test,
};
use crate::author::author_key_index::{
    AuthorKeyEntry, check_author_key_conflict, lookup_author_key_entries,
};
use crate::foundation::file_codec::decode_envelope_file;
use crate::foundation::fsutil::len_to_u64;
use crate::framed_source::{
    read_array_from, read_bytes_u64_from, read_frame_body_from, read_u64_from, skip_object_frames,
    stream_position_usize, u64_to_usize,
};
use crate::object_store::ObjectReadSnapshot;

/// What the bundle's header says: read before any lock, by the import and the preview alike.
pub(super) struct BundleHeader {
    pub(super) origin_ref_name: String,
    pub(super) received_ref_name: String,
    pub(super) count: u64,
    pub(super) total_len: u64,
    pub(super) object_count: usize,
    has_author_key_section: bool,
    has_manifest_section: bool,
}

/// One refusal `bundle import` of this delivery would raise.
pub(super) struct ImportRefusal {
    /// The stable id: the one `bundle import`'s `refusal-v1` document already prints where it prints one (`rollback`, `diverged`,
    /// `does-not-connect`, `forged-publication-signature`), a new one only where the import prints none.
    pub(super) id: &'static str,
    /// The object it refuses on, when it is about one.
    pub(super) object: Option<(ObjectType, ObjectId)>,
    /// What `bundle import` returns, unchanged.
    pub(super) error: PrikkError,
}

impl ImportRefusal {
    fn new(kind: &'static str, object: Option<(ObjectType, ObjectId)>, error: PrikkError) -> Self {
        let id = match &error {
            PrikkError::BundleNonFastForward { class, .. } => class,
            PrikkError::ForgedPublicationSignature { .. } => "forged-publication-signature",
            _ => kind,
        };
        Self { id, object, error }
    }
}

/// The decision: the plan an import applies, and every refusal.
pub(super) struct ImportDecision {
    pub(super) origin_ref_name: String,
    pub(super) received_ref_name: String,
    pub(super) object_count: usize,
    pub(super) prepared: Vec<PreparedObject>,
    pub(super) index_by_id: BTreeMap<ObjectId, usize>,
    pub(super) author_keys: Vec<AuthorKeyEntry>,
    pub(super) manifest: Option<DecodedManifest>,
    pub(super) ref_state_id: ObjectId,
    pub(super) tip_block_id: ObjectId,
    pub(super) class: BundleConnectivityClass,
    pub(super) ref_state_chain: Option<RefStateChainRelation>,
    pub(super) adoption: adopt::AdoptionDecision,
    pub(super) write_order: Vec<usize>,
    pub(super) refusals: Vec<ImportRefusal>,
}

/// Read the bundle's header (the magic, the declared size and count, the ref name): the checks that need no repository and no lock.
pub(super) fn read_header(
    source: &mut BundleSource,
    options: &BundleImportOptions,
) -> Result<BundleHeader> {
    // Header-only checks, before any lock (Q1): the declared total size and the format magic.
    let total_len = source.seek(SeekFrom::End(0)).map_err(|err| {
        PrikkError::Integrity(format!("bundle import: failed to measure the input: {err}"))
    })?;
    source.seek(SeekFrom::Start(0)).map_err(|err| {
        PrikkError::Integrity(format!("bundle import: failed to rewind the input: {err}"))
    })?;
    if total_len > len_to_u64(options.max_total_bytes)? {
        return Err(PrikkError::MalformedData(format!(
            "bundle is {total_len} bytes, over the configured limit of {} bytes",
            options.max_total_bytes
        )));
    }
    let magic: [u8; 8] = read_array_from(source)?;
    let (has_author_key_section, has_manifest_section) = if &magic == BUNDLE_MAGIC {
        (true, true)
    } else if &magic == RETIRED_BUNDLE_MAGIC_V2 {
        (true, false)
    } else if &magic == RETIRED_BUNDLE_MAGIC_V1 {
        (false, false)
    } else {
        return Err(PrikkError::MalformedData(
            "invalid bundle magic".to_string(),
        ));
    };
    // The origin ref name is a header field too (distinct from the exported RefState's own signed
    // `ref_name`, checked against it below) -- cheap, needed for `validate_received_ref` before the
    // lock, and part of the judgment call Q1 leaves to this implementation.
    let origin_ref_name = String::from_utf8(read_bytes_u64_from(source, options.max_total_bytes)?)
        .map_err(|err| {
            PrikkError::MalformedData(format!("invalid bundle ref name utf-8: {err}"))
        })?;
    let count = read_u64_from(source)?;
    if count > len_to_u64(options.max_object_count)? {
        return Err(PrikkError::MalformedData(format!(
            "bundle declares {count} objects, over the configured limit of {}",
            options.max_object_count
        )));
    }
    let object_count = u64_to_usize(count)?;

    let received_ref_name = format!("remotes/{origin_ref_name}");
    crate::received::validate_received_ref(&received_ref_name)?;
    Ok(BundleHeader {
        origin_ref_name,
        received_ref_name,
        count,
        total_len,
        object_count,
        has_author_key_section,
        has_manifest_section,
    })
}

/// Decide what `bundle import` of this delivery does, against this repository, reading only. See the module doc. `Err` is a walk that
/// cannot go on (an unreadable or structurally broken bundle, an I/O failure); `refusals` is every policy refusal, in the import's order.
#[allow(clippy::too_many_lines)]
pub(super) fn decide(
    layout: &crate::foundation::layout::RepositoryLayout,
    options: &BundleImportOptions,
    adoption: Option<&AdoptionRequest>,
    header: BundleHeader,
    source: &mut BundleSource,
    store: &mut ObjectReadSnapshot,
) -> Result<ImportDecision> {
    let BundleHeader {
        origin_ref_name,
        received_ref_name,
        count,
        total_len,
        object_count,
        has_author_key_section,
        has_manifest_section,
    } = header;
    crate::received::received_index::require_received_index_clean_tail(layout)?;

    let objects_start = stream_position_usize(source)?;
    // RFC 156 §4 needs the author-key section's own transported material before admission can run
    // for a Patch a newly-carried key_id introduces -- skip past the object frames (without reading
    // any of their bodies) to reach it, read it, then seek back to do the real per-object pass.
    skip_object_frames(source, count, options.max_object_bytes)?;
    let mut author_keys: Vec<AuthorKeyEntry> = Vec::new();
    if has_author_key_section {
        let author_key_count = read_u64_from(source)?;
        if author_key_count > len_to_u64(options.max_object_count)? {
            return Err(PrikkError::MalformedData(format!(
                "bundle declares {author_key_count} author key entries, over the configured limit \
                 of {}",
                options.max_object_count
            )));
        }
        for _ in 0..author_key_count {
            let key_id = String::from_utf8(read_bytes_u64_from(source, options.max_total_bytes)?)
                .map_err(|err| {
                PrikkError::MalformedData(format!("invalid bundle author key_id utf-8: {err}"))
            })?;
            Signature::validate_key_id(&key_id)?;
            let public_key = read_array_from::<32>(source)?;
            author_keys.push(AuthorKeyEntry { key_id, public_key });
        }
    }
    let manifest = if has_manifest_section {
        let repository_format = u32::try_from(read_u64_from(source)?).map_err(|_| {
            PrikkError::MalformedData(
                "bundle manifest declares a repository format number that does not fit in u32"
                    .to_string(),
            )
        })?;
        // 0.57.0 U4: the bundle's declared format used to be informational; a number this build does not know is
        // now refused, as the archive's always was.
        crate::artifact_format::require_known_declared_format(
            crate::artifact_format::ArtifactKind::Bundle,
            repository_format,
        )?;
        let tool_version = String::from_utf8(read_bytes_u64_from(source, options.max_total_bytes)?)
            .map_err(|err| {
                PrikkError::MalformedData(format!(
                    "invalid bundle manifest tool version utf-8: {err}"
                ))
            })?;
        let declared_ref_name = String::from_utf8(read_bytes_u64_from(
            source,
            options.max_total_bytes,
        )?)
        .map_err(|err| {
            PrikkError::MalformedData(format!("invalid bundle manifest ref name utf-8: {err}"))
        })?;
        let declared_object_count = read_u64_from(source)?;
        let scope = BundleScope::from_wire_tag(read_u64_from(source)?)?;
        Some(DecodedManifest {
            repository_format,
            tool_version,
            scope,
            declared_ref_name,
            declared_object_count,
        })
    } else {
        None
    };
    let end_position = stream_position_usize(source)?;
    if end_position as u64 != total_len {
        return Err(PrikkError::MalformedData(
            "trailing bytes in bundle".to_string(),
        ));
    }
    // 0.57.0 U4: a bundle from a repository that can hold non-ASCII paths (format 8 and later) is refused by one that
    // cannot, before the first object is written, naming `prikk format upgrade`.
    if let Some(declared) = &manifest {
        crate::artifact_format::require_receiver_can_hold(
            crate::artifact_format::ArtifactKind::Bundle,
            declared.repository_format,
            layout.format(),
        )?;
    }

    source
        .seek(SeekFrom::Start(objects_start as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to seek back to the object frames: {err}"
            ))
        })?;

    let admitter = CarriedSignatureAdmitter::new(layout, "bundle-import")?;
    // The first admission or store refusal in frame order: kept, not raised, so a preview can list it and the import raises it first.
    let mut pass1_refusal: Option<ImportRefusal> = None;
    let author_material_closure = |key_id: &str| -> Result<Vec<AuthorKeyEntry>> {
        let mut entries = lookup_author_key_entries(layout, key_id)?;
        entries.extend(
            author_keys
                .iter()
                .filter(|entry| entry.key_id == key_id)
                .cloned(),
        );
        Ok(entries)
    };

    // 0.55.0 step 3 Part 3b-i2 (Q1/Q2, fusing what used to be `validate_bundle_contents`'s own
    // decode and `import_bundle`'s own pass 1): one forward walk over the object frames, reading
    // each exactly once, in file order. For every object it decides RFC 156 §4 admission and the
    // store's own write decision (R4), and extracts the handful of ids `validate_bundle_closure`
    // below needs (Q1) -- never the decoded envelope itself past its own turn in this loop, and
    // never more than one frame's content in memory at a time.
    let mut prepared: Vec<PreparedObject> = Vec::new();
    let mut index_by_id: BTreeMap<ObjectId, usize> = BTreeMap::new();
    let mut membership: BTreeMap<ObjectId, ObjectType> = BTreeMap::new();
    let mut closure_by_id: BTreeMap<ObjectId, ClosureMetadata> = BTreeMap::new();
    for _ in 0..count {
        let start = stream_position_usize(source)?;
        let (declared_bytes, body) = read_frame_body_from(source, options.max_object_bytes)?;
        let end = start
            .checked_add(8)
            .and_then(|v| v.checked_add(body.len()))
            .ok_or_else(|| {
                PrikkError::Integrity("bundle import: frame range overflow".to_string())
            })?;
        let real_frame_digest = prikk_hash::sha256_parts(&[&declared_bytes.to_be_bytes(), &body]);
        let envelope = decode_envelope_file(&body)?;
        // 0.55.0 step 3 Part 3b-ii2 (N1): `body` (the largest buffer this turn holds) is never
        // read again past this point -- `envelope.canonical_payload` already owns its own copy.
        // Dropped here rather than left to the end of this loop's own scope, so it is not still
        // resident while `admit_one`/`closure_metadata_for` run their own allocations below.
        drop(body);
        let object_id = envelope.object_id();

        // 0.55.0 step 3 Part 3b-i's own fix, restated for the streaming walk: a bundle may carry
        // the same id twice with byte-identical bytes ("an exact repeat is harmless and allowed").
        // Resolved here, as each occurrence is read, rather than after the fact -- the second
        // occurrence of a harmless repeat is never admitted at all, so R4's cross-pass assertion
        // can no longer be tripped by this import's own duplicate the way it once was.
        if let Some(&existing_index) = index_by_id.get(&object_id) {
            if !prepared_matches_existing(
                source,
                &prepared,
                existing_index,
                real_frame_digest,
                &envelope,
            )? {
                return Err(PrikkError::MalformedData(format!(
                    "bundle carries object {object_id} twice with different envelopes -- \
                     refusing the whole bundle"
                )));
            }
            continue;
        }

        let already_present = store.contains_object(envelope.object_type, object_id);
        let object_type = envelope.object_type;
        // 0.55.0 step 3 Part 3b-ii: extracted before `admit_one`, not after -- closure metadata
        // never depends on the admission result, and `admit_one` now takes `envelope` by value
        // (below), so this must read it first.
        let (same_kind_dependencies, closure) =
            closure_metadata_for(&envelope, Some(layout.format()))?;
        // RFC 156 §4 rules 1-3, format 7: for every carried object this repository already holds,
        // which of its signatures may join the stored record. Verified here, before anything is
        // written; an invalid one refuses the whole import.
        let mut merged = false;
        if pass1_refusal.is_none() {
            match admitter.admit_one(&*store, envelope, &author_material_closure) {
                Ok(admitted) => {
                    if let Err(error) = store.check_write(&admitted.envelope) {
                        pass1_refusal = Some(ImportRefusal::new(
                            "store-refused",
                            Some((object_type, object_id)),
                            error,
                        ));
                    }
                    merged = admitted.merged;
                    // 0.55.0 step 3 Part 3b-ii2 (N1): `admitted.envelope` (the decoded payload, same size as `body` above) is never
                    // read again once `check_write` has judged it -- pass 1 keeps only `merged` from here on.
                    drop(admitted);
                }
                Err(error) => {
                    pass1_refusal = Some(ImportRefusal::new(
                        "admission-refused",
                        Some((object_type, object_id)),
                        error,
                    ));
                }
            }
        }
        #[cfg_attr(not(test), allow(unused_mut))]
        let mut admission_outcome = if !already_present {
            AdmissionOutcomeKind::New
        } else if merged {
            AdmissionOutcomeKind::Merged
        } else {
            AdmissionOutcomeKind::AlreadyPresent
        };
        #[cfg(test)]
        {
            admission_outcome = corrupted_admission_outcome_for_test(object_id, admission_outcome);
        }
        #[cfg_attr(not(test), allow(unused_mut))]
        let mut frame_digest = real_frame_digest;
        #[cfg(test)]
        {
            frame_digest = corrupted_frame_digest_for_test(object_id, frame_digest);
        }

        let index = prepared.len();
        prepared.push(PreparedObject {
            object_id,
            object_type,
            same_kind_dependencies,
            frame_range: start..end,
            frame_digest,
            admission_outcome,
        });
        index_by_id.insert(object_id, index);
        membership.insert(object_id, object_type);
        closure_by_id.insert(object_id, closure);
    }

    // The same structural/closure validation `validate_bundle_contents` used to run before any
    // lock was even taken (Q1): now run against the cheap metadata collected above, under this
    // same held lock, after admission -- both are decide-only (neither writes), so running
    // admission first changes only which refusal surfaces for a bundle invalid on more than one
    // ground, never the "nothing written on any refusal" guarantee.
    // 0.55.0 step 3 Part 3b-iv: `validate_bundle_closure` also returns the resolved tip block --
    // `import` itself has no use for it (a received pointer names a RefState, not a Block), so
    // it's discarded here; `verify`/`preview` are the callers that need it.
    let closure = validate_bundle_closure(
        source,
        &origin_ref_name,
        &prepared,
        &index_by_id,
        &membership,
        &closure_by_id,
        Some(&*store),
        &author_keys,
        manifest.as_ref(),
        count,
    );
    let (ref_state_id, bundle_key_ids, tip_block_id) = match closure {
        Ok(resolved) => resolved,
        // A delivery that is broken as well as refused says what the import said first: the pass-1 refusal, as it always did.
        Err(error) => return Err(pass1_refusal.map_or(error, |refusal| refusal.error)),
    };

    // RFC 163 §2, Addendum 1 item 1 / the author-key conflict check: both still run before the
    // first object write, under the same lock the pointer write later runs under.
    let mut refusals: Vec<ImportRefusal> = pass1_refusal.into_iter().collect();
    for (key_id, public_key) in &bundle_key_ids {
        if let Err(error) = check_author_key_conflict(layout, key_id, *public_key) {
            refusals.push(ImportRefusal::new("author-key-conflict", None, error));
            break;
        }
    }

    // `BundleImportReport` carries no manifest field (unlike `BundleVerifyReport`/
    // `BundlePreviewReport`) -- `manifest` above was consulted only for the cross-check and is
    // dropped here, same as the pre-streaming code's own `contents.manifest` was for this caller.

    // 0.56.0 step 3a (A2/A3): read the pointer this delivery would replace, classify it, and
    // refuse a non-fast-forward by default -- all still before the first object write (R4's own
    // discipline below), under the `ReceivedIndex`/`ObjectStore` locks already held continuously
    // since before pass 1. Pass 1 has already finished, so the delivered tip's full closure
    // (including whatever RefState chain it carries) is already resolved in `prepared`.
    let received_pointer = crate::received::read_received_pointer(layout, &received_ref_name)?;
    // Purely over pass-1 metadata already in memory (`closure_by_id`) -- no reader, no re-decode
    // of anything the bundle carries; see `classify_import_connectivity`'s own doc.
    let (connectivity_class, ref_state_chain) = match &received_pointer {
        None => (BundleConnectivityClass::NoLocalHistory, None),
        Some(pointer) => {
            let (class, chain) = classify_import_connectivity(
                &*store,
                &closure_by_id,
                pointer,
                tip_block_id,
                ref_state_id,
                &received_ref_name,
            )?;
            (class, Some(chain))
        }
    };
    #[cfg(test)]
    let connectivity_class = force_connectivity_class_for_test(connectivity_class);
    // A3: when a pointer already existed, a rollback, a divergent rewrite, or an unrelated history
    // all refuse by default -- naming the class, as its own `refusal-v1` id would once `bundle
    // import --format json` exists (0.56.0 step 3b, not this round).
    if let Some(refusal) = non_fast_forward_refusal(
        received_pointer.is_some(),
        connectivity_class,
        &received_ref_name,
        options.allow_non_fast_forward,
    ) {
        refusals.push(ImportRefusal::new("class", None, refusal));
    }

    // 0.56.0 F2 item 1 (stikk's part-6 finding, one block back): refuse a forged MAINTAINER
    // signature on *every* Block and RefState this delivery is about to admit, not only the
    // delivered tip's own -- the review's own measured gap, `.git-exclude/reviewed/
    // 0.56.0-step-3-f-review-v1.md` "F2 item 1": a sound tip over a forged ancestor used to
    // import clean (the old check here resolved only `ref_state_id`/`tip_block_id`), and only a
    // later `verify` caught it. `prepared` already carries at most one entry per unique object
    // id in this bundle (pass 1's own dedup, `index_by_id.insert` above never runs twice for the
    // same id), so filtering it for `Block`/`RefState` and checking each once is exactly "every
    // Block and RefState, deduplicated" -- one Ed25519 verification per signature, linear in the
    // delivery, on objects pass 1 has already decoded. This subsumes the former tip-only check
    // (the tip's own ids are themselves entries in `prepared`), so there is no separate tip check
    // left to run. Read through a bundle-only `BundleAndLocalReader`, built the same way
    // `preview_bundle` already builds one (`local: None` -- nothing here is about the local
    // store, only the bundle's own delivered content). `&mut *source` reborrows rather than moves
    // `source`, so the outer binding is still usable for the write loop below once this block
    // ends. Still before the first object write, the same "all still before the first object
    // write" zone as the connectivity refusal just above.
    let adopted_policy = crate::trust::load_maintainer_trust_policy_or_empty(layout)?;
    {
        let source_cell: RefCell<&mut BundleSource> = RefCell::new(&mut *source);
        let cache: RefCell<BTreeMap<ObjectId, ObjectEnvelope>> = RefCell::new(BTreeMap::new());
        let bundle_only_reader = BundleAndLocalReader {
            source: &source_cell,
            prepared: &prepared,
            index_by_id: &index_by_id,
            cache: &cache,
            local: None,
        };
        // 0.57.0 step 1b: the one check, shared with `bundle preview` (which reports what it
        // finds instead of refusing).
        if let Some(forged) = find_forged_publication_signature(
            "bundle-import",
            &adopted_policy,
            &bundle_only_reader,
        )? {
            // At most one signature entry (0.57.0 step 1b2): the first the import meets. A held-object forgery found in pass 1 is met
            // before the class check, a new-object one after it; the same forged object is not listed twice.
            if !refusals
                .iter()
                .any(|refusal| refusal.id == "forged-publication-signature")
            {
                refusals.push(ImportRefusal::new(
                    "forged-publication-signature",
                    Some((forged.object_type, forged.object_id)),
                    forged.error,
                ));
            }
        }
    }

    // 0.57.0 S1b: decide, still before the first write, whether `--adopt` lands the delivered branch as a local branch (see
    // `bundle/adopt.rs`): the shared start (absent ref, queue, class), the signer's standing, and the whole RefState chain's trust.
    // A delivery that cannot be adopted still lands under `remotes/`, with the reason reported. For the current branch the clean
    // worktree and nothing-in-the-way refusals are here too, so they too leave nothing written.
    let adoption_decision = {
        let source_cell: RefCell<&mut BundleSource> = RefCell::new(&mut *source);
        let cache: RefCell<BTreeMap<ObjectId, ObjectEnvelope>> = RefCell::new(BTreeMap::new());
        let reader = BundleAndLocalReader {
            source: &source_cell,
            prepared: &prepared,
            index_by_id: &index_by_id,
            cache: &cache,
            local: Some(&*store),
        };
        let planned = adopt::plan_adoption(
            layout,
            adoption,
            &origin_ref_name,
            &received_ref_name,
            ref_state_id,
            connectivity_class,
            ref_state_chain,
            options.allow_non_fast_forward,
            &adopted_policy,
            &reader,
        );
        match planned {
            Ok(decision) => match (&decision, adoption) {
                (adopt::AdoptionDecision::Adopt(_), Some(adoption)) => {
                    match adopt_local_ref::refuse_current_branch_obstacles(
                        layout,
                        adoption.current_branch.as_deref(),
                        &origin_ref_name,
                        &reader,
                        tip_block_id,
                    ) {
                        Ok(()) => decision,
                        // Refused: nothing is adopted, and the report says where it would land only when it would land.
                        Err(error) => {
                            refusals.push(ImportRefusal::new("adoption-refused", None, error));
                            adopt::AdoptionDecision::NotRequested
                        }
                    }
                }
                _ => decision,
            },
            // A hard adoption refusal. The class refusal is raised again from inside it (the shared adoption start): that one is already
            // collected, so it is not listed twice.
            Err(error) => {
                if !matches!(error, PrikkError::BundleNonFastForward { .. }) {
                    refusals.push(ImportRefusal::new("adoption-refused", None, error));
                }
                adopt::AdoptionDecision::NotRequested
            }
        }
    };
    // 0.49.0 step 0, finding 3 / 0.55.0 step 3 Part 3b-i (R1): dependency order, computed purely
    // over `prepared`'s own cheap `(id, type, dependency ids)` metadata.
    let write_order = dependency_order_indices(&prepared)?;

    Ok(ImportDecision {
        origin_ref_name,
        received_ref_name,
        object_count,
        prepared,
        index_by_id,
        author_keys,
        manifest,
        ref_state_id,
        tip_block_id,
        class: connectivity_class,
        ref_state_chain,
        adoption: adoption_decision,
        write_order,
        refusals,
    })
}
