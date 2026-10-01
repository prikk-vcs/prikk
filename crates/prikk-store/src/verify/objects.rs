//! Container-based object verification (RFC 102 Stage 3) and dormant loose-file temp-debris
//! classification (design-v1.md §12.3 item 3: kept, cannot fire under format-3, not removed as a
//! side effect of this stage).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectEnvelope, ObjectId, ObjectType};

use super::{
    AuthorSignatureVerification, BlockSealVerification, ObjectVerification,
    PublicationTrustVerifier, verify_block_payload,
};
use crate::block_state::{BlockStateOutcome, LineageStateMemo, verify_blocks_topological};
use crate::foundation::container::{self, ContainerRecordOutcome, ContainerRecordStatus};
use crate::foundation::fsutil::{EntryKind, inspect_entry, list_directory, read_file_if_exists};
use crate::foundation::index::replay_index;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, persisted_object_types};
use crate::object_store::ObjectReader;
use crate::signature_diagnostics::{
    SignatureEnvelopeIssue, SignatureEnvelopeSource, classify_signature_envelope,
};

/// Outcome of attempting to verify one persisted object record (DC-95 Stage 2 Level 2, Phase A). No
/// `NotEvaluated` variant: Phase A's per-object checks (decode, schema, signature, trust, reference
/// existence) have no real dependency on any *other* object's own outcome (Step 0 §1.1) -- every
/// object is independently attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectItemStatus {
    /// The object's own checks all passed, and it has a matching index entry.
    Evaluated(ObjectVerification),
    /// The object's own checks all passed, but no index entry names it (design-v1.md §12/§10.2's
    /// ruling): rebuildable, so **not** a failure -- explicitly excluded from `has_item_failure()`,
    /// the same way `Evaluated` is. Carries the same data `Evaluated` does; only the classification
    /// differs.
    Unindexed(ObjectVerification),
    /// Some check for this specific object failed -- either the container record's own framing
    /// (bad checksum, malformed envelope) or a downstream check (schema, signature, trust,
    /// reference existence). Its signature-envelope findings and (for a `Block`) merge-baseline
    /// divergence and `pending_v3_blocks` contribution are *not* recorded -- this object's own
    /// verification did not run to completion, so nothing derived partway through it is reported.
    Failed {
        /// The error the check raised.
        message: String,
    },
}

/// One object record's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectItemOutcome {
    /// The object-type container this record was scanned under.
    pub object_type: ObjectType,
    /// A display-only locator for this record -- `container_slot_path`'s own path with the record's
    /// byte offset appended (`#1234`). Not a real filesystem path (a container holds many records
    /// per file); kept as `PathBuf` because every consumer of this field only ever calls `.display()`
    /// on it.
    pub path: PathBuf,
    /// How this object's own verification resolved.
    pub status: ObjectItemStatus,
}

/// A frame in an object container that does not parse and is not *complete* (its own checksum never verified, so it cannot be a
/// structurally sound envelope that merely failed a later check): an **interrupted append** (RFC 162 rule 2, superseding RFC 160 F3
/// Addendum 1's index-membership rule -- "the index is never evidence of anything"). It is reported here as a warning, unconditionally
/// -- not damage by default. Separately, `verify.rs`'s own connectivity pass checks whether anything that still matters (a sealed
/// block's state, a queued patch, a ref tip) references an object this scan could not read; if so, *that* check fails and names the
/// referencing work directly, regardless of whether the corresponding frame is reported here as a remnant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InterruptedAppend {
    /// The object-type container the frame is in.
    pub object_type: ObjectType,
    /// A display-only locator, as [`ObjectItemOutcome::path`]'s.
    pub path: PathBuf,
    /// The frame's byte offset in its container.
    pub offset: usize,
    /// Why the frame does not parse.
    pub message: String,
}

pub(super) struct ObjectSummary {
    /// Phase A: one outcome per object record scanned, in scan order (container order, per type, in
    /// `persisted_object_types()` order).
    pub(super) item_outcomes: Vec<ObjectItemOutcome>,
    /// Phase B: one outcome per `CurrentV6` Block whose Phase A check succeeded, in the
    /// state-dependency order `verify_blocks_topological` resolved them (DC-92 §4.2) -- not scan
    /// order.
    pub(super) topological_outcomes: Vec<BlockStateOutcome>,
    pub(super) temp_paths: Vec<PathBuf>,
    pub(super) signature_issues: Vec<SignatureEnvelopeIssue>,
    pub(super) merge_baseline_divergences: Vec<super::MergeBaselineDivergence>,
    pub(super) block_seals: Vec<BlockSealVerification>,
    /// Unparseable frames nothing names (see [`InterruptedAppend`]).
    pub(super) interrupted_appends: Vec<InterruptedAppend>,
    /// RFC 162 Addendum 1 fix 3: the object index's own trailing-partial byte count, set once from
    /// the same `index_replay` rule 1 already scans -- never merged by [`Self::add`], since it is a
    /// whole-index fact, not a per-container-type one.
    pub(super) trailing_partial_index_bytes: usize,
    /// RFC 162 Addendum 1 fix 3: whether the object index has an interior record that failed to
    /// decode. Same non-merge rule as `trailing_partial_index_bytes`.
    pub(super) index_interior_damage: bool,
    /// RFC 164 Addendum 1 (N7): one entry per persisted object type, in `persisted_object_types()`
    /// order, naming its own container's trailing partial byte count -- the WAL/pointer-index-style
    /// tail-by-position `decode_container_records` already computes (`sound_frame_after_partial`),
    /// simply never surfaced past this module before now. A genuinely torn frame with nothing sound
    /// after it stays out of `interrupted_appends` (that path is for a frame whose own shape is not
    /// tail-shaped, i.e. `TrailingPartial` never reaches it) -- this is the aggregate count `verify`
    /// reports as a short warning, mirroring the seven Rule-A files' own line exactly.
    pub(super) object_container_tails: Vec<(ObjectType, usize)>,
    /// RFC 164 Rule E: a stored object's own dangling reference, when the object making it is not
    /// itself reachable from committed state.
    pub(super) unreferenced_remnants: Vec<super::UnreferencedRemnant>,
}

impl ObjectSummary {
    fn empty() -> Self {
        Self {
            item_outcomes: Vec::new(),
            topological_outcomes: Vec::new(),
            temp_paths: Vec::new(),
            signature_issues: Vec::new(),
            merge_baseline_divergences: Vec::new(),
            block_seals: Vec::new(),
            interrupted_appends: Vec::new(),
            trailing_partial_index_bytes: 0,
            index_interior_damage: false,
            object_container_tails: Vec::new(),
            unreferenced_remnants: Vec::new(),
        }
    }

    fn add(&mut self, other: Self) {
        self.item_outcomes.extend(other.item_outcomes);
        self.topological_outcomes.extend(other.topological_outcomes);
        self.temp_paths.extend(other.temp_paths);
        self.signature_issues.extend(other.signature_issues);
        self.merge_baseline_divergences
            .extend(other.merge_baseline_divergences);
        self.block_seals.extend(other.block_seals);
        self.interrupted_appends.extend(other.interrupted_appends);
        self.object_container_tails
            .extend(other.object_container_tails);
        self.unreferenced_remnants
            .extend(other.unreferenced_remnants);
    }
}

pub(super) fn verify_objects(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    trust_verifier: &mut PublicationTrustVerifier<'_>,
) -> Result<ObjectSummary> {
    // DC-92 §4.2: Phase A (below) collects every CurrentV6 Block's already-decoded payload instead
    // of verifying its state inline, in whatever order the generic scan visits objects. Phase B
    // (`verify_blocks_topological`, after the loop) verifies them in state-dependency order instead,
    // against one shared memo constructed here and evicted from as it goes.
    //
    // DC-95 Stage 2 Level 2: Phase A and Phase B are independently item-contained (Step 0 §1). A
    // single object's own Phase A failure does not prevent scanning every other object. Phase B's
    // own item containment lives in `verify_blocks_topological` itself.
    let mut lineage_memo = LineageStateMemo::new();
    let mut pending_v3_blocks: Vec<(ObjectId, BlockPayload)> = Vec::new();
    let mut summary = ObjectSummary::empty();

    // RFC 162 rule 1: the object index is a pure cache, and a reader never refuses while the
    // containers are sound. This used to abort the whole `Objects` stage on any index item failure
    // (RFC 102 Stage 2's own reasoning, since superseded); now, when the index itself is damaged, its
    // membership is derived from a fresh in-memory container scan instead -- the same fallback
    // `object_store.rs::resolve_object_location` uses for every other reader, chosen here for the same
    // reason: `verify` is documented read-only end to end (RFC 111 §6.1), so it must not persist a
    // repair the way `doctor --repair-index` does.
    let index_replay = replay_index(layout)?;
    // Addendum 1 fix 3: "accepted" (rule 1's own scan-fallback below) must not mean "unreported" --
    // captured once here, from the same replay rule 1 already pays for, before it is either kept or
    // discarded in favor of a fresh container scan.
    summary.trailing_partial_index_bytes = index_replay.trailing_partial_bytes;
    summary.index_interior_damage = index_replay.has_item_failure();
    let index_entries: Vec<crate::foundation::index::IndexEntry> =
        if index_replay.has_item_failure() {
            crate::foundation::index::rebuild_index_from_containers(layout)?
        } else {
            index_replay.entries.clone()
        };
    // RFC 160 F3: **the small record containers `verify` would otherwise read only by chance.** The author key index is consulted only
    // when a patch sits in a container, the trust policy only when a block or ref state needs its signer checked, and an unsealed
    // repository needs neither -- so a damaged entry in either (a header whose length no record could have, a snapshot whose body will
    // not decode) was reported by nobody unless some other check happened to read it. Each is reported here, on the same footing as
    // the object index above: a stage failure naming the container, before anything is classified against it.
    for (name, damaged) in [
        (
            "author key container",
            crate::author::author_key_index::replay_author_keys(layout)?.has_item_failure(),
        ),
        (
            "trust key container",
            crate::trust_index::replay_trust_keys(layout)?.has_item_failure(),
        ),
        (
            "trust policy container",
            crate::trust_index::replay_trust_policy(layout)?.has_item_failure(),
        ),
    ] {
        if damaged {
            return Err(PrikkError::Integrity(format!(
                "{name} has a damaged entry; run doctor for diagnosis (nothing is repaired automatically)"
            )));
        }
    }
    let indexed_ids: HashSet<ObjectId> =
        index_entries.iter().map(|entry| entry.object_id).collect();

    // design-v1.md §12/§10.2's ruling: "the bytes found are validated by recomputing the content
    // hash... a mismatch is a reported defect." Ordinary reads (`FileObjectStore::read_object`) check
    // this lazily, one id at a time. `verify`'s own job is the full, proactive scan (the same ruling:
    // "`verify` does the full scan") -- so every index entry is cross-checked here against what its
    // own claimed location actually decodes to, not left to be discovered only if and when something
    // happens to read that exact id later.
    //
    // A *decode* failure at the entry's own location (checksum mismatch, malformed envelope) is
    // deliberately **not** escalated here -- it is already reported as its own item-level
    // `ObjectItemStatus::Failed` by the per-record container scan below (RFC 102 Stage 2's
    // isolate-and-continue containment), and re-erroring on it here would turn an already-contained
    // item defect into a whole-stage abort. Only a location that decodes *successfully but to the
    // wrong id* is a genuine index-integrity defect, not merely a damaged record the index happens to
    // point at.
    //
    // **But an indexed entry whose record cannot be read at all is reported by nobody unless it is reported here** (RFC 160 §7). The
    // container scan tolerates a frame whose header claims more bytes than remain as a *torn tail* -- the harmless remnant of an
    // interrupted append -- and a genuine torn tail can never have an index entry, because the index is appended only after the
    // record is durable. So an entry that names a record no read can produce is damage the scan calls a torn tail; it is collected
    // here and reported as its own `Failed` item after the scan, unless the scan already reported a failure at that same offset.
    let mut unreadable: Vec<(&crate::foundation::index::IndexEntry, String)> = Vec::new();
    for entry in &index_entries {
        let envelope = match crate::foundation::index::read_object_envelope_at(layout, entry) {
            Ok(envelope) => envelope,
            Err(err) => {
                unreadable.push((entry, err.to_string()));
                continue;
            }
        };
        let computed = envelope.object_id();
        if computed != entry.object_id {
            return Err(PrikkError::Integrity(format!(
                "index entry for {} resolves to an envelope with computed id {computed}",
                entry.object_id
            )));
        }
    }

    // RFC 164 Rule E: computed once per run, from committed state only, and reused for every
    // object type's own missing-reference classification below.
    let reachable = super::reachability::compute_reachable_object_ids(layout, object_store)?;
    for object_type in persisted_object_types() {
        summary.add(verify_object_type_container(
            layout,
            object_store,
            object_type,
            trust_verifier,
            &mut pending_v3_blocks,
            &indexed_ids,
            &reachable,
        )?);
    }
    for (entry, message) in unreadable {
        let locator = layout
            .container_slot_path(entry.object_type, entry.slot)
            .join(format!("#{}", entry.offset));
        let already_reported = summary.item_outcomes.iter().any(|outcome| {
            outcome.object_type == entry.object_type
                && outcome.path == locator
                && matches!(outcome.status, ObjectItemStatus::Failed { .. })
        });
        // RFC 162 rule 2: the container scan no longer knows about index membership, so it always
        // classifies this same frame as an unreferenced remnant first (`interrupted_appends`) -- an
        // entry that still names it, promoted to `Failed` here, is damage, not an unreferenced one, so
        // drop the now-contradictory warning for the same offset before adding the failed item.
        let entry_offset = usize::try_from(entry.offset).ok();
        summary.interrupted_appends.retain(|remnant| {
            !(remnant.object_type == entry.object_type && Some(remnant.offset) == entry_offset)
        });
        if already_reported {
            continue;
        }
        summary.item_outcomes.push(ObjectItemOutcome {
            object_type: entry.object_type,
            path: locator,
            status: ObjectItemStatus::Failed {
                message: format!(
                    "index entry for {} names a record at offset {} that cannot be read: {message}",
                    entry.object_id, entry.offset
                ),
            },
        });
    }
    summary.temp_paths = scan_loose_file_temp_debris(layout)?;

    let topological =
        verify_blocks_topological(object_store, &pending_v3_blocks, &mut lineage_memo)?;
    summary.topological_outcomes = topological.outcomes;
    Ok(summary)
}

fn verify_object_type_container(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    object_type: ObjectType,
    trust_verifier: &mut PublicationTrustVerifier<'_>,
    pending_v3_blocks: &mut Vec<(ObjectId, BlockPayload)>,
    indexed_ids: &HashSet<ObjectId>,
    reachable: &std::collections::BTreeSet<ObjectId>,
) -> Result<ObjectSummary> {
    let mut summary = ObjectSummary::empty();
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("verify-scan");
    let container_path = layout.container_slot_path(object_type, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        // Every container name is allocated at `init` (handoff criterion 1); a missing file here is
        // the same "nothing to scan" case an empty container already reads as, not a structural
        // error -- mirrors `Wal::replay()`'s own missing-file tolerance.
        return Ok(summary);
    };
    let replay = container::decode_container_records(object_type, &bytes)?;
    // RFC 164 Addendum 1 (N7): a reporting-only aggregate, deliberately not a reclassification.
    // `replay.trailing_partial_bytes` is only ever set by the narrow case where fewer bytes remain
    // than one header (`TrailingPartial`) -- object containers' own `Invalid` branch, unlike the
    // seven Rule-A files' decode loops, never checks whether a sound frame follows a longer garbage
    // run before marking it `Failed`, so a longer positional tail still reaches `interrupted_appends`
    // as a per-record warning instead. When the **last** outcome in this container is exactly that
    // shape (`Failed { complete: false, .. }`, meaning nothing sound was found after it -- otherwise
    // a later `Evaluated` outcome would exist), the bytes from its own offset to end of file are, in
    // fact, positionally a tail: summed here for the same aggregate line the other files get, without
    // touching the underlying resync/classification logic at all.
    let tail_bytes = if replay.trailing_partial_bytes != 0 {
        replay.trailing_partial_bytes
    } else {
        match replay.record_outcomes.last() {
            Some(ContainerRecordOutcome {
                offset,
                status:
                    ContainerRecordStatus::Failed {
                        complete: false, ..
                    },
            }) => bytes.len().saturating_sub(*offset),
            _ => 0,
        }
    };
    summary
        .object_container_tails
        .push((object_type, tail_bytes));

    // RFC 102 Stage 2: `records` holds only sound frames, in the same order `record_outcomes`
    // visits its `Evaluated` entries -- both built in lockstep by `decode_container_records`.
    let mut records = replay.records.into_iter();
    for outcome in &replay.record_outcomes {
        let locator = container_path.join(format!("#{}", outcome.offset));
        let ContainerRecordStatus::Evaluated { .. } = &outcome.status else {
            let ContainerRecordStatus::Failed { message, complete } = &outcome.status else {
                return Err(PrikkError::Integrity(
                    "container record outcome is neither Evaluated nor Failed".to_string(),
                ));
            };
            // RFC 162 rule 2: **index membership is no longer the witness.** A frame that does not parse is an unreferenced remnant,
            // reported as a warning, unless it is *complete*: a frame whose own checksum verified (an envelope that will not decode, the
            // wrong type) is not what an interrupted append leaves, which is a prefix of a frame. Connectivity -- whether anything still
            // referencing work (a sealed block's state, a queued patch, a ref tip) names an object this scan cannot read -- is checked
            // separately, in `verify.rs`'s own connectivity pass, and names the referencing work directly rather than reclassifying the
            // frame itself (RFC 160 F3 Addendum 1's index-membership rule is what this replaces).
            if *complete {
                summary.item_outcomes.push(ObjectItemOutcome {
                    object_type,
                    path: locator,
                    status: ObjectItemStatus::Failed {
                        message: message.clone(),
                    },
                });
            } else {
                summary.interrupted_appends.push(InterruptedAppend {
                    object_type,
                    path: locator,
                    offset: outcome.offset,
                    message: message.clone(),
                });
            }
            continue;
        };
        let Some(record) = records.next() else {
            return Err(PrikkError::Integrity(
                "container replay outcome/record count mismatch".to_string(),
            ));
        };
        // DC-95 Stage 2 Level 2: this object's own failure is caught here, at the item boundary,
        // rather than propagated -- every *other* record in this and every other container is
        // still attempted.
        let mut ctx = ObjectRecordContext {
            pending_v3_blocks: &mut *pending_v3_blocks,
            classifier: super::ReachabilityClassifier {
                reachable,
                remnants: &mut summary.unreferenced_remnants,
            },
        };
        match verify_object_record(
            layout,
            object_store,
            object_type,
            &locator,
            &record.envelope,
            trust_verifier,
            &mut ctx,
        ) {
            Ok((object, signature_issues, merge_baseline_divergence)) => {
                summary.signature_issues.extend(signature_issues);
                summary
                    .merge_baseline_divergences
                    .extend(merge_baseline_divergence);
                if object.object_type == ObjectType::Block {
                    if let Some(sealed_by_key_id) = object.sealed_by_key_id.clone() {
                        summary.block_seals.push(BlockSealVerification {
                            block_id: object.object_id,
                            sealed_by_key_id,
                        });
                    }
                }
                let status = if indexed_ids.contains(&object.object_id) {
                    ObjectItemStatus::Evaluated(object)
                } else {
                    ObjectItemStatus::Unindexed(object)
                };
                summary.item_outcomes.push(ObjectItemOutcome {
                    object_type,
                    path: locator,
                    status,
                });
            }
            Err(err) => {
                summary.item_outcomes.push(ObjectItemOutcome {
                    object_type,
                    path: locator,
                    status: ObjectItemStatus::Failed {
                        message: err.to_string(),
                    },
                });
            }
        }
    }
    Ok(summary)
}

/// `pending_v3_blocks` and the RFC 164 Rule E classifier, bundled purely to keep
/// `verify_object_record`'s own argument count low (`clippy::too_many_arguments`).
struct ObjectRecordContext<'a> {
    pending_v3_blocks: &'a mut Vec<(ObjectId, BlockPayload)>,
    classifier: super::ReachabilityClassifier<'a>,
}

fn verify_object_record(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    object_type: ObjectType,
    locator: &Path,
    envelope: &ObjectEnvelope,
    trust_verifier: &mut PublicationTrustVerifier<'_>,
    ctx: &mut ObjectRecordContext<'_>,
) -> Result<(
    ObjectVerification,
    Vec<SignatureEnvelopeIssue>,
    Option<super::MergeBaselineDivergence>,
)> {
    // `object_type` mismatch is impossible to reach here: `container::parse_frame_at` checks
    // `envelope.object_type != object_type` itself, right after decoding, so a frame whose body
    // claims a different type than the container it lives in already surfaced as
    // `ContainerRecordStatus::Failed` and never reaches this function at all.
    crate::format::validate_read_schema(layout.format(), envelope)?;
    let object_id = envelope.object_id();
    let signature_issues = classify_signature_envelope(
        envelope,
        SignatureEnvelopeSource::Object {
            object_type,
            object_id,
        },
    )?;
    let sealed_by_key_id = if matches!(object_type, ObjectType::Block | ObjectType::RefState) {
        trust_verifier.verify(envelope)?
    } else {
        None
    };
    // DC-53 Stage 1: a Patch's AUTHOR signature is checked against recorded key material here. A
    // signature that fails to verify against *recorded* material propagates as an `Err` via `?`
    // below -- it never reaches `author_verification` -- because that outcome is a genuine
    // authorship-integrity defect (forgery or corruption), not a trust opinion (D3).
    let author_verification = if object_type == ObjectType::Patch {
        crate::author::author_key_index::verify_author_signature(layout, envelope)?.map(
            |(key_id, sound)| {
                if sound {
                    AuthorSignatureVerification::Sound { key_id }
                } else {
                    AuthorSignatureVerification::Unverifiable { key_id }
                }
            },
        )
    } else {
        None
    };
    let (rollback_patch_count, merge_baseline_divergence) = if object_type == ObjectType::Block {
        verify_block_payload(
            object_store,
            object_id,
            layout.format(),
            &envelope.canonical_payload,
            ctx.pending_v3_blocks,
            &mut ctx.classifier,
        )?
    } else {
        (0, None)
    };
    Ok((
        ObjectVerification {
            object_id,
            object_type,
            path: locator.to_path_buf(),
            rollback_patch_count,
            sealed_by_key_id,
            author_verification,
        },
        signature_issues,
        merge_baseline_divergence,
    ))
}

/// Scan every persisted object type's **loose-file** directory tree for `.pobj.tmp.` debris only
/// (design-v1.md §12.3 item 3: `object_temp_paths`/`PRIKK-DOCTOR-OBJECT-TEMP-DEBRIS` are kept,
/// dormant -- a format-3 repository can no longer produce this debris via `FileObjectStore`, which
/// now writes containers, but retiring the diagnostic is an RFC-level act alongside G5, not a side
/// effect of this stage). A real (non-temp) `.pobj` file found here is unconditionally unexpected
/// under format-3 -- nothing writes one -- and fails closed exactly like the pre-existing structural
/// checks below already do for a non-directory/non-file in the wrong place.
fn scan_loose_file_temp_debris(layout: &RepositoryLayout) -> Result<Vec<PathBuf>> {
    let mut temp_paths = Vec::new();
    for object_type in persisted_object_types() {
        let dir = layout.object_type_dir(object_type);
        let relative_dir = layout.repository_relative(&dir)?;
        match inspect_entry(layout.repository_mutation_root(), &relative_dir)? {
            None => continue,
            Some(EntryKind::Directory) => {}
            Some(_) => {
                return Err(PrikkError::Integrity(format!(
                    "unexpected non-directory in object type directory: {}",
                    dir.display()
                )));
            }
        }
        let mut prefix_entries = list_directory(layout.repository_mutation_root(), &relative_dir)?;
        prefix_entries.sort_by(|left, right| {
            left.name
                .as_encoded_bytes()
                .cmp(right.name.as_encoded_bytes())
        });
        for prefix_entry in prefix_entries {
            let prefix_path = dir.join(&prefix_entry.name);
            if prefix_entry.kind != EntryKind::Directory {
                return Err(PrikkError::Integrity(format!(
                    "unexpected non-directory in object type directory: {}",
                    prefix_path.display()
                )));
            }
            let relative_prefix = layout.repository_relative(&prefix_path)?;
            let mut entries = list_directory(layout.repository_mutation_root(), &relative_prefix)?;
            entries.sort_by(|left, right| {
                left.name
                    .as_encoded_bytes()
                    .cmp(right.name.as_encoded_bytes())
            });
            for entry in entries {
                let path = prefix_path.join(&entry.name);
                if entry.kind != EntryKind::Regular {
                    return Err(PrikkError::Integrity(format!(
                        "unexpected non-file in object prefix directory: {}",
                        path.display()
                    )));
                }
                if is_object_temp_path(&path) {
                    temp_paths.push(path);
                    continue;
                }
                return Err(PrikkError::Integrity(format!(
                    "unexpected loose object file under format-3 (containers own object storage \
                     now): {}",
                    path.display()
                )));
            }
        }
    }
    Ok(temp_paths)
}

fn is_object_temp_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some((object_name, suffix)) = name.split_once(".pobj.tmp.") else {
        return false;
    };
    let Some((pid, random)) = suffix.split_once('.') else {
        return false;
    };
    object_name.len() == 64
        && object_name.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && random.len() == 32
        && random.bytes().all(|byte| byte.is_ascii_hexdigit())
}
