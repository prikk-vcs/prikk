//! Incremental baseline lifecycle-state cache (DC-64).
//!
//! Accelerates the commit path's baseline reconstruction by applying only the *newest* block's
//! patch operations to a persisted predecessor state, instead of replaying a lineage's entire
//! history on every commit. Scoped to the commit path only — see the design document at
//! `rfcs/handoffs/DC-64-baseline-reconstruction-cost/incremental-baseline-cache-design-v1.md`,
//! required reading before changing anything here, including why this is safe despite the trust
//! ladder in `cache_ladder.rs` requiring full-replay certification for a superficially similar
//! problem (that ladder guards a different, merge-only decision this cache never makes).
//!
//! Rebuildable and never authoritative (NFR-PERF-04): any problem loading the persisted cache — a
//! missing file, a checksum mismatch, a decode failure — is treated as an absent cache, never a hard
//! error, and always falls through to the unmodified `replay_derived_state` full-replay path.

use prikk_error::Result;
use prikk_object::{CanonicalWriter, ObjectId, WireType};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::fsutil::{read_file_if_exists, write_file_atomically};
use crate::foundation::layout::RepositoryLayout;
use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState, Tombstone};
use crate::object_store::ObjectReader;
use crate::path::RepoPath;

use super::{ReplayDerivedLifecycleState, replay, replay_derived_state};

const CACHE_FILE_NAME: &str = "lifecycle-state.v1";
pub(crate) const CACHE_MAGIC: &[u8] = b"PRIKK-LIFECYCLE-INCREMENTAL-CACHE-v1\0";
const CACHE_SCHEMA_VERSION: u32 = 1;

// After `CHECKPOINT_CADENCE` consecutive incremental steps on one lineage, the next commit is forced
// through an unmodified full replay regardless of cache eligibility. This is the only control on how
// long a persistence fault that survives the checksum and `from_replay`'s structural check could live
// before an independent reconstruction overwrites the cache with ground truth. See the design
// document §5 for the exposure/amortized-overhead reasoning behind the value; RFC 136 §10.2 made the
// reanchor bound and the snapshot checkpoint cadence one number.
use crate::snapshot::CHECKPOINT_CADENCE;

pub(crate) struct IncrementalCache {
    baseline_block_id: ObjectId,
    horizon_id: ObjectId,
    steps_since_reanchor: u32,
    state: NodeLifecycleState,
}

/// One disagreement between the persisted incremental cache and an independent full replay of the
/// block it currently claims to represent — the persistence-fault case the checksum and
/// `from_replay`'s structural check do not, by themselves, catch (design document §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleCacheDivergence {
    /// The block the cache claimed to represent when the divergence was found.
    pub baseline_block_id: ObjectId,
    /// Human-readable detail: either a content disagreement or that independent verification
    /// itself could not complete (e.g. the claimed block became unreadable).
    pub detail: String,
}

/// Check the persisted cache, if any, against an independent full replay of the block it claims to
/// represent. Never errors: a replay failure is itself reported as a divergence rather than
/// propagated, so `verify` always completes with a full picture rather than aborting on this check.
/// This answers a narrower question than "is the cache eligible for the next commit" — it verifies
/// only what the cache is *currently* offering, on the DC-56 `verify_divergence` pattern.
pub(crate) fn verify_divergence(
    reader: &impl ObjectReader,
    layout: &RepositoryLayout,
) -> Vec<LifecycleCacheDivergence> {
    let Some(cached) = load(layout) else {
        return Vec::new();
    };
    match replay_derived_state(reader, cached.baseline_block_id, cached.horizon_id) {
        Ok(replayed) if replayed.state() == &cached.state => Vec::new(),
        Ok(_) => vec![LifecycleCacheDivergence {
            baseline_block_id: cached.baseline_block_id,
            detail: "cached lifecycle state disagrees with an independent full replay".to_string(),
        }],
        Err(err) => vec![LifecycleCacheDivergence {
            baseline_block_id: cached.baseline_block_id,
            detail: format!("cached lifecycle state could not be independently verified: {err}"),
        }],
    }
}

/// Resolve the baseline lifecycle state for `baseline_block_id`/`horizon_id`, using an incremental
/// step from a cached predecessor when eligible and falling back to an unmodified full replay
/// otherwise, **refreshing the cache** -- what the tests below exercise. Production callers name the choice
/// through [`resolve_baseline_state_with`]; the return contract is identical to `replay_derived_state`.
#[cfg(test)]
pub(crate) fn resolve_baseline_state(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    baseline_block_id: ObjectId,
    horizon_id: ObjectId,
) -> Result<ReplayDerivedLifecycleState> {
    resolve_baseline_state_with(
        layout,
        reader,
        baseline_block_id,
        horizon_id,
        CacheWrite::Refresh,
    )
}

/// Whether resolving a baseline refreshes the rebuildable cache under `.prikk/cache/`.
///
/// **`Never` is for a report that must write nothing** (`prikk diff`, RFC 153 §6.2): the cache is best-effort and
/// never authoritative, but a *read-only* command that rewrote it would still have written under `.prikk/`, which
/// the increment's own control checks for byte by byte. A caller that skips the refresh loses only the
/// acceleration of the next `commit`, exactly as a failed save would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheWrite {
    /// Refresh the cache with the state just resolved (`commit`, `worktree-status`, branch switching).
    Refresh,
    /// Read the cache when it helps and never write it.
    Never,
}

/// [`resolve_baseline_state`], with the choice of whether to refresh the cache.
pub(crate) fn resolve_baseline_state_with(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    baseline_block_id: ObjectId,
    horizon_id: ObjectId,
    cache_write: CacheWrite,
) -> Result<ReplayDerivedLifecycleState> {
    if let Some(cached) = load(layout) {
        if cached.horizon_id == horizon_id && cached.steps_since_reanchor < CHECKPOINT_CADENCE {
            if let Some(state) = same_baseline_hit(&cached, baseline_block_id) {
                // RFC 136 increment 2c, Addendum 2. A hit counts as a step, so DC-64 §5's independent full replay
                // still happens within `CHECKPOINT_CADENCE` uses however they are made up.
                let result = ReplayDerivedLifecycleState::from_replay(baseline_block_id, state)?;
                if cache_write == CacheWrite::Refresh {
                    persist(
                        layout,
                        baseline_block_id,
                        horizon_id,
                        cached.steps_since_reanchor + 1,
                        result.state(),
                    );
                }
                return Ok(result);
            }
            if let Some(state) = try_incremental_step(layout, reader, &cached, baseline_block_id)? {
                let result = ReplayDerivedLifecycleState::from_replay(baseline_block_id, state)?;
                if cache_write == CacheWrite::Refresh {
                    persist(
                        layout,
                        baseline_block_id,
                        horizon_id,
                        cached.steps_since_reanchor + 1,
                        result.state(),
                    );
                }
                return Ok(result);
            }
        }
    }
    let result = replay_derived_state(reader, baseline_block_id, horizon_id)?;
    if cache_write == CacheWrite::Refresh {
        persist(layout, baseline_block_id, horizon_id, 0, result.state());
    }
    Ok(result)
}

/// A request for the cached baseline **itself** (RFC 136 increment 2c, Addendum 2). DC-64 §3.2 defined eligibility
/// as "the new block's parent is the cache's baseline" and never considered asking again for the same block, so
/// every `commit` after the first between two `seal`s, and a `commit` after `worktree-status`, fell through to a
/// full replay and reset the step count. The cached state is exactly what a one-block step would start from and
/// trust entirely; a hit trusts the same state and applies nothing. The caller still binds it through
/// `ReplayDerivedLifecycleState::from_replay`, so its validation is not bypassed.
fn same_baseline_hit(
    cached: &IncrementalCache,
    baseline_block_id: ObjectId,
) -> Option<NodeLifecycleState> {
    (cached.baseline_block_id == baseline_block_id).then(|| cached.state.clone())
}

/// Attempt the incremental step. `Ok(None)` means "not eligible" — parent mismatch, multi-parent, the
/// new block could not be read (the full-replay path re-derives it with its own proper error class),
/// or — DC-65 — the block's operations need a `TextFile` node's materialized content that this
/// step's fresh, single-block text cache cannot supply. That last case is not a cache-trust failure:
/// applying one block's operations against a cached predecessor uses the identical
/// `apply_state_effect` fold full replay uses, but a fold that spans only one block cannot
/// materialize a node whose current content is itself an *earlier*, already-cached-away block's
/// `EditText` result — full replay's `TextCache` accumulates across the whole lineage and never has
/// this gap. **RFC 136 increment 2c closes it without a full replay:** the missing text is taken from the
/// nearest replay-verified anchor ([`anchored_text_retry`]); only if that too is unavailable is this
/// commit's baseline a full replay. See the design document §9a. A genuine application failure of any
/// other class propagates as `Err`, still not folded into the fallback path — see §3.
fn try_incremental_step(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    cached: &IncrementalCache,
    baseline_block_id: ObjectId,
) -> Result<Option<NodeLifecycleState>> {
    let Ok(block) = replay::read_block(reader, baseline_block_id) else {
        return Ok(None);
    };
    if block.parent_block_ids.as_slice() != [cached.baseline_block_id] {
        return Ok(None);
    }
    let mut state = cached.state.clone();
    match replay::apply_one_block(reader, &block, &mut state, false) {
        Ok(()) => Ok(Some(state)),
        Err(replay::LifecycleReplayError::MissingBlobForLifecycleEffect { .. }) => {
            anchored_text_retry(layout, reader, cached, &block)
        }
        Err(other) => Err(other.into()),
    }
}

/// RFC 136 increment 2c. The one-block step needed the text of a node whose content came from an earlier block's
/// `EditText`. Take it from the nearest replay-verified anchor (`anchored_text::anchored_texts`), which returns a
/// text only if it hashes to the blob id the **cached** state names for its node, and apply the block to the
/// **cached** state -- history fields intact -- with that text in hand. No state is anchored (§10.3c ruling 3
/// governs one); the anchor supplies text. `None` -- today's full replay -- whenever the anchor cannot: there is
/// none in reach, or it is a verified anchor that failed and was named for the CLI.
fn anchored_text_retry(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    cached: &IncrementalCache,
    block: &prikk_object::BlockPayload,
) -> Result<Option<NodeLifecycleState>> {
    let Ok(edited) = replay::edited_text_nodes(reader, block) else {
        return Ok(None);
    };
    // The nodes whose text this block edits and whose current content nobody stored: the miss itself.
    let mut wanted = std::collections::BTreeMap::new();
    for node_id in edited {
        let Some(live) = cached.state.live_node(&node_id) else {
            continue;
        };
        let NodeContent::File { blob_id, .. } = &live.content else {
            continue;
        };
        if live.kind == prikk_object::NodeKind::TextFile
            && matches!(
                reader.has_object(*blob_id, prikk_object::ObjectType::Blob),
                Ok(false)
            )
        {
            wanted.insert(node_id, *blob_id);
        }
    }
    let Some(mut text_cache) =
        super::anchored_text::anchored_texts(layout, reader, cached.baseline_block_id, &wanted)
    else {
        return Ok(None);
    };
    let mut state = cached.state.clone();
    match replay::apply_one_block_carrying_text(reader, block, &mut state, &mut text_cache) {
        Ok(()) => Ok(Some(state)),
        Err(replay::LifecycleReplayError::MissingBlobForLifecycleEffect { .. }) => Ok(None),
        Err(other) => Err(other.into()),
    }
}

/// Persist the refreshed cache. Best-effort: a save failure does not fail the commit that just
/// succeeded — losing the opportunity to accelerate the *next* commit is a performance regression,
/// not a correctness one, and the cache is rebuildable by design.
fn persist(
    layout: &RepositoryLayout,
    baseline_block_id: ObjectId,
    horizon_id: ObjectId,
    steps_since_reanchor: u32,
    state: &NodeLifecycleState,
) {
    let cache = IncrementalCache {
        baseline_block_id,
        horizon_id,
        steps_since_reanchor,
        state: state.clone(),
    };
    let _ = save(layout, &cache);
}

fn cache_path(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.cache_dir().join(CACHE_FILE_NAME)
}

fn load(layout: &RepositoryLayout) -> Option<IncrementalCache> {
    let relative = layout.repository_relative(&cache_path(layout)).ok()?;
    let bytes = read_file_if_exists(layout.repository_mutation_root(), &relative).ok()??;
    decode(&bytes)
}

fn save(layout: &RepositoryLayout, cache: &IncrementalCache) -> Result<()> {
    let relative = layout.repository_relative(&cache_path(layout))?;
    write_file_atomically(layout.repository_mutation_root(), &relative, &encode(cache))
}

fn encode(cache: &IncrementalCache) -> Vec<u8> {
    let mut writer = CanonicalWriter::new();
    let _ = writer.field_u32(1, CACHE_SCHEMA_VERSION);
    let _ = writer.field_object_id(2, &cache.baseline_block_id);
    let _ = writer.field_object_id(3, &cache.horizon_id);
    let _ = writer.field_u32(4, cache.steps_since_reanchor);
    for (node_id, node) in cache.state.live_nodes() {
        if let Ok(record) = encode_node_record(node_id, &node.path, node.kind, &node.content) {
            let _ = writer.field_raw(10, WireType::RecordListItem, &record);
        }
    }
    for (node_id, tombstone) in cache.state.tombstones() {
        if let Ok(record) =
            encode_node_record(node_id, &tombstone.path, tombstone.kind, &tombstone.content)
        {
            let _ = writer.field_raw(11, WireType::RecordListItem, &record);
        }
    }
    let body = writer.finish();
    let checksum = prikk_hash::sha256(&body);

    let mut out = Vec::with_capacity(CACHE_MAGIC.len() + 32 + body.len());
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    out
}

pub(crate) fn decode(bytes: &[u8]) -> Option<IncrementalCache> {
    let after_magic = bytes.strip_prefix(CACHE_MAGIC)?;
    if after_magic.len() < 32 {
        return None;
    }
    let (checksum, body) = after_magic.split_at(32);
    if prikk_hash::sha256(body) != checksum {
        return None;
    }

    let mut cursor = ByteCursor::new(body);
    let mut schema_version: Option<u32> = None;
    let mut baseline_block_id: Option<ObjectId> = None;
    let mut horizon_id: Option<ObjectId> = None;
    let mut steps_since_reanchor: Option<u32> = None;
    let mut state = NodeLifecycleState::new();

    let mut last_tag: Option<u16> = None;
    while let Some(field) = next_field(&mut cursor)? {
        if let Some(previous) = last_tag {
            if field.tag < previous {
                return None;
            }
        }
        last_tag = Some(field.tag);
        match field.tag {
            1 => {
                if field.wire != WireType::U32 as u8 || schema_version.is_some() {
                    return None;
                }
                schema_version = Some(u32::from_be_bytes(field.value.try_into().ok()?));
            }
            2 => {
                if field.wire != WireType::ObjectId as u8 || baseline_block_id.is_some() {
                    return None;
                }
                baseline_block_id = Some(ObjectId::from_bytes(field.value.try_into().ok()?));
            }
            3 => {
                if field.wire != WireType::ObjectId as u8 || horizon_id.is_some() {
                    return None;
                }
                horizon_id = Some(ObjectId::from_bytes(field.value.try_into().ok()?));
            }
            4 => {
                if field.wire != WireType::U32 as u8 || steps_since_reanchor.is_some() {
                    return None;
                }
                steps_since_reanchor = Some(u32::from_be_bytes(field.value.try_into().ok()?));
            }
            10 => {
                if field.wire != WireType::RecordListItem as u8 {
                    return None;
                }
                let (node_id, path, kind, content) = decode_node_record(field.value)?;
                state
                    .seed_live_node(
                        node_id,
                        LiveNode {
                            path,
                            kind,
                            content,
                        },
                    )
                    .ok()?;
            }
            11 => {
                if field.wire != WireType::RecordListItem as u8 {
                    return None;
                }
                let (node_id, path, kind, content) = decode_node_record(field.value)?;
                state
                    .seed_tombstone(
                        node_id,
                        Tombstone {
                            kind,
                            content,
                            path,
                        },
                    )
                    .ok()?;
            }
            _ => return None,
        }
    }

    if schema_version? != CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(IncrementalCache {
        baseline_block_id: baseline_block_id?,
        horizon_id: horizon_id?,
        steps_since_reanchor: steps_since_reanchor?,
        state,
    })
}

struct Field<'a> {
    tag: u16,
    wire: u8,
    value: &'a [u8],
}

fn next_field<'a>(cursor: &mut ByteCursor<'a>) -> Option<Option<Field<'a>>> {
    if cursor.is_finished() {
        return Some(None);
    }
    let tag = cursor.read_u16().ok()?;
    let wire = cursor.read_array::<1>().ok()?[0];
    let len = usize::try_from(cursor.read_u64().ok()?).ok()?;
    let value = cursor.read_exact(len).ok()?;
    Some(Some(Field { tag, wire, value }))
}

fn encode_node_record(
    node_id: &prikk_object::NodeId,
    path: &RepoPath,
    kind: prikk_object::NodeKind,
    content: &NodeContent,
) -> Result<Vec<u8>> {
    let mut writer = CanonicalWriter::new();
    writer.field_repo_path(1, path.as_str())?;
    writer.field_bytes(2, node_id.as_bytes())?;
    writer.field_enum_u16(3, kind.code())?;
    match content {
        NodeContent::File { blob_id, mode } => {
            writer.field_object_id(4, blob_id)?;
            writer.field_u32(5, *mode)?;
        }
        NodeContent::Symlink { target } => {
            writer.field_string(6, target)?;
        }
    }
    Ok(writer.finish())
}

fn decode_node_record(
    bytes: &[u8],
) -> Option<(
    prikk_object::NodeId,
    RepoPath,
    prikk_object::NodeKind,
    NodeContent,
)> {
    let mut cursor = ByteCursor::new(bytes);
    let mut path: Option<RepoPath> = None;
    let mut node_id: Option<prikk_object::NodeId> = None;
    let mut kind: Option<prikk_object::NodeKind> = None;
    let mut blob_id: Option<ObjectId> = None;
    let mut mode: Option<u32> = None;
    let mut target: Option<String> = None;

    let mut last_tag: Option<u16> = None;
    while let Some(field) = next_field(&mut cursor)? {
        if let Some(previous) = last_tag {
            if field.tag < previous {
                return None;
            }
        }
        last_tag = Some(field.tag);
        match field.tag {
            1 => {
                if field.wire != WireType::RepoPath as u8 || path.is_some() {
                    return None;
                }
                path = Some(RepoPath::parse(core::str::from_utf8(field.value).ok()?).ok()?);
            }
            2 => {
                if field.wire != WireType::Bytes as u8 || node_id.is_some() {
                    return None;
                }
                node_id =
                    Some(prikk_object::NodeId::try_from_bytes(field.value.try_into().ok()?).ok()?);
            }
            3 => {
                if field.wire != WireType::EnumU16 as u8 || kind.is_some() {
                    return None;
                }
                let code = u16::from_be_bytes(field.value.try_into().ok()?);
                kind = Some(prikk_object::NodeKind::from_code(code).ok()?);
            }
            4 => {
                if field.wire != WireType::ObjectId as u8 || blob_id.is_some() {
                    return None;
                }
                blob_id = Some(ObjectId::from_bytes(field.value.try_into().ok()?));
            }
            5 => {
                if field.wire != WireType::U32 as u8 || mode.is_some() {
                    return None;
                }
                mode = Some(u32::from_be_bytes(field.value.try_into().ok()?));
            }
            6 => {
                if field.wire != WireType::String as u8 || target.is_some() {
                    return None;
                }
                target = Some(core::str::from_utf8(field.value).ok()?.to_string());
            }
            _ => return None,
        }
    }

    let path = path?;
    let node_id = node_id?;
    let kind = kind?;
    let content = match kind {
        prikk_object::NodeKind::TextFile | prikk_object::NodeKind::BinaryFile => {
            if target.is_some() {
                return None;
            }
            NodeContent::File {
                blob_id: blob_id?,
                mode: mode?,
            }
        }
        prikk_object::NodeKind::Symlink => {
            if blob_id.is_some() || mode.is_some() {
                return None;
            }
            NodeContent::Symlink { target: target? }
        }
    };
    Some((node_id, path, kind, content))
}

#[cfg(test)]
mod tests;

// Test-support instrument (warm-cache `commit` anomaly measurement, RFC 136): read-only, never in a
// shipped build. It reports what the persisted cache holds, and which rung `resolve_baseline_state`
// would take for a baseline and why. The decision is reproduced step for step from
// `resolve_baseline_state` and `try_incremental_step` above, through the same `load`, `read_block` and
// `apply_one_block`, with nothing persisted. A caller cross-checks it against the rung the real binary
// took, read from the cache header before and after the command. The ref-resolving entry point is
// `patch_replay::baseline_cache_rung_for_test_support`.

/// The persisted cache's header.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleCacheHeader {
    /// The block the cached state represents.
    pub baseline_block_id: ObjectId,
    /// The lineage genesis it was derived under.
    pub horizon_id: ObjectId,
    /// Incremental steps since the last full replay.
    pub steps_since_reanchor: u32,
}

/// The rung `resolve_baseline_state` takes, and the exact reason on a miss.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineCacheRung {
    /// The ref is unpublished: no baseline, the cache is not consulted.
    Genesis,
    /// Incremental step from the cached predecessor.
    Incremental,
    /// The request is for the cached baseline itself: the cached state is returned and counted as a step
    /// (RFC 136 increment 2c, Addendum 2).
    SameBaselineHit,
    /// Full replay: no cache file, or it failed to load (checksum, decode, schema).
    FullReplayNoUsableCache,
    /// Full replay: the cache was derived under another lineage genesis.
    FullReplayHorizonMismatch,
    /// Full replay forced: the reanchor bound was reached.
    FullReplayReanchorDue {
        /// The cached step count.
        steps_since_reanchor: u32,
    },
    /// Full replay: the baseline block could not be read.
    FullReplayBlockUnreadable,
    /// Full replay: the baseline block's parents are not exactly the cached block.
    FullReplayParentMismatch {
        /// What the cache represents.
        cached_baseline_block_id: ObjectId,
        /// The baseline block's parents.
        parent_block_ids: Vec<ObjectId>,
    },
    /// Full replay: the baseline block needs a text file's materialized content that a one-block step
    /// cannot supply (DC-65).
    FullReplayMissingBlobForLifecycleEffect {
        /// The replay error, as displayed.
        detail: String,
    },
    /// The step failed with another replay error; the real path propagates it.
    StepError {
        /// The replay error, as displayed.
        detail: String,
    },
}

/// The persisted cache header, if the cache loads.
#[cfg(feature = "test-support")]
#[must_use]
pub fn lifecycle_cache_header_for_test_support(
    layout: &RepositoryLayout,
) -> Option<LifecycleCacheHeader> {
    load(layout).map(|cache| LifecycleCacheHeader {
        baseline_block_id: cache.baseline_block_id,
        horizon_id: cache.horizon_id,
        steps_since_reanchor: cache.steps_since_reanchor,
    })
}

/// Which rung `resolve_baseline_state` would take for `baseline_block_id`/`horizon_id` now, and why.
#[cfg(feature = "test-support")]
pub(crate) fn baseline_cache_rung_at_for_test_support(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    baseline_block_id: ObjectId,
    horizon_id: ObjectId,
) -> BaselineCacheRung {
    let Some(cached) = load(layout) else {
        return BaselineCacheRung::FullReplayNoUsableCache;
    };
    if cached.horizon_id != horizon_id {
        return BaselineCacheRung::FullReplayHorizonMismatch;
    }
    if cached.steps_since_reanchor >= CHECKPOINT_CADENCE {
        return BaselineCacheRung::FullReplayReanchorDue {
            steps_since_reanchor: cached.steps_since_reanchor,
        };
    }
    if same_baseline_hit(&cached, baseline_block_id).is_some() {
        return BaselineCacheRung::SameBaselineHit;
    }
    let Ok(block) = replay::read_block(reader, baseline_block_id) else {
        return BaselineCacheRung::FullReplayBlockUnreadable;
    };
    if block.parent_block_ids.as_slice() != [cached.baseline_block_id] {
        return BaselineCacheRung::FullReplayParentMismatch {
            cached_baseline_block_id: cached.baseline_block_id,
            parent_block_ids: block.parent_block_ids.clone(),
        };
    }
    let mut state = cached.state.clone();
    match replay::apply_one_block(reader, &block, &mut state, false) {
        Ok(()) => BaselineCacheRung::Incremental,
        Err(err @ replay::LifecycleReplayError::MissingBlobForLifecycleEffect { .. }) => {
            match anchored_text_retry(layout, reader, &cached, &block) {
                Ok(Some(_)) => BaselineCacheRung::Incremental,
                Ok(None) => BaselineCacheRung::FullReplayMissingBlobForLifecycleEffect {
                    detail: format!("{err}"),
                },
                Err(other) => BaselineCacheRung::StepError {
                    detail: format!("{other}"),
                },
            }
        }
        Err(err) => BaselineCacheRung::StepError {
            detail: format!("{err}"),
        },
    }
}

// Test-support instruments for RFC 136 increment 2c's design round. Read-only against the object
// store, never in a shipped build. `ladder_walk_for_test_support` replays what a corpus build's
// successive `commit`s do to the DC-64 cache -- each one resolves its baseline through
// `resolve_baseline_state_with` over the cache the previous one persisted -- in one process, and
// names, per tip, the rung it took (from the mirror above, read *before* the step), the header it
// left, and whether the state it returned equals an independent full replay's in every field.

/// One tip of [`ladder_walk_for_test_support`].
#[cfg(feature = "test-support")]
#[derive(Debug, Clone)]
pub struct LadderTip {
    /// The block resolved as a baseline.
    pub block_id: ObjectId,
    /// The rung the mirror reported for it, read before the step ran.
    pub rung: BaselineCacheRung,
    /// The persisted header's step count after the step, if a cache was persisted.
    pub steps_after: Option<u32>,
    /// The persisted header's baseline after the step.
    pub baseline_after: Option<ObjectId>,
    /// Wall time of `resolve_baseline_state_with` alone.
    pub elapsed: std::time::Duration,
    /// `Some(true)` when the resolved state equals full replay's, every field of it (live nodes, path
    /// index, `latest_tombstone_by_id`, `seen_ids`); `None` when the comparison was not asked for.
    pub identical: Option<bool>,
    /// SHA-256 of the resolved state's `Debug` rendering (every map and set in it is ordered), and
    /// full replay's, when compared. Equal digests are equal states; this is for the report.
    pub digests: Option<(String, String)>,
    /// Live nodes in the resolved state.
    pub live_nodes: usize,
    /// Tombstones in the resolved state.
    pub tombstones: usize,
}

/// Resolve every block of `chain` (a ref's lineage, oldest first), through the real ladder over a cache that
/// starts absent and is refreshed after each step; `compare` also runs full replay at each tip and
/// compares the whole state. **Writes the cache under `layout`** -- give it a copy. The ref-resolving entry
/// point is `patch_replay::ladder_walk_for_test_support`.
///
/// # Errors
///
/// A step fails (the real path would propagate it too), or `chain` is empty.
#[cfg(feature = "test-support")]
pub(crate) fn ladder_walk_over_chain_for_test_support(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    chain: &[ObjectId],
    compare: bool,
) -> Result<Vec<LadderTip>> {
    let horizon = *chain
        .first()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("the lineage is empty".to_string()))?;
    let cache = cache_path(layout);
    let _ = std::fs::remove_file(&cache);
    let mut tips = Vec::with_capacity(chain.len());
    for block_id in chain.iter().copied() {
        let rung = baseline_cache_rung_at_for_test_support(layout, reader, block_id, horizon);
        let start = std::time::Instant::now();
        let resolved =
            resolve_baseline_state_with(layout, reader, block_id, horizon, CacheWrite::Refresh)?;
        let elapsed = start.elapsed();
        let header = load(layout);
        let (identical, digests) = if compare {
            let full = replay_derived_state(reader, block_id, horizon)?;
            let digest = |state: &NodeLifecycleState| {
                prikk_hash::to_hex(&prikk_hash::sha256(format!("{state:?}").as_bytes()))
            };
            (
                Some(full.state() == resolved.state()),
                Some((digest(resolved.state()), digest(full.state()))),
            )
        } else {
            (None, None)
        };
        tips.push(LadderTip {
            block_id,
            rung,
            steps_after: header.as_ref().map(|cache| cache.steps_since_reanchor),
            baseline_after: header.as_ref().map(|cache| cache.baseline_block_id),
            elapsed,
            identical,
            digests,
            live_nodes: resolved.state().live_nodes().count(),
            tombstones: resolved.state().tombstones().count(),
        });
    }
    Ok(tips)
}

/// What the history fields of one block's lifecycle state weigh, for RFC 136 2c option (ii) (store them
/// with the anchor).
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleStateShape {
    /// Live nodes: the leaf set of the block's state root, which a snapshot already carries.
    pub live_nodes: usize,
    /// Tombstones (`latest_tombstone_by_id`): what a snapshot does not carry.
    pub tombstones: usize,
    /// Bytes the cache codec spends on the live nodes' records (record framing included).
    pub live_record_bytes: usize,
    /// Bytes the cache codec spends on the tombstones' records (record framing included).
    pub tombstone_record_bytes: usize,
    /// The whole persisted cache file, for the same state.
    pub cache_file_bytes: usize,
}

/// The shape of `block_id`'s replay-derived lifecycle state, sized with the DC-64 codec. The ref-resolving
/// entry point is `patch_replay::lifecycle_state_shape_for_test_support`.
///
/// # Errors
///
/// The store cannot be read, or the lineage does not replay.
#[cfg(feature = "test-support")]
pub(crate) fn lifecycle_state_shape_at_for_test_support(
    reader: &impl ObjectReader,
    block_id: ObjectId,
    horizon: ObjectId,
) -> Result<LifecycleStateShape> {
    let replayed = replay_derived_state(reader, block_id, horizon)?;
    let state = replayed.state();
    // Field header of one record-list item: tag (2), wire (1), length (8).
    const ITEM_OVERHEAD: usize = 11;
    let mut live_record_bytes = 0;
    for (node_id, node) in state.live_nodes() {
        live_record_bytes += ITEM_OVERHEAD
            + encode_node_record(node_id, &node.path, node.kind, &node.content)?.len();
    }
    let mut tombstone_record_bytes = 0;
    for (node_id, tombstone) in state.tombstones() {
        tombstone_record_bytes += ITEM_OVERHEAD
            + encode_node_record(node_id, &tombstone.path, tombstone.kind, &tombstone.content)?
                .len();
    }
    let file = encode(&IncrementalCache {
        baseline_block_id: block_id,
        horizon_id: horizon,
        steps_since_reanchor: 0,
        state: state.clone(),
    });
    Ok(LifecycleStateShape {
        live_nodes: state.live_nodes().count(),
        tombstones: state.tombstones().count(),
        live_record_bytes,
        tombstone_record_bytes,
        cache_file_bytes: file.len(),
    })
}

/// SHA-256 of the persisted cache's lifecycle state (`Debug` rendering; every map and set in it is
/// ordered), or `None` when no cache loads. The header is not in it, so a cache written by a full replay
/// and one written by an incremental step over the same tip compare equal exactly when their states do.
#[cfg(feature = "test-support")]
#[must_use]
pub fn lifecycle_cache_state_digest_for_test_support(layout: &RepositoryLayout) -> Option<String> {
    load(layout).map(|cache| {
        prikk_hash::to_hex(&prikk_hash::sha256(format!("{:?}", cache.state).as_bytes()))
    })
}

/// Rewrite the persisted cache so its first live text node names a content id no text has (RFC 136 increment 2c
/// control: a carried text that fails its hash against the cache's claim must fall back and be named). The cache
/// keeps its baseline, horizon and step count, so the next `commit`'s one-block step reads it as valid.
/// Returns `false` when there is no usable cache or no live text node.
///
/// # Errors
///
/// The poisoned cache cannot be written.
#[cfg(feature = "test-support")]
pub fn poison_lifecycle_cache_for_test_support(layout: &RepositoryLayout) -> Result<bool> {
    let Some(cached) = load(layout) else {
        return Ok(false);
    };
    let wrong = ObjectId::from_bytes([0xEE; 32]);
    let mut poisoned = NodeLifecycleState::new();
    let mut changed = false;
    for (node_id, node) in cached.state.live_nodes() {
        let mut node = node.clone();
        if !changed && node.kind == prikk_object::NodeKind::TextFile {
            if let NodeContent::File { mode, .. } = node.content {
                node.content = NodeContent::File {
                    blob_id: wrong,
                    mode,
                };
                changed = true;
            }
        }
        poisoned.seed_live_node(*node_id, node)?;
    }
    for (node_id, tombstone) in cached.state.tombstones() {
        poisoned.seed_tombstone(*node_id, tombstone.clone())?;
    }
    if changed {
        save(
            layout,
            &IncrementalCache {
                baseline_block_id: cached.baseline_block_id,
                horizon_id: cached.horizon_id,
                steps_since_reanchor: cached.steps_since_reanchor,
                state: poisoned,
            },
        )?;
    }
    Ok(changed)
}
