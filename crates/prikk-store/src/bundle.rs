//! History exchange artifact (DC-78 §D6): a **verifiable subset**, never a summary. A bundle carries
//! the exported ref's RefState plus every object reachable from its target Block back to genesis
//! (ruling 2) — the same objects `verify_repository` already checks for any locally sealed history,
//! unconditionally, regardless of which ref (if any) points to them. Import writes exactly those
//! objects plus one `received` pointer (`crate::received`); it never touches `refs/by-id/`, never
//! advances a local ref, and never adopts a MAINTAINER key into the local trust policy — adopting
//! trust for an imported key remains the operator's own explicit `trust maintainer add` call. The
//! receiver's confidence comes from running ordinary, unmodified `verify_repository` afterward: this
//! module adds a serialization boundary and an import path, and deliberately no new verification
//! machinery (§D6's "no new verification path").
//!
//! **`heads/*` and `tags/*` both export** (`resolve_ref_target_block`). A branch's `target_object_id`
//! names its Block directly; a tag's names a Tag object one hop away
//! (`TagPayload.target_block_id` is the Block), mirroring the two-hop model
//! `refs/verify/scan.rs`'s own `ensure_block_exists`/tag-target check already uses. **The Tag
//! envelope itself is part of the exported object set, not just the Block closure it points to**:
//! the tag ref's `RefState` travels unconditionally either way, and a receiver holding a signed
//! RefState that names a Tag object they do not have fails `verify` with a message naming exactly
//! that (DC-78 bundle-export tag gap follow-up, `bundle-export-tag-ref-gap-v1.md` — found while
//! investigating RFC 115 Stage 1's own reachable-set question, fixed independently of it).
//!
//! **DC-53 Stage 2, D6: `PBNDL002` carries an AUTHOR key-material section.** The section's scope is
//! exactly the AUTHOR `key_id`s of the Patches this bundle carries, derived from the exported objects
//! themselves, never the whole local `author_key_index` container -- exporting everything this
//! repository has ever seen would leak every author it has observed to every recipient, a disclosure
//! the sender did not choose. Material is optional per-`key_id`: a bundle omits a `key_id` this
//! repository never recorded material for (import still succeeds; the Patch reads Unverifiable, not
//! Sound and not a failure). **Import records material; `verify` decides (D7)** -- import performs no
//! cryptographic check of whether a transported key actually matches any Patch's signature, the same
//! way it already writes objects without re-verifying them; a transported key that doesn't verify a
//! Patch's signature is recorded anyway and `verify` reports that Patch `Failed`, reached through the
//! transport path rather than local authoring but the same underlying check (D3's third row).
//!
//! **Two independent conflict checks, and both are fully pre-write.** Before any write: the bundle's
//! own author-key section must not itself claim two different public keys for one `key_id` -- a
//! hostile or merely stale bundle must fail the whole import rather than leave a receiver with an
//! unresolvable, permanently unverifiable `key_id` (`author_key_index.rs`'s own container has no
//! prune/repair path). The second check -- a transported key conflicting with material this
//! repository *already* has -- is checked the same way: **every** transported key is validated
//! against local material, via `check_author_key_conflict`, before any of them is recorded, both
//! inside the one `ActiveLock` held for the section. This is why: a refused import must leave the
//! author-key container exactly as untouched as a refused bundle-internal check leaves it -- checking
//! and recording one entry at a time let a conflict at entry `k` leave entries `1..k-1` durably
//! appended first (DC-53 Stage 2 follow-up, `multi-key-import-partial-write-v1.md`). Recording itself
//! still goes through `record_author_key_material`, Step 1's own function, unchanged -- no separate
//! transport-side pinning rule, and no second copy of the conflict definition (Step 1's C2 ruling).
//!
//! **`PBNDL001` (Stage 1 and earlier) is accepted on import, never emitted on export** (DC-53 Stage 2
//! follow-up, `bundle-v1-import-regression-v1.md`). `layout.rs`'s own retired-repository-format
//! messages instruct a user to open an old repository with an old prikk build and `bundle export`
//! from it; that build only ever produces `PBNDL001`, and refusing to import it here severed the one
//! migration path those messages promise, in both directions at once -- found and fixed the same day
//! it shipped. A `PBNDL001` bundle decodes exactly like a `PBNDL002` one with an empty author-key
//! section: not a special case, since DC-53 already defines "no recorded material" as `Unverifiable`
//! (vector 7). Read compatibility only, the same asymmetry every repository-format transition in this
//! project already has -- read what the past wrote, write only the present.
//!
//! **`import_bundle` validates closure completeness before any write** (DC-78
//! `import-closure-validation-handoff-v1.md`): the exported ref's target must resolve, and every blob
//! (whether a Patch's own operations or a Block's own `snapshot_blob_ref` names it), patch, and block
//! parent a carried object names must be present -- carried by this bundle, or already in this
//! repository. `accept_exchange_artifact` (`patch_exchange/accept.rs`) already refused
//! the same class of defect at receipt; `import_bundle` did not, so a bundle whose target object it
//! never shipped used to import successfully and land a dangling received pointer, visible only at the
//! next `verify`, long after the import that caused it. **This is an intentional behaviour change: a
//! bundle that previously imported may now be refused.** That is the point, not a regression -- a tag
//! bundle produced before the DC-78 tag-export fix (`d605c10`), which carries the RefState but not the
//! Tag object, is the concrete case: it now fails at import, naming the bundle at the moment it is
//! offered, instead of importing and failing a later `verify` with no indication of which import caused
//! it.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;

mod preview;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    AttestationPayload, BlockPayload, ObjectEnvelope, ObjectId, ObjectType, RefKind,
    RefStatePayload, Signature, SignerRole, TagPayload,
};

use crate::author::author_key_index::{
    AuthorKeyEntry, check_author_key_conflict, lookup_author_key_entries,
    record_author_key_material,
};
#[cfg(test)]
use crate::foundation::byte_cursor::ByteCursor;
#[cfg(test)]
use crate::foundation::file_codec::read_bounded_object_frame;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_bytes_u64, push_u64,
};
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::layout::{
    DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryFormat, RepositoryLayout,
};
use crate::lock::{ActiveLock, acquire_container_locks};
use crate::object_store::{ObjectReadSnapshot, ObjectReader, ObjectWriteSession};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations,
};
use crate::refs::RefStore;

/// DC-44 increment 3, `bundle-manifest-handoff-v1.md`: the format bump that made room for the
/// self-describing manifest section. Always emitted on export; `PBNDL001` and `PBNDL002` are both
/// still accepted on import (see `RETIRED_BUNDLE_MAGIC_V1`/`RETIRED_BUNDLE_MAGIC_V2`), so this is a
/// write-side-only version, not a hard cutover -- the same asymmetry the `PBNDL001` -> `PBNDL002`
/// bump already established.
const BUNDLE_MAGIC: &[u8; 8] = b"PBNDL003";
/// `PBNDL001` (Stage 1 and earlier bundles, no author-key section, no manifest). Accepted on import
/// -- see `decode_bundle`'s own doc and the module doc's follow-up note -- never emitted on export.
const RETIRED_BUNDLE_MAGIC_V1: &[u8; 8] = b"PBNDL001";
/// `PBNDL002` (DC-53 Stage 2 through DC-44 increment 2: an author-key section, but no manifest).
/// Accepted on import, never emitted on export -- same read-only asymmetry as `PBNDL001`.
const RETIRED_BUNDLE_MAGIC_V2: &[u8; 8] = b"PBNDL002";

/// DC-86 default hard block on a bundle's declared object count, checked as early as the format
/// allows — right after the count header field, before a single object is decoded. Not a claim about
/// what any real bundle needs; a ceiling an operator can rely on existing at all.
pub const DEFAULT_BUNDLE_MAX_OBJECT_COUNT: usize = 100_000;

/// DC-86 default hard block on a bundle's total encoded byte length, checked before any decoding
/// begins. This length-prefixed format can never decode to more logical content than its encoded
/// input size, so bounding the input bytes is a tight, cheap proxy for bounding decoded bytes —
/// cheaper than decoding first only to discover the result should have been refused. 256 MiB.
pub const DEFAULT_BUNDLE_MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// RFC 158 Stage A §3: the default per-object bound, equal to the total default so that nothing
/// importing on 0.46.0 starts refusing — an object can never exceed its own bundle's 256 MiB total
/// at the defaults, so this bound is inert until an operator lowers it or a later stage lets an
/// artifact legitimately exceed 256 MiB.
pub const DEFAULT_BUNDLE_MAX_OBJECT_BYTES: usize = DEFAULT_BUNDLE_MAX_TOTAL_BYTES;

/// DC-86 resource bound for [`import_bundle`], checked before any object is decoded or written —
/// DC-57's shape: a hard block ahead of any write, with a documented default the CLI may override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleImportOptions {
    /// Maximum objects a bundle may declare. Refused before the decode loop runs.
    pub max_object_count: usize,
    /// Maximum encoded byte length a bundle may have. Refused before `decode_bundle` runs at all.
    pub max_total_bytes: usize,
    /// RFC 158 Stage A §3: maximum encoded byte length any *one* object frame in the bundle may
    /// have. Refused on the length prefix, before that frame is copied or decoded.
    pub max_object_bytes: usize,
}

impl BundleImportOptions {
    /// [`DEFAULT_BUNDLE_MAX_OBJECT_COUNT`], [`DEFAULT_BUNDLE_MAX_TOTAL_BYTES`], and
    /// [`DEFAULT_BUNDLE_MAX_OBJECT_BYTES`].
    #[must_use]
    pub const fn default_limits() -> Self {
        Self {
            max_object_count: DEFAULT_BUNDLE_MAX_OBJECT_COUNT,
            max_total_bytes: DEFAULT_BUNDLE_MAX_TOTAL_BYTES,
            max_object_bytes: DEFAULT_BUNDLE_MAX_OBJECT_BYTES,
        }
    }

    /// Override the maximum object count.
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

/// DC-44 increment 3, `bundle-manifest-handoff-v1.md` §1: a bundle is one ref's closure, not a
/// claim about the rest of the source repository. `SingleRef` is the only variant today because
/// `export_bundle` is single-ref by design (§5 -- multi-ref export is a different increment); a
/// future multi-ref export would add a variant here, never repurpose this one to mean something
/// else. Stated in the manifest so a restoring operator meets the limitation in the artifact
/// itself, not only on a documentation page they may not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleScope {
    /// This bundle carries exactly one ref's full genesis-complete closure. Other refs, if any
    /// exist in the exporting repository at export time, are not included, and this bundle makes
    /// no claim about them -- silence about a ref is not evidence it does not exist.
    SingleRef,
}

impl BundleScope {
    const fn wire_tag(self) -> u64 {
        match self {
            Self::SingleRef => 0,
        }
    }

    fn from_wire_tag(tag: u64) -> Result<Self> {
        match tag {
            0 => Ok(Self::SingleRef),
            other => Err(PrikkError::MalformedData(format!(
                "bundle manifest declares unknown scope tag {other}"
            ))),
        }
    }
}

/// DC-44 increment 3: a bundle's self-describing manifest (`PBNDL003` only -- `None` on
/// [`BundleVerifyReport`] for a `PBNDL001`/`PBNDL002` bundle, which carries no manifest section at
/// all). Every field here answers a question [`verify_bundle`] could not answer before this
/// increment (handoff §2's own criterion) -- object digests were considered and rejected: an
/// object's id already is a content hash of its own bytes, and increment 1 already verifies every
/// closure reference resolves to an object whose freshly recomputed id matches, so a manifest-level
/// digest would duplicate a check that already exists rather than add one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleManifest {
    /// The on-disk repository format (`layout::RepositoryFormat`) that produced this bundle --
    /// answers "which repository format produced this," which `verify_bundle` could not say
    /// before (handoff §2). A different axis from the bundle wire format (`PBNDL00x`) itself, the
    /// same distinction `layout.rs`'s own `RepositoryFormat` doc already draws.
    pub repository_format: u32,
    /// The exporting `prikk` build's own version (`CARGO_PKG_VERSION`) -- provenance for triage,
    /// not a compatibility signal (the magic bump is what actually gates compatibility).
    pub tool_version: String,
    /// §1's honesty gap, closed: this bundle is one ref's closure, and other refs (if any) are not
    /// included. The only variant today; see [`BundleScope`].
    pub scope: BundleScope,
}

/// Summary of a bundle export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleExportReport {
    /// The exported ref's own name in the source repository.
    pub ref_name: String,
    /// The exported ref's target Block at export time.
    pub tip_block_id: ObjectId,
    /// Total objects carried in the bundle (the RefState plus its full genesis-complete closure).
    pub object_count: usize,
    /// DC-53 Stage 2, D6: AUTHOR key entries carried in the bundle's author-key section -- one per
    /// distinct `key_id` among the bundle's Patches for which this repository has local material,
    /// never a count of the Patches themselves.
    pub author_key_count: usize,
    /// DC-44 increment 3: this export's own self-describing manifest. Always present -- export
    /// always emits `PBNDL003`.
    pub manifest: BundleManifest,
}

/// Summary of an offline bundle verification (DC-44 increment 1). Never written by anything that
/// touches a repository — [`verify_bundle`] reads only the file bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleVerifyReport {
    /// The exported ref's own name in the source repository.
    pub ref_name: String,
    /// The exported ref's RefState object id.
    pub ref_state_id: ObjectId,
    /// The ref's resolved tip Block id (one hop for a Branch, two for a Tag — the same resolution
    /// `export_bundle` performs).
    pub tip_block_id: ObjectId,
    /// Total objects carried in the bundle.
    pub object_count: usize,
    /// AUTHOR key entries carried in the bundle's author-key section.
    pub author_key_count: usize,
    /// DC-44 increment 3: this bundle's own self-describing manifest, if it carries one.
    /// `None` for a `PBNDL001`/`PBNDL002` bundle -- both predate the manifest section, and the
    /// absence itself is meaningful, not a decode failure (§4.2: their decode path is unchanged).
    pub manifest: Option<BundleManifest>,
}

/// Summary of a bundle import.
///
/// `#[non_exhaustive]` from RFC 156, which added two fields: the next one is then not a break.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleImportReport {
    /// The local received-namespace name the import was recorded under (`remotes/<origin ref name>`).
    pub ref_name: String,
    /// The imported RefState's object id, now the received pointer's target.
    pub ref_state_id: ObjectId,
    /// Total objects the bundle carried.
    pub object_count: usize,
    /// Objects that did not already exist in this repository's object store before this import.
    pub written_object_count: usize,
    /// RFC 156, format 7: objects this repository already held that gained at least one signature from
    /// the bundle's copy, written as a superseding record.
    pub merged_object_count: usize,
    /// RFC 156 §4: signatures the bundle carried for objects already held here that were not stored —
    /// a maintainer key not adopted here, or an author key with no material to verify against.
    pub dropped_signatures: Vec<crate::DroppedSignature>,
    /// DC-53 Stage 2, D7: AUTHOR key entries the bundle carried and this import recorded locally.
    /// Continuity only, not a trust decision -- unlike a trusted maintainer key, recording this
    /// grants no admission judgement, only lets `verify` distinguish Sound from Unverifiable for the
    /// Patches it covers.
    pub recorded_author_key_count: usize,
}

/// How a bundle's own history relates to the local ref it was previewed against (RFC 144 §4m.3
/// answer #1 -- "does it apply at all?"). Never an error: a bundle whose history shares no ancestry
/// with the local ref is a legitimate, reportable outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundlePreviewConnectivity {
    /// The local ref named has never been published (RFC 144 §4n.1) -- there is no local target
    /// to compare against, not merely an empty one. Distinct from `FastForward`, which asserts
    /// *local history is a prefix of the bundle's*: a ref with no history has no prefix to be
    /// one. The bundle's entire content previews as `Created`; there is nothing to conflict with.
    NoLocalHistory,
    /// No shared ancestry with the local ref at all.
    DoesNotConnect,
    /// The bundle's own target block is already an ancestor of the local ref's current target --
    /// this bundle brings nothing this repository does not already have.
    AlreadyIncluded,
    /// The local ref's current target is an ancestor of the bundle's own target: a fast-forward.
    /// No conflict is possible by construction.
    FastForward,
    /// Both sides carry commits since their common ancestor. See [`BundlePreviewConflict`] for
    /// whether this would conflict.
    Diverged,
}

impl BundlePreviewConnectivity {
    /// Stable CLI/JSON label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoLocalHistory => "no-local-history",
            Self::DoesNotConnect => "does-not-connect",
            Self::AlreadyIncluded => "already-included",
            Self::FastForward => "fast-forward",
            Self::Diverged => "diverged",
        }
    }
}

/// The answer to RFC 144 §4m.3's answer #2 -- "would it conflict?" -- present only when
/// [`BundlePreviewConnectivity::FastForward`] or [`BundlePreviewConnectivity::Diverged`] (a
/// disconnected or already-included bundle has no such question to answer). **A conflict is the
/// result of a successful preview, not a failure of one**: this is a field, never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundlePreviewConflict {
    /// The bundle's own new operations replayed cleanly onto the local ref's current state.
    AppliesCleanly,
    /// Replaying the bundle's own new operations onto the local ref's current state failed --
    /// `detail` names the underlying replay error.
    Conflict {
        /// The replay failure that constitutes the conflict.
        detail: String,
    },
    /// More than one common ancestor has equal claim to being the merge base (a genuinely
    /// ambiguous history). Not guessed at -- an honest "not answerable today," per RFC 144 §4m's
    /// own instruction that this beats a field that is always populated and sometimes wrong.
    Undetermined {
        /// Why this could not be determined.
        reason: String,
    },
}

impl BundlePreviewConflict {
    /// Stable CLI/JSON label.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AppliesCleanly => "applies-cleanly",
            Self::Conflict { .. } => "conflict",
            Self::Undetermined { .. } => "undetermined",
        }
    }
}

/// One node-granularity effect the bundle's own new operations would have on the local ref's
/// current state (RFC 144 §4m.3). **Never a rename** -- see [`BundlePreviewReport`]'s own doc for
/// the honesty limit this reflects (§4g.3/§4m.4): until rename authoring lands, a moved path
/// previews as a delete plus a create, and the report says so rather than let a reader infer
/// intent the repository does not record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundlePreviewEffect {
    /// Repository-relative path.
    pub path: String,
    /// What would happen to this path.
    pub kind: BundlePreviewEffectKind,
    /// Byte length in the local ref's current state, when the path currently exists.
    pub current_bytes: Option<u64>,
    /// Byte length after the bundle's own new operations, when the path would exist.
    pub after_bytes: Option<u64>,
}

/// See [`BundlePreviewEffect`]'s own doc for why this has no `Renamed` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundlePreviewEffectKind {
    /// The path does not currently exist and would be created.
    Created,
    /// The path currently exists and would be removed.
    Deleted,
    /// The path's content would change.
    Edited,
    /// The path's content is unchanged but its mode bits would change.
    PermissionChanged,
}

impl BundlePreviewEffectKind {
    /// Stable CLI/JSON label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Deleted => "deleted",
            Self::Edited => "edited",
            Self::PermissionChanged => "permission-changed",
        }
    }
}

/// Full report of previewing a bundle's impact on a local ref (RFC 144 §4m), never mutating
/// anything -- see [`preview_bundle`]'s own doc comment for the write-nothing guarantee and its
/// control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundlePreviewReport {
    /// The bundle's own exported ref name (not the local ref it was previewed against).
    pub bundle_ref_name: String,
    /// The local ref this preview compared against.
    pub local_ref_name: String,
    /// How the bundle's history relates to the local ref (§4m.3 answer #1).
    pub connectivity: BundlePreviewConnectivity,
    /// Would applying the bundle conflict (§4m.3 answer #2)? `None` only for
    /// [`BundlePreviewConnectivity::DoesNotConnect`] or `AlreadyIncluded`, where the question does
    /// not apply.
    pub conflict: Option<BundlePreviewConflict>,
    /// The MAINTAINER key id(s) that sealed the bundle's own exported RefState (§4m.3 answer #3).
    /// **Reported, not trusted** -- this is recorded signer identity, the same "continuity only,
    /// not a trust decision" disclaimer `bundle import`/`bundle verify` already print, matching
    /// their own care per the handoff's own instruction. Never independently verified against a
    /// trust policy here; `verify` after import is what decides trust.
    pub sealed_by: Vec<String>,
    /// Node-granularity effects, sorted by path. Empty when connectivity is `DoesNotConnect`,
    /// `AlreadyIncluded`, or the conflict answer is `Conflict`/`Undetermined` (a replay that failed
    /// or was never attempted has no well-defined "after" state to diff).
    pub effects: Vec<BundlePreviewEffect>,
    /// This bundle's own self-describing manifest, if it carries one -- same semantics as
    /// [`BundleVerifyReport::manifest`].
    pub manifest: Option<BundleManifest>,
}

/// Preview what a bundle would do to `ref_name` in this repository, **writing nothing** (RFC 144
/// §4m.2's own hard requirement -- no objects, no refs, no received pointer, no trust state, no
/// cache write; proven by a control, `bundle_preview_writes_nothing`, not by this doc comment).
///
/// **The reader, and why it is safe not to write the bundle's objects first** (§4m.3's
/// implementation crux): the bundle is self-contained, so its own objects are readable without
/// being written, via `BundleAndLocalReader` (crate-private) -- the repository's real object store overlaid with
/// the bundle's own object set held in memory. Every read this function performs, and every read
/// the replay machinery it calls performs, goes through that composed reader or a bundle-only view
/// of it; nothing here ever opens an [`ObjectWriteSession`] or any other write path. **This is
/// deliberately not "import to a temporary directory, then delete"**: that would write, could
/// leave residue on a crash, and would make the write-nothing control a statement about cleanup
/// rather than about behaviour (the handoff's own instruction).
///
/// **No incremental lifecycle cache is touched.** That cache is a best-effort, persisted
/// optimization real replay paths use and write to disk; this function never calls into it
/// (`patch_replay::preview`'s own `walk_and_replay` is a plain in-memory fold, the same primitive
/// `patch_replay`'s own read paths use before any cache lookup) -- see this function's own control
/// for the perturbation that proves it.
pub fn preview_bundle(
    layout: &RepositoryLayout,
    source: &mut BundleSource,
    options: &BundleImportOptions,
    ref_name: &str,
) -> Result<BundlePreviewReport> {
    Ok(preview_bundle_reporting_anchor(layout, source, options, ref_name)?.0)
}

/// [`preview_bundle`], plus every snapshot the preview's chain walks could not anchor at (RFC 136
/// §10.3b.4). The report is unchanged either way; a caller prints each fallback on stderr.
pub fn preview_bundle_reporting_anchor(
    layout: &RepositoryLayout,
    source: &mut BundleSource,
    options: &BundleImportOptions,
    ref_name: &str,
) -> Result<(
    BundlePreviewReport,
    Vec<crate::patch_replay::SnapshotAnchorFallback>,
)> {
    let read_snapshot = ObjectReadSnapshot::open(layout)?;
    let metadata = decode_bundle_closure_metadata(source, options)?;
    // No lock: this never writes, so there is nothing to protect. `local: Some(&read_snapshot)`
    // -- a referenced object may be satisfied by the bundle or the repository's own existing
    // objects (D7's incremental-import rule). `validate_bundle_closure` already resolves and
    // present-checks the bundle's own exported-ref tip as part of item 1, so `bundle_target` is
    // returned here rather than re-derived with a second `resolve_ref_tip_block` call.
    let (ref_state_id, _bundle_key_ids, bundle_target) = validate_bundle_closure(
        source,
        &metadata.origin_ref_name,
        &metadata.prepared,
        &metadata.index_by_id,
        &metadata.membership,
        &metadata.closure_by_id,
        Some(&read_snapshot),
        &metadata.author_keys,
        metadata.manifest.as_ref(),
        metadata.object_count,
    )?;

    // 0.55.0 step 3 Part 3b-iv: one shared seekable source and one shared decode cache behind
    // both readers built from it below -- a read through either reuses a decode the other
    // already paid for, and `source` is borrowed, never duplicated (there is only one real
    // handle to seek).
    let source_cell: RefCell<&mut BundleSource> = RefCell::new(source);
    let cache: RefCell<BTreeMap<ObjectId, ObjectEnvelope>> = RefCell::new(BTreeMap::new());
    let bundle_only_reader = BundleAndLocalReader {
        source: &source_cell,
        prepared: &metadata.prepared,
        index_by_id: &metadata.index_by_id,
        cache: &cache,
        local: None,
    };

    // 0.55.0 step 3 Part 3b-iv: the exported ref's own RefState envelope, read on demand through
    // the same reader everything else here uses, rather than kept in a full map.
    let sealed_by: Vec<String> = bundle_only_reader
        .read_object(ref_state_id)?
        .into_iter()
        .flat_map(|envelope| envelope.signatures.into_iter())
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
        .map(|signature| signature.key_id)
        .collect();

    let local_ref_state_id = RefStore::new(layout.clone()).read_current_ref_state_id(ref_name)?;

    // §4n.1: a local ref with no published RefState at all -- not merely an empty one -- has no
    // target to compare against, so this is answered without ever attempting
    // `preview_impact`'s own ancestor walk. Reachable and common (the tutorial's own sequence is
    // `init` -> `commit` -> `seal`, so nothing is published until the first seal); it is a
    // legitimate connectivity state (§4m.3 rule 1's own family), not an error.
    let mut anchor_fallbacks = Vec::new();
    let preview = match local_ref_state_id {
        None => preview::preview_new_repository_impact(
            &bundle_only_reader,
            bundle_target,
            &mut anchor_fallbacks,
        )?,
        Some(local_ref_state_id) => {
            let local_ref_state_envelope = read_snapshot
                .read_typed(local_ref_state_id, ObjectType::RefState)?
                .ok_or_else(|| {
                    PrikkError::Integrity(format!(
                        "ref {ref_name} points to missing RefState {local_ref_state_id}"
                    ))
                })?;
            let local_ref_state_payload = RefStatePayload::decode_canonical(
                &local_ref_state_envelope.canonical_payload,
                local_ref_state_envelope.schema_version,
            )?;
            let (local_target, _) =
                crate::refs::resolve_ref_tip_block(&read_snapshot, &local_ref_state_payload)?;
            let combined_reader = BundleAndLocalReader {
                source: &source_cell,
                prepared: &metadata.prepared,
                index_by_id: &metadata.index_by_id,
                cache: &cache,
                local: Some(&read_snapshot),
            };
            preview::preview_impact(
                &combined_reader,
                &bundle_only_reader,
                local_target,
                bundle_target,
                &mut anchor_fallbacks,
            )?
        }
    };

    let connectivity = match preview.connectivity {
        preview::BundleConnectivity::NoLocalHistory => BundlePreviewConnectivity::NoLocalHistory,
        preview::BundleConnectivity::DoesNotConnect => BundlePreviewConnectivity::DoesNotConnect,
        preview::BundleConnectivity::AlreadyIncluded => BundlePreviewConnectivity::AlreadyIncluded,
        preview::BundleConnectivity::FastForward => BundlePreviewConnectivity::FastForward,
        preview::BundleConnectivity::Diverged => BundlePreviewConnectivity::Diverged,
    };
    let conflict = preview.conflict.map(|answer| match answer {
        preview::ConflictAnswer::AppliesCleanly => BundlePreviewConflict::AppliesCleanly,
        preview::ConflictAnswer::Conflict { detail } => BundlePreviewConflict::Conflict { detail },
        preview::ConflictAnswer::Undetermined { reason } => {
            BundlePreviewConflict::Undetermined { reason }
        }
    });
    let effects = preview
        .effects
        .into_iter()
        .map(|effect| BundlePreviewEffect {
            path: effect.path,
            kind: match effect.kind {
                preview::BundleImpactEffectKind::Created => BundlePreviewEffectKind::Created,
                preview::BundleImpactEffectKind::Deleted => BundlePreviewEffectKind::Deleted,
                preview::BundleImpactEffectKind::Edited => BundlePreviewEffectKind::Edited,
                preview::BundleImpactEffectKind::PermissionChanged => {
                    BundlePreviewEffectKind::PermissionChanged
                }
            },
            current_bytes: effect.current_bytes,
            after_bytes: effect.after_bytes,
        })
        .collect();

    Ok((
        BundlePreviewReport {
            bundle_ref_name: metadata.origin_ref_name,
            local_ref_name: ref_name.to_string(),
            connectivity,
            conflict,
            sealed_by,
            effects,
            manifest: metadata.manifest.map(|raw| BundleManifest {
                repository_format: raw.repository_format,
                tool_version: raw.tool_version,
                scope: raw.scope,
            }),
        },
        anchor_fallbacks,
    ))
}

/// The index and own encoded length of the first object in `objects` whose own encoded frame
/// exceeds `bound`, or `None` if every one is within it. Returns the length alongside the index so
/// the caller never needs to re-index `objects` (and re-encode) just to report it. Parameterized on
/// `bound` (rather than reading `DEFAULT_BUNDLE_MAX_OBJECT_BYTES` directly) so a test can exercise
/// this against a small bound without building a real over-256-MiB object. `export_bundle`'s own
/// call site always passes the real default.
fn first_over_bound_object_index(
    objects: &[ObjectEnvelope],
    bound: u64,
) -> Result<Option<(usize, u64)>> {
    for (index, envelope) in objects.iter().enumerate() {
        let encoded_len = len_to_u64(encode_envelope_file(envelope)?.len())?;
        if encoded_len > bound {
            return Ok(Some((index, encoded_len)));
        }
    }
    Ok(None)
}

/// 0.52.0 step 1 Part A round 2 (A-2): the real per-object bound, except on this thread when a test
/// has armed [`lower_bundle_object_over_bound_threshold_for_test`] -- the same end-to-end
/// "nothing written" test seam `archive.rs`'s own `object_over_bound_threshold` uses, so a test can
/// exercise `export_bundle`'s own real refusal path without a real 257 MiB object.
fn bundle_object_over_bound_threshold() -> u64 {
    #[cfg(feature = "test-support")]
    {
        if let Some(bound) = BUNDLE_OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST.with(|cell| *cell.borrow())
        {
            return bound;
        }
    }
    DEFAULT_BUNDLE_MAX_OBJECT_BYTES as u64
}

#[cfg(feature = "test-support")]
std::thread_local! {
    static BUNDLE_OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST: std::cell::RefCell<Option<u64>> =
        const { std::cell::RefCell::new(None) };
}

/// Arm a lowered per-object bound for `export_bundle`'s own refusal, on this thread only, until
/// [`clear_bundle_object_over_bound_threshold_for_test`] runs. Re-exported at the crate root under
/// `feature = "test-support"` so `prikk-cli`'s own tests can call it.
#[cfg(feature = "test-support")]
pub fn lower_bundle_object_over_bound_threshold_for_test(bound: u64) {
    BUNDLE_OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST.with(|cell| *cell.borrow_mut() = Some(bound));
}

/// Disarm whatever [`lower_bundle_object_over_bound_threshold_for_test`] last armed on this thread.
#[cfg(feature = "test-support")]
pub fn clear_bundle_object_over_bound_threshold_for_test() {
    BUNDLE_OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST.with(|cell| *cell.borrow_mut() = None);
}

/// Export a genesis-complete, verifiable subset of objects for `ref_name` (DC-78 §D4/§D6). Walks the
/// full Block ancestor closure (all parents, not mainline-only — ruling 2's "genesis-complete") plus
/// every Patch and Blob those blocks reference, plus the exported RefState's own required
/// attestations. Returns the report and the encoded bundle bytes; writing them to a file is a CLI
/// concern, not this crate's.
pub fn export_bundle(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(BundleExportReport, Vec<u8>)> {
    let ref_store = RefStore::new(layout.clone());
    // RFC 111 §6.1: export is read-only end to end (never calls `write_object`), so it takes one
    // decoded index snapshot here instead of paying a fresh decode per `read_required` call below --
    // and there can be many, one per object in the whole exported closure.
    let object_store = ObjectReadSnapshot::open(layout)?;
    crate::ref_resolution::require_existing_ref(
        layout,
        ref_name,
        crate::ref_resolution::ReceivedRefs::Refused,
    )?;
    let Some(ref_state_id) = ref_store.read_current_ref_state_id(ref_name)? else {
        return Err(PrikkError::Integrity(format!(
            "ref {ref_name} does not exist, nothing to export"
        )));
    };
    let ref_state_envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing RefState object: {ref_state_id}")))?;
    let ref_state_payload = RefStatePayload::decode_canonical(
        &ref_state_envelope.canonical_payload,
        ref_state_envelope.schema_version,
    )?;
    let mut tag_envelopes: Vec<ObjectEnvelope> = Vec::new();
    let tip_block_id =
        resolve_ref_target_block(&object_store, &ref_state_payload, &mut tag_envelopes)?;

    // Genesis-complete (ruling 2) covers the publication chain too, not only the Block DAG: a
    // received ref's `log --ref` walks `previous_ref_state_id` exactly as a local ref's does, so
    // every earlier RefState this ref ever published is required, not only the tip.
    let mut ref_state_chain: Vec<ObjectEnvelope> = vec![ref_state_envelope];
    let mut required_attestation_ids: BTreeSet<ObjectId> = ref_state_payload
        .required_attestation_ids
        .iter()
        .copied()
        .collect();
    let mut ancestors = crate::merge::evidence::ancestors_inclusive(&object_store, tip_block_id)?;
    let mut previous = ref_state_payload.previous_ref_state_id;
    let mut seen_ref_states: BTreeSet<ObjectId> = BTreeSet::from([ref_state_id]);
    while let Some(previous_id) = previous {
        if !seen_ref_states.insert(previous_id) {
            return Err(PrikkError::Integrity(format!(
                "RefState chain for {ref_name} contains a cycle at {previous_id}"
            )));
        }
        let envelope = read_required(&object_store, previous_id, ObjectType::RefState)?;
        let payload = RefStatePayload::decode_canonical(
            &envelope.canonical_payload,
            envelope.schema_version,
        )?;
        required_attestation_ids.extend(payload.required_attestation_ids.iter().copied());
        let target_block_id =
            resolve_ref_target_block(&object_store, &payload, &mut tag_envelopes)?;
        ancestors.extend(crate::merge::evidence::ancestors_inclusive(
            &object_store,
            target_block_id,
        )?);
        previous = payload.previous_ref_state_id;
        ref_state_chain.push(envelope);
    }

    let mut patch_ids: BTreeSet<ObjectId> = BTreeSet::new();
    let mut blob_ids: BTreeSet<ObjectId> = BTreeSet::new();
    for (block_id, payload) in &ancestors {
        patch_ids.extend(payload.patch_ids.iter().copied());
        // RFC 136 increment 1b: a checkpoint's manifest names content Blobs no patch names (an
        // edited file's text, stored at the checkpoint). The manifest is validated first, so a
        // damaged one is refused for what it is before any id it names is trusted.
        if let Some(manifest) =
            crate::snapshot::validate_snapshot_manifest(&object_store, *block_id, payload)?
        {
            blob_ids.extend(payload.snapshot_blob_ref);
            blob_ids.extend(
                manifest
                    .entries
                    .iter()
                    .filter_map(|entry| match &entry.content {
                        crate::state_root::StateRootContent::Blob(blob_id) => Some(*blob_id),
                        crate::state_root::StateRootContent::Symlink(_) => None,
                    }),
            );
        }
    }

    let mut objects: Vec<ObjectEnvelope> = ref_state_chain;
    objects.append(&mut tag_envelopes);
    for block_id in ancestors.keys() {
        objects.push(read_required(&object_store, *block_id, ObjectType::Block)?);
    }
    let mut patch_envelopes: Vec<ObjectEnvelope> = Vec::with_capacity(patch_ids.len());
    for patch_id in &patch_ids {
        let envelope = read_required(&object_store, *patch_id, ObjectType::Patch)?;
        // A Patch's operations can themselves reference Blobs (CreateFile, file-kind DeleteNode,
        // ReplaceBinary) independently of any Block's snapshot_blob_ref — a repository-local
        // verify never notices a missing one of these, since it never replays lifecycle state for
        // an export; the receiver's ordinary verify does, so every one of these must travel too.
        for operation in crate::patch_replay::decode::decode_patch_operations(
            &envelope.canonical_payload,
            envelope.schema_version,
        )? {
            match operation.kind {
                crate::patch_replay::decode::DecodedOperationKind::CreateFile {
                    blob_id, ..
                } => {
                    blob_ids.insert(blob_id);
                }
                crate::patch_replay::decode::DecodedOperationKind::ReplaceBinary {
                    old_blob_id,
                    new_blob_id,
                    ..
                } => {
                    blob_ids.insert(old_blob_id);
                    blob_ids.insert(new_blob_id);
                }
                crate::patch_replay::decode::DecodedOperationKind::DeleteNode {
                    preimage:
                        crate::patch_replay::decode::DecodedDeletePreimage::File { old_blob_id, .. },
                    ..
                } => {
                    blob_ids.insert(old_blob_id);
                }
                _ => {}
            }
        }
        patch_envelopes.push(envelope);
    }
    objects.extend(patch_envelopes);
    // DC-78 v2: a deletion's preimage Blob is required by every released prikk, but a text file whose
    // content only ever arrived as `EditText` spans has none stored (DC-65). Derive those by replay and
    // carry them; export writes nothing locally. Anything still absent fails exactly as before.
    // `has_object` answers from the index; `read_typed` would read every Blob's bytes just to ask
    // whether it exists (review v2, finding 3).
    let mut missing: BTreeSet<ObjectId> = BTreeSet::new();
    for blob_id in &blob_ids {
        if !object_store.has_object(*blob_id, ObjectType::Blob)? {
            missing.insert(*blob_id);
        }
    }
    let mut derived =
        crate::patch_replay::derive_deleted_content(&object_store, tip_block_id, &missing)?;
    for blob_id in &blob_ids {
        match derived.found.remove(blob_id) {
            Some((node_kind, bytes)) => {
                objects.push(crate::blob_access::blob_envelope_for_kind(
                    bytes, node_kind,
                )?);
            }
            None => objects.push(read_required_blob(
                &object_store,
                *blob_id,
                derived.unsupported_chain.as_deref(),
            )?),
        }
    }
    for attestation_id in &required_attestation_ids {
        objects.push(read_required(
            &object_store,
            *attestation_id,
            ObjectType::Attestation,
        )?);
    }

    // DC-53 Stage 2, D6: the author-key section's scope is exactly the AUTHOR key_ids of the
    // Patches this bundle carries -- derived from `objects` itself, never the whole local
    // `author_key_index` container (which would leak every author this repository has ever seen).
    let mut author_key_ids: BTreeSet<String> = BTreeSet::new();
    for envelope in &objects {
        if envelope.object_type != ObjectType::Patch {
            continue;
        }
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
            // No local material for this key_id -- omitted from the section, not an error (§3's
            // "material is optional per-author", vector 7).
            continue;
        };
        if distinct.any(|public_key| public_key != first) {
            // A legacy local conflict for a key_id this export would otherwise carry. Only possible
            // from a repository predating Stage 2's own rejection of new conflicts (Step 1's
            // migration scan found none in this project's own fixtures). Fail rather than silently
            // pick one: presenting the receiver with an arbitrarily-chosen key would look like a
            // provenance claim this sender's own repository does not actually make.
            return Err(PrikkError::Integrity(format!(
                "author key_id {key_id} has more than one distinct recorded public key locally; \
                 refusing to export a provenance claim this repository's own material does not \
                 agree on -- run doctor, though no repair exists for this container"
            )));
        }
        author_keys.push(AuthorKeyEntry {
            key_id: key_id.clone(),
            public_key: first,
        });
    }

    // 0.52.0 step 1 Part A, item 1 (external review 023): refuse before encoding anything if any
    // object this bundle would carry is itself over the same per-object bound `decode_bundle`
    // enforces on import (`DEFAULT_BUNDLE_MAX_OBJECT_BYTES`) -- otherwise export succeeds today and
    // only that later read refuses, after the user already has a file they believe is a backup.
    let bound = bundle_object_over_bound_threshold();
    if let Some((_index, declared_bytes)) = first_over_bound_object_index(&objects, bound)? {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes,
            bound_bytes: bound,
        });
    }

    let object_count = objects.len();
    let author_key_count = author_keys.len();
    let manifest = BundleManifest {
        repository_format: repository_format_number(layout.format()),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        scope: BundleScope::SingleRef,
    };
    let bytes = encode_bundle(ref_name, &objects, &author_keys, &manifest)?;
    Ok((
        BundleExportReport {
            ref_name: ref_name.to_string(),
            tip_block_id,
            object_count,
            author_key_count,
            manifest,
        },
        bytes,
    ))
}

/// 0.55.0 step 3 Part 3b-i2 (Q2): what `bundle import` reads from -- an open regular file's own
/// handle, seekable in place, or (for a non-regular input: a FIFO, a pipe) the existing fully
/// buffered fallback wrapped in a `Cursor` so both branches satisfy the identical `Read + Seek`
/// interface pass 1 and pass 2 both need. Built by `prikk-cli`'s `bounded_read` module (which
/// decides, from the open handle's own metadata, which variant to construct) -- `prikk-store` has
/// no notion of a CLI bound or a path, and only ever reads from whichever variant it is handed.
pub enum BundleSource {
    /// A regular file, already opened by the caller. Pass 1 still enforces the real per-object and
    /// per-import bounds against the bytes it actually reads, independently of any declared size
    /// (RFC 158 Stage A's own "metadata can lie" rule) -- this variant carries no promise beyond
    /// "seekable."
    File(std::fs::File),
    /// A non-regular input, already read fully into memory and bounded by the caller -- wrapped so
    /// it exposes the identical `Read + Seek` surface the file-backed variant does.
    Buffered(std::io::Cursor<Vec<u8>>),
}

impl Read for BundleSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.read(buf),
            Self::Buffered(cursor) => cursor.read(buf),
        }
    }
}

impl Seek for BundleSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(file) => file.seek(pos),
            Self::Buffered(cursor) => cursor.seek(pos),
        }
    }
}

fn u64_to_usize(value: u64) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| PrikkError::MalformedData("u64 length does not fit usize".to_string()))
}

fn stream_position_usize(source: &mut BundleSource) -> Result<usize> {
    let position = source.stream_position().map_err(|err| {
        PrikkError::Integrity(format!(
            "bundle import: failed to read the input's current position: {err}"
        ))
    })?;
    u64_to_usize(position)
}

fn read_exact_from(source: &mut BundleSource, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0_u8; len];
    source
        .read_exact(&mut buf)
        .map_err(|err| PrikkError::MalformedData(format!("unexpected end of bundle: {err}")))?;
    Ok(buf)
}

fn read_array_from<const N: usize>(source: &mut BundleSource) -> Result<[u8; N]> {
    let bytes = read_exact_from(source, N)?;
    let mut out = [0_u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn read_u64_from(source: &mut BundleSource) -> Result<u64> {
    Ok(u64::from_be_bytes(read_array_from::<8>(source)?))
}

/// A u64-length-prefixed byte string, read from the stream -- the streaming counterpart of
/// `ByteCursor::read_bytes_u64`, bounded by `max_len` (the bundle's own declared total size: no
/// legitimate ref name, author key id, or manifest string can exceed the bundle that carries it).
fn read_bytes_u64_from(source: &mut BundleSource, max_len: usize) -> Result<Vec<u8>> {
    let len = read_u64_from(source)?;
    if len > len_to_u64(max_len)? {
        return Err(PrikkError::MalformedData(format!(
            "bundle declares a {len}-byte field, over the bundle's own declared total size"
        )));
    }
    read_exact_from(source, u64_to_usize(len)?)
}

/// One object frame's declared length and body, read separately -- 0.55.0 step 3 Part 3b-ii2
/// (N1): pass 1 used to concatenate the 8-byte length prefix and the body into one combined
/// `frame` buffer solely so R3's digest could be computed over "the length-prefixed frame exactly
/// as read" in a single `sha256` call. `sha256_parts` already exists for exactly this split --
/// "a checksum covers a header and a body that are held apart" -- so this reads only `body`
/// (never a second, prefix-prepended copy of it), and the caller hashes the prefix and body as
/// two parts instead. Refuses on the length prefix, before the body is read, exactly as before.
fn read_frame_body_from(
    source: &mut BundleSource,
    max_object_bytes: usize,
) -> Result<(u64, Vec<u8>)> {
    let declared_bytes = read_u64_from(source)?;
    let bound_bytes = len_to_u64(max_object_bytes)?;
    if declared_bytes > bound_bytes {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes,
            bound_bytes,
        });
    }
    let body = read_exact_from(source, u64_to_usize(declared_bytes)?)?;
    Ok((declared_bytes, body))
}

/// 0.55.0 step 3 Part 3b-i2: the author-key section comes *after* every object frame in the wire
/// format (`encode_bundle`'s own section order), but RFC 156 §4 admission needs that section's own
/// transported material available *while* streaming the object frames, to verify a signature a
/// newly-carried key_id introduces. Skipped here without reading a single frame's body: each
/// frame's own length prefix says exactly how far to seek forward, so this walk costs `count` seeks
/// and transfers no object content -- landing exactly at the author-key section's own first byte.
fn skip_object_frames(
    source: &mut BundleSource,
    count: u64,
    max_object_bytes: usize,
) -> Result<()> {
    let bound_bytes = len_to_u64(max_object_bytes)?;
    for _ in 0..count {
        let declared_bytes = read_u64_from(source)?;
        if declared_bytes > bound_bytes {
            return Err(PrikkError::ObjectOverBound {
                declared_bytes,
                bound_bytes,
            });
        }
        let offset = i64::try_from(declared_bytes).map_err(|_| {
            PrikkError::MalformedData("u64 length does not fit a seek offset".to_string())
        })?;
        source.seek(SeekFrom::Current(offset)).map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to skip an object frame: {err}"
            ))
        })?;
    }
    Ok(())
}

/// 0.55.0 step 3 Part 3b-i2 (Q1): the handful of ids `validate_bundle_contents`'s own closure
/// checks used to read straight off a fully decoded [`ObjectEnvelope`] -- extracted once, during
/// the same streaming pass that decides admission, and never the envelope or its payload again.
/// Default (empty/`None`) for every field a given object type does not carry.
#[derive(Default)]
struct ClosureMetadata {
    /// RefState only.
    ref_name: Option<String>,
    /// RefState only.
    kind: Option<RefKind>,
    /// RefState only.
    target_object_id: Option<ObjectId>,
    /// RefState only.
    previous_ref_state_id: Option<ObjectId>,
    /// RefState only.
    required_attestation_ids: Vec<ObjectId>,
    /// Block only.
    patch_ids: Vec<ObjectId>,
    /// Block only.
    parent_block_ids: Vec<ObjectId>,
    /// Block only.
    snapshot_blob_ref: Option<ObjectId>,
    /// Patch only.
    referenced_blob_ids: Vec<ObjectId>,
    /// Attestation or Tag.
    target_block_id: Option<ObjectId>,
}

/// This envelope's own [`same_kind_dependencies`] plus its [`ClosureMetadata`], decoding each
/// type-specific payload at most once -- never the envelope itself past this call.
fn closure_metadata_for(envelope: &ObjectEnvelope) -> Result<(Vec<ObjectId>, ClosureMetadata)> {
    let mut closure = ClosureMetadata::default();
    let same_kind_dependencies = match envelope.object_type {
        ObjectType::Block => {
            let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
            let mut deps = payload.parent_block_ids.clone();
            deps.extend(payload.merge_baseline_block_id);
            closure.parent_block_ids = payload.parent_block_ids;
            closure.patch_ids = payload.patch_ids;
            closure.snapshot_blob_ref = payload.snapshot_blob_ref;
            deps
        }
        ObjectType::RefState => {
            let payload = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            let deps = payload.previous_ref_state_id.into_iter().collect();
            closure.ref_name = Some(payload.ref_name);
            closure.kind = Some(payload.kind);
            closure.target_object_id = Some(payload.target_object_id);
            closure.previous_ref_state_id = payload.previous_ref_state_id;
            closure.required_attestation_ids = payload.required_attestation_ids;
            deps
        }
        ObjectType::Patch => {
            let mut referenced = Vec::new();
            for operation in
                decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?
            {
                referenced.extend(bundle_referenced_blob_ids(&operation.kind));
            }
            closure.referenced_blob_ids = referenced;
            Vec::new()
        }
        ObjectType::Attestation => {
            let payload = AttestationPayload::decode_canonical(&envelope.canonical_payload)?;
            closure.target_block_id = Some(payload.target_block_id);
            Vec::new()
        }
        ObjectType::Tag => {
            let payload = TagPayload::decode_canonical(&envelope.canonical_payload)?;
            closure.target_block_id = Some(payload.target_block_id);
            Vec::new()
        }
        ObjectType::Blob
        | ObjectType::RecognitionClaim
        | ObjectType::RefUpdate
        | ObjectType::RecoveryNote
        | ObjectType::BlockSummaryCache => Vec::new(),
    };
    Ok((same_kind_dependencies, closure))
}

/// 0.55.0 step 3 Part 3b-i2: whether a newly-decoded duplicate occurrence of an id already in
/// `prepared` is a byte-identical repeat (cheap: digests already match) or needs the first
/// occurrence re-read and decoded to compare full [`ObjectEnvelope`] equality, exactly as
/// `validate_bundle_contents` used to (its own "same id, different signature bytes" rule, restated
/// against a real file instead of an in-memory map). Restores the stream's position to where it was
/// when called, whichever branch it took.
fn prepared_matches_existing(
    source: &mut BundleSource,
    prepared: &[PreparedObject],
    existing_index: usize,
    new_digest: prikk_hash::Sha256Digest,
    new_envelope: &ObjectEnvelope,
) -> Result<bool> {
    let Some(existing) = prepared.get(existing_index) else {
        return Err(PrikkError::Integrity(
            "bundle import: a duplicate id's first occurrence is no longer in the prepared set"
                .to_string(),
        ));
    };
    if existing.frame_digest == new_digest {
        return Ok(true);
    }
    let resume_at = stream_position_usize(source)?;
    source
        .seek(SeekFrom::Start(existing.frame_range.start as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to re-read a duplicate object's first occurrence: {err}"
            ))
        })?;
    let existing_frame = read_exact_from(
        source,
        existing.frame_range.end - existing.frame_range.start,
    )?;
    source
        .seek(SeekFrom::Start(resume_at as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to resume reading after checking a duplicate object: {err}"
            ))
        })?;
    let Some(existing_body) = existing_frame.get(8..) else {
        return Err(PrikkError::Integrity(
            "bundle import: a prepared object's own frame is shorter than its length prefix"
                .to_string(),
        ));
    };
    let existing_envelope = decode_envelope_file(existing_body)?;
    Ok(&existing_envelope == new_envelope)
}

/// 0.55.0 step 3 Part 3b-i2: a carried snapshot blob's own manifest, read for the content-blob ids
/// it names (RFC 136 increment 1b) -- fetched on demand, by seeking back to that one blob's own
/// recorded frame range, never more than one frame's content at a time.
fn carried_snapshot_manifest_blob_ids(
    source: &mut BundleSource,
    prepared: &[PreparedObject],
    blob_index: usize,
) -> Result<Vec<ObjectId>> {
    let Some(blob) = prepared.get(blob_index) else {
        return Err(PrikkError::Integrity(
            "bundle import: a snapshot blob's own prepared entry is missing".to_string(),
        ));
    };
    let resume_at = stream_position_usize(source)?;
    source
        .seek(SeekFrom::Start(blob.frame_range.start as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to re-read a snapshot blob: {err}"
            ))
        })?;
    let frame = read_exact_from(source, blob.frame_range.end - blob.frame_range.start)?;
    source
        .seek(SeekFrom::Start(resume_at as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to resume reading after a snapshot blob: {err}"
            ))
        })?;
    let Some(frame_body) = frame.get(8..) else {
        return Err(PrikkError::Integrity(
            "bundle import: a prepared object's own frame is shorter than its length prefix"
                .to_string(),
        ));
    };
    let envelope = decode_envelope_file(frame_body)?;
    let manifest = crate::snapshot::SnapshotManifest::decode(
        &crate::blob_access::decode_snapshot_blob(&envelope.canonical_payload)?,
    )?;
    Ok(manifest
        .entries
        .iter()
        .filter_map(|entry| match &entry.content {
            crate::state_root::StateRootContent::Blob(blob_id) => Some(*blob_id),
            crate::state_root::StateRootContent::Symlink(_) => None,
        })
        .collect())
}

/// 0.55.0 step 3 Part 3b-iv: seek to `prepared[index]`'s own recorded frame range, read exactly
/// that many bytes, decode the envelope, then seek back to wherever `source` was positioned
/// before this call -- the same on-demand re-read shape `prepared_matches_existing`/
/// `carried_snapshot_manifest_blob_ids` above already use inline, factored out here as its own
/// function because `BundleAndLocalReader`'s own on-demand reader (below) needs the identical
/// seek-read-decode-and-resume sequence, by index rather than by a frame range it already has in
/// hand.
fn read_prepared_object_at(
    source: &mut BundleSource,
    prepared: &[PreparedObject],
    index: usize,
) -> Result<ObjectEnvelope> {
    let Some(object) = prepared.get(index) else {
        return Err(PrikkError::Integrity(
            "bundle decode: a prepared object's own entry is missing".to_string(),
        ));
    };
    let resume_at = stream_position_usize(source)?;
    source
        .seek(SeekFrom::Start(object.frame_range.start as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!("bundle decode: failed to re-read an object: {err}"))
        })?;
    let frame = read_exact_from(source, object.frame_range.end - object.frame_range.start)?;
    source
        .seek(SeekFrom::Start(resume_at as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle decode: failed to resume reading after an on-demand re-read: {err}"
            ))
        })?;
    let Some(frame_body) = frame.get(8..) else {
        return Err(PrikkError::Integrity(
            "bundle decode: a prepared object's own frame is shorter than its length prefix"
                .to_string(),
        ));
    };
    decode_envelope_file(frame_body)
}

/// 0.55.0 step 3 Part 3b-i2 (Q1): `validate_bundle_contents`'s own closure checks (items 1-4 plus
/// the manifest cross-check and the bundle-internal author-key consistency check), run against the
/// cheap per-object metadata the streaming walk collected instead of a full decoded envelope set --
/// the same checks, in the same order, with the same refusal wording, reusing `local`'s own real
/// reads for anything satisfied by the receiving repository rather than carried by this bundle
/// (D7's incremental-import rule, unchanged).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn validate_bundle_closure(
    source: &mut BundleSource,
    origin_ref_name: &str,
    prepared: &[PreparedObject],
    index_by_id: &BTreeMap<ObjectId, usize>,
    membership: &BTreeMap<ObjectId, ObjectType>,
    closure_by_id: &BTreeMap<ObjectId, ClosureMetadata>,
    local: Option<&ObjectReadSnapshot>,
    author_keys: &[AuthorKeyEntry],
    manifest: Option<&DecodedManifest>,
    object_count: u64,
) -> Result<(ObjectId, BTreeMap<String, [u8; 32]>, ObjectId)> {
    let Some(ref_state_object) = prepared.first() else {
        return Err(PrikkError::MalformedData(
            "bundle contains no objects".to_string(),
        ));
    };
    if ref_state_object.object_type != ObjectType::RefState {
        return Err(PrikkError::MalformedData(
            "bundle's first object must be the exported ref's RefState".to_string(),
        ));
    }
    let ref_state_id = ref_state_object.object_id;
    let Some(ref_state_closure) = closure_by_id.get(&ref_state_id) else {
        return Err(PrikkError::Integrity(
            "bundle import: the exported ref's own RefState has no recorded closure metadata"
                .to_string(),
        ));
    };

    // DC-53 Stage 2, D7/C2: the bundle's own author-key section must not disagree with itself.
    let mut bundle_key_ids: BTreeMap<String, [u8; 32]> = BTreeMap::new();
    for entry in author_keys {
        match bundle_key_ids.get(entry.key_id.as_str()) {
            Some(existing) if *existing != entry.public_key => {
                return Err(PrikkError::MalformedData(format!(
                    "bundle's author-key section carries two different public keys for key_id {} \
                     -- refusing the whole bundle",
                    entry.key_id
                )));
            }
            Some(_) => {}
            None => {
                bundle_key_ids.insert(entry.key_id.clone(), entry.public_key);
            }
        }
    }

    let local_available = local.is_some();
    let present = |object_type: ObjectType, id: ObjectId| -> bool {
        membership.get(&id) == Some(&object_type)
            || local.is_some_and(|snapshot| snapshot.contains_object(object_type, id))
    };
    let missing_clause = if local_available {
        "neither carried by this bundle nor already present in this repository -- refusing the \
         whole import, no partial write"
    } else {
        "not carried by this bundle -- refusing to verify; an offline check with no repository to \
         ask cannot know whether a receiver would already hold it"
    };

    // Item 1: the exported ref's target resolves (`ensure_ref_target_valid`'s own logic, against
    // this metadata instead of a full `ObjectReader`). 0.55.0 step 3 Part 3b-iv: the resolved
    // block is returned too -- `verify`/`preview` both need exactly this value (`resolve_ref_
    // tip_block`'s own result), and it is already present-checked by the time this function
    // returns `Ok`, so returning it here means neither caller re-derives it a second time.
    let tip_block_id = match ref_state_closure.kind {
        Some(RefKind::Branch) => {
            let target = ref_state_closure.target_object_id.ok_or_else(|| {
                PrikkError::Integrity(
                    "bundle import: RefState has no recorded target object id".to_string(),
                )
            })?;
            if !present(ObjectType::Block, target) {
                return Err(PrikkError::Integrity(format!(
                    "ref object {ref_state_id} targets missing block {target}"
                )));
            }
            target
        }
        Some(RefKind::Tag) => {
            let tag_id = ref_state_closure.target_object_id.ok_or_else(|| {
                PrikkError::Integrity(
                    "bundle import: RefState has no recorded target object id".to_string(),
                )
            })?;
            let target_block_id = match closure_by_id.get(&tag_id) {
                Some(tag_closure) if membership.get(&tag_id) == Some(&ObjectType::Tag) => {
                    tag_closure.target_block_id.ok_or_else(|| {
                        PrikkError::Integrity(format!(
                            "ref object {ref_state_id} targets tag {tag_id}, which has no \
                             recorded target block"
                        ))
                    })?
                }
                _ => {
                    let tag_envelope = local.and_then(|snapshot| {
                        snapshot.read_typed(tag_id, ObjectType::Tag).ok().flatten()
                    });
                    let Some(tag_envelope) = tag_envelope else {
                        return Err(PrikkError::Integrity(format!(
                            "ref object {ref_state_id} targets missing tag {tag_id}"
                        )));
                    };
                    TagPayload::decode_canonical(&tag_envelope.canonical_payload)?.target_block_id
                }
            };
            if !present(ObjectType::Block, target_block_id) {
                return Err(PrikkError::Integrity(format!(
                    "ref object {ref_state_id} targets missing block {target_block_id}"
                )));
            }
            target_block_id
        }
        None => {
            return Err(PrikkError::Integrity(
                "bundle import: RefState has no recorded kind".to_string(),
            ));
        }
    };

    // 0.49.0 step 5, round 2 item 1: the exported ref's own chain and the attestations each link
    // requires must be present, carried by this bundle or already here.
    let mut chain_seen: BTreeSet<ObjectId> = BTreeSet::new();
    let mut required_attestation_ids = ref_state_closure.required_attestation_ids.clone();
    let mut previous_ref_state_id = ref_state_closure.previous_ref_state_id;
    loop {
        for attestation_id in &required_attestation_ids {
            if !present(ObjectType::Attestation, *attestation_id) {
                return Err(PrikkError::Integrity(format!(
                    "RefState for {origin_ref_name} requires attestation {attestation_id}, which is {missing_clause}"
                )));
            }
            let target_block_id = match closure_by_id.get(attestation_id) {
                Some(attestation_closure)
                    if membership.get(attestation_id) == Some(&ObjectType::Attestation) =>
                {
                    attestation_closure.target_block_id.ok_or_else(|| {
                        PrikkError::Integrity(format!(
                            "attestation {attestation_id} has no recorded target block"
                        ))
                    })?
                }
                _ => {
                    let envelope = local
                        .and_then(|snapshot| snapshot.read_object(*attestation_id).ok().flatten())
                        .ok_or_else(|| {
                            PrikkError::Integrity(format!(
                                "RefState for {origin_ref_name} requires attestation {attestation_id}, which is {missing_clause}"
                            ))
                        })?;
                    AttestationPayload::decode_canonical(&envelope.canonical_payload)?
                        .target_block_id
                }
            };
            if !present(ObjectType::Block, target_block_id) {
                return Err(PrikkError::Integrity(format!(
                    "attestation {attestation_id} targets block {target_block_id}, which is {missing_clause}"
                )));
            }
        }
        let Some(previous) = previous_ref_state_id else {
            break;
        };
        if !chain_seen.insert(previous) {
            return Err(PrikkError::MalformedData(format!(
                "RefState chain for {origin_ref_name} contains a cycle at {previous}"
            )));
        }
        if !present(ObjectType::RefState, previous) {
            return Err(PrikkError::Integrity(format!(
                "RefState chain for {origin_ref_name} names previous RefState {previous}, which is {missing_clause}"
            )));
        }
        match closure_by_id.get(&previous) {
            Some(link_closure) if membership.get(&previous) == Some(&ObjectType::RefState) => {
                required_attestation_ids = link_closure.required_attestation_ids.clone();
                previous_ref_state_id = link_closure.previous_ref_state_id;
            }
            _ => break,
        }
    }

    // DC-44 increment 3, control 4: a manifest that disagrees with the payload is refused.
    if let Some(raw) = manifest {
        if raw.declared_ref_name != origin_ref_name {
            return Err(PrikkError::MalformedData(format!(
                "bundle's manifest declares ref {}, but the bundle's own header names {} -- \
                 refusing the whole bundle",
                raw.declared_ref_name, origin_ref_name
            )));
        }
        let ref_name = ref_state_closure.ref_name.as_deref().unwrap_or_default();
        if raw.declared_ref_name != ref_name {
            return Err(PrikkError::MalformedData(format!(
                "bundle's manifest declares ref {}, but the exported RefState's own signed ref \
                 name is {} -- refusing the whole bundle",
                raw.declared_ref_name, ref_name
            )));
        }
        if raw.declared_object_count != object_count {
            return Err(PrikkError::MalformedData(format!(
                "bundle's manifest declares {} objects, but the bundle actually carries {} -- \
                 refusing the whole bundle",
                raw.declared_object_count, object_count
            )));
        }
    }

    // Items 2 and 3: every blob a carried patch's operations reference, and every patch a carried
    // block names, must be present.
    for (id, closure) in closure_by_id {
        if membership.get(id) != Some(&ObjectType::Patch) {
            continue;
        }
        for blob_id in &closure.referenced_blob_ids {
            if !present(ObjectType::Blob, *blob_id) {
                return Err(PrikkError::Integrity(format!(
                    "patch {id} references blob {blob_id}, which is {missing_clause}"
                )));
            }
        }
    }
    for (id, closure) in closure_by_id {
        if membership.get(id) != Some(&ObjectType::Block) {
            continue;
        }
        let block_id = *id;
        for patch_id in &closure.patch_ids {
            if !present(ObjectType::Patch, *patch_id) {
                return Err(PrikkError::Integrity(format!(
                    "block {block_id} names patch {patch_id}, which is {missing_clause}"
                )));
            }
        }
        // Item 4: every parent a carried block names must be present too.
        for parent_block_id in &closure.parent_block_ids {
            if !present(ObjectType::Block, *parent_block_id) {
                return Err(PrikkError::Integrity(format!(
                    "block {block_id} names parent {parent_block_id}, which is {missing_clause}"
                )));
            }
        }
        if let Some(snapshot_blob_id) = closure.snapshot_blob_ref {
            if !present(ObjectType::Blob, snapshot_blob_id) {
                return Err(PrikkError::Integrity(format!(
                    "block {block_id} names snapshot blob {snapshot_blob_id}, which is \
                     {missing_clause}"
                )));
            }
            // RFC 136 increment 1b: a carried manifest names content blobs no patch names. Only
            // checked when the snapshot blob is itself carried -- a purely local one was checked
            // when it arrived, exactly as today.
            if membership.get(&snapshot_blob_id) == Some(&ObjectType::Blob) {
                if let Some(&blob_index) = index_by_id.get(&snapshot_blob_id) {
                    for content_blob_id in
                        carried_snapshot_manifest_blob_ids(source, prepared, blob_index)?
                    {
                        if !present(ObjectType::Blob, content_blob_id) {
                            return Err(PrikkError::Integrity(format!(
                                "block {block_id} snapshot blob {snapshot_blob_id} names content \
                                 blob {content_blob_id}, which is {missing_clause}"
                            )));
                        }
                    }
                }
            }
        }
    }

    Ok((ref_state_id, bundle_key_ids, tip_block_id))
}

/// `decode_bundle_closure_metadata`'s own return shape -- everything [`validate_bundle_closure`]
/// needs, for a caller that never writes and runs no admission. 0.55.0 step 3 Part 3b-iv.
struct BundleClosureMetadata {
    origin_ref_name: String,
    prepared: Vec<PreparedObject>,
    index_by_id: BTreeMap<ObjectId, usize>,
    membership: BTreeMap<ObjectId, ObjectType>,
    closure_by_id: BTreeMap<ObjectId, ClosureMetadata>,
    author_keys: Vec<AuthorKeyEntry>,
    manifest: Option<DecodedManifest>,
    object_count: u64,
}

/// 0.55.0 step 3 Part 3b-iv (P1): the lean sibling [`verify_bundle`]/[`preview_bundle_reporting_
/// anchor`] call instead of their own now-removed `validate_bundle_contents` -- the same header
/// parsing and per-object decode loop [`import_bundle`]'s own pass 1 runs (same order: decode,
/// then the duplicate-id short-circuit, then closure-metadata extraction, so a harmless exact-
/// duplicate frame costs exactly what it costs there, never twice), but with no admission and no
/// lock: neither caller here writes anything, so there is nothing an admission decision or a held
/// lock would protect. `import_bundle`'s own loop is deliberately **not** changed to call this --
/// it keeps interleaving admission inline, in the one pass its own already-measured memory
/// profile depends on; this function duplicates the *mechanical* per-object walk, not the
/// *closure rules* themselves (`validate_bundle_closure`, called by all three commands,
/// unchanged, and the actual drift risk this split exists to remove).
fn decode_bundle_closure_metadata(
    source: &mut BundleSource,
    options: &BundleImportOptions,
) -> Result<BundleClosureMetadata> {
    let total_len = source.seek(SeekFrom::End(0)).map_err(|err| {
        PrikkError::Integrity(format!("bundle decode: failed to measure the input: {err}"))
    })?;
    source.seek(SeekFrom::Start(0)).map_err(|err| {
        PrikkError::Integrity(format!("bundle decode: failed to rewind the input: {err}"))
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

    let objects_start = stream_position_usize(source)?;
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

    source
        .seek(SeekFrom::Start(objects_start as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle decode: failed to seek back to the object frames: {err}"
            ))
        })?;

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
                PrikkError::Integrity("bundle decode: frame range overflow".to_string())
            })?;
        let real_frame_digest = prikk_hash::sha256_parts(&[&declared_bytes.to_be_bytes(), &body]);
        let envelope = decode_envelope_file(&body)?;
        drop(body);
        let object_id = envelope.object_id();

        // Same ordering as `import_bundle`'s own pass 1 (0.55.0 step 3 Part 3b-i): a harmless,
        // byte-identical duplicate frame never pays `closure_metadata_for`'s own decode cost a
        // second time -- the short-circuit below runs before that call, just as it does there.
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

        let object_type = envelope.object_type;
        let (same_kind_dependencies, closure) = closure_metadata_for(&envelope)?;
        drop(envelope);

        let index = prepared.len();
        prepared.push(PreparedObject {
            object_id,
            object_type,
            same_kind_dependencies,
            frame_range: start..end,
            frame_digest: real_frame_digest,
            // Unused by this function's own callers (`validate_bundle_closure` never reads this
            // field, confirmed by reading its full body) -- neither `verify` nor `preview` admits
            // anything, so there is no real outcome to record.
            admission_outcome: AdmissionOutcomeKind::New,
        });
        index_by_id.insert(object_id, index);
        membership.insert(object_id, object_type);
        closure_by_id.insert(object_id, closure);
    }

    Ok(BundleClosureMetadata {
        origin_ref_name,
        prepared,
        index_by_id,
        membership,
        closure_by_id,
        author_keys,
        manifest,
        object_count: count,
    })
}

/// Import a bundle's objects and record a `received` pointer for its ref (DC-78 §D4). Never touches
/// `refs/by-id/`, never advances a local ref, and never adopts any MAINTAINER key into the local trust
/// policy — the imported RefState/Blocks remain ordinary, structurally-checkable objects that `verify`
/// will report as untrusted until the operator explicitly runs `trust maintainer add` for the key that
/// sealed them. That is a deliberate choice, not an oversight: auto-adopting a key seen in imported
/// history would be a new, unreviewed trust mechanism, exactly what §D6 rules out.
///
/// 0.55.0 step 3 Part 3b-i2 (D1/Q1/Q2): reads `source` forward exactly once for the real work (plus
/// one cheap seek-only skip-walk to reach the author-key section ahead of the object frames it
/// follows in the wire format, and pass 2's own seeks back by recorded range) -- never the whole
/// bundle materialized at once. `validate_bundle_contents`'s own structural and closure checks now
/// run inside this same locked streaming pass as admission (`validate_bundle_closure`), not
/// before it: a concurrent writer acting in the window before the lock is now seen by the *whole*
/// locked decision, not only the author-key-conflict check (Q1's own reasoning for why the two race
/// tests below keep their seam unchanged).
pub fn import_bundle(
    layout: &RepositoryLayout,
    source: &mut BundleSource,
    options: &BundleImportOptions,
) -> Result<BundleImportReport> {
    let read_snapshot = ObjectReadSnapshot::open(layout)?;

    // Header-only checks, before the lock (Q1): the declared total size and the format magic.
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

    // Gated like its only caller: `bundle`'s tests are Linux-only.
    #[cfg(test)]
    if let Some(change) = BEFORE_IMPORT_LOCK.with(|slot| slot.borrow_mut().take()) {
        change();
    }

    // **A refused import writes nothing.** Every check that can refuse now runs inside this one
    // locked streaming pass, and the locks are held from here through the last write:
    //
    // - `ActiveLock` serializes author-key recording against every other recorder (`commit`,
    //   `rollback-draft`, `sync accept`, another import).
    // - The received-index lock is taken here rather than just before the pointer write, so a
    //   concurrent compaction refuses this import up front instead of after its objects are written.
    // - `ObjectStore` joins this same acquisition, held across every object write below
    //   (`write_object_under_held_lock`, the shape `archive import` already uses).
    let active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    let _container_locks = acquire_container_locks(
        layout,
        &[
            LockableContainer::ReceivedIndex,
            LockableContainer::ObjectStore,
        ],
    )?;
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

    source
        .seek(SeekFrom::Start(objects_start as u64))
        .map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to seek back to the object frames: {err}"
            ))
        })?;

    let mut object_store = ObjectWriteSession::open(layout)?;
    let admitter = CarriedSignatureAdmitter::new(layout)?;
    let mut author_material_closure = |key_id: &str| -> Result<Vec<AuthorKeyEntry>> {
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

        let already_present = object_store.contains_object(envelope.object_type, object_id)?;
        let object_type = envelope.object_type;
        // 0.55.0 step 3 Part 3b-ii: extracted before `admit_one`, not after -- closure metadata
        // never depends on the admission result, and `admit_one` now takes `envelope` by value
        // (below), so this must read it first.
        let (same_kind_dependencies, closure) = closure_metadata_for(&envelope)?;
        // RFC 156 §4 rules 1-3, format 7: for every carried object this repository already holds,
        // which of its signatures may join the stored record. Verified here, before anything is
        // written; an invalid one refuses the whole import.
        let admitted = admitter.admit_one(&object_store, envelope, &mut author_material_closure)?;
        object_store.check_write(&admitted.envelope)?;
        let merged = admitted.merged;
        // 0.55.0 step 3 Part 3b-ii2 (N1): `admitted.envelope` (the decoded payload, same size as
        // `body` above) is never read again once `check_write` has validated it -- pass 1 keeps
        // only `merged` from here on. Dropped explicitly rather than left alive through the push
        // below, for the same reason as `body`'s own early drop.
        drop(admitted);
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
    let (ref_state_id, bundle_key_ids, _tip_block_id) = validate_bundle_closure(
        source,
        &origin_ref_name,
        &prepared,
        &index_by_id,
        &membership,
        &closure_by_id,
        Some(&read_snapshot),
        &author_keys,
        manifest.as_ref(),
        count,
    )?;

    // RFC 163 §2, Addendum 1 item 1 / the author-key conflict check: both still run before the
    // first object write, under the same lock the pointer write later runs under.
    for (key_id, public_key) in &bundle_key_ids {
        check_author_key_conflict(layout, key_id, *public_key)?;
    }

    // `BundleImportReport` carries no manifest field (unlike `BundleVerifyReport`/
    // `BundlePreviewReport`) -- `manifest` above was consulted only for the cross-check and is
    // dropped here, same as the pre-streaming code's own `contents.manifest` was for this caller.

    // 0.49.0 step 0, finding 3 / 0.55.0 step 3 Part 3b-i (R1): dependency order, computed purely
    // over `prepared`'s own cheap `(id, type, dependency ids)` metadata.
    let write_order = dependency_order_indices(&prepared)?;

    #[cfg(test)]
    if let Some(change) = CORRUPT_FILE_BETWEEN_PASSES_FOR_TEST.with(|slot| slot.borrow_mut().take())
    {
        change();
    }

    // Past this point only I/O can stop the import -- never a decision about the bundle's content,
    // and never a concurrent writer: `_container_locks` already holds `ObjectStore` for the whole
    // loop, so `write_object_under_held_lock` only re-checks that the lock is still held, rather
    // than acquiring and releasing it once per object.
    let mut written_object_count = 0_usize;
    let mut merged_object_count = 0_usize;
    let mut dropped_signatures: Vec<crate::DroppedSignature> = Vec::new();
    #[cfg(feature = "test-support")]
    let mut test_support_objects_written_so_far = 0_usize;
    for index in write_order {
        let Some(prepared_object) = prepared.get(index) else {
            return Err(PrikkError::Integrity(
                "bundle import: dependency ordering produced an index outside the prepared set"
                    .to_string(),
            ));
        };
        check_simulated_pass_two_read_failure_for_test(
            prepared_object.object_id,
            written_object_count,
            object_count,
        )?;
        let read_failure_message = || {
            format!(
                "bundle import: object {} could not be re-read for its own pass-2 check -- \
                 {written_object_count} of {object_count} objects were already written, each \
                 validated and admitted before being written; no received pointer was written; \
                 re-running the import once the file is stable will complete it",
                prepared_object.object_id
            )
        };
        if let Err(_seek_err) =
            source.seek(SeekFrom::Start(prepared_object.frame_range.start as u64))
        {
            return Err(PrikkError::Integrity(read_failure_message()));
        }
        let frame_len = prepared_object.frame_range.end - prepared_object.frame_range.start;
        let frame = match read_exact_from(source, frame_len) {
            Ok(frame) => frame,
            Err(_read_err) => return Err(PrikkError::Integrity(read_failure_message())),
        };
        // R3: the per-frame digest, recomputed from the same bytes pass 1 read and re-verified
        // before this object is admitted or written again. K2: this is not the manifest digest
        // `bundle.rs:209`-`215` declined to add -- that field would duplicate the check that an
        // object's id is already a content hash of its own bytes. This digest is a private,
        // in-memory record kept only between this function's own two passes, covering the whole
        // frame including its signatures, which the object id does not cover.
        if prikk_hash::sha256(&frame) != prepared_object.frame_digest {
            return Err(PrikkError::Integrity(format!(
                "bundle import: object {} changed between validation and writing -- \
                 {written_object_count} of {object_count} objects were already written, each \
                 validated and admitted before being written; no received pointer was written; \
                 re-running the import once the file is stable will complete it",
                prepared_object.object_id
            )));
        }
        let Some(frame_body) = frame.get(8..) else {
            return Err(PrikkError::Integrity(
                "bundle import: a prepared object's own frame is shorter than its length prefix"
                    .to_string(),
            ));
        };
        let envelope = decode_envelope_file(frame_body)?;
        let already_present =
            object_store.contains_object(prepared_object.object_type, prepared_object.object_id)?;
        let admitted = admitter.admit_one(&object_store, envelope, &mut author_material_closure)?;
        let recomputed_outcome = if !already_present {
            AdmissionOutcomeKind::New
        } else if admitted.merged {
            AdmissionOutcomeKind::Merged
        } else {
            AdmissionOutcomeKind::AlreadyPresent
        };
        // R4's own assertion: the store cannot change while this lock is held, so pass 2 must
        // reach the identical outcome pass 1 recorded. A mismatch here is an internal error,
        // before this object's write -- never a legitimate result.
        if recomputed_outcome != prepared_object.admission_outcome {
            return Err(PrikkError::Integrity(format!(
                "bundle import: object {} admitted differently on its second pass ({recomputed_outcome:?}) \
                 than its first ({:?}) -- the store must not change while the import's own lock is \
                 held; refusing before this object's write",
                prepared_object.object_id, prepared_object.admission_outcome
            )));
        }
        if admitted.merged {
            merged_object_count = merged_object_count
                .checked_add(1)
                .ok_or_else(|| PrikkError::Integrity("merged-object count overflow".to_string()))?;
        }
        dropped_signatures.extend(admitted.dropped);
        if !already_present {
            written_object_count = written_object_count.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("bundle import written-object count overflow".to_string())
            })?;
        }
        // 0.55.0 step 3 Part 3b-ii: for a `New` object, `admitted.envelope` is `frame_body`
        // decoded and handed back unchanged (`admit_one`'s own new-object branch) -- its canonical
        // encoding is provably `frame_body` itself (R3's digest, verified above, is exactly over
        // these bytes), so the hint lets the write skip re-encoding what it already has. A `Merge`
        // genuinely changes the content, so it always re-encodes (the write path only honors the
        // hint when its own fresh decision confirms `New`, never trusting the caller's label
        // alone).
        let known_body = (recomputed_outcome == AdmissionOutcomeKind::New).then_some(frame_body);
        object_store.write_object_under_held_lock_with_known_body(
            &admitted.envelope,
            &_container_locks,
            known_body,
        )?;
        #[cfg(feature = "test-support")]
        {
            test_support_objects_written_so_far =
                test_support_objects_written_so_far.saturating_add(1);
            block_after_n_objects_for_real_kill_test(test_support_objects_written_so_far);
        }
    }

    #[cfg(test)]
    if let Some(change) = DURING_IMPORT_WRITES.with(|slot| slot.borrow_mut().take()) {
        change();
    }

    // DC-53 Stage 2, D7: import records material, `verify` decides. Every entry was checked above,
    // under this same lock, so recording cannot meet a conflict here.
    let mut recorded_author_key_count = 0_usize;
    for entry in &author_keys {
        record_author_key_material(layout, &entry.key_id, entry.public_key, &active_lock)?;
        recorded_author_key_count = recorded_author_key_count.checked_add(1).ok_or_else(|| {
            PrikkError::Integrity("bundle import recorded-author-key count overflow".to_string())
        })?;
    }

    crate::received::write_received_pointer(layout, &received_ref_name, ref_state_id)?;

    Ok(BundleImportReport {
        ref_name: received_ref_name,
        ref_state_id,
        object_count,
        written_object_count,
        merged_object_count,
        dropped_signatures,
        recorded_author_key_count,
    })
}

/// Test-only convenience over [`import_bundle`]'s own `bytes: &[u8]`-shaped predecessor: wraps
/// `bytes` in a [`BundleSource::Buffered`] cursor and calls through. Every existing test built a
/// whole bundle's bytes in memory already (it is the thing under test, or a fixture built by
/// `export_bundle`, which still returns `Vec<u8>`) -- this keeps that construction unchanged across
/// the many existing call sites 0.55.0 step 3 Part 3b-i2's own signature change would otherwise
/// touch one at a time, with no production behavior difference from calling [`import_bundle`]
/// directly on a `BundleSource::Buffered` (the same path a real non-regular-file input takes).
#[cfg(test)]
pub(crate) fn import_bundle_from_bytes(
    layout: &RepositoryLayout,
    bytes: &[u8],
    options: &BundleImportOptions,
) -> Result<BundleImportReport> {
    let mut source = BundleSource::Buffered(std::io::Cursor::new(bytes.to_vec()));
    import_bundle(layout, &mut source, options)
}

/// Test-only convenience over [`verify_bundle`]'s own `bytes: &[u8]`-shaped predecessor -- 0.55.0
/// step 3 Part 3b-iii's own counterpart to [`import_bundle_from_bytes`], for the same reason: keep
/// every existing bytes-in-memory test fixture unchanged, with no production behavior difference
/// from calling [`verify_bundle`] directly on a `BundleSource::Buffered`.
#[cfg(test)]
pub(crate) fn verify_bundle_from_bytes(
    bytes: &[u8],
    options: &BundleImportOptions,
) -> Result<BundleVerifyReport> {
    let mut source = BundleSource::Buffered(std::io::Cursor::new(bytes.to_vec()));
    verify_bundle(&mut source, options)
}

/// Test-only convenience over [`preview_bundle`]'s own `bytes: &[u8]`-shaped predecessor -- see
/// [`verify_bundle_from_bytes`]'s own doc comment.
#[cfg(test)]
pub(crate) fn preview_bundle_from_bytes(
    layout: &RepositoryLayout,
    bytes: &[u8],
    options: &BundleImportOptions,
    ref_name: &str,
) -> Result<BundlePreviewReport> {
    let mut source = BundleSource::Buffered(std::io::Cursor::new(bytes.to_vec()));
    preview_bundle(layout, &mut source, options, ref_name)
}

/// Test-only convenience over [`preview_bundle_reporting_anchor`]'s own `bytes: &[u8]`-shaped
/// predecessor -- see [`verify_bundle_from_bytes`]'s own doc comment.
#[cfg(test)]
pub(crate) fn preview_bundle_reporting_anchor_from_bytes(
    layout: &RepositoryLayout,
    bytes: &[u8],
    options: &BundleImportOptions,
    ref_name: &str,
) -> Result<(
    BundlePreviewReport,
    Vec<crate::patch_replay::SnapshotAnchorFallback>,
)> {
    let mut source = BundleSource::Buffered(std::io::Cursor::new(bytes.to_vec()));
    preview_bundle_reporting_anchor(layout, &mut source, options, ref_name)
}

/// 0.55.0 step 3 Part 3a (R6): the one seam reachable from a real, separately compiled `prikk`
/// process, gated by the `test-support` feature (never the shipped binary) rather than
/// `#[cfg(test)]` (which only ever compiles into this crate's own unit-test binary) -- the same
/// shape `archive::import`'s own `block_after_n_objects_for_real_kill_test` already uses, for the
/// identical reason: a seam that returns `Err` lets `Drop` release every lock normally, and a real
/// crash does not.
///
/// A no-op unless `PRIKK_BUNDLE_IMPORT_BLOCK_AFTER_N_FOR_TEST` is set to `total_objects_written`'s
/// own value -- read fresh on every call, the same reasoning as the archive-import twin. When it
/// matches: writes a marker file at the path named by `PRIKK_BUNDLE_IMPORT_BLOCKED_MARKER_FOR_TEST`,
/// then blocks forever, so the harness's own real `SIGKILL` leaves real lock files on disk the way
/// an actual crash would.
#[cfg(feature = "test-support")]
fn block_after_n_objects_for_real_kill_test(total_objects_written: usize) {
    let Ok(block_after) = std::env::var("PRIKK_BUNDLE_IMPORT_BLOCK_AFTER_N_FOR_TEST") else {
        return;
    };
    let Ok(block_after) = block_after.parse::<usize>() else {
        return;
    };
    if block_after != total_objects_written {
        return;
    }
    if let Ok(marker_path) = std::env::var("PRIKK_BUNDLE_IMPORT_BLOCKED_MARKER_FOR_TEST") {
        let _ = std::fs::write(marker_path, b"blocked\n");
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// The object model's own acyclic reference order (0.49.0 step 0, finding 3): a `Block` names
/// `Patch`, `Blob` (its snapshot) and earlier `Block`s; `Attestation` and `Tag` name a `Block`;
/// `RefState` names a `Block`, an earlier `RefState`, and `Attestation`s. `RecognitionClaim` names a
/// `Block` and `Patch`es but is never existence-checked against them (its own module doc), so it
/// carries no ordering requirement -- ranked last anyway, for consistency, not correctness.
/// `Patch` and `Blob` name no other object type. Every writer that durably appends more than one
/// object in a single command must write them in this order (an object only after everything it
/// can reference is durable), so an interruption partway through never leaves a dangling forward
/// reference for `verify`'s object-connectivity stage to miss -- see
/// `objects_in_dependency_order`'s own callers (`import_bundle`, `patch_exchange::accept`) for what
/// this closes.
///
/// `pub(crate)`, not private: RFC 155 Part I-A's own `archive::import` needs the same per-type
/// order to process one object-container section at a time (never the whole archive's objects in
/// memory at once, unlike `objects_in_dependency_order`'s own single-`Vec` callers here) -- reusing
/// this ranking is what keeps that per-type order and this one from drifting apart.
pub(crate) fn object_type_dependency_rank(object_type: ObjectType) -> u8 {
    match object_type {
        ObjectType::Blob | ObjectType::Patch => 0,
        ObjectType::Block => 1,
        ObjectType::Attestation => 2,
        ObjectType::RefState => 3,
        ObjectType::Tag => 4,
        ObjectType::RecognitionClaim => 5,
        // Never carried as a bulk-written envelope in a bundle or exchange artifact (`RefUpdate` is
        // stored inline in the ref log; `RecoveryNote` inline in `refs/recovery/`;
        // `BlockSummaryCache` is a rebuildable cache) -- ranked last defensively, not because any
        // caller is known to reach this arm.
        ObjectType::RefUpdate | ObjectType::RecoveryNote | ObjectType::BlockSummaryCache => 6,
    }
}

/// This envelope's own same-kind dependencies among `envelopes` -- the ids, of the *same*
/// `ObjectType`, it must be written after. Only `Block` (parents, `mainline_parent_id` -- already a
/// member of `parent_block_ids`, so adding it again would be redundant, not wrong --, and
/// `merge_baseline_block_id`, which is not necessarily a parent) and `RefState`
/// (`previous_ref_state_id`) carry one; every other type's own same-kind fields (Addendum 1 review
/// §1: "any other same-kind reference the payload structs show") were checked and carry none.
fn same_kind_dependencies(envelope: &ObjectEnvelope) -> Result<Vec<ObjectId>> {
    match envelope.object_type {
        ObjectType::Block => {
            let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
            let mut deps = payload.parent_block_ids;
            deps.extend(payload.merge_baseline_block_id);
            Ok(deps)
        }
        ObjectType::RefState => {
            let payload = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            Ok(payload.previous_ref_state_id.into_iter().collect())
        }
        ObjectType::Blob
        | ObjectType::Patch
        | ObjectType::Attestation
        | ObjectType::Tag
        | ObjectType::RecognitionClaim
        | ObjectType::RefUpdate
        | ObjectType::RecoveryNote
        | ObjectType::BlockSummaryCache => Ok(Vec::new()),
    }
}

/// Orders one same-`ObjectType` run so a referenced id (per [`same_kind_dependencies`]) is always
/// written before whatever names it -- Kahn's algorithm, scoped to ids present in `group` only: an
/// id this batch does not carry is either already durable (nothing to order against here) or
/// missing (already refused earlier, above this function's only caller), never a cycle risk this
/// function introduces. Each pass keeps the earliest-ready envelope first, the same "trust the
/// caller's own order where the graph allows it" spirit the old stable sort documented.
fn same_kind_topological_order(group: Vec<ObjectEnvelope>) -> Result<Vec<ObjectEnvelope>> {
    let mut pending: Vec<(ObjectId, Vec<ObjectId>, ObjectEnvelope)> =
        Vec::with_capacity(group.len());
    for envelope in group {
        let id = envelope.object_id();
        let dependencies = same_kind_dependencies(&envelope)?;
        pending.push((id, dependencies, envelope));
    }
    let present: BTreeSet<ObjectId> = pending.iter().map(|(id, ..)| *id).collect();

    let mut written: BTreeSet<ObjectId> = BTreeSet::new();
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let before = pending.len();
        let mut still_pending = Vec::with_capacity(pending.len());
        for (id, dependencies, envelope) in pending {
            let ready = dependencies
                .iter()
                .all(|dependency| !present.contains(dependency) || written.contains(dependency));
            if ready {
                written.insert(id);
                ordered.push(envelope);
            } else {
                still_pending.push((id, dependencies, envelope));
            }
        }
        pending = still_pending;
        if pending.len() == before {
            // Cannot happen for real content -- parent/previous chains are acyclic by construction,
            // and `verify`'s own ancestry walk would already refuse a genuine cycle -- but refuse
            // rather than write an arbitrary order if a malformed bundle somehow produces one.
            return Err(PrikkError::Integrity(
                "bundle contents: a same-kind object reference forms a cycle among carried objects"
                    .to_string(),
            ));
        }
    }
    Ok(ordered)
}

/// Orders `envelopes` into the dependency order `object_type_dependency_rank` defines across kinds,
/// then, within each contiguous same-kind run that sort produces, topologically by
/// [`same_kind_topological_order`] -- Addendum 1 (review `stability-follow-up-review-v1`): the
/// original stable-only sort left same-kind order exactly as the caller supplied it, which is safe
/// for a chain of `Block`s already collected ancestor-first but not for a bundle whose own carried
/// order lists a child before its parent.
pub(crate) fn objects_in_dependency_order(
    envelopes: Vec<ObjectEnvelope>,
) -> Result<Vec<ObjectEnvelope>> {
    let mut envelopes = envelopes;
    envelopes.sort_by_key(|envelope| object_type_dependency_rank(envelope.object_type));

    let mut result = Vec::with_capacity(envelopes.len());
    let mut group: Vec<ObjectEnvelope> = Vec::new();
    for envelope in envelopes {
        if let Some(last) = group.last() {
            if last.object_type != envelope.object_type {
                result.extend(same_kind_topological_order(std::mem::take(&mut group))?);
            }
        }
        group.push(envelope);
    }
    if !group.is_empty() {
        result.extend(same_kind_topological_order(group)?);
    }
    Ok(result)
}

/// 0.55.0 step 3 Part 3b-i (R4): what pass 1 keeps for one carried object between `import_bundle`'s
/// own two passes -- never its decoded envelope, payload, or signatures. `frame_range` is where
/// pass 2 re-finds the object's own bytes in the original bundle bytes (length prefix included);
/// `frame_digest` (R3) and `admission_outcome` (R4) are what pass 2 must reproduce exactly, from
/// those bytes alone, before writing.
pub(crate) struct PreparedObject {
    object_id: ObjectId,
    object_type: ObjectType,
    same_kind_dependencies: Vec<ObjectId>,
    frame_range: Range<usize>,
    frame_digest: prikk_hash::Sha256Digest,
    admission_outcome: AdmissionOutcomeKind,
}

/// 0.55.0 step 3 Part 3b-i (R4): the admission outcome pass 1 decided for one object, which pass 2
/// must reach again, from the same unchanged lock-held store state, before writing it -- a
/// different outcome the second time is an internal error, not a legitimate result ("the store
/// cannot change while the lock is held," R4's own reasoning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionOutcomeKind {
    /// Not held locally before this import; stored as carried.
    New,
    /// Held locally already, and this bundle's own copy added no signature.
    AlreadyPresent,
    /// Held locally already, and this bundle's own copy added at least one signature (RFC 156
    /// format 7).
    Merged,
}

/// 0.55.0 step 3 Part 3b-i (R1): [`objects_in_dependency_order`]'s own two-stage algorithm --
/// cross-kind rank, then same-kind topological order -- as a thin wrapper over `prepared`'s own
/// cheap `(id, type, dependency ids)` metadata, returning a permutation of indices into `prepared`
/// rather than cloning or moving any envelope. The ranking and topological-sort logic themselves
/// are unchanged from [`object_type_dependency_rank`]/[`same_kind_topological_order`]; only the
/// carried payload (an index, not an envelope) differs.
fn dependency_order_indices(prepared: &[PreparedObject]) -> Result<Vec<usize>> {
    let mut by_rank: Vec<usize> = (0..prepared.len()).collect();
    by_rank.sort_by_key(|&index| {
        prepared.get(index).map_or(u8::MAX, |object| {
            object_type_dependency_rank(object.object_type)
        })
    });

    let mut result = Vec::with_capacity(prepared.len());
    let mut group: Vec<usize> = Vec::new();
    let mut group_type: Option<ObjectType> = None;
    for index in by_rank {
        let Some(object) = prepared.get(index) else {
            return Err(PrikkError::Integrity(
                "bundle import: dependency ordering produced an index outside the prepared set"
                    .to_string(),
            ));
        };
        if let Some(current_type) = group_type {
            if current_type != object.object_type {
                result.extend(same_kind_topological_order_indices(
                    prepared,
                    std::mem::take(&mut group),
                )?);
            }
        }
        group_type = Some(object.object_type);
        group.push(index);
    }
    if !group.is_empty() {
        result.extend(same_kind_topological_order_indices(prepared, group)?);
    }
    Ok(result)
}

/// [`same_kind_topological_order`]'s own Kahn's-algorithm body, over indices into `prepared`
/// instead of over owned envelopes -- see [`dependency_order_indices`].
fn same_kind_topological_order_indices(
    prepared: &[PreparedObject],
    group: Vec<usize>,
) -> Result<Vec<usize>> {
    let present: BTreeSet<ObjectId> = group
        .iter()
        .filter_map(|&index| prepared.get(index).map(|object| object.object_id))
        .collect();

    let mut pending = group;
    let mut written: BTreeSet<ObjectId> = BTreeSet::new();
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let before = pending.len();
        let mut still_pending = Vec::with_capacity(pending.len());
        for index in pending {
            let Some(object) = prepared.get(index) else {
                return Err(PrikkError::Integrity(
                    "bundle import: dependency ordering produced an index outside the prepared \
                     set"
                    .to_string(),
                ));
            };
            let ready = object
                .same_kind_dependencies
                .iter()
                .all(|dependency| !present.contains(dependency) || written.contains(dependency));
            if ready {
                written.insert(object.object_id);
                ordered.push(index);
            } else {
                still_pending.push(index);
            }
        }
        pending = still_pending;
        if pending.len() == before {
            // Cannot happen for real content -- see `same_kind_topological_order`'s own identical
            // reasoning; refuse rather than write an arbitrary order if a malformed bundle somehow
            // produces one.
            return Err(PrikkError::Integrity(
                "bundle contents: a same-kind object reference forms a cycle among carried objects"
                    .to_string(),
            ));
        }
    }
    Ok(ordered)
}

#[cfg(test)]
thread_local! {
    static CORRUPT_ADMISSION_OUTCOME_FOR_TEST: std::cell::Cell<Option<ObjectId>> =
        const { std::cell::Cell::new(None) };
    static SIMULATE_PASS_TWO_READ_FAILURE_FOR_TEST: std::cell::Cell<Option<ObjectId>> =
        const { std::cell::Cell::new(None) };
    static CORRUPT_FRAME_DIGEST_FOR_TEST: std::cell::Cell<Option<ObjectId>> =
        const { std::cell::Cell::new(None) };
}

/// Test seam, unreachable from production: pass 1 will record a deliberately wrong admission
/// outcome for the named object, so pass 2's own, correct recomputation disagrees with it -- the
/// only way to exercise R4's cross-pass assertion, which a correctly held lock makes otherwise
/// unreachable (K1's second case-table row).
#[cfg(test)]
pub(crate) fn corrupt_admission_outcome_for_test(object_id: ObjectId) {
    CORRUPT_ADMISSION_OUTCOME_FOR_TEST.with(|slot| slot.set(Some(object_id)));
}

/// Test seam, unreachable from production: pass 1 will record a deliberately wrong frame digest
/// for the named object, so pass 2's own, correct recomputation disagrees with it -- R3's digest
/// check exists to catch the frame changing between a real file's two reads. 0.55.0 step 3 Part
/// 3b-i2 made that reachable for real too, through [`corrupt_file_between_passes_for_test`]; this
/// seam stays for a one-object-targeted test that needs no real file mutation.
#[cfg(test)]
pub(crate) fn corrupt_frame_digest_for_test(object_id: ObjectId) {
    CORRUPT_FRAME_DIGEST_FOR_TEST.with(|slot| slot.set(Some(object_id)));
}

#[cfg(test)]
fn corrupted_frame_digest_for_test(
    object_id: ObjectId,
    digest: prikk_hash::Sha256Digest,
) -> prikk_hash::Sha256Digest {
    if CORRUPT_FRAME_DIGEST_FOR_TEST.with(std::cell::Cell::get) != Some(object_id) {
        return digest;
    }
    let mut corrupted = digest;
    if let Some(first_byte) = corrupted.first_mut() {
        *first_byte ^= 0x01;
    }
    corrupted
}

#[cfg(test)]
fn corrupted_admission_outcome_for_test(
    object_id: ObjectId,
    outcome: AdmissionOutcomeKind,
) -> AdmissionOutcomeKind {
    if CORRUPT_ADMISSION_OUTCOME_FOR_TEST.with(std::cell::Cell::get) != Some(object_id) {
        return outcome;
    }
    match outcome {
        AdmissionOutcomeKind::New => AdmissionOutcomeKind::AlreadyPresent,
        AdmissionOutcomeKind::AlreadyPresent | AdmissionOutcomeKind::Merged => {
            AdmissionOutcomeKind::New
        }
    }
}

/// Test seam, unreachable from production: pass 2 will refuse as though it could not re-read the
/// named object's own frame, with the same message shape a real read failure would produce (K1's
/// first case-table row). 0.55.0 step 3 Part 3b-i2: a real read failure between the two passes is
/// now reachable too, through [`corrupt_file_between_passes_for_test`]'s own real file mutation --
/// this seam stays, for a surgical, single-object-targeted test that needs no real file I/O.
#[cfg(test)]
pub(crate) fn simulate_pass_two_read_failure_for_test(object_id: ObjectId) {
    SIMULATE_PASS_TWO_READ_FAILURE_FOR_TEST.with(|slot| slot.set(Some(object_id)));
}

#[cfg(test)]
fn check_simulated_pass_two_read_failure_for_test(
    object_id: ObjectId,
    written_object_count: usize,
    object_count: usize,
) -> Result<()> {
    if SIMULATE_PASS_TWO_READ_FAILURE_FOR_TEST.with(std::cell::Cell::get) != Some(object_id) {
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "bundle import: object {object_id} could not be re-read for its own pass-2 check -- \
         {written_object_count} of {object_count} objects were already written, each validated \
         and admitted before being written; no received pointer was written; re-running the \
         import once the file is stable will complete it"
    )))
}

#[cfg(not(test))]
fn check_simulated_pass_two_read_failure_for_test(
    _object_id: ObjectId,
    _written_object_count: usize,
    _object_count: usize,
) -> Result<()> {
    Ok(())
}

/// Every carried envelope as it will be handed to the store, after RFC 156 §4's admission.
pub(crate) struct CarriedAdmission {
    /// One per carried object, in order: unchanged for an object this repository does not hold (or in
    /// format 6); the stored record plus every admitted signature for one it does.
    pub(crate) envelopes: Vec<ObjectEnvelope>,
    /// Objects this repository held that gained at least one signature.
    pub(crate) merged_object_count: usize,
    /// Signatures carried for held objects and not stored.
    pub(crate) dropped: Vec<crate::DroppedSignature>,
}

/// One carried envelope's own RFC 156 §4 admission outcome -- [`CarriedSignatureAdmitter::
/// admit_one`]'s own return type, factored out of [`admit_carried_signatures`]'s own loop body (RFC
/// 155 Part P2b) so a streaming caller can admit and write one object at a time, never collecting a
/// whole batch of carried envelopes first just to call this rule.
pub(crate) struct AdmittedOne {
    pub(crate) envelope: ObjectEnvelope,
    pub(crate) merged: bool,
    pub(crate) dropped: Vec<crate::DroppedSignature>,
}

/// RFC 156 §4 rules 1–3's own policy, loaded once and reused across many [`Self::admit_one`] calls
/// -- the batching `admit_carried_signatures` already did per call, now also available to a caller
/// that admits one envelope at a time instead of a whole pre-built slice (RFC 155 Part P2b:
/// `archive::import`'s stage 1 used to collect a whole object type's own envelopes into one `Vec`
/// specifically so it could call `admit_carried_signatures` once; this type lets it load the same
/// policy once and admit each envelope as it streams in, instead).
pub(crate) struct CarriedSignatureAdmitter {
    policy: crate::trust::MaintainerTrustPolicy,
    several_records: bool,
}

impl CarriedSignatureAdmitter {
    pub(crate) fn new(layout: &RepositoryLayout) -> Result<Self> {
        Ok(Self {
            policy: crate::trust::load_maintainer_trust_policy_or_empty(layout)?,
            several_records: layout.format().holds_several_records_per_id(),
        })
    }

    /// RFC 156 §4 rules 1-3 for one carried object -- the exact decision [`admit_carried_signatures`]
    /// makes per envelope in its own loop; factored out, not re-derived, so the two never drift
    /// apart.
    ///
    /// Takes `envelope` by value (0.55.0 step 3 Part 3b-ii): the two "nothing to merge" branches
    /// (not already stored, and format 6) move it straight into the returned [`AdmittedOne`]
    /// instead of cloning it, since neither branch changes it. A caller that still needs the
    /// original afterward for its own reasons (`admit_carried_signatures`, over a borrowed slice
    /// it does not own) clones at its own call site -- a scoped cost, not this function's.
    pub(crate) fn admit_one(
        &self,
        stored_objects: &impl ObjectReader,
        envelope: ObjectEnvelope,
        mut author_material: impl FnMut(&str) -> Result<Vec<AuthorKeyEntry>>,
    ) -> Result<AdmittedOne> {
        // Read through the caller's session, opened under the active lock, so the count below is the one
        // every other importer sees (RFC 156 §7.4 rule 5).
        let Some(stored) = stored_objects.read_object(envelope.object_id())? else {
            // A new object is stored as carried; the bound applies to it as to a merge.
            crate::signature_admission::require_within_signature_bound(&envelope, &self.policy)?;
            return Ok(AdmittedOne {
                envelope,
                merged: false,
                dropped: Vec::new(),
            });
        };
        if !self.several_records {
            // Format 6: the store keeps the stored record or refuses a different one; nothing merges.
            return Ok(AdmittedOne {
                envelope,
                merged: false,
                dropped: Vec::new(),
            });
        }
        let decided = crate::signature_admission::admit_signatures(
            stored,
            &envelope,
            &self.policy,
            &mut author_material,
        )?;
        if decided.adds_signatures {
            // Judged as it would be stored: the union. A copy adding nothing writes nothing, so a set a
            // local writer filled past the limit never refuses a re-import.
            crate::signature_admission::require_within_signature_bound(
                &decided.envelope,
                &self.policy,
            )?;
        }
        Ok(AdmittedOne {
            envelope: decided.envelope,
            merged: decided.adds_signatures,
            dropped: decided.dropped,
        })
    }
}

/// RFC 156 §4 rules 1–3 over a batch of carried objects — `bundle import` and `sync accept` both. Reads
/// only; the caller writes `envelopes` afterwards. A thin loop over [`CarriedSignatureAdmitter::
/// admit_one`] now (RFC 155 Part P2b) -- unchanged behavior and signature for this function's own
/// two existing callers; see [`CarriedSignatureAdmitter`] for the streaming-shaped alternative
/// `archive::import`'s stage 1 uses instead of calling this one over a whole pre-collected type.
pub(crate) fn admit_carried_signatures(
    layout: &RepositoryLayout,
    stored_objects: &impl ObjectReader,
    carried: &[ObjectEnvelope],
    mut author_material: impl FnMut(&str) -> Result<Vec<AuthorKeyEntry>>,
) -> Result<CarriedAdmission> {
    let admitter = CarriedSignatureAdmitter::new(layout)?;
    let mut admission = CarriedAdmission {
        envelopes: Vec::with_capacity(carried.len()),
        merged_object_count: 0,
        dropped: Vec::new(),
    };
    for envelope in carried {
        // 0.55.0 step 3 Part 3b-ii: `admit_one` now takes ownership (so its own two "nothing to
        // merge" branches can move rather than clone); this loop only ever holds a borrow from
        // `carried`, so it clones here instead -- a scoped cost for this caller, not `admit_one`'s.
        let admitted =
            admitter.admit_one(stored_objects, envelope.clone(), &mut author_material)?;
        if admitted.merged {
            admission.merged_object_count = admission
                .merged_object_count
                .checked_add(1)
                .ok_or_else(|| PrikkError::Integrity("merged-object count overflow".to_string()))?;
        }
        admission.dropped.extend(admitted.dropped);
        admission.envelopes.push(admitted.envelope);
    }
    Ok(admission)
}

#[cfg(test)]
thread_local! {
    static BEFORE_IMPORT_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static DURING_IMPORT_WRITES: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static CORRUPT_FILE_BETWEEN_PASSES_FOR_TEST: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam, unreachable from production: run `change` once, after the bundle's own header is
/// read (magic, ref name, declared count) and before `import_bundle` takes its locks — where a
/// concurrent writer could record an author key. 0.55.0 step 3 Part 3b-i2 (Q1): the structural and
/// closure checks that used to run here too, before any lock, now run *inside* the locked streaming
/// pass instead -- so a concurrent writer acting in this same window is now seen by the whole
/// locked per-object decision, not only the author-key-conflict check, and this seam still fires at
/// the identical point relative to lock acquisition.
#[cfg(test)]
pub(crate) fn before_import_lock_for_test(change: impl FnOnce() + 'static) {
    BEFORE_IMPORT_LOCK.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}

/// Test seam, unreachable from production: run `change` once, after `import_bundle`'s object writes
/// and before it records author keys — while it still holds `ActiveLock`.
#[cfg(test)]
pub(crate) fn during_import_writes_for_test(change: impl FnOnce() + 'static) {
    DURING_IMPORT_WRITES.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}

/// Test seam, unreachable from production: run `change` once, after pass 1 (admission and closure
/// validation) finishes and before pass 2's first seek -- 0.55.0 step 3 Part 3b-i2 (Q2, K1): now
/// that `bundle import` holds the file open across both passes instead of a `bytes: &[u8]`
/// snapshot, a real mutation of the bundle's own backing file (truncate or rewrite specific bytes)
/// becomes observable here for the first time. The callback closes over the file's own path and
/// mutates it directly through a second handle -- truncating or rewriting through a second handle
/// on the same path is visible to the first handle's next read, since file length and content are
/// properties of the inode, not the handle; `import_bundle` itself never needs that path.
#[cfg(test)]
pub(crate) fn corrupt_file_between_passes_for_test(change: impl FnOnce() + 'static) {
    CORRUPT_FILE_BETWEEN_PASSES_FOR_TEST.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}

/// Read a bundle and report whether it is structurally sound and internally consistent, **without
/// writing anything and without needing a repository** (DC-44 increment 1,
/// `bundle-offline-verify-handoff-v1.md`). Runs the exact same decode and closure validation
/// [`import_bundle`] runs before its first write, via [`decode_bundle_closure_metadata`] and
/// [`validate_bundle_closure`] — §2's "reuse the decode path, do not re-implement it," now one
/// validator for all three commands rather than two that had to be kept in agreement by hand
/// (0.55.0 step 3 Part 3b-iv).
///
/// **What this proves**: the bundle decodes under the same rules `import_bundle` would apply
/// (magic, framing, declared counts against actual content, every object's envelope structurally
/// valid); and every object another carried object names by id — a Block's `patch_ids`,
/// `parent_block_ids`, and `snapshot_blob_ref`, a Patch operation's blob ids, the exported ref's
/// own target — resolves to an object *in this bundle* whose own freshly recomputed
/// [`ObjectEnvelope::object_id`] equals the id it was named by. That equality is the real
/// integrity check: an object whose bytes are corrupted recomputes to a different id, so whatever
/// named the original id no longer finds it, exactly the way a corrupted patch stops resolving
/// from the block that names it.
///
/// **What this does not prove** (§3.2 — stated here, not just in documentation, so a caller
/// cannot mistake "verified" for "trusted"): no signature is cryptographically checked. Every
/// object's signature is only checked *structurally* (`ObjectEnvelope::validate`, reused from
/// `decode_envelope_file` — algorithm shape, non-empty bytes), because that is all a bundle file
/// carries the material to check — verifying an AUTHOR or MAINTAINER signature against a real key
/// needs trust material this file does not carry (the bundle's own optional author-key section
/// records transported material for later import; it is never independently verified even then —
/// "import records material; verify decides," unchanged by this increment). A verified bundle is
/// not yet a *trusted* one.
///
/// **What this cannot prove relative to an arbitrary receiving repository**: `import_bundle`'s own
/// closure check accepts a reference satisfied by objects the bundle carries *or* objects the
/// target repository already has (an incremental import onto non-empty history). Offline
/// verification has no repository to ask, so it requires every reference to resolve within the
/// bundle itself. Every bundle `export_bundle` produces already satisfies this — export walks the
/// full ancestor closure back to genesis unconditionally (ruling 2), never assuming a receiver
/// already holds part of the history — so this is not a weaker approximation for a real export;
/// it is stricter than `import` only for a hand-crafted or already-imported-elsewhere bundle that
/// deliberately omits something on the assumption a specific receiver already has it, which this
/// tool never produces.
pub fn verify_bundle(
    source: &mut BundleSource,
    options: &BundleImportOptions,
) -> Result<BundleVerifyReport> {
    let metadata = decode_bundle_closure_metadata(source, options)?;
    // No lock, `local: None` -- offline verification has no repository to ask (this function's
    // own doc comment, "what this cannot prove" section), and nothing here writes, so there is
    // nothing a lock would protect. `validate_bundle_closure` already resolves and present-checks
    // the ref's own tip as part of item 1, so it is returned here rather than re-derived.
    let (ref_state_id, _bundle_key_ids, tip_block_id) = validate_bundle_closure(
        source,
        &metadata.origin_ref_name,
        &metadata.prepared,
        &metadata.index_by_id,
        &metadata.membership,
        &metadata.closure_by_id,
        None,
        &metadata.author_keys,
        metadata.manifest.as_ref(),
        metadata.object_count,
    )?;
    Ok(BundleVerifyReport {
        ref_name: metadata.origin_ref_name,
        ref_state_id,
        tip_block_id,
        object_count: u64_to_usize(metadata.object_count)?,
        author_key_count: metadata.author_keys.len(),
        manifest: metadata.manifest.map(|raw| BundleManifest {
            repository_format: raw.repository_format,
            tool_version: raw.tool_version,
            scope: raw.scope,
        }),
    })
}

/// The numeric on-disk repository format for the manifest's `repository_format` field. A local
/// mapping, not a `layout.rs` addition: `RepositoryFormat` is a "different axis" from the bundle
/// wire format by design (`layout.rs`'s own doc comment), so this stays a small translation at the
/// one place that needs a bare number, rather than teaching `layout.rs` about bundles. Kept as an
/// exhaustive match deliberately -- `layout.rs`'s own doc already notes the enum's shape carries
/// meaning, so a future format 7 variant must force this match to be revisited, not silently fall
/// through to a stale number.
const fn repository_format_number(format: RepositoryFormat) -> u32 {
    match format {
        RepositoryFormat::CurrentV6 => 6,
        // RFC 156 §5b. Informational in the manifest — no receiver refuses on it — so a format-7
        // sender's bundle still imports into an older receiver.
        RepositoryFormat::V7 => 7,
    }
}

/// A Blob the export must carry, naming the block-chain shape when that is why it could not be derived
/// (review v2, finding 2): a bare `missing blob object` would hide the reason.
fn read_required_blob(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
    unsupported_chain: Option<&str>,
) -> Result<ObjectEnvelope> {
    match object_store.read_typed(blob_id, ObjectType::Blob)? {
        Some(envelope) => Ok(envelope),
        None => Err(PrikkError::Integrity(match unsupported_chain {
            Some(detail) => format!(
                "missing blob object: {blob_id}; its content could not be derived because this \
                 history's block chain could not be walked ({detail})"
            ),
            None => format!("missing blob object: {blob_id}"),
        })),
    }
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

/// `preview_bundle_reporting_anchor`'s own "present" definition: carried by this bundle, or
/// already in this repository when one is available. `local` is `None` for a bundle-only view
/// (the bundle's own ancestor walk must never be credited with local objects it did not itself
/// carry); `Some` for the composed view the actual comparison runs against.
///
/// 0.55.0 step 3 Part 3b-iv (P1): random access by id is not whole-bundle residency. This reads
/// each bundle-carried object on demand -- seeking to `prepared[index_by_id[id]]`'s own recorded
/// frame range through `source` and decoding just that one frame -- instead of holding a
/// `BTreeMap<ObjectId, ObjectEnvelope>` of the whole bundle. `source`/`cache` are shared (via
/// `&RefCell`, not owned) across every `BundleAndLocalReader` built from the same bundle in one
/// `preview` call, so the bundle-only and composed views reuse one decode per id between them,
/// and the replay machinery's own repeat lookups (confirmed real: the same Block id is read
/// 2-4+ times within one `preview_impact` call, by the ancestor-closure walk, the anchor search,
/// and the main replay loop) cost one seek-and-decode each, not one per call.
///
/// `pub(crate)` (RFC 144 §4m.3): this is the "composed, read-only reader -- the repository's
/// object store overlaid with the bundle's own object set" the bundle-impact preview needs,
/// reused rather than reimplemented -- see `preview_bundle`'s own doc comment.
pub(crate) struct BundleAndLocalReader<'a> {
    pub(crate) source: &'a RefCell<&'a mut BundleSource>,
    pub(crate) prepared: &'a [PreparedObject],
    pub(crate) index_by_id: &'a BTreeMap<ObjectId, usize>,
    pub(crate) cache: &'a RefCell<BTreeMap<ObjectId, ObjectEnvelope>>,
    pub(crate) local: Option<&'a ObjectReadSnapshot>,
}

impl ObjectReader for BundleAndLocalReader<'_> {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        if let Some(cached) = self.cache.borrow().get(&id) {
            return Ok(Some(cached.clone()));
        }
        if let Some(&index) = self.index_by_id.get(&id) {
            let envelope = {
                let mut guard = self.source.borrow_mut();
                read_prepared_object_at(&mut guard, self.prepared, index)?
            };
            self.cache.borrow_mut().insert(id, envelope.clone());
            return Ok(Some(envelope));
        }
        match self.local {
            Some(local) => local.read_object(id),
            None => Ok(None),
        }
    }
}

/// Every blob id one decoded patch operation references -- restated from
/// `patch_exchange/accept.rs`'s own `referenced_blob_ids` (the same three kinds `export_bundle`'s
/// closure walk above also scans for), for `import_bundle`'s own closure-completeness check. Kept as
/// a separate, per-module copy rather than a cross-module `pub(crate)` call: `accept.rs`'s own doc
/// comment already restates this same match once for `artifact.rs`'s benefit, so a third small copy
/// here follows the precedent this file already has, not a new one.
fn bundle_referenced_blob_ids(kind: &DecodedOperationKind) -> Vec<ObjectId> {
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

/// Resolve `ref_state_payload.target_object_id` to the Block it ultimately names -- one hop for a
/// `Branch` (the target *is* the Block), two hops for a `Tag` (the target is a Tag object; its own
/// `target_block_id` is the Block). The two-hop resolution itself is `refs::resolve_ref_tip_block`
/// (ref-tip-resolver-consolidation handoff), shared with `patch_set_digest.rs` and
/// `patch_exchange.rs`; this wrapper's own job is pushing the resolved Tag envelope onto
/// `tag_envelopes` so the caller can include it in the exported object set -- omitting it would hand
/// a receiver a signed RefState naming an object they do not have, which fails their `verify` with a
/// message naming exactly that (DC-78 bundle-export tag gap follow-up,
/// `bundle-export-tag-ref-gap-v1.md`). The accumulator stays here, not in the shared resolver, since
/// this is the only one of three callers that needs it.
fn resolve_ref_target_block(
    object_store: &impl ObjectReader,
    ref_state_payload: &RefStatePayload,
    tag_envelopes: &mut Vec<ObjectEnvelope>,
) -> Result<ObjectId> {
    let (target_block_id, tag_envelope) =
        crate::refs::resolve_ref_tip_block(object_store, ref_state_payload)?;
    if let Some(tag_envelope) = tag_envelope {
        tag_envelopes.push(tag_envelope);
    }
    Ok(target_block_id)
}

pub(crate) fn encode_bundle(
    ref_name: &str,
    objects: &[ObjectEnvelope],
    author_keys: &[AuthorKeyEntry],
    manifest: &BundleManifest,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(BUNDLE_MAGIC);
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, len_to_u64(objects.len())?);
    for envelope in objects {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    // DC-53 Stage 2, D6: the author-key section, appended after the object list.
    push_u64(&mut out, len_to_u64(author_keys.len())?);
    for entry in author_keys {
        push_bytes_u64(&mut out, entry.key_id.as_bytes())?;
        out.extend_from_slice(&entry.public_key);
    }
    // DC-44 increment 3: the manifest section, appended after the author-key section (§4.2 --
    // one more section in the same append-only shape, not a rearrangement of what came before).
    // `ref_name`/`objects.len()` are written a second time here, deliberately: not new
    // information, but an independently-set declaration that `validate_bundle_contents` checks
    // against the header and the RefState's own signed `ref_name` (control 4) -- the same
    // "hostile bundle disagreeing with itself" defense the author-key section's own internal
    // consistency check already established above.
    push_u64(&mut out, u64::from(manifest.repository_format));
    push_bytes_u64(&mut out, manifest.tool_version.as_bytes())?;
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, len_to_u64(objects.len())?);
    push_u64(&mut out, manifest.scope.wire_tag());
    Ok(out)
}

/// Encode a `PBNDL001`-shaped bundle -- what a Stage-1-or-earlier build actually produced, and the
/// only thing `decode_bundle`'s `PBNDL001` acceptance needs to handle correctly. Test-only: no
/// production caller ever emits this format (`encode_bundle` above always writes `BUNDLE_MAGIC`).
/// Built from the same encoding primitives as `encode_bundle`, mirroring its pre-author-key-section
/// body exactly, rather than hand-editing bytes -- a hand-built fixture would prove the parser
/// accepts a byte shape, not that the real historical format actually decodes.
#[cfg(test)]
fn encode_bundle_v1_for_test(ref_name: &str, objects: &[ObjectEnvelope]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(RETIRED_BUNDLE_MAGIC_V1);
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, len_to_u64(objects.len())?);
    for envelope in objects {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    Ok(out)
}

/// Encode a `PBNDL002`-shaped bundle -- an author-key section, but no manifest. DC-44 increment 3's
/// own control 2 ("`PBNDL001` and `PBNDL002` must still import ... with real bytes, not a
/// hand-built approximation"), mirroring `encode_bundle_v1_for_test`'s own precedent: built from the
/// same encoding primitives as `encode_bundle`, its pre-manifest-section body exactly, rather than
/// hand-editing bytes. Test-only: no production caller ever emits this format since `encode_bundle`
/// above always writes `BUNDLE_MAGIC` (`PBNDL003`).
#[cfg(test)]
fn encode_bundle_v2_for_test(
    ref_name: &str,
    objects: &[ObjectEnvelope],
    author_keys: &[AuthorKeyEntry],
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(RETIRED_BUNDLE_MAGIC_V2);
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, len_to_u64(objects.len())?);
    for envelope in objects {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, len_to_u64(author_keys.len())?);
    for entry in author_keys {
        push_bytes_u64(&mut out, entry.key_id.as_bytes())?;
        out.extend_from_slice(&entry.public_key);
    }
    Ok(out)
}

/// Everything decoded from a bundle's manifest section (`PBNDL003` only), before cross-checking
/// against the rest of the payload. `declared_ref_name`/`declared_object_count` exist on the wire
/// only to be checked against independent sources in `validate_bundle_contents` -- the public
/// [`BundleManifest`], built only after that check passes, drops both. `PartialEq`/`Debug` are for
/// the round-trip property test's own benefit (`proptest_decode_bundle.rs`), not a production need.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DecodedManifest {
    repository_format: u32,
    tool_version: String,
    scope: BundleScope,
    declared_ref_name: String,
    declared_object_count: u64,
}

/// `decode_bundle`'s own return shape, named so the signature reads as "ref name, objects,
/// author keys, manifest" instead of a four-tuple clippy's own `type_complexity` lint flags.
/// 0.55.0 step 3 Part 3b-iii: `decode_bundle` no longer has a production caller (`preview`/`verify`
/// decode straight into `bundle_objects_by_id` instead, never a separate `Vec<ObjectEnvelope>`) --
/// it stays, `#[cfg(test)]`, as the one decoder every test fixture that needs to inspect or tamper
/// with a bundle's own decoded objects before re-encoding them still uses.
#[cfg(test)]
pub(crate) type DecodedBundle = (
    String,
    Vec<ObjectEnvelope>,
    Vec<AuthorKeyEntry>,
    Option<DecodedManifest>,
);

#[cfg(test)]
pub(crate) fn decode_bundle(
    bytes: &[u8],
    max_object_count: usize,
    max_object_bytes: usize,
) -> Result<DecodedBundle> {
    let mut cursor = ByteCursor::new(bytes);
    let magic = cursor.read_array::<8>()?;
    // DC-53 Stage 2 follow-up (bundle-v1-import-regression-v1.md): `PBNDL001` is accepted here, not
    // refused. `layout.rs`'s own retired-format messages instruct a user to open an old repository
    // with an old prikk build and `bundle export` from it -- that build only ever emits `PBNDL001`,
    // and a current build refusing to import it severed the one migration path those messages
    // promise, in both directions at once. Export still only ever emits `BUNDLE_MAGIC`
    // (`PBNDL003`) -- this is read compatibility only, the same asymmetry every repository-format
    // transition in this project already has (read what the past wrote, write only the present).
    // A `PBNDL001` bundle is a `PBNDL002`/`PBNDL003` bundle without the author-key section (and, for
    // `PBNDL001`/`PBNDL002` alike, without the manifest section either): not a special case, since
    // DC-53 already defines "no recorded material" as `Unverifiable` (vector 7), so decoding one
    // simply treats the author-key set as empty and the manifest as absent rather than reading
    // sections that were never written. **`PBNDL002`'s own decode path below is byte-for-byte
    // unchanged by this bump** (DC-44 increment 3 handoff §4.2): `has_author_key_section` still
    // reads exactly the same way for it as before, and the only new code this magic can ever reach
    // is `has_manifest_section == false`, i.e. none.
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
    let ref_name_bytes = cursor.read_bytes_u64()?;
    let ref_name = String::from_utf8(ref_name_bytes).map_err(|err| {
        PrikkError::MalformedData(format!("invalid bundle ref name utf-8: {err}"))
    })?;
    let count = cursor.read_u64()?;
    // DC-86: refused here, before the loop below decodes a single object — a declared count over
    // the limit must not cost more than reading one u64 to reject, regardless of how large `count`
    // claims to be or how much of `bytes` actually backs it.
    if count > len_to_u64(max_object_count)? {
        return Err(PrikkError::MalformedData(format!(
            "bundle declares {count} objects, over the configured limit of {max_object_count}"
        )));
    }
    let mut objects = Vec::new();
    for _ in 0..count {
        // RFC 158 Stage A §3: checked on the length prefix, before this frame is copied or
        // decoded -- the same declared-count discipline DC-86 already applies to `count` above,
        // now applied per object rather than to the whole set.
        let encoded = read_bounded_object_frame(&mut cursor, max_object_bytes)?;
        objects.push(decode_envelope_file(&encoded)?);
    }
    // DC-53 Stage 2, D6/C1 (plan review): the same declared-count bound DC-86 already applies to
    // the object count, applied here too -- a second declared count in a format a hostile sender
    // fully controls, with no bound of its own, would reopen the hole DC-86 closed. An
    // author-key entry can never legitimately outnumber the Patches in the same bundle, so reusing
    // `max_object_count` as the ceiling needs no new option surface.
    let mut author_keys = Vec::new();
    if has_author_key_section {
        let author_key_count = cursor.read_u64()?;
        if author_key_count > len_to_u64(max_object_count)? {
            return Err(PrikkError::MalformedData(format!(
                "bundle declares {author_key_count} author key entries, over the configured limit \
                 of {max_object_count}"
            )));
        }
        for _ in 0..author_key_count {
            let key_id_bytes = cursor.read_bytes_u64()?;
            let key_id = String::from_utf8(key_id_bytes).map_err(|err| {
                PrikkError::MalformedData(format!("invalid bundle author key_id utf-8: {err}"))
            })?;
            // DC-53 Stage 2, plan review: reuse `Signature::validate_key_id` rather than a second
            // notion of what a legal key id is -- it is the same rule these ids must satisfy to
            // ever match a signature's own `key_id`.
            Signature::validate_key_id(&key_id)?;
            let public_key = cursor.read_array::<32>()?;
            author_keys.push(AuthorKeyEntry { key_id, public_key });
        }
    }
    // DC-44 increment 3: the manifest section, `PBNDL003` only. `declared_ref_name` and
    // `declared_object_count` are read here and checked in `validate_bundle_contents`, once the
    // rest of the payload it must agree with is also decoded (§4.2 -- this branch is unreachable
    // for `PBNDL001`/`PBNDL002`, so their own decode path above is untouched).
    let manifest = if has_manifest_section {
        let repository_format = u32::try_from(cursor.read_u64()?).map_err(|_| {
            PrikkError::MalformedData(
                "bundle manifest declares a repository format number that does not fit in u32"
                    .to_string(),
            )
        })?;
        let tool_version_bytes = cursor.read_bytes_u64()?;
        let tool_version = String::from_utf8(tool_version_bytes).map_err(|err| {
            PrikkError::MalformedData(format!("invalid bundle manifest tool version utf-8: {err}"))
        })?;
        let declared_ref_name_bytes = cursor.read_bytes_u64()?;
        let declared_ref_name = String::from_utf8(declared_ref_name_bytes).map_err(|err| {
            PrikkError::MalformedData(format!("invalid bundle manifest ref name utf-8: {err}"))
        })?;
        let declared_object_count = cursor.read_u64()?;
        let scope = BundleScope::from_wire_tag(cursor.read_u64()?)?;
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
    // A `PBNDL001`/`PBNDL002` bundle's bytes end right after the last section it actually has --
    // no manifest (either format) or author-key section (`PBNDL001` only) was ever written, so
    // there is nothing further to consume and this check still catches genuine trailing garbage on
    // any format.
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in bundle".to_string(),
        ));
    }
    Ok((ref_name, objects, author_keys, manifest))
}

#[cfg(test)]
mod tests;
