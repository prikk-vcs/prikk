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
use crate::foundation::container::{self, ContainerRecordStatus};
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

    // RFC 102 Stage 2: a damaged index entry blocks `index::lookup_object_location` (a single
    // lookup), but Phase A here needs *membership*, not a single lookup -- a container-scale
    // question the index's own item-containment already answers via `record_outcomes`, not
    // something this loop needs to re-derive. A damaged index is therefore reported as this whole
    // stage's own structural failure (matching how a damaged repository directory shape already
    // aborts `verify_objects` today), not silently treated as "nothing is indexed."
    let index_replay = replay_index(layout)?;
    if index_replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "object index has a damaged entry; run doctor before verify can classify indexing"
                .to_string(),
        ));
    }
    let indexed_ids: HashSet<ObjectId> = index_replay
        .entries
        .iter()
        .map(|entry| entry.object_id)
        .collect();

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
    for entry in &index_replay.entries {
        let Ok(envelope) = crate::foundation::index::read_object_envelope_at(layout, entry) else {
            continue;
        };
        let computed = envelope.object_id();
        if computed != entry.object_id {
            return Err(PrikkError::Integrity(format!(
                "index entry for {} resolves to an envelope with computed id {computed}",
                entry.object_id
            )));
        }
    }

    for object_type in persisted_object_types() {
        summary.add(verify_object_type_container(
            layout,
            object_store,
            object_type,
            trust_verifier,
            &mut pending_v3_blocks,
            &indexed_ids,
        )?);
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
) -> Result<ObjectSummary> {
    let mut summary = ObjectSummary::empty();
    let container_path = layout.container_slot_path(object_type, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        // Every container name is allocated at `init` (handoff criterion 1); a missing file here is
        // the same "nothing to scan" case an empty container already reads as, not a structural
        // error -- mirrors `Wal::replay()`'s own missing-file tolerance.
        return Ok(summary);
    };
    let replay = container::decode_container_records(object_type, &bytes)?;

    // RFC 102 Stage 2: `records` holds only sound frames, in the same order `record_outcomes`
    // visits its `Evaluated` entries -- both built in lockstep by `decode_container_records`.
    let mut records = replay.records.into_iter();
    for outcome in &replay.record_outcomes {
        let locator = container_path.join(format!("#{}", outcome.offset));
        let ContainerRecordStatus::Evaluated { .. } = &outcome.status else {
            let ContainerRecordStatus::Failed { message } = &outcome.status else {
                return Err(PrikkError::Integrity(
                    "container record outcome is neither Evaluated nor Failed".to_string(),
                ));
            };
            summary.item_outcomes.push(ObjectItemOutcome {
                object_type,
                path: locator,
                status: ObjectItemStatus::Failed {
                    message: message.clone(),
                },
            });
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
        match verify_object_record(
            layout,
            object_store,
            object_type,
            &locator,
            &record.envelope,
            trust_verifier,
            pending_v3_blocks,
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

fn verify_object_record(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    object_type: ObjectType,
    locator: &Path,
    envelope: &ObjectEnvelope,
    trust_verifier: &mut PublicationTrustVerifier<'_>,
    pending_v3_blocks: &mut Vec<(ObjectId, BlockPayload)>,
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
            pending_v3_blocks,
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
