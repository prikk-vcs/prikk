//! `prikk extract`'s own replay (0.54.0 step 3 Part 2): a parallel, write-as-you-go pass over the
//! same block/patch chain [`super::anchor::replay_chain_capturing`] walks, built so that `extract`
//! never holds every live file's content resident at once -- the property `ChainReplay.files`
//! cannot give it (module doc, `anchor.rs`): nothing ever evicts an entry once inserted there.
//!
//! **Why a parallel pass, not a shared one** (the design's own trickiest point, stated precisely
//! here since the report must explain it): `ChainReplay`/`apply.rs` index live state by **path**
//! (`files: BTreeMap<String, Vec<u8>>`), because every entry must be resident to answer "what are
//! this point's bytes" for every caller that function already serves (`cat`, `tree`, `checkout
//! --patch-plan`, merge evidence, ...). `extract` asks a narrower question -- "where do these
//! bytes ultimately come from" -- so this module indexes live state by **node id**
//! ([`ExtractLiveNode`], keyed in [`NodeId`]) and tracks content as one of two shapes:
//!
//! - [`ExtractContent::Blob`]: the node's current content is exactly some stored Blob's bytes,
//!   unread until the caller actually writes this file. True for every file at the admitted anchor
//!   (ruling 1: a snapshot's own entries are Blob ids, text included) and for any binary file
//!   created or replaced after it (`CreateFile`, `ReplaceBinary` never hold binary bytes resident
//!   here -- they just update which Blob id the node now names). Both still pay one bounded read of
//!   the new blob during replay, to confirm the patch's own claim about its kind is actually true
//!   (`apply.rs`'s own `apply_replace_binary` makes the identical check); the bytes are discarded
//!   immediately rather than kept, since only the id is ever needed again.
//! - [`ExtractContent::Resident`]: the node's current content cannot be named by one Blob id any
//!   more -- a text file `EditText` has touched since the anchor (DC-65: the pre-edit blob is
//!   deliberately never rewritten, so a text node's *tracked* id goes stale the moment an edit
//!   lands). The **first** `EditText` after the anchor promotes the node from `Blob` to `Resident`
//!   by reading its base content exactly once; every further edit mutates the same resident bytes
//!   in place, the same splice `apply.rs`'s own `apply_edit_text` performs (duplicated here, not
//!   reused, for the same reason `apply_rename_batch`'s own doc gives for not sharing with
//!   `lifecycle_cache`'s rename resolution: "each was written against its own state's own
//!   indexing").
//!
//! **A renamed node moves for free.** Because content lives under the node id, not the path, a
//! `RenamePath` only ever updates `ExtractLiveNode::path` and this module's own `path -> NodeId`
//! index -- never the content, streamed or resident. A file renamed after the anchor with no
//! content change is still written straight from its anchor blob id, at its **final** path (RFC
//! 157's own node-identity rule, applied to writing rather than listing).
//!
//! **The pre-walk is this module's first phase, not a separate function.** [`replay_for_extract`]
//! searches for an admitted anchor (reading Block envelopes, and -- for a candidate that carries
//! one -- that candidate's own snapshot *manifest* Blob, a bounded, fixed-size validation, never a
//! file-content Blob) and refuses, by name, **before looking at a single Patch**, once no admitted
//! anchor exists and the point's own history reaches back far enough that replaying it from genesis
//! would be the unbounded cost the design exists to avoid paying silently.

use std::collections::BTreeMap;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, NodeKind, ObjectId, text_span_hash};

use crate::anchor_trust::{
    Admission, AnchorSite, AnchorTrust, MAX_ANCHOR_DISTANCE, NotUsableReason,
};
use crate::extract_refusal::{ExtractError, ExtractRefusalKind, NoCheckpointReason};
use crate::lifecycle_cache::replay::LifecycleReplayError;
use crate::object_store::ObjectReader;
use crate::snapshot::SnapshotManifest;
use crate::state_root::StateRootContent;
use crate::text_span;

use super::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations, ensure_apply_supported,
};
use super::read::{read_blob_bytes_with_kind, read_block, read_patch};

/// Where one live node's current content comes from, in extract's own streamed replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtractContent {
    /// Stream straight from this Blob id at write time; never read here.
    Blob(ObjectId),
    /// Held resident: a text file an `EditText` has touched since the anchor.
    Resident(Vec<u8>),
}

/// One node as extract's own replay tracks it -- the unit this module writes from, indexed by node
/// id so a rename moves nothing but the path (module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtractLiveNode {
    path: String,
    kind: NodeKind,
    mode: u32,
    content: ExtractContent,
}

/// One file `extract` will write, at its final (possibly renamed) path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtractFile {
    pub(crate) path: String,
    pub(crate) mode: u32,
    pub(crate) content: ExtractContent,
}

/// What [`replay_for_extract`] produced: the files to write, in canonical path order, and whether a
/// real admitted anchor was used (`provisional: no`) or the replay fell back to a full genesis
/// replay (`provisional: yes`) -- the same signal `replayed_block_count == block_count` already
/// gives the ref/block-point path, restated here since this replay never builds a `PatchReplaySnapshot`.
#[derive(Debug)]
pub(crate) struct ExtractReplayOutcome {
    pub(crate) provisional: bool,
    pub(crate) files: Vec<ExtractFile>,
}

/// **The pre-walk, then the replay** (module doc): find the nearest admitted anchor by walking
/// Block envelopes only (reading one candidate's own snapshot *manifest* only when a candidate
/// carries one), refuse by name before touching a single Patch when no admitted anchor exists and
/// the point's own history is longer than this repository ever expects a healthy lineage to need
/// (`MAX_ANCHOR_DISTANCE`), then replay `block_ids` forward from the anchor (or from genesis) into
/// [`ExtractReplayOutcome`].
///
/// # Errors
///
/// Refuses (`Precondition`, `no-checkpoint-in-reach`) past the distance bound without
/// `allow_full_replay`; otherwise, the replay's own failures -- an unsupported operation, a
/// missing object, or an integrity mismatch (carried with no classification, `ExtractError::kind`
/// is `None`).
pub(crate) fn replay_for_extract(
    reader: &impl ObjectReader,
    block_ids: &[ObjectId],
    trust: &AnchorTrust,
    allow_full_replay: bool,
) -> std::result::Result<ExtractReplayOutcome, ExtractError> {
    let (anchor, provisional) =
        match search_admitted_anchor(reader, block_ids, trust, AnchorSite::Extract)? {
            AnchorSearch::Admitted { index, manifest } => (Some((index, manifest)), false),
            AnchorSearch::GaveUp { reason } => {
                refuse_past_the_bound_unless_allowed(block_ids.len(), allow_full_replay, reason)?;
                (None, true)
            }
        };

    let mut live: BTreeMap<NodeId, ExtractLiveNode> = BTreeMap::new();
    let mut by_path: BTreeMap<String, NodeId> = BTreeMap::new();
    let replay_from = match anchor {
        Some((index, manifest)) => {
            seed_from_manifest(manifest, &mut live, &mut by_path)?;
            index + 1
        }
        None => 0,
    };

    for block_id in block_ids.get(replay_from..).unwrap_or_default() {
        let block = read_block(reader, *block_id)?;
        for patch_id in block.patch_ids {
            let patch = read_patch(reader, patch_id)?;
            let operations =
                decode_patch_operations(&patch.canonical_payload, patch.schema_version)?;
            apply_operations_for_extract(reader, &mut live, &mut by_path, operations)?;
        }
    }

    let mut files: Vec<ExtractFile> = live
        .into_values()
        .map(|node| ExtractFile {
            path: node.path,
            mode: node.mode,
            content: node.content,
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ExtractReplayOutcome { provisional, files })
}

/// What the admitted-anchor search found: an admitted anchor (with its validated manifest, never its
/// files -- the whole point of the pre-walk) at `index` into `block_ids`, or none within the distance
/// bound this repository trusts (§8.2), either because a candidate's manifest or signature failed
/// (a recorded finding ends the search, same as [`super::anchor::find_anchor`]) or because the search
/// ran out of distance to look in.
enum AnchorSearch {
    Admitted {
        index: usize,
        manifest: SnapshotManifest,
    },
    /// 0.54.0 step 3 Part 5d: `reason` is `extract`'s own `no-checkpoint-in-reach` sub-classification,
    /// computed across the whole walk regardless of why it ended (ran out of blocks, the distance
    /// bound, or a `Signal`) -- see [`classify_no_checkpoint_reason`]. The caller decides whether this
    /// is actually a refusal at all (`refuse_past_the_bound_unless_allowed` checks the *chain's* own
    /// total length, not where the walk itself gave up).
    GaveUp { reason: NoCheckpointReason },
}

/// [`super::anchor::find_anchor`]'s own decision (same walk, Block envelopes first, admission only
/// for a candidate that carries a snapshot), **without** that function's own
/// `snapshot_files_from_manifest` call on success -- this is exactly the extra cost the pre-walk
/// exists to never pay before the bound is checked. A manifest Blob (bounded, fixed-size) is still
/// read to validate a candidate; no file-content Blob ever is. `site` is threaded through by the
/// caller (0.54.0 step 3 Part 3b) rather than hardcoded, the same reason `anchor.rs`'s own
/// `find_anchor` threads one: the ref/block-id path (`AnchorSite::Extract`) and the `--claim` path
/// (`AnchorSite::ExtractClaim`) share this identical search, but each must count and diagnose
/// itself under its own name (`anchor_trust.rs`'s own top-of-file doc).
fn search_admitted_anchor(
    reader: &impl ObjectReader,
    block_ids: &[ObjectId],
    trust: &AnchorTrust,
    site: AnchorSite,
) -> Result<AnchorSearch> {
    // 0.54.0 step 3 Part 5d: evidence gathered across the whole walk, over every candidate that
    // carried a snapshot (`block.snapshot_blob_ref.is_some()`), regardless of how the walk itself
    // ends -- `classify_no_checkpoint_reason`'s own priority order is checked once, after the walk,
    // not in walk order.
    let mut saw_snapshot_candidate = false;
    let mut saw_signer_not_adopted = false;
    for (index, block_id) in block_ids.iter().enumerate().rev() {
        let block = read_block(reader, *block_id)?;
        if block.snapshot_blob_ref.is_none() {
            continue;
        }
        saw_snapshot_candidate = true;
        let distance = block_ids.len() - 1 - index;
        match trust.admit(reader, *block_id, distance, site) {
            Admission::Usable(manifest) => return Ok(AnchorSearch::Admitted { index, manifest }),
            Admission::NotUsable(NotUsableReason::TooFar) => {
                return Ok(AnchorSearch::GaveUp {
                    reason: classify_no_checkpoint_reason(
                        saw_snapshot_candidate,
                        saw_signer_not_adopted,
                    ),
                });
            }
            Admission::NotUsable(NotUsableReason::SignerNotAdopted) => {
                saw_signer_not_adopted = true;
            }
            Admission::NotUsable(
                NotUsableReason::NotRecorded | NotUsableReason::OtherNotUsable,
            ) => {}
            Admission::Signal(_finding) => {
                return Ok(AnchorSearch::GaveUp {
                    reason: classify_no_checkpoint_reason(
                        saw_snapshot_candidate,
                        saw_signer_not_adopted,
                    ),
                });
            }
        }
    }
    Ok(AnchorSearch::GaveUp {
        reason: classify_no_checkpoint_reason(saw_snapshot_candidate, saw_signer_not_adopted),
    })
}

/// `no-checkpoint-in-reach`'s own three-way reason, from what [`search_admitted_anchor`]'s own walk
/// found (0.54.0 step 3 Part 5d): `signer-not-adopted` if a recorded checkpoint's signer was ever
/// found untrusted anywhere in the walk (the most specific, actionable reason, so checked first
/// regardless of where in the walk it occurred); else `not-recorded` if any block carried a
/// snapshot at all (whether it was simply never recorded, or failed some other admission check --
/// `NotUsableReason::OtherNotUsable`/a `Signal` fold into this same bucket, since no caller needs a
/// fourth reason today); else `no-checkpoint`.
fn classify_no_checkpoint_reason(
    saw_snapshot_candidate: bool,
    saw_signer_not_adopted: bool,
) -> NoCheckpointReason {
    if saw_signer_not_adopted {
        NoCheckpointReason::SignerNotAdopted
    } else if saw_snapshot_candidate {
        NoCheckpointReason::NotRecorded
    } else {
        NoCheckpointReason::NoCheckpoint
    }
}

/// The pre-walk's own bound check (0.54.0 step 3 Part 3b, factored out so the `--claim` path can
/// share it exactly): past `MAX_ANCHOR_DISTANCE` blocks of history with no admitted anchor found
/// anywhere in it, refuse by name unless `allow_full_replay` is set -- the same refusal
/// `replay_for_extract`'s own inline check already gave, now shared verbatim with
/// `replay_for_extract_chain`'s own pre-walk over the chain's own anchor.
fn refuse_past_the_bound_unless_allowed(
    chain_len: usize,
    allow_full_replay: bool,
    reason: NoCheckpointReason,
) -> std::result::Result<(), ExtractError> {
    refuse_past_the_bound_unless_allowed_for_chain(chain_len, 0, allow_full_replay, reason)
}

/// The same pre-walk bound, extended for a claim chain (0.56.0 step 3e, D3): `hop_count` is how many
/// unsealed claim links stand between the named claim and `anchor_chain_len`'s own real block
/// anchor (0 for the ordinary ref/block-id path and for a single claim whose own declared parent is
/// itself sealed or received -- both already covered by [`refuse_past_the_bound_unless_allowed`]'s
/// own call into this). **The rule (D3, ruled): the anchor's own distance, plus the chain's own hop
/// count, together must not exceed [`MAX_ANCHOR_DISTANCE`]** -- naming both numbers in the refusal,
/// so an operator can tell which part is large. `claim-chain-too-deep` was proposed and not adopted
/// (D3): this is the *same* bound and the *same* id (`no-checkpoint-in-reach`) the ref/block-id path
/// already has, not a second one.
fn refuse_past_the_bound_unless_allowed_for_chain(
    anchor_chain_len: usize,
    hop_count: usize,
    allow_full_replay: bool,
    reason: NoCheckpointReason,
) -> std::result::Result<(), ExtractError> {
    let combined = anchor_chain_len + hop_count;
    if !allow_full_replay && combined > MAX_ANCHOR_DISTANCE + 1 {
        return Err(ExtractError::classified(
            ExtractRefusalKind::NoCheckpointInReach(reason),
            PrikkError::Precondition(format!(
                "no admitted snapshot within {MAX_ANCHOR_DISTANCE} blocks of this point; its own \
                 history reaches back {anchor_chain_len} blocks without one, and this chain adds \
                 {hop_count} further unsealed claim hop(s) beyond that -- older or crafted \
                 history, or a long chain of unsealed claims, per this repository's own checkpoint \
                 cadence; refusing before reading a single patch, to avoid an unbounded replay from \
                 genesis -- pass --allow-full-replay to read it anyway"
            )),
        ));
    }
    Ok(())
}

/// Seed `live`/`by_path` from an admitted anchor's own validated manifest entries -- never their
/// bytes (ruling 1: every entry, text included, is a Blob id). A symlink entry is refused, the same
/// refusal `snapshot_files_from_manifest` gives one, since extract supports the file-level subset
/// only, same as every other patch-replay reader.
fn seed_from_manifest(
    manifest: SnapshotManifest,
    live: &mut BTreeMap<NodeId, ExtractLiveNode>,
    by_path: &mut BTreeMap<String, NodeId>,
) -> Result<()> {
    for entry in manifest.entries {
        let StateRootContent::Blob(blob_id) = entry.content else {
            return Err(PrikkError::UnsupportedObjectType(format!(
                "anchor snapshot holds symlink {}; extract supports the file-level subset only",
                entry.path.as_str()
            )));
        };
        let path = entry.path.as_str().to_string();
        by_path.insert(path.clone(), entry.node_id);
        live.insert(
            entry.node_id,
            ExtractLiveNode {
                path,
                kind: entry.kind,
                mode: entry.mode,
                content: ExtractContent::Blob(blob_id),
            },
        );
    }
    Ok(())
}

/// Apply one decoded operation sequence -- already concatenated across one patch -- onto `live`/
/// `by_path`, batching consecutive `RenamePath` runs exactly as
/// [`super::apply_operation_sequence`]'s own loop does, for the same reason (a swap's two halves
/// must never observe each other's not-yet-moved state).
fn apply_operations_for_extract(
    reader: &impl ObjectReader,
    live: &mut BTreeMap<NodeId, ExtractLiveNode>,
    by_path: &mut BTreeMap<String, NodeId>,
    operations: Vec<super::decode::DecodedPatchOperation>,
) -> Result<()> {
    let mut operations = operations.into_iter().peekable();
    while let Some(first) = operations.next() {
        ensure_apply_supported(&first)?;
        if matches!(first.kind, DecodedOperationKind::RenamePath { .. }) {
            let mut run = vec![first];
            while let Some(next) = operations
                .next_if(|next| matches!(next.kind, DecodedOperationKind::RenamePath { .. }))
            {
                ensure_apply_supported(&next)?;
                run.push(next);
            }
            apply_rename_batch_for_extract(live, by_path, run)?;
        } else {
            apply_one_for_extract(reader, live, by_path, first.kind)?;
        }
    }
    Ok(())
}

fn apply_one_for_extract(
    reader: &impl ObjectReader,
    live: &mut BTreeMap<NodeId, ExtractLiveNode>,
    by_path: &mut BTreeMap<String, NodeId>,
    kind: DecodedOperationKind,
) -> Result<()> {
    match kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => {
            if by_path.contains_key(&path) {
                return Err(PrikkError::Integrity(format!(
                    "CreateFile would overwrite existing path {path}"
                )));
            }
            if live.contains_key(&node_id) {
                return Err(PrikkError::Integrity(
                    "CreateFile would introduce an already-live node_id".to_string(),
                ));
            }
            // The blob is read once, here, to learn whether the new node is text or binary --
            // `DecodedOperationKind::CreateFile` carries no kind field of its own (`apply.rs`'s own
            // ordinary path pays the identical read, for the identical reason). A binary node's
            // bytes are then dropped: only its id is kept, and `extract` streams it fresh at write
            // time, never holding it resident across the rest of this replay. A text node's bytes
            // are kept resident immediately, since they were just read and nothing is saved by
            // discarding them only to re-read them at the first edit, or at write time.
            let (node_kind, bytes) = read_blob_bytes_with_kind(reader, blob_id)?;
            let content = match node_kind {
                NodeKind::BinaryFile => ExtractContent::Blob(blob_id),
                NodeKind::TextFile => ExtractContent::Resident(bytes),
                NodeKind::Symlink => {
                    return Err(PrikkError::UnsupportedObjectType(
                        "CreateFile names a symlink blob; extract supports the file-level subset \
                         only"
                            .to_string(),
                    ));
                }
            };
            by_path.insert(path.clone(), node_id);
            live.insert(
                node_id,
                ExtractLiveNode {
                    path,
                    kind: node_kind,
                    mode,
                    content,
                },
            );
        }
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage:
                DecodedDeletePreimage::File {
                    old_node_kind,
                    old_blob_id,
                    old_mode: _,
                },
        } => {
            let node = live.get(&node_id).ok_or_else(|| {
                PrikkError::Integrity(format!("DeleteNode path is absent: {path}"))
            })?;
            if node.path != path {
                return Err(PrikkError::Integrity(format!(
                    "DeleteNode path {path} does not match live node path {}",
                    node.path
                )));
            }
            if node.kind != old_node_kind {
                return Err(PrikkError::Integrity(
                    "DeleteNode old_node_kind does not match live node kind".to_string(),
                ));
            }
            match &node.content {
                ExtractContent::Blob(current) if *current == old_blob_id => {}
                ExtractContent::Resident(bytes) => {
                    crate::blob_access::ensure_blob_matches_node_kind(
                        bytes,
                        old_blob_id,
                        old_node_kind,
                    )?;
                }
                ExtractContent::Blob(current) => {
                    return Err(PrikkError::Integrity(format!(
                        "DeleteNode old_blob_id mismatch: expected {old_blob_id}, got {current}"
                    )));
                }
            }
            by_path.remove(&path);
            live.remove(&node_id);
        }
        DecodedOperationKind::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            replacement_text,
            old_span_text,
            left_anchor_len,
            right_anchor_len,
        } => {
            let node = live.get(&node_id).ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "EditText failed before blob load: node {node_id:?} is not live"
                ))
            })?;
            if node.kind != NodeKind::TextFile {
                return Err(PrikkError::Integrity(format!(
                    "EditText failed before blob load: node {node_id:?} is {:?}, not TextFile",
                    node.kind
                )));
            }
            // Promote to resident on the first edit since the anchor (module doc); every further
            // edit mutates the already-resident bytes.
            let current_text: Vec<u8> = match &node.content {
                ExtractContent::Resident(bytes) => bytes.clone(),
                ExtractContent::Blob(blob_id) => read_blob_bytes_with_kind(reader, *blob_id)?.1,
            };
            if text_span_hash(&old_span_text) != old_span_hash {
                return Err(PrikkError::Integrity(format!(
                    "EditText hash verification failed before localization for node {node_id:?}"
                )));
            }
            if core::str::from_utf8(&current_text).is_err() {
                return Err(PrikkError::Integrity(format!(
                    "EditText failed during UTF-8 validation for node {node_id:?}"
                )));
            }
            let (start, end) = text_span::resolve_text_span(
                &current_text,
                &old_span_text,
                &left_anchor_hash,
                &right_anchor_hash,
                &span_id,
                node_id,
                &old_span_hash,
                left_anchor_len,
                right_anchor_len,
            )
            .map_err(|reason| {
                PrikkError::Integrity(format!(
                    "EditText failed during localization for node {node_id:?}: {reason}"
                ))
            })?;
            let new_text = text_span::splice_text(&current_text, start, end, &replacement_text)
                .map_err(|err| {
                    PrikkError::Integrity(format!(
                        "EditText failed during splice for node {node_id:?}: {err}"
                    ))
                })?;
            // Safe: the `live.get` lookup above already proved this key exists, and nothing between
            // there and here removes it.
            if let Some(node) = live.get_mut(&node_id) {
                node.content = ExtractContent::Resident(new_text);
            }
        }
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => {
            let node = live.get(&node_id).ok_or_else(|| {
                PrikkError::Integrity(format!("ReplaceBinary target node {node_id:?} is not live"))
            })?;
            if node.kind != NodeKind::BinaryFile {
                return Err(PrikkError::Integrity(format!(
                    "ReplaceBinary target node {node_id:?} is {:?}, not BinaryFile",
                    node.kind
                )));
            }
            match &node.content {
                ExtractContent::Blob(current) if *current == old_blob_id => {}
                other => {
                    return Err(PrikkError::Integrity(format!(
                        "ReplaceBinary old_blob_id mismatch for node {node_id:?}: expected \
                         {old_blob_id}, got {other:?}"
                    )));
                }
            }
            // The new blob is read once, here, to confirm it is actually binary content -- the
            // same validation `apply.rs`'s own `apply_replace_binary` always pays (it holds every
            // node resident so the read is free there; here it is the same bounded, one-time cost
            // `CreateFile`'s own binary case already accepts above, for the identical reason: a
            // patch's own claim about a blob's kind is not proof). The bytes are discarded
            // immediately; only the id is kept, so this never holds binary content resident.
            let (new_kind, _) = read_blob_bytes_with_kind(reader, new_blob_id)?;
            if new_kind != NodeKind::BinaryFile {
                return Err(PrikkError::Integrity(format!(
                    "ReplaceBinary new blob {new_blob_id} is not a binary-file blob"
                )));
            }
            if let Some(node) = live.get_mut(&node_id) {
                node.content = ExtractContent::Blob(new_blob_id);
            }
        }
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => {
            let node = live.get_mut(&node_id).ok_or_else(|| {
                PrikkError::Integrity(format!("ChangePerm target node {node_id:?} is not live"))
            })?;
            if node.mode != old_mode {
                return Err(PrikkError::Integrity(format!(
                    "ChangePerm old_mode does not match live node {node_id:?}'s current mode"
                )));
            }
            node.mode = new_mode;
        }
        DecodedOperationKind::RenamePath { .. } => {
            return Err(PrikkError::MalformedData(
                "apply_one_for_extract received a RenamePath -- the caller must route consecutive \
                 RenamePath runs to apply_rename_batch_for_extract instead, never here"
                    .to_string(),
            ));
        }
        _ => {
            return Err(PrikkError::MalformedData(
                "apply_one_for_extract received an operation kind ensure_apply_supported should \
                 have refused"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// [`super::apply::apply_rename_batch`], over `live`/`by_path` instead of `files`/`live_nodes`: a
/// rename only ever moves a path, never content (module doc), so this never touches
/// `ExtractLiveNode::content` at all -- the one genuine simplification this module's own indexing
/// buys over the path-keyed original.
fn apply_rename_batch_for_extract(
    live: &mut BTreeMap<NodeId, ExtractLiveNode>,
    by_path: &mut BTreeMap<String, NodeId>,
    renames: Vec<super::decode::DecodedPatchOperation>,
) -> Result<()> {
    if renames.is_empty() {
        return Ok(());
    }
    let mut resolved = Vec::with_capacity(renames.len());
    let mut sources_seen = std::collections::BTreeSet::new();
    let mut destinations_seen = std::collections::BTreeSet::new();
    for operation in renames {
        let DecodedOperationKind::RenamePath {
            node_id,
            old_path,
            new_path,
        } = operation.kind
        else {
            return Err(PrikkError::MalformedData(
                "apply_rename_batch_for_extract received a non-RenamePath operation".to_string(),
            ));
        };
        let node = live.get(&node_id).ok_or_else(|| {
            PrikkError::Integrity(format!("RenamePath target node {node_id:?} is not live"))
        })?;
        if node.path != old_path {
            return Err(PrikkError::Integrity(format!(
                "RenamePath old_path {old_path} does not match live node {node_id:?}'s current \
                 path {}",
                node.path
            )));
        }
        if !sources_seen.insert(node_id) {
            return Err(PrikkError::Integrity(format!(
                "RenamePath renames node {node_id:?} more than once in the same run"
            )));
        }
        if !destinations_seen.insert(new_path.clone()) {
            return Err(PrikkError::Integrity(format!(
                "RenamePath names destination path {new_path} more than once in the same run"
            )));
        }
        resolved.push((node_id, old_path, new_path));
    }
    // Vacate every source across the whole run before claiming any target (same two-phase shape as
    // `apply_rename_batch`, and for the identical reason: a swap's target must never read as
    // occupied by the very node this run is about to move out of it).
    for (node_id, old_path, _) in &resolved {
        by_path.remove(old_path.as_str());
        let _ = node_id;
    }
    for (_, _, new_path) in &resolved {
        if by_path.contains_key(new_path.as_str()) {
            return Err(PrikkError::Integrity(format!(
                "RenamePath target path {new_path} is occupied by another live node"
            )));
        }
    }
    for (node_id, _, new_path) in resolved {
        by_path.insert(new_path.clone(), node_id);
        if let Some(node) = live.get_mut(&node_id) {
            node.path = new_path;
        }
    }
    Ok(())
}

/// Whether `err` represents a genuine fold mismatch -- the patch's own claim about the state it is
/// applied to is wrong (0.56.0 step 3e, D4) -- as opposed to a different missing or malformed object
/// the replay needed along the way. **Narrow on purpose**:
/// `prepare_extract_plan_for_claim_still_falls_back_when_a_different_object_is_missing`
/// (`crate::extract::tests`) pins that a claim naming a Patch object that was simply never written
/// stays `kind: None` (the ordinary `damaged-repository` fallback), not reclassified here just
/// because it happens to arrive through the same `LifecycleReplayError` type.
fn is_fold_mismatch(err: &LifecycleReplayError) -> bool {
    matches!(
        err,
        LifecycleReplayError::InconsistentLifecycleEffect { .. }
            | LifecycleReplayError::MissingBlobForLifecycleEffect { .. }
            | LifecycleReplayError::TextSpanResolutionFailed { .. }
            | LifecycleReplayError::OutsideVersionRuleInLineage { .. }
    )
}

/// [`replay_for_extract_chain`]'s own error: like [`ExtractError`], but `Fold` carries which link
/// (an index into the caller's own chain, root to foot) failed to fold, rather than the already
/// -resolved claim id -- the crate-root caller (`crate::extract::prepare_extract_plan_for_claim`)
/// is the one that knows the chain, and maps the index back to a claim id there.
#[derive(Debug)]
pub(crate) enum ExtractChainFoldError {
    /// D3's own bound, already fully classified (`no-checkpoint-in-reach`).
    Bound(ExtractError),
    /// A case-insensitive path collision in the folded result, already fully classified
    /// (`case-collision`) -- caught as a `Lineage`-shaped error from `entries_from_state` rather
    /// than a `Patch`-shaped one, the same classification the single-link path always gave this.
    Collision(ExtractError),
    /// The anchor's own sealed lineage did not resolve -- this repository's own sealed history is
    /// broken, propagated unclassified exactly as before this round.
    Lineage(PrikkError),
    /// Link `link_index` (into the caller's own chain, root to foot) does not fold -- a genuine
    /// mismatch between the claim's own patches and the state they are folded onto ([`is_fold_mismatch`]).
    Fold { link_index: usize, err: LifecycleReplayError },
    /// Any other failure applying a link's own patches (for example, a patch id that does not exist
    /// or does not decode) -- propagated unclassified, exactly as every such failure already was
    /// before this round.
    Other(PrikkError),
}

impl std::fmt::Display for ExtractChainFoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bound(err) | Self::Collision(err) => write!(f, "{err}"),
            Self::Lineage(err) | Self::Other(err) => write!(f, "{err}"),
            Self::Fold { err, .. } => write!(
                f,
                "extract --claim refused: the claim's own patches do not apply cleanly to its \
                 declared parent's state ({err})"
            ),
        }
    }
}

/// **The chain-aware fold** (0.56.0 step 3e, D4): `anchor` is where the chain walk
/// (`crate::claim_chain::walk_claim_chain`) stopped (a real, reachable block, or `None` for
/// genesis); `link_patch_ids` is every link's own `patch_ids`, root to foot (`link_patch_ids[0]`
/// folds directly onto `anchor`'s own resolved state; each further entry folds onto the previous
/// one's result); `hop_count` is the chain's own length past the anchor, fed straight into D3's own
/// bound (`refuse_past_the_bound_unless_allowed_for_chain`) alongside the anchor's own real-block
/// distance.
///
/// **The first link reuses [`crate::block_state::derive_candidate`] exactly as the single-claim path
/// always did** (the identical anchor-aware resolution, so a chain of length 1 behaves exactly as
/// before this round); **every further link folds with
/// [`crate::lifecycle_cache::replay::apply_candidate_patches`]** onto the state the previous link
/// already produced -- `derive_candidate`'s own API always resolves a *real* Block's state, which no
/// link past the first one has (every link past the first stands for a block that was never sealed).
///
/// # Errors
///
/// See [`ExtractChainFoldError`]'s own variants.
pub(crate) fn replay_for_extract_chain(
    layout: &crate::RepositoryLayout,
    reader: &impl ObjectReader,
    anchor: Option<ObjectId>,
    hop_count: usize,
    link_patch_ids: &[Vec<ObjectId>],
    allow_full_replay: bool,
) -> std::result::Result<ExtractReplayOutcome, ExtractChainFoldError> {
    // D3: the identical pre-walk bound the ref/block-id path and the old single-claim path both
    // already had, now also counting the chain's own hops. A root (genesis) anchor has no real
    // block-chain distance to bound, same as before this round.
    if let Some(anchor_id) = anchor {
        let chain = super::read::single_parent_chain(reader, anchor_id)
            .map_err(ExtractChainFoldError::Lineage)?;
        let trust = crate::anchor_trust::AnchorTrust::load(layout);
        if let AnchorSearch::GaveUp { reason } =
            search_admitted_anchor(reader, &chain, &trust, AnchorSite::ExtractClaim)
                .map_err(ExtractChainFoldError::Lineage)?
        {
            refuse_past_the_bound_unless_allowed_for_chain(
                chain.len(),
                hop_count,
                allow_full_replay,
                reason,
            )
            .map_err(ExtractChainFoldError::Bound)?;
        }
    }

    let Some((first_patch_ids, remaining)) = link_patch_ids.split_first() else {
        return Err(ExtractChainFoldError::Lineage(PrikkError::Integrity(
            "extract --claim chain walk produced an empty chain -- internal inconsistency"
                .to_string(),
        )));
    };

    let derived = crate::block_state::derive_candidate(
        layout,
        reader,
        anchor,
        first_patch_ids,
        crate::block_state::StateAnchoring::Anchored(AnchorSite::ExtractClaim),
    )
    .map_err(|err| match err {
        crate::block_state::CandidateStateDerivationError::Lineage(
            err @ PrikkError::InvalidName(_),
        ) => ExtractChainFoldError::Collision(ExtractError::classified(
            ExtractRefusalKind::CaseCollision,
            err,
        )),
        crate::block_state::CandidateStateDerivationError::Lineage(err) => {
            ExtractChainFoldError::Lineage(err)
        }
        crate::block_state::CandidateStateDerivationError::Patch(err) => {
            if is_fold_mismatch(&err) {
                ExtractChainFoldError::Fold {
                    link_index: 0,
                    err,
                }
            } else {
                ExtractChainFoldError::Other(err.into())
            }
        }
    })?;

    let mut state = derived.state;
    let mut text_cache = derived.text_cache;
    let anchored_used = derived.anchored;

    for (offset, patch_ids) in remaining.iter().enumerate() {
        let link_index = offset + 1;
        crate::lifecycle_cache::replay::apply_candidate_patches(
            reader,
            &mut state,
            &mut text_cache,
            patch_ids,
        )
        .map_err(|err| {
            if is_fold_mismatch(&err) {
                ExtractChainFoldError::Fold { link_index, err }
            } else {
                ExtractChainFoldError::Other(err.into())
            }
        })?;
    }

    // A chain of length 1 (the ordinary single-claim case, unchanged from before this round):
    // `derive_candidate` already computed these entries from the identical, never-further-folded
    // `state`, so reuse them rather than paying `entries_from_state`'s own walk a second time over
    // nothing new. A longer chain folded further patches since, so its own final state needs a
    // fresh derivation.
    let entries = if remaining.is_empty() {
        derived.entries
    } else {
        crate::state_root::entries_from_state(&state).map_err(|err| match err {
            err @ PrikkError::InvalidName(_) => ExtractChainFoldError::Collision(
                ExtractError::classified(ExtractRefusalKind::CaseCollision, err),
            ),
            err => ExtractChainFoldError::Lineage(err),
        })?
    };

    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        let StateRootContent::Blob(blob_id) = entry.content else {
            return Err(ExtractChainFoldError::Other(PrikkError::UnsupportedObjectType(
                format!(
                    "claimed state holds symlink {}; extract supports the file-level subset only",
                    entry.path.as_str()
                ),
            )));
        };
        let content = match text_cache.get(&entry.node_id) {
            Some(bytes) => ExtractContent::Resident(bytes.clone()),
            None => ExtractContent::Blob(blob_id),
        };
        files.push(ExtractFile {
            path: entry.path.as_str().to_string(),
            mode: entry.mode,
            content,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ExtractReplayOutcome {
        provisional: !anchored_used,
        files,
    })
}

#[cfg(test)]
mod tests;
