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
const CACHE_MAGIC: &[u8] = b"PRIKK-LIFECYCLE-INCREMENTAL-CACHE-v1\0";
const CACHE_SCHEMA_VERSION: u32 = 1;

/// After this many consecutive incremental steps on one lineage, the next commit is forced through
/// an unmodified full replay regardless of cache eligibility. This is the only control on how long a
/// persistence fault that survives the checksum and `from_replay`'s structural check could live
/// before an independent reconstruction overwrites the cache with ground truth. See the design
/// document §5 for the exposure/amortized-overhead reasoning behind this exact value.
const REANCHOR_BOUND: u32 = 64;

struct IncrementalCache {
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
pub fn verify_divergence(
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
/// otherwise. This is the sole entry point `node_authoring.rs` calls in place of
/// `replay_derived_state` directly; the return contract is identical.
pub(crate) fn resolve_baseline_state(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    baseline_block_id: ObjectId,
    horizon_id: ObjectId,
) -> Result<ReplayDerivedLifecycleState> {
    if let Some(cached) = load(layout) {
        if cached.horizon_id == horizon_id && cached.steps_since_reanchor < REANCHOR_BOUND {
            if let Some(state) = try_incremental_step(reader, &cached, baseline_block_id)? {
                let result = ReplayDerivedLifecycleState::from_replay(baseline_block_id, state)?;
                persist(
                    layout,
                    baseline_block_id,
                    horizon_id,
                    cached.steps_since_reanchor + 1,
                    result.state(),
                );
                return Ok(result);
            }
        }
    }
    let result = replay_derived_state(reader, baseline_block_id, horizon_id)?;
    persist(layout, baseline_block_id, horizon_id, 0, result.state());
    Ok(result)
}

/// Attempt the incremental step. `Ok(None)` means "not eligible" — parent mismatch, multi-parent, the
/// new block could not be read (the full-replay path re-derives it with its own proper error class),
/// or — DC-65 — the block's operations need a `TextFile` node's materialized content that this
/// step's fresh, single-block text cache cannot supply. That last case is not a cache-trust failure:
/// applying one block's operations against a cached predecessor uses the identical
/// `apply_state_effect` fold full replay uses, but a fold that spans only one block cannot
/// materialize a node whose current content is itself an *earlier*, already-cached-away block's
/// `EditText` result — full replay's `TextCache` accumulates across the whole lineage and never has
/// this gap. Structurally falling back to full replay for this one commit is the correct, general
/// fix (rather than a narrower per-node fallback), consistent with the DC-65 invariant that any
/// consumer needing a `TextFile` node's actual bytes must be able to materialize them, never assume
/// a stored object. See the design document §9a. A genuine application failure of any other class
/// propagates as `Err`, still not folded into the fallback path — see §3.
fn try_incremental_step(
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

fn decode(bytes: &[u8]) -> Option<IncrementalCache> {
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
