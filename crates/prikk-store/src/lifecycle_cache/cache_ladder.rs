//! Test-only lifecycle-cache codec and trust-ladder scaffolding (DC-09 Phase 4.4-2b.1).
//!
//! Split out of `lifecycle_cache.rs` (DC-58) — no behaviour change, all items moved verbatim,
//! `#[cfg(test)]` on each item replaced by gating the whole module instead (this file is declared
//! `#[cfg(test)] mod cache_ladder;` in the parent). Everything here was already test-only before
//! the split: per the parent module's doc, blob-kind verification, provenance-vs-baseline
//! staleness, and replay reconstruction/compare are later slices, so nothing here is yet consumed
//! by an identity decision. Re-exported at `lifecycle_cache.rs` (also `#[cfg(test)]`) so
//! `lifecycle_cache::tests` keeps resolving every `super::X` path unchanged.

use std::fmt;

use prikk_error::{PrikkError, Result};
use prikk_object::{CanonicalWriter, NodeId, NodeKind, ObjectId, WireType};

use crate::foundation::byte_cursor::ByteCursor;
use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState, Tombstone};
use crate::object_store::ObjectReader;

use super::{
    BlobKindResolver, ReplayDerivedLifecycleState, StoreBackedResolver, replay,
    replay_derived_state,
};

const LIFECYCLE_CACHE_MAGIC: &[u8] = b"PRIKK-NODE-LIFECYCLE-CACHE-v1\0";
const WINDOW_HASH_DOMAIN: &[u8] = b"PRIKK-LIFECYCLE-CACHE-WINDOW-v1";
pub(crate) const CACHE_SCHEMA_VERSION: u32 = 1;

/// Parent-policy of the derivation window (design v3 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentPolicy {
    /// Single-parent authoritative lineage segment (the only v1 policy).
    SingleParent,
    /// Reserved for DC-13 merge-aware baselines; rejected (fail closed) in v1.
    Dc13MergeAware,
}

impl ParentPolicy {
    const fn code(self) -> u16 {
        match self {
            Self::SingleParent => 1,
            Self::Dc13MergeAware => 2,
        }
    }

    fn from_code(code: u16) -> Result<Self> {
        match code {
            1 => Ok(Self::SingleParent),
            2 => Ok(Self::Dc13MergeAware),
            other => Err(malformed(format!("unknown parent_policy code {other}"))),
        }
    }
}

/// Compute the exact `replay_window_hash` preimage (design v3 §P1-3):
/// `SHA-256(domain || u64be(count) || raw32(block_id_0) || … || raw32(block_id_n))`,
/// with `block_id_0 == lineage_horizon_id` and `block_id_n == baseline_block_id`. The
/// caller is responsible for supplying the *actual* walked single-parent chain.
pub(crate) fn compute_window_hash(ordered_block_ids: &[ObjectId]) -> [u8; 32] {
    let mut preimage =
        Vec::with_capacity(WINDOW_HASH_DOMAIN.len() + 8 + ordered_block_ids.len() * 32);
    preimage.extend_from_slice(WINDOW_HASH_DOMAIN);
    preimage.extend_from_slice(&(ordered_block_ids.len() as u64).to_be_bytes());
    for block_id in ordered_block_ids {
        preimage.extend_from_slice(block_id.as_bytes());
    }
    prikk_hash::sha256(&preimage)
}

/// A decoded, structurally + cross-set validated lifecycle cache. **Not** authority for
/// identity decisions (design v3 §0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedLifecycleCache {
    pub(crate) schema_version: u32,
    pub(crate) baseline_block_id: ObjectId,
    pub(crate) lineage_horizon_id: ObjectId,
    pub(crate) parent_policy: ParentPolicy,
    pub(crate) replay_window_hash: [u8; 32],
    /// Optional materialization hint. **Not** certified by `ComparedLifecycleCache` (E3): the
    /// compare proves only the live/tombstone lifecycle state. Must not back materialization
    /// acceleration without its own validation against the authoritative state/root.
    pub(crate) snapshot_blob_id: Option<ObjectId>,
    pub(crate) live_entries: Vec<(NodeId, LiveNode)>,
    pub(crate) tombstones: Vec<(NodeId, Tombstone)>,
    pub(crate) seen_ids: Vec<NodeId>,
}

impl DecodedLifecycleCache {
    /// Encode to the persisted wire form (magic || canonical TLV) **after** fail-closed
    /// validation. Production callers use this: it refuses to persist a cache the importer
    /// would later reject (erratum P1).
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        self.serialize()
    }

    /// Raw serializer with no validation. Private; reached only through [`Self::encode`]
    /// in production (which validates first) and through the test-only fixture path.
    fn serialize(&self) -> Result<Vec<u8>> {
        let mut writer = CanonicalWriter::new();
        writer.field_u32(1, self.schema_version)?;
        writer.field_object_id(2, &self.baseline_block_id)?;
        writer.field_object_id(3, &self.lineage_horizon_id)?;
        writer.field_enum_u16(4, self.parent_policy.code())?;
        writer.field_bytes(5, &self.replay_window_hash)?;
        if let Some(snapshot) = &self.snapshot_blob_id {
            writer.field_object_id(6, snapshot)?;
        }
        for (node_id, node) in &self.live_entries {
            let record = encode_node_record(node_id, &node.path, node.kind, &node.content)?;
            writer.field_raw(10, WireType::RecordListItem, &record)?;
        }
        for (node_id, tombstone) in &self.tombstones {
            let record =
                encode_node_record(node_id, &tombstone.path, tombstone.kind, &tombstone.content)?;
            writer.field_raw(11, WireType::RecordListItem, &record)?;
        }
        let mut seen_bytes = Vec::with_capacity(self.seen_ids.len() * 32);
        for id in &self.seen_ids {
            seen_bytes.extend_from_slice(id.as_bytes());
        }
        writer.field_bytes(12, &seen_bytes)?;

        let mut out = LIFECYCLE_CACHE_MAGIC.to_vec();
        out.extend_from_slice(&writer.finish());
        Ok(out)
    }

    /// Test-only: serialize without validation, to craft malformed fixtures for decode
    /// negatives. Never reachable in production.
    pub(super) fn encode_unchecked(&self) -> Result<Vec<u8>> {
        self.serialize()
    }

    /// Decode and fully validate a persisted cache. Fail-closed on any structural or
    /// cross-set violation. Does **not** perform blob-kind verification (later slice).
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let Some(body) = bytes.strip_prefix(LIFECYCLE_CACHE_MAGIC) else {
            return Err(malformed(
                "missing or wrong lifecycle-cache magic".to_string(),
            ));
        };

        let mut cursor = ByteCursor::new(body);
        let mut schema_version: Option<u32> = None;
        let mut baseline_block_id: Option<ObjectId> = None;
        let mut lineage_horizon_id: Option<ObjectId> = None;
        let mut parent_policy: Option<ParentPolicy> = None;
        let mut replay_window_hash: Option<[u8; 32]> = None;
        let mut snapshot_blob_id: Option<ObjectId> = None;
        let mut seen_raw: Option<Vec<u8>> = None;
        let mut live_entries: Vec<(NodeId, LiveNode)> = Vec::new();
        let mut tombstones: Vec<(NodeId, Tombstone)> = Vec::new();

        let mut last_tag: Option<u16> = None;
        while let Some(field) = next_field(&mut cursor)? {
            ensure_nondecreasing_tag(&mut last_tag, field.tag)?;
            match field.tag {
                1 => set_once(&mut schema_version, read_u32(&field)?, "schema_version")?,
                2 => set_once(&mut baseline_block_id, read_object_id(&field)?, "baseline")?,
                3 => set_once(&mut lineage_horizon_id, read_object_id(&field)?, "horizon")?,
                4 => set_once(
                    &mut parent_policy,
                    ParentPolicy::from_code(read_enum_u16(&field)?)?,
                    "parent_policy",
                )?,
                5 => set_once(&mut replay_window_hash, read_hash32(&field)?, "window_hash")?,
                6 => set_once(&mut snapshot_blob_id, read_object_id(&field)?, "snapshot")?,
                10 => {
                    require_wire(&field, WireType::RecordListItem)?;
                    live_entries.push(decode_live_entry(field.value)?);
                }
                11 => {
                    require_wire(&field, WireType::RecordListItem)?;
                    tombstones.push(decode_tombstone(field.value)?);
                }
                12 => {
                    require_wire(&field, WireType::Bytes)?;
                    set_once(&mut seen_raw, field.value.to_vec(), "seen_ids")?;
                }
                other => return Err(malformed(format!("unknown lifecycle-cache tag {other}"))),
            }
        }

        let cache = Self {
            schema_version: schema_version
                .ok_or_else(|| malformed("missing schema_version".to_string()))?,
            baseline_block_id: baseline_block_id
                .ok_or_else(|| malformed("missing baseline_block_id".to_string()))?,
            lineage_horizon_id: lineage_horizon_id
                .ok_or_else(|| malformed("missing lineage_horizon_id".to_string()))?,
            parent_policy: parent_policy
                .ok_or_else(|| malformed("missing parent_policy".to_string()))?,
            replay_window_hash: replay_window_hash
                .ok_or_else(|| malformed("missing replay_window_hash".to_string()))?,
            snapshot_blob_id,
            live_entries,
            tombstones,
            seen_ids: decode_seen_ids(
                &seen_raw.ok_or_else(|| malformed("missing seen_ids".to_string()))?,
            )?,
        };
        cache.validate()?;
        Ok(cache)
    }

    /// Structural + cross-set validation (design v3 §P1-2/§P1-6). Provenance staleness
    /// (window-hash vs the real chain) and blob-kind verification are caller/later-slice
    /// concerns and are deliberately not done here.
    fn validate(&self) -> Result<()> {
        if self.schema_version != CACHE_SCHEMA_VERSION {
            return Err(stale_provenance(format!(
                "unsupported lifecycle-cache schema_version {}",
                self.schema_version
            )));
        }
        if self.parent_policy != ParentPolicy::SingleParent {
            return Err(stale_provenance(
                "lifecycle cache parent_policy is not single_parent (merge deferred to DC-13)"
                    .to_string(),
            ));
        }

        // live_entries: sorted by canonical repo_path, unique path and unique node_id.
        let mut prev_path: Option<&str> = None;
        let mut live_ids = std::collections::BTreeSet::new();
        for (node_id, node) in &self.live_entries {
            validate_node_record_shape(*node_id, node.kind, &node.content)?;
            let path = node.path.as_str();
            if let Some(previous) = prev_path {
                if path <= previous {
                    return Err(malformed(
                        "live_entries not strictly sorted by repo_path".to_string(),
                    ));
                }
            }
            prev_path = Some(path);
            if !live_ids.insert(*node_id) {
                return Err(malformed("duplicate live node_id".to_string()));
            }
        }

        // tombstones: sorted by raw node_id, unique node_id.
        let mut prev_tomb: Option<NodeId> = None;
        let mut tomb_ids = std::collections::BTreeSet::new();
        for (node_id, tombstone) in &self.tombstones {
            validate_node_record_shape(*node_id, tombstone.kind, &tombstone.content)?;
            if let Some(previous) = prev_tomb {
                if node_id.as_bytes() <= previous.as_bytes() {
                    return Err(malformed(
                        "tombstones not strictly sorted by node_id".to_string(),
                    ));
                }
            }
            prev_tomb = Some(*node_id);
            if !tomb_ids.insert(*node_id) {
                return Err(malformed("duplicate tombstone node_id".to_string()));
            }
        }

        // No id both live and tombstoned.
        if live_ids.intersection(&tomb_ids).next().is_some() {
            return Err(malformed(
                "node_id appears in both live and tombstone sets".to_string(),
            ));
        }

        // seen_ids strictly ascending + equals exactly live ∪ tombstoned.
        let mut prev_seen: Option<NodeId> = None;
        let mut seen_set = std::collections::BTreeSet::new();
        for id in &self.seen_ids {
            crate::node::node_lifecycle::ensure_node_id_nonzero(*id)?;
            if let Some(previous) = prev_seen {
                if id.as_bytes() <= previous.as_bytes() {
                    return Err(malformed("seen_ids not strictly ascending".to_string()));
                }
            }
            prev_seen = Some(*id);
            seen_set.insert(*id);
        }
        let union: std::collections::BTreeSet<NodeId> =
            live_ids.union(&tomb_ids).copied().collect();
        if seen_set != union {
            return Err(malformed(
                "seen_ids must equal exactly live ∪ tombstoned".to_string(),
            ));
        }
        Ok(())
    }

    /// Operational provenance (4.4-2b.2 step 2): recompute `replay_window_hash` over the
    /// **actually walked** single-parent block chain from `baseline_block_id` back to
    /// `lineage_horizon_id`, never over cache-supplied block ids. The walk is the shared
    /// [`replay::walk_single_parent_chain`] function authoritative replay also calls — so a
    /// non-merge lineage cannot drift between the two. **A `Merge` block in the window is a
    /// different story**: unlike authoritative replay's `ReaderLineage`, `ResolverLineage` below
    /// does not reduce a well-formed `Merge` to its mainline parent, so *any* `Merge` block —
    /// well-formed or not — fails this check, unconditionally (`ParentPolicy::Dc13MergeAware` names
    /// this as a reserved, unused-in-v1 extension point). Also fails closed on a cycle, a genesis
    /// that is not the claimed horizon, or a hash mismatch.
    fn verify_window_against_chain(&self, resolver: &impl BlockParentResolver) -> Result<()> {
        let walked = replay::walk_single_parent_chain(
            &ResolverLineage(resolver),
            self.baseline_block_id,
            self.lineage_horizon_id,
        )
        .map_err(|e| stale_provenance(e.to_string()))?;

        // `walked` is apply order (horizon/genesis first … baseline last). Provenance needs only
        // the ids == [block_id_0 .. n] for the window-hash preimage.
        let chain: Vec<ObjectId> = walked.into_iter().map(|(id, _parents)| id).collect();
        if compute_window_hash(&chain) != self.replay_window_hash {
            return Err(stale_provenance(
                "replay_window_hash does not match the walked single-parent chain".to_string(),
            ));
        }
        Ok(())
    }
}

/// Parent-resolver-backed lineage source, so cache provenance walks the lineage through the same
/// [`replay::walk_single_parent_chain`] as authoritative replay. Its `Block` is just the parent id
/// list (provenance needs no payload). A resolver error (missing, non-Block, decode failure) is
/// surfaced as a fail-closed unreadable-block walk error.
struct ResolverLineage<'a, P: BlockParentResolver>(&'a P);

impl<P: BlockParentResolver> replay::LineageBlockReader for ResolverLineage<'_, P> {
    type Block = Vec<ObjectId>;

    fn read_lineage_block(
        &self,
        block_id: ObjectId,
    ) -> std::result::Result<Vec<ObjectId>, replay::LifecycleReplayError> {
        self.0.parent_block_ids(&block_id).map_err(|e| {
            replay::LifecycleReplayError::UnreadableBlockInLineage {
                block_id,
                detail: e.to_string(),
            }
        })
    }

    fn parents_of(block: &Vec<ObjectId>) -> Vec<ObjectId> {
        block.clone()
    }
}

// --- field-level codec helpers ---

struct Field<'a> {
    tag: u16,
    wire: u8,
    value: &'a [u8],
}

fn next_field<'a>(cursor: &mut ByteCursor<'a>) -> Result<Option<Field<'a>>> {
    if cursor.is_finished() {
        return Ok(None);
    }
    let tag = cursor.read_u16()?;
    let wire = cursor.read_array::<1>()?[0];
    let len = usize::try_from(cursor.read_u64()?)
        .map_err(|_| malformed("field length exceeds usize".to_string()))?;
    let value = cursor.read_exact(len)?;
    Ok(Some(Field { tag, wire, value }))
}

/// Reject a node record whose `node_id` is the reserved all-zero value or whose
/// `kind`/`content` disagree — the same rule the substrate enforces at every seeding
/// boundary, so a production-encoded cache is structurally equivalent to a decoded one.
fn validate_node_record_shape(
    node_id: NodeId,
    kind: NodeKind,
    content: &NodeContent,
) -> Result<()> {
    crate::node::node_lifecycle::ensure_node_id_nonzero(node_id)?;
    crate::node::node_lifecycle::validate_kind_content_shape(kind, content)?;
    Ok(())
}

fn ensure_nondecreasing_tag(last: &mut Option<u16>, tag: u16) -> Result<()> {
    if let Some(prev) = *last {
        if tag < prev {
            return Err(malformed(format!(
                "non-canonical TLV tag order: {tag} after {prev}"
            )));
        }
    }
    *last = Some(tag);
    Ok(())
}

fn require_wire(field: &Field<'_>, expected: WireType) -> Result<()> {
    if field.wire == expected as u8 {
        Ok(())
    } else {
        Err(malformed(format!(
            "tag {} has wire type 0x{:02x}, expected 0x{:02x}",
            field.tag, field.wire, expected as u8
        )))
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<()> {
    if slot.is_some() {
        return Err(malformed(format!("duplicate singleton field {name}")));
    }
    *slot = Some(value);
    Ok(())
}

fn read_u32(field: &Field<'_>) -> Result<u32> {
    require_wire(field, WireType::U32)?;
    let array: [u8; 4] = field
        .value
        .try_into()
        .map_err(|_| malformed("u32 field wrong length".to_string()))?;
    Ok(u32::from_be_bytes(array))
}

fn read_enum_u16(field: &Field<'_>) -> Result<u16> {
    require_wire(field, WireType::EnumU16)?;
    let array: [u8; 2] = field
        .value
        .try_into()
        .map_err(|_| malformed("enum_u16 field wrong length".to_string()))?;
    Ok(u16::from_be_bytes(array))
}

fn read_object_id(field: &Field<'_>) -> Result<ObjectId> {
    require_wire(field, WireType::ObjectId)?;
    let array: [u8; 32] = field
        .value
        .try_into()
        .map_err(|_| malformed("object_id field wrong length".to_string()))?;
    Ok(ObjectId::from_bytes(array))
}

fn read_hash32(field: &Field<'_>) -> Result<[u8; 32]> {
    require_wire(field, WireType::Bytes)?;
    field
        .value
        .try_into()
        .map_err(|_| malformed("expected 32-byte hash".to_string()))
}

fn read_node_id_bytes(field: &Field<'_>) -> Result<NodeId> {
    require_wire(field, WireType::Bytes)?;
    let array: [u8; 32] = field
        .value
        .try_into()
        .map_err(|_| malformed("node_id field wrong length".to_string()))?;
    NodeId::try_from_bytes(array)
}

fn read_repo_path(field: &Field<'_>) -> Result<crate::path::RepoPath> {
    require_wire(field, WireType::RepoPath)?;
    let text = core::str::from_utf8(field.value)
        .map_err(|_| malformed("repo_path is not UTF-8".to_string()))?;
    crate::path::RepoPath::parse(text)
}

fn read_symlink_target(field: &Field<'_>) -> Result<String> {
    require_wire(field, WireType::String)?;
    let text = core::str::from_utf8(field.value)
        .map_err(|_| malformed("symlink target is not UTF-8".to_string()))?;
    Ok(text.to_string())
}

/// Shared node-record body decode (file/symlink discriminator). Used for both live and
/// tombstone records — the wire shape is identical (path, node_id, kind, content).
fn decode_node_record(
    bytes: &[u8],
) -> Result<(NodeId, crate::path::RepoPath, NodeKind, NodeContent)> {
    let mut cursor = ByteCursor::new(bytes);
    let mut path: Option<crate::path::RepoPath> = None;
    let mut node_id: Option<NodeId> = None;
    let mut node_kind: Option<NodeKind> = None;
    let mut blob_id: Option<ObjectId> = None;
    let mut mode: Option<u32> = None;
    let mut target: Option<String> = None;

    let mut last_tag: Option<u16> = None;
    while let Some(field) = next_field(&mut cursor)? {
        ensure_nondecreasing_tag(&mut last_tag, field.tag)?;
        match field.tag {
            1 => set_once(&mut path, read_repo_path(&field)?, "path")?,
            2 => set_once(&mut node_id, read_node_id_bytes(&field)?, "node_id")?,
            3 => set_once(
                &mut node_kind,
                NodeKind::from_code(read_enum_u16(&field)?)?,
                "node_kind",
            )?,
            4 => set_once(&mut blob_id, read_object_id(&field)?, "blob_id")?,
            5 => set_once(&mut mode, read_u32(&field)?, "normalized_mode")?,
            6 => set_once(&mut target, read_symlink_target(&field)?, "symlink_target")?,
            other => return Err(malformed(format!("unknown node-record tag {other}"))),
        }
    }

    let path = path.ok_or_else(|| malformed("node record missing path".to_string()))?;
    let node_id = node_id.ok_or_else(|| malformed("node record missing node_id".to_string()))?;
    let node_kind =
        node_kind.ok_or_else(|| malformed("node record missing node_kind".to_string()))?;

    let content = match node_kind {
        NodeKind::TextFile | NodeKind::BinaryFile => {
            if target.is_some() {
                return Err(malformed(
                    "file node record must not carry a symlink target".to_string(),
                ));
            }
            let blob_id =
                blob_id.ok_or_else(|| malformed("file node record missing blob_id".to_string()))?;
            let mode = mode
                .ok_or_else(|| malformed("file node record missing normalized_mode".to_string()))?;
            NodeContent::File { blob_id, mode }
        }
        NodeKind::Symlink => {
            // Design v3 §P1-4: symlink entries forbid field 5 (and blob_id) entirely.
            if blob_id.is_some() || mode.is_some() {
                return Err(malformed(
                    "symlink node record must not carry blob_id or normalized_mode".to_string(),
                ));
            }
            let target = target
                .ok_or_else(|| malformed("symlink node record missing target".to_string()))?;
            NodeContent::Symlink { target }
        }
    };
    Ok((node_id, path, node_kind, content))
}

fn decode_live_entry(bytes: &[u8]) -> Result<(NodeId, LiveNode)> {
    let (node_id, path, kind, content) = decode_node_record(bytes)?;
    Ok((
        node_id,
        LiveNode {
            path,
            kind,
            content,
        },
    ))
}

fn decode_tombstone(bytes: &[u8]) -> Result<(NodeId, Tombstone)> {
    let (node_id, path, kind, content) = decode_node_record(bytes)?;
    Ok((
        node_id,
        Tombstone {
            kind,
            content,
            path,
        },
    ))
}

fn decode_seen_ids(raw: &[u8]) -> Result<Vec<NodeId>> {
    if raw.len() % 32 != 0 {
        return Err(malformed(
            "seen_ids length is not a multiple of 32".to_string(),
        ));
    }
    let mut ids = Vec::with_capacity(raw.len() / 32);
    for chunk in raw.chunks_exact(32) {
        let array: [u8; 32] = chunk
            .try_into()
            .map_err(|_| malformed("seen_id chunk wrong length".to_string()))?;
        ids.push(NodeId::try_from_bytes(array)?);
    }
    Ok(ids)
}

fn encode_node_record(
    node_id: &NodeId,
    path: &crate::path::RepoPath,
    kind: NodeKind,
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

fn malformed(detail: String) -> PrikkError {
    PrikkError::MalformedData(format!("lifecycle cache: {detail}"))
}

fn stale_provenance(detail: String) -> PrikkError {
    PrikkError::Integrity(format!("lifecycle cache stale provenance: {detail}"))
}

/// Resolves a block's parent block ids, in seal order, for operational provenance
/// verification. Empty at genesis. v1 lifecycle windows require a single-parent chain;
/// more than one parent fails closed. A real store-backed resolver (reading `Block`
/// objects) is wired in the threading slice; this trait keeps the walk testable.
pub(crate) trait BlockParentResolver {
    fn parent_block_ids(&self, block_id: &ObjectId) -> Result<Vec<ObjectId>>;
}

/// First trust rung (4.4-2b.2 steps 1–2): a decoded cache that has passed structural
/// validation, **operational provenance** (its `replay_window_hash` recomputed over the
/// actually walked single-parent chain), and file-entry blob-kind verification. Still
/// **NOT** authority for a `node_id` reuse or restoration-equivalence decision — those
/// require replay-derived or replay-compared state (later rungs). There is deliberately no
/// method here that yields such a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedLifecycleCache {
    decoded: DecodedLifecycleCache,
}

impl ValidatedLifecycleCache {
    /// Build the rung: structural re-validation (the input is not trusted to have come from
    /// `decode`, since `pub(crate)` fields allow direct construction), then operational
    /// provenance against the walked single-parent chain, then blob-kind verification. Any
    /// step fails closed.
    pub(crate) fn from_decoded(
        decoded: DecodedLifecycleCache,
        blob_resolver: &impl BlobKindResolver,
        parent_resolver: &impl BlockParentResolver,
    ) -> Result<Self> {
        decoded.validate()?;
        decoded.verify_window_against_chain(parent_resolver)?;
        for (_, node) in &decoded.live_entries {
            verify_file_blob_kind(node.kind, &node.content, blob_resolver)?;
        }
        for (_, tombstone) in &decoded.tombstones {
            verify_file_blob_kind(tombstone.kind, &tombstone.content, blob_resolver)?;
        }
        Ok(Self { decoded })
    }

    /// P2-2: bind the cache to the caller's intended baseline. A cache valid for some other
    /// checkpoint must not be accepted where `expected_baseline_block_id` was meant.
    pub(crate) fn from_decoded_for_baseline(
        decoded: DecodedLifecycleCache,
        expected_baseline_block_id: ObjectId,
        blob_resolver: &impl BlobKindResolver,
        parent_resolver: &impl BlockParentResolver,
    ) -> Result<Self> {
        if decoded.baseline_block_id != expected_baseline_block_id {
            return Err(stale_provenance(
                "cache baseline does not match the caller's intended baseline".to_string(),
            ));
        }
        Self::from_decoded(decoded, blob_resolver, parent_resolver)
    }

    /// Rebuild a `NodeLifecycleState` from the (already validated) cache, for comparison
    /// against authoritative replay. This is **not** authority by itself.
    fn to_node_lifecycle_state(&self) -> Result<NodeLifecycleState> {
        let mut state = NodeLifecycleState::new();
        for (node_id, node) in &self.decoded.live_entries {
            state.seed_live_node(*node_id, node.clone())?;
        }
        for (node_id, tombstone) in &self.decoded.tombstones {
            state.seed_tombstone(*node_id, tombstone.clone())?;
        }
        Ok(state)
    }
}

/// A validated cache **proven equal** to authoritative replay for the same baseline (rung 4).
/// This is the only cache-derived rung that may participate in restoration-equivalence /
/// `node_id` reuse decisions once wired — and even then only because it equals replay. The
/// decisive guarantee: a cache with correct provenance but false live/tombstone contents is
/// rejected here, because the rebuilt state will not equal the replayed state.
///
/// **Scope of the certification (E3).** The compare proves equality of the lifecycle *state*
/// rebuilt from `live_entries` and `tombstones` only. It does **not** certify any cache-adjacent
/// materialization hint such as `snapshot_blob_id`. Accelerated consumers (checkout/status) may
/// rely only on the certified lifecycle entries; `snapshot_blob_id` must not back materialization
/// acceleration unless and until it gains its own validation path against the authoritative
/// state/root. A narrowed accessor exposing only the certified entries should land with the first
/// consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComparedLifecycleCache {
    validated: ValidatedLifecycleCache,
}

/// Why a cache failed to certify against authoritative replay (E1). The classes are distinct so a
/// future caller can branch on the cause: a **cache** fault (`BaselineMismatch`, `HorizonMismatch`,
/// `CacheRejected`, `ContentMismatch`) is a reason to drop the cache and fall back to authoritative
/// replay, whereas `ReplayUnavailable` means authoritative history itself could not be
/// reconstructed and must be surfaced as an integrity fault — never silently bypassed by trusting
/// the cache.
#[derive(Debug)]
pub(crate) enum CacheCertificationError {
    /// The cache's declared baseline is not the caller's intended baseline.
    BaselineMismatch { expected: ObjectId, found: ObjectId },
    /// The cache's declared lineage horizon is not the caller's intended horizon.
    HorizonMismatch { expected: ObjectId, found: ObjectId },
    /// Structural, provenance, or blob-kind validation rejected the cache.
    CacheRejected(PrikkError),
    /// Authoritative replay for the baseline could not be produced (integrity fault, not a mere
    /// cache miss).
    ReplayUnavailable(PrikkError),
    /// The cache validated and replay succeeded, but the rebuilt cache state is not equal to
    /// authoritative replay (correct provenance, false live/tombstone contents).
    ContentMismatch,
}

impl fmt::Display for CacheCertificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BaselineMismatch { expected, found } => write!(
                f,
                "lifecycle cache certification failed: baseline mismatch \
                 (caller expected {expected}, cache declares {found})"
            ),
            Self::HorizonMismatch { expected, found } => write!(
                f,
                "lifecycle cache certification failed: horizon mismatch \
                 (caller expected {expected}, cache declares {found})"
            ),
            Self::CacheRejected(e) => write!(
                f,
                "lifecycle cache certification failed: cache rejected by validation: {e}"
            ),
            Self::ReplayUnavailable(e) => write!(
                f,
                "lifecycle cache certification failed: authoritative replay unavailable: {e}"
            ),
            Self::ContentMismatch => write!(
                f,
                "lifecycle cache certification failed: cache contents disagree with \
                 authoritative replay"
            ),
        }
    }
}

impl std::error::Error for CacheCertificationError {}

impl From<CacheCertificationError> for PrikkError {
    /// At the `PrikkError` boundary every certification failure is an integrity fault. Callers that
    /// need to *branch* (drop-cache-and-replay vs. surface-unavailability) must consume the
    /// structured [`CacheCertificationError`] directly rather than this flattened form.
    fn from(e: CacheCertificationError) -> Self {
        PrikkError::Integrity(e.to_string())
    }
}

impl ComparedLifecycleCache {
    /// Compare a validated cache to authoritative replay for the **same** baseline. Fails closed
    /// with a structured [`CacheCertificationError`] on a baseline mismatch or any disagreement
    /// between the cache contents and the replayed state (the right-provenance/false-tombstone
    /// case).
    pub(crate) fn from_validated_and_replay(
        validated: ValidatedLifecycleCache,
        replay: &ReplayDerivedLifecycleState,
    ) -> std::result::Result<Self, CacheCertificationError> {
        if validated.decoded.baseline_block_id != replay.baseline_block_id {
            return Err(CacheCertificationError::BaselineMismatch {
                expected: replay.baseline_block_id,
                found: validated.decoded.baseline_block_id,
            });
        }
        let cache_state = validated
            .to_node_lifecycle_state()
            .map_err(CacheCertificationError::CacheRejected)?;
        if cache_state != replay.state {
            return Err(CacheCertificationError::ContentMismatch);
        }
        Ok(Self { validated })
    }
}

/// Rung-4 producer: certify a decoded cache against authoritative replay, yielding the **only**
/// cache-derived rung permitted to accelerate identity (reuse / restoration-equivalence)
/// decisions. The caller's intended baseline **and** horizon are both bound explicitly up front
/// (E2); the cache is then validated (structure, provenance over the shared lineage walk,
/// blob-kind), authoritatively replayed for the same baseline, and compared in full. Any failure
/// returns a structured [`CacheCertificationError`] so callers can distinguish a droppable cache
/// fault from authoritative-history unavailability. The cache is an accelerator proven equal to
/// replay — never a root of trust. On any failure, callers fall back to [`replay_derived_state`].
pub(crate) fn certified_compared_cache<R: ObjectReader>(
    reader: &R,
    decoded: DecodedLifecycleCache,
    expected_baseline_block_id: ObjectId,
    expected_lineage_horizon_id: ObjectId,
) -> std::result::Result<ComparedLifecycleCache, CacheCertificationError> {
    // Bind the caller's intended baseline and horizon explicitly and symmetrically, before any
    // validation or replay. Both would otherwise fail closed implicitly (baseline through the
    // validator, horizon through the shared genesis walk), but the producer API exposes the
    // dedicated mismatch class for each rather than collapsing baseline into `CacheRejected`.
    if decoded.baseline_block_id != expected_baseline_block_id {
        return Err(CacheCertificationError::BaselineMismatch {
            expected: expected_baseline_block_id,
            found: decoded.baseline_block_id,
        });
    }
    if decoded.lineage_horizon_id != expected_lineage_horizon_id {
        return Err(CacheCertificationError::HorizonMismatch {
            expected: expected_lineage_horizon_id,
            found: decoded.lineage_horizon_id,
        });
    }
    let resolver = StoreBackedResolver::new(reader);
    let validated = ValidatedLifecycleCache::from_decoded_for_baseline(
        decoded,
        expected_baseline_block_id,
        &resolver,
        &resolver,
    )
    .map_err(CacheCertificationError::CacheRejected)?;
    let replay = replay_derived_state(
        reader,
        expected_baseline_block_id,
        expected_lineage_horizon_id,
    )
    .map_err(CacheCertificationError::ReplayUnavailable)?;
    ComparedLifecycleCache::from_validated_and_replay(validated, &replay)
}

/// Verify one file entry's referenced blob kind matches its `NodeKind` (symlink entries
/// carry no blob and pass). Reuses the canonical `NodeKind::from_file_blob_kind` rule.
fn verify_file_blob_kind(
    kind: NodeKind,
    content: &NodeContent,
    resolver: &impl BlobKindResolver,
) -> Result<()> {
    let NodeContent::File { blob_id, .. } = content else {
        return Ok(());
    };
    match resolver.blob_kind(blob_id)? {
        None => Err(PrikkError::Integrity(format!(
            "lifecycle cache: blob required for kind verification is missing: {blob_id}"
        ))),
        Some(blob_kind) => {
            let implied = NodeKind::from_file_blob_kind(blob_kind)?;
            if implied == kind {
                Ok(())
            } else {
                Err(PrikkError::Integrity(format!(
                    "lifecycle cache: file node kind {kind:?} disagrees with referenced \
                     blob kind (implies {implied:?})"
                )))
            }
        }
    }
}
