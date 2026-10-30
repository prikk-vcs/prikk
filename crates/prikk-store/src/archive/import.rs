//! RFC 155 implementation handoff: `archive import`'s stages 0-3, the import journal, the
//! writer-refusal interlock, and RFC 156's identity merge (§9.5/§9.6/§9.7) -- Parts I-A, I-A2, and
//! I-B, in that order. The public CLI surface this module backs is `prikk archive import <file>
//! [--adopt]` (`prikk-cli::archive::run_import`, Part I-B's own deliverable) and `prikk doctor
//! --cancel-import <id> [--plan-only]` (Part I-A's own).
//!
//! **The journal is the first write** (§9.5): before it exists, stage 0's checks (the archive's own
//! verify, the author-key conflict check, the landing plan) have all already run, read-only, and a
//! refusal there writes nothing. Once it exists, every container lock it names is held until the
//! commit point, which empties it (keeping its name, RFC 168 §6's conservative choice). While it
//! exists, every ordinary repository writer refuses -- enforced structurally inside
//! [`crate::lock::ActiveLock::acquire`] itself (see that function's own doc comment), not by
//! per-call-site discipline, so a future writer this part's own author never anticipated is
//! covered automatically rather than silently missed.
//!
//! **Stage 0 reuses [`super::verify_archive`] directly, not a copy** (the handoff's own words, and
//! `verify.rs`'s own doc comment on why that is what makes Part I-A possible at all): a refused
//! import here is a refused `archive verify`, named identically. This module's own second pass
//! over the same archive (to decode and write what verify only checksums) reuses the same
//! low-level section decoders verify already uses ([`ManifestView`](super::verify), made visible to
//! this sibling module through their shared parent) rather than re-deriving the frame format a
//! second time -- see each reused function's own doc note in `verify.rs`.
//!
//! **Stage 1 never holds more than one decoded object's own bytes in memory at a time** (RFC 155
//! Part P2b, review `release-0.51.0-prep-P2-review-v1`: the handoff's own prohibited-shortcut,
//! "holding the whole archive or every object in memory," held only *across* types at first --
//! within one type, every one of its objects was still collected into a `Vec` before any of them
//! was admitted or written, which measured at 2.254x the archive's own size for a 1 GiB fixture
//! concentrated in one type. Fixed by streaming: each record `for_each_object_record` decodes is
//! admitted through [`crate::bundle::CarriedSignatureAdmitter`] and written immediately, one at a
//! time, before the next is even decoded.) Processed in [`crate::bundle::
//! object_type_dependency_rank`]'s own cross-type order -- the same ranking `bundle import` and
//! `patch_exchange::accept` already use. **No same-kind re-sort is needed within a type**: an
//! archive's own `ObjectContainer` section is a verbatim stream of the live container file's own
//! bytes (`archive.rs`'s own `write_section`), so decode order here is exactly the container's own
//! on-disk append order, and this codebase's write-once, content-addressed object stores can only
//! ever append a same-kind dependency (a `Block`'s own parents, a `RefState`'s own
//! `previous_ref_state_id`) *before* whatever will later reference it -- container order is
//! already a valid topological order for these, unlike a *bundle*'s own carried order, which
//! [`crate::bundle::objects_in_dependency_order`]'s own doc comment notes can list a child before
//! its parent (that function keeps re-sorting for `bundle import`'s own, differently-assembled
//! envelope list; this module no longer needs it). **An id already present locally may gain a
//! carried signature** (§9.7, RFC 156 §4's merge), through [`crate::bundle::
//! CarriedSignatureAdmitter::admit_one`] -- the same rule `bundle import`/`patch_exchange::accept`
//! apply through [`crate::bundle::admit_carried_signatures`], factored so both share one admission
//! decision rather than this module re-deriving it. A signature that does not qualify (an unadopted
//! MAINTAINER key, an unrecorded AUTHOR key, an unverifiable role) is dropped and named, never
//! silently discarded.
//!
//! **Stage 2 records carried author keys through the unchanged DC-78 path**
//! (`check_author_key_conflict` at stage 0, `record_author_key_material` here) -- both already
//! idempotent, so a resume safely re-runs stage 2 from its start without double-recording anything.
//!
//! **RFC 155 Part I-A2 (review `rfc155-impl-IA-review-v1`, finding IA2): the author-key container
//! is journaled and cut back on cancel.** Part I-A's own first cut read §9.5's journaled-container
//! list literally and left it out, reasoning that `record_author_key_material`'s own idempotency
//! made a leftover record harmless. That reasoning had a real hole: a cancel that leaves a
//! recorded author key behind binds that `key_id` **for good** -- one `key_id` binds to one public
//! key (DC-53), so a later, genuine key for the same id would then be refused as a conflict rather
//! than recorded. §9.5 is corrected (2026-10-11) to list this container; this module now journals
//! its starting length the same way as every other appendable container, and `cancel_import` cuts
//! it back, with the same recovery-log save first, as part of the same loop.
//!
//! **RFC 155 Part I-A2 (finding IA1): the `ObjectStore` container lock is held structurally,
//! through stage 1, not just by inspection.** Part I-A's own first cut did not hold it (reasoning:
//! `write_object` already takes it per append, so holding it continuously would be import
//! conflicting with its own per-append acquisition), leaning instead on "every other object
//! appender already goes through the default session's `ActiveLock`" -- true, but an argument,
//! not the structural guarantee §9.5 actually promises. Stage 1 now holds `ObjectStore` together
//! with the other three container locks, from the journal to the commit point, and appends
//! through [`crate::object_store::ObjectWriteSession::write_object_under_held_lock`] instead of
//! the ordinary per-append path -- so a concurrent object append from *any* handle, default
//! session or not, genuinely refuses with `LockConflict` for the whole of stage 1, not merely "in
//! every case this module's own author could think of."
//!
//! **RFC 155 Part I-B2 (review `rfc155-impl-IB-review-v1`, finding IB1): the other three container
//! locks are now held structurally too, closing a real gap, not a gap on paper.** Part I-B's own
//! first cut, when stage 3 (ref landing) first existed, chose not to hold `RefPointerIndex`/
//! `RefLog`/`ReceivedIndex` continuously, arguing that every other repository writer already
//! refuses while this import's own journal is present, so nothing else could touch them in between
//! -- true for `ActiveLock`-based writers, but `compact --pointer-index`/`--received-index`/
//! `--trust-policy`, `doctor --rebuild-pointer-index`, and `doctor --repair-index`/
//! `--repair-pointer-index-tail` each take only a container lock, with no `ActiveLock` at all, so
//! none of them were actually blocked. All four container locks are now held from the journal to
//! the commit point, literally as §9.5 states, through
//! [`crate::refs::RefStore::publish_with_object_store_under_held_lock`] and
//! [`crate::received::write_received_pointer_under_held_lock`] in stage 3, and
//! [`crate::lock::acquire_container_locks`] itself now refuses while a journal is present (see its
//! own doc comment) -- so every one of those lock-only writers, present and future, is covered too.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek};
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlockPayload, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload, RefUpdatePayload,
    SignerRole, TagPayload,
};

use crate::author::author_key_index::{
    check_author_key_conflict, lookup_author_key_entries, record_author_key_material,
};
use crate::bundle::{
    BundleConnectivityClass, CarriedSignatureAdmitter, RefStateChainRelation,
    classify_connectivity_from_sets, classify_landing_connectivity, object_type_dependency_rank,
    ref_state_chain_contains,
};
use crate::foundation::fsutil::{
    create_new_file_required, ensure_directory_required, read_file_if_exists,
    remove_file_if_present_required, truncate_existing_file_required,
};
use crate::foundation::layout::persisted_object_types;
use crate::foundation::layout::{ContainerSlot, LockableContainer, RepositoryLayout};
use crate::lock::{
    ActiveLock, ContainerLockGuard, acquire_container_locks_allowing_import_journal,
};
use crate::object_store::{ObjectReader, ObjectWriteSession};
use crate::received::received_index::{
    decode_received_index_entries_for_resolver, reduce_received_index_entries,
};
use crate::refs::{
    RefPublication, RefStore, decode_pointer_index_entries_for_resolver,
    decode_ref_log_records_from_bytes, reduce_pointer_index_entries,
};
use crate::signature_admission::DroppedSignature;
use crate::trust::{load_maintainer_trust_policy_or_empty, verify_trusted_publication_envelope};

use super::ArchiveSectionKind;
use super::verify::{
    find_entry, for_each_object_record, read_author_material, read_end_trailer, read_manifest,
    read_section_body_fully,
};

/// Journal magic -- distinct from every other file this repository writes, so a resync or a
/// misread never confuses it with anything else.
const JOURNAL_MAGIC: &[u8; 8] = b"PRIMPJ01";
const JOURNAL_VERSION: u16 = 1;
/// DC-86 discipline: every length decoded from the journal is checked against one of these before
/// an allocation or a copy -- the journal is this repository's own write, never adversarial input
/// the way an archive file is, but a torn write from a real crash (the one case this bounds
/// against) must still refuse cleanly rather than read whatever garbage follows as a length.
const MAX_JOURNAL_STRING_BYTES: usize = 4096;
const MAX_JOURNAL_CONTAINER_COUNT: usize = 64;
const MAX_JOURNAL_LANDING_COUNT: usize = 10_000_000;

/// One container the journal names: its own stable label (for messages), its path (relative to the
/// repository root, `/`-separated, matching `recovery_log`'s own convention), and the length it
/// had before this import's own first write.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JournaledContainer {
    label: String,
    relative_path: String,
    starting_length: u64,
    /// Which `LockableContainer` this file lives under -- `cancel_import`'s own cut needs this to
    /// take the object-store lock briefly, per cut, rather than for its whole run (see this
    /// module's own doc note on why the object-store lock is never held for a whole import run).
    kind: JournaledContainerKind,
}

/// Which `LockableContainer` a journaled file belongs to -- `AuthorKey` has none (RFC 155 Part
/// I-A2's own fix, review finding IA2: §9.5 is corrected to journal this container too, but it is
/// protected solely by the default session's `ActiveLock`, the same way `record_author_key_
/// material` always was, not by a `LockableContainer` of its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournaledContainerKind {
    ObjectStore,
    RefLog,
    RefPointerIndex,
    ReceivedIndex,
    AuthorKey,
}

impl JournaledContainerKind {
    fn to_wire(self) -> u8 {
        match self {
            Self::ObjectStore => 0,
            Self::RefLog => 1,
            Self::RefPointerIndex => 2,
            Self::ReceivedIndex => 3,
            Self::AuthorKey => 4,
        }
    }

    fn from_wire(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::ObjectStore),
            1 => Ok(Self::RefLog),
            2 => Ok(Self::RefPointerIndex),
            3 => Ok(Self::ReceivedIndex),
            4 => Ok(Self::AuthorKey),
            other => Err(PrikkError::Integrity(format!(
                "import journal names an unknown container kind {other}"
            ))),
        }
    }
}

/// Which of the archive's own two resolved ref families a planned landing came from -- RFC 155
/// Part I-B's own fix (review `rfc155-impl-IA-review-v1`, finding A3): the two are **not** landed
/// the same way. §9.6: "the archive's received pointers stay received under their own names; its
/// local refs land under `remotes/<ref>`, or locally with `--adopt`."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LandingOrigin {
    /// From the archive's own resolved ref-pointer-index (its own local branches/tags). Lands
    /// under `remotes/<ref_name>` by default, or as a real local ref under `--adopt`.
    Local,
    /// From the archive's own resolved received-index (refs the *source* repository itself had
    /// only received, never advanced locally). `ref_name` is already `remotes/`-shaped (that
    /// index's own stored convention -- see `received.rs`'s own doc on `ReceivedPointer`), so it
    /// lands verbatim, never re-wrapped a second time.
    Received,
}

impl LandingOrigin {
    fn to_wire(self) -> u8 {
        match self {
            Self::Local => 0,
            Self::Received => 1,
        }
    }

    fn from_wire(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::Local),
            1 => Ok(Self::Received),
            other => Err(PrikkError::Integrity(format!(
                "import journal names an unknown landing origin {other}"
            ))),
        }
    }
}

/// One ref the archive carries, and where it would land if stage 3 ran today (§9.6's default --
/// `--adopt` may change this at stage 3 for a [`LandingOrigin::Local`] entry). Recorded in the
/// journal so a resumed stage 3 has a plan to act on without re-deriving it from the archive a
/// second time.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlannedRefLanding {
    /// Which resolved family this came from, and therefore how it is landed.
    pub origin: LandingOrigin,
    /// The ref name as the archive's own resolved pointer/received index carries it (bare for
    /// [`LandingOrigin::Local`], e.g. `heads/main`; already `remotes/`-shaped for
    /// [`LandingOrigin::Received`]).
    pub ref_name: String,
    /// This ref's own resolved tip `RefState` id, from the archive's own resolved index -- stage 1
    /// already wrote the object this names (`RefState` is one of the seven persisted object
    /// types), so stage 3 reads it back rather than re-deriving it.
    pub ref_state_id: ObjectId,
    /// Where this ref lands absent `--adopt` (or always, for [`LandingOrigin::Received]`): the
    /// archive's own received-index entries land under this name verbatim; the archive's own
    /// local refs land under `remotes/<ref_name>`.
    pub destination: String,
}

struct ImportJournal {
    archive_identity: [u8; 32],
    containers: Vec<JournaledContainer>,
    landing: Vec<PlannedRefLanding>,
}

/// The id `prikk doctor --cancel-import <id>` takes, and the one `doctor`'s own issue names.
/// Delegates to `foundation::import_journal_marker`'s own algorithm so the two can never disagree
/// (that module's own doc note explains why it exists as a separate, lower layer than this one).
fn journal_short_id(archive_identity: &[u8; 32]) -> String {
    crate::foundation::import_journal_marker::short_id(archive_identity)
}

/// `prikk archive import <file> [--adopt] [--allow-non-fast-forward]` (§9.8; the second flag,
/// 0.56.0 step 3c, design review v2 ruling A3, extending 3a's own `bundle import` flag of the
/// same name to this importer).
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct ArchiveImportOptions {
    /// §9.6's narrowed `--adopt`: land a [`LandingOrigin::Local`] ref as a real local ref when it
    /// is absent locally, signed by an adopted key, verified, and its CAS against absent holds --
    /// every other ref (and every [`LandingOrigin::Received`] one, always) still lands under
    /// `remotes/`, with the reason named.
    pub adopt: bool,
    /// 0.56.0 step 3c (A3): accept a landing that is a `rollback`, `diverged`, or
    /// `does-not-connect` against the pointer it would replace -- a received landing's own
    /// current received pointer of the same name, or (under `--adopt`) the received pointer
    /// already held at the destination this entry would otherwise land at. `false` by default,
    /// matching `bundle import`'s own flag of the same name (0.56.0 step 3a).
    pub allow_non_fast_forward: bool,
}

/// How one planned landing actually resolved at stage 3.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LandingOutcome {
    /// The ref name as planned (see [`PlannedRefLanding::ref_name`]).
    pub ref_name: String,
    /// Where it actually landed: a received pointer's own full name (`remotes/...` already, or
    /// the default `remotes/<ref_name>` wrap), or the bare local name when `--adopt` applied.
    pub landed_as: String,
    /// `true` only when a [`LandingOrigin::Local`] ref landed as a real local ref under
    /// `--adopt`; `false` for every received-pointer landing (whether by default or because
    /// `--adopt` could not apply).
    pub adopted: bool,
    /// Why this landed as a received pointer instead of a real local ref, when `--adopt` was
    /// requested but one of §9.6's four conditions did not hold. `None` when `--adopt` was not
    /// requested, when this is a [`LandingOrigin::Received`] entry (adoption never applies), or
    /// when adoption actually succeeded with nothing worth naming.
    ///
    /// 0.56.0 step 3c (A3): also carries the connectivity class (`"rollback"`, `"diverged"`, or
    /// `"does-not-connect"`) when adoption proceeded *despite* that class, against a received
    /// pointer already held at this entry's own destination, because `--allow-non-fast-forward`
    /// was given -- a genuinely new fact about the repository worth naming even though nothing
    /// refused and adoption succeeded. Never set for `already-at-tip`/`fast-forward` (nothing to
    /// name: see this field's own doc note above, "when adoption actually succeeded" is still the
    /// default for the ordinary, safe case).
    pub remote_reason: Option<String>,
    /// `false` only when this entry was not landed at all, because it collided with another ref's
    /// own, already-landed destination (RFC 155 §9.6, amended 2026-10-12, external review 023: the
    /// exporter's own local ref wins the name; the colliding carried received pointer is not
    /// written). `true` for every ordinary outcome -- this can only ever be `false` when an
    /// exporter's own local ref and one of its own carried received pointers resolve to the same
    /// `remotes/<ref>` name.
    pub landed: bool,
    /// Set only when `landed` is `false`: the `RefState` id this entry would have landed, had it
    /// not collided -- named so nothing is silently dropped from the report even though nothing
    /// was written here. Its own object was still imported by stage 1, which this never touches.
    pub collision_target: Option<ObjectId>,
    /// Which resolved family this landing came from (I-E item 4): the CLI's own default-import
    /// trust note must give the `--adopt` hint only for a [`LandingOrigin::Local`] entry -- a
    /// received pointer can never be adopted (`--adopt` only ever applies to the exporter's own
    /// ref), so the hint must never be offered for one. `false`/irrelevant-shaped default when
    /// `landed` is `false`, since nothing is checked for a landing that never happened.
    pub origin: LandingOrigin,
    /// `true` when this landing's own tip carries a maintainer signature **this destination
    /// repository already trusts** (checked through `verify_trusted_publication_envelope` against
    /// the destination's own adopted policy, independent of whether `--adopt` applied) -- I-E item
    /// 4's own re-scoping of the review's finding: the review's own three rows showed the prior
    /// note fired for every landed-but-not-adopted ref regardless of whether the signer was
    /// actually trusted, printing "verify fails" beside a tip that in fact verifies fine. `false`
    /// when `landed` is `false` (nothing was written to check).
    pub publication_trusted: bool,
    /// Every distinct MAINTAINER signer key id on this landing's own tip (I-F item 1), sorted and
    /// deduplicated -- so the CLI's own trust note can name a concrete key for `trust maintainer
    /// add` to act on, since `verify`'s own lines name only object ids, never a key id. Empty when
    /// the tip's own object could not be read (should not happen in practice; stage 1 already
    /// wrote it before stage 3 ever runs) or when nothing was checked.
    pub signing_key_ids: Vec<String>,
    /// 0.56.0 step 3c2 (R2): this landing's own connectivity class against the pointer it would
    /// replace (or did replace, for a collided entry) -- `NoLocalHistory` when no such pointer
    /// existed. The same six values `bundle import`'s own `BundleImportReport::class` and
    /// `bundle preview`'s own `"class"` field already carry, additive here too so
    /// `prepo-import-report-v1` can report every landing's own classification, not only a
    /// refused one.
    pub class: BundleConnectivityClass,
    /// 0.56.0 step 3c2 (R2): the `RefState`-chain relation to the pointer `class` was computed
    /// against -- `None` exactly when `class` is `NoLocalHistory` (there was no prior `RefState`
    /// to chain against).
    pub ref_state_chain: Option<RefStateChainRelation>,
}

/// Every distinct MAINTAINER signer key id on `envelope`, sorted and deduplicated (I-F item 1) --
/// used only to name a concrete key in the CLI's own trust note, never to decide trust itself
/// (that is [`verify_trusted_publication_envelope`]'s own job).
fn maintainer_signer_key_ids(envelope: &ObjectEnvelope) -> Vec<String> {
    let mut ids: Vec<String> = envelope
        .signatures
        .iter()
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
        .map(|signature| signature.key_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Summary of an RFC 155 `archive import` run: stages 0-3, the journal, and the commit point.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArchiveImportReport {
    /// Whether this run started a fresh journal (`false`) or resumed one an earlier, interrupted
    /// run left behind (`true`).
    pub resumed: bool,
    /// Objects actually, newly written in this run, per type -- an object already present
    /// locally, and left as it was (no signature to merge, or format 6), is not counted here.
    pub written_object_counts: Vec<(ObjectType, usize)>,
    /// Objects already present locally that gained at least one carried signature this run, per
    /// type (§9.7, RFC 156 §4's merge through `admit_carried_signatures`) -- format 7 only; always
    /// zero on a format-6 repository, which keeps one record per id and merges nothing.
    pub merged_object_counts: Vec<(ObjectType, usize)>,
    /// Every carried signature this run could not admit to a stored object, named (§9.7: "a
    /// dropped signature named").
    pub dropped_signatures: Vec<DroppedSignature>,
    /// Author keys recorded in this run (idempotent; a key already on file is not counted again).
    pub recorded_author_key_count: usize,
    /// Every ref the archive carries, and how it actually landed (stage 3).
    pub landing_outcomes: Vec<LandingOutcome>,
    /// §9.7: "re-importing the same archive reports 'already imported'" -- `true` when this run
    /// (resumed or fresh) found every object already present, merged nothing, recorded no new
    /// author key, and every landing already matched what this run would have done -- a
    /// zero-delta run, computed from this same report's own other fields rather than tracked
    /// separately, so it can never disagree with them. See this module's own doc note on why no
    /// new persistent "already imported" ledger exists.
    pub already_imported: bool,
}

/// `doctor`'s own `PRIKK-DOCTOR-INTERRUPTED-IMPORT` issue reads this: `Some(id)` when a journal is
/// present (an import started and did not reach its commit point), `None` otherwise. Never writes
/// anything, never acquires a lock -- a plain read, safe to call from `doctor`'s own read-only
/// pass.
pub fn interrupted_import_short_id(layout: &RepositoryLayout) -> Result<Option<String>> {
    crate::foundation::import_journal_marker::short_id_if_present(layout)
}

/// Run every stage over `source`: stage 0's checks, the journal, stage 1's objects in dependency
/// order (merging a carried signature into an id already held, through RFC 156 §4), stage 2's
/// author keys, and stage 3's ref landing (§9.6/§9.7). A refusal at stage 0 writes nothing; past
/// the journal, only I/O or a concurrent lock holder can stop this, never a content decision
/// (mirrors `import_bundle`'s own documented shape) -- except stage 3's own per-ref `--adopt`
/// decision, which is not a refusal at all: a ref that cannot be adopted still lands, under
/// `remotes/`, with the reason named (§9.6).
pub fn import_archive<R: Read + Seek>(
    layout: &RepositoryLayout,
    source: &mut R,
    options: &ArchiveImportOptions,
) -> Result<ArchiveImportReport> {
    // Stage 0, part 1: the archive's own verify, Part V's code, not a copy.
    let verify_report = super::verify_archive(source)?;
    if !verify_report.is_internally_consistent() {
        return Err(PrikkError::Integrity(format!(
            "archive is not internally consistent -- refusing the import; see `archive verify` \
             for the detail ({} unresolvable signature(s), {} damage line(s))",
            verify_report.unresolvable_signatures.len(),
            verify_report.damaged.len()
        )));
    }

    // Re-locate the manifest over the same, now-confirmed-consistent reader -- this module's own
    // second pass, not a second check (see this module's own doc comment).
    let (_total_len, manifest_offset, manifest_length, manifest_checksum) =
        read_end_trailer(source)?;
    let manifest = read_manifest(source, manifest_offset, manifest_length, manifest_checksum)?;
    let archive_identity = manifest_checksum;
    let author_material = read_author_material(source, &manifest)?;

    // RFC 155 §9.1: a conflict refuses the whole import before any write -- read-only, checked for
    // every carried (key_id, public_key) pair before stage 1 ever runs. One key_id binds to one
    // public key (DC-53), so checking every entry under a key_id (not just the first) catches a
    // conflict *within* the archive's own carried material too, not only against this repository.
    // RFC 155 Part I-B: also the cheapest place to learn whether *any* carried author key is new to
    // this repository -- needed only for `already_imported`'s own zero-delta computation, below;
    // `record_author_key_material` itself returns nothing a caller can use for that (it is a thin,
    // idempotent check-then-append wrapper around this same check).
    let mut any_new_author_key = false;
    for entries in author_material.values() {
        for entry in entries {
            if check_author_key_conflict(layout, &entry.key_id, entry.public_key)?
                == crate::author::author_key_index::AuthorKeyCheck::New
            {
                any_new_author_key = true;
            }
        }
    }

    let existing_journal = read_journal(layout)?;
    if let Some(other) = &existing_journal {
        if other.archive_identity != archive_identity {
            let id = journal_short_id(&other.archive_identity);
            return Err(PrikkError::LockConflict(format!(
                "a different archive import is already in progress (id {id}); run the same \
                 import again, or `prikk doctor --cancel-import {id}`"
            )));
        }
    }

    // All four container locks, held from here to the commit point, literally as §9.5 requires
    // (RFC 155 Part I-B2, review `rfc155-impl-IB-review-v1` finding IB1(a), closing the named
    // deviation Part I-B's own report flagged: the three ref-side locks used to be acquired later,
    // per call, by `publish`/`write_received_pointer` themselves).
    //
    // 0.56.0 step 3c2 (R1): acquired *before* a fresh start's own journal is ever created, not
    // after (3c's own, now-corrected shape) -- the fresh branch below classifies every planned
    // landing against the local store before writing anything at all, and that read needs the
    // identical race-free guarantee against a concurrent writer that every later read/write in
    // this function already has under this same guard. Unconditionally the "allowing" variant: a
    // fresh start has no journal yet (trivially passes either way); a resume's own journal is
    // already present, from this same import, and must not refuse itself.
    let _container_locks = acquire_container_locks_allowing_import_journal(
        layout,
        &[
            LockableContainer::ObjectStore,
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
        ],
    )?;
    let active_lock = ActiveLock::acquire_for_write_allowing_import_journal(
        layout,
        crate::foundation::layout::DEFAULT_ACTIVE_NAME,
    )?;
    // Moved up from immediately before stage 1 (0.56.0 step 3c2): the fresh branch below also
    // uses this as the local-side reader for its own early classification, under the same locks
    // just acquired -- one open, reused for both purposes, not two.
    let mut object_store = ObjectWriteSession::open(layout)?;

    let (journal, resumed) = match existing_journal {
        Some(journal) => (journal, true),
        None => {
            let landing = compute_landing_plan(source, &manifest)?;
            // 0.56.0 step 3c2 (R1): classify every planned landing against the local store
            // *before* the journal is ever created and before stage 1 writes a single object --
            // a refusal here leaves the repository exactly as it was found: no object, no
            // journal. See `classify_every_planned_landing_or_refuse`'s own doc comment for why
            // the delivered side must be read from the archive itself (stage 1 has not admitted
            // any of its objects yet) rather than through `classify_landing_connectivity`'s own
            // reader, which the unchanged, later stage-3 check below still uses (for the narrower
            // case of a pointer that moves between an interrupted run and its resume).
            classify_every_planned_landing_or_refuse(
                layout,
                source,
                &manifest,
                &landing,
                options,
                &object_store,
            )?;
            let containers = compute_container_starting_lengths(layout)?;
            let journal = ImportJournal {
                archive_identity,
                containers,
                landing,
            };
            write_journal(layout, &journal)?;
            // Test seam, unreachable from production: a kill right after the journal's own first
            // write, before any stage-1/2 byte -- the case table's own "kill right after the
            // journal" row.
            #[cfg(test)]
            if STOP_AFTER_JOURNAL_FOR_TEST.with(std::cell::Cell::take) {
                return Err(PrikkError::Integrity(
                    "import stopped for test: simulated kill right after the journal".to_string(),
                ));
            }
            (journal, false)
        }
    };

    // Stage 1: one object type at a time, in container (= dependency) order, one decoded object
    // at a time within a type -- never the whole archive's, or even a whole type's, objects in
    // memory together (RFC 155 Part P2b; see this module's own doc comment for why container
    // order needs no re-sort). RFC 155 Part I-B (§9.7, review finding IA1's own restriction
    // lifted): an id already present locally may now gain a carried signature through RFC 156 §4's
    // merge, the same way `import_bundle` and `patch_exchange::accept` already admit one -- via
    // `CarriedSignatureAdmitter::admit_one`, not a second copy of that rule.
    let mut object_types: Vec<ObjectType> = persisted_object_types().to_vec();
    object_types.sort_by_key(|object_type| object_type_dependency_rank(*object_type));
    let mut written_object_counts = Vec::with_capacity(object_types.len());
    let mut merged_object_counts = Vec::with_capacity(object_types.len());
    let mut dropped_signatures = Vec::new();
    // Test seams, unreachable from production: taken once, at the top, so a resumed call (which
    // starts this function fresh) does not re-trigger them -- the case table's own "kill
    // mid-objects" and "full disk" rows, which (unlike the two seams above) must fire *between*
    // two object writes, not only before or after the whole of stage 1.
    #[cfg(test)]
    let kill_mid_objects_after_n: Option<usize> =
        KILL_MID_OBJECTS_AFTER_N_FOR_TEST.with(std::cell::Cell::take);
    #[cfg(test)]
    let fail_mid_objects_after_n: Option<usize> =
        FAIL_MID_OBJECTS_AFTER_N_FOR_TEST.with(std::cell::Cell::take);
    #[cfg(test)]
    let mut total_objects_written_so_far = 0usize;
    // RFC 155 Part I-B2, review finding IB2: every kill in this module's own unit tests is a
    // seam that returns an `Err`, so `Drop` still runs and releases every lock normally -- a real
    // `SIGKILL` does not. This is the one seam that can actually be reached by a real, separately
    // compiled `prikk` process (gated by the `test-support` feature, not `#[cfg(test)]`, which
    // only ever compiles into `prikk-store`'s own unit-test binary and could never be reached from
    // a child process spawned by an integration test): with
    // `PRIKK_ARCHIVE_IMPORT_BLOCK_AFTER_N_FOR_TEST=<n>` set, it writes a marker file (named by
    // `PRIKK_ARCHIVE_IMPORT_BLOCKED_MARKER_FOR_TEST`) after the `n`th object write and then blocks
    // forever, so the test harness can wait for the marker, send a genuine `SIGKILL`, and inspect
    // exactly what a real interrupted process leaves behind -- stale lock files included, which no
    // `#[cfg(test)]` seam (whose `Err` always runs every `Drop`) could ever exercise.
    #[cfg(feature = "test-support")]
    let mut test_support_objects_written_so_far = 0usize;
    // RFC 155 Part P2b (review `release-0.51.0-prep-P2-review-v1`): stream within a type instead
    // of collecting its whole batch of envelopes first. `export_archive` writes each
    // `ObjectContainer` section as a verbatim stream of the live container file's own bytes
    // (`archive.rs`'s own `write_section`/`stream_copy`, never a re-encode), so the records
    // `for_each_object_record` decodes here arrive in exactly the container's own on-disk append
    // order. Because every object store in this codebase is write-once and content-addressed, a
    // same-kind dependency (a `Block`'s own parents, a `RefState`'s own `previous_ref_state_id` --
    // `same_kind_dependencies`' own two cases) can only ever be *appended* after the record that
    // will later reference it already exists on disk -- so container order is already a valid
    // topological order for same-kind references, and `objects_in_dependency_order`'s own re-sort
    // (which needed the whole type's envelopes collected first, precisely to sort them) is
    // unnecessary here. (This does **not** generalize to a *bundle*'s own carried order, which
    // `objects_in_dependency_order`'s own doc comment notes can list a child before its parent --
    // a bundle's envelope list is assembled by a different, non-append-order process; this
    // reasoning is specific to an archive's own verbatim container sections.) Each decoded record
    // is admitted through RFC 156 §4 and written immediately, one at a time, never collecting more
    // than one envelope's own memory at once; `admit_one`'s own repeated, incremental reads against
    // `object_store` (which this same loop is concurrently writing to) see a consistent answer
    // because every id within one archive's own single container is distinct from every other --
    // this run's own earlier writes for this type never change a later admission decision for a
    // different id. `object_store.check_write` is not called separately before the write any more
    // either: `write_object_under_held_lock` already re-validates via its own `decide_write` call,
    // so a second, separate pre-pass over the whole type was redundant work, not an extra
    // guarantee -- and a `check_write` failure partway through a type is exactly as resumable as
    // any other mid-type kill this module's own journal/cancel mechanism already handles.
    for object_type in object_types {
        let entry = find_entry(&manifest, ArchiveSectionKind::ObjectContainer(object_type))?;
        let admitter = CarriedSignatureAdmitter::new(layout)?;
        let mut written = 0usize;
        let mut type_merged_count = 0usize;
        for_each_object_record(source, entry, object_type, |envelope| {
            // RFC 156 §4 rules 1-3: for every carried object of this type this repository already
            // holds, which of its signatures may join the stored record -- verified here, before
            // this one object is written, the same rule `import_bundle` already follows, over one
            // envelope at a time instead of a whole pre-built batch (`CarriedSignatureAdmitter`'s
            // own doc comment). A format-6 repository's own store refuses a second envelope for an
            // id it holds instead of merging (`admit_one`'s own `!several_records` branch); nothing
            // here duplicates that rule.
            // 0.55.0 step 3 Part 3b-ii: `admit_one` now takes ownership (so its own two "nothing to
            // merge" branches can move rather than clone); this callback only ever holds a borrow
            // from `for_each_object_record`'s own record decode, so it clones here instead -- a
            // scoped cost for this caller, not `admit_one`'s.
            let admitted = admitter.admit_one(&object_store, envelope.clone(), |key_id| {
                let mut entries = lookup_author_key_entries(layout, key_id)?;
                if let Some(carried) = author_material.get(key_id) {
                    entries.extend(carried.iter().cloned());
                }
                Ok(entries)
            })?;
            if admitted.merged {
                type_merged_count = type_merged_count.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity("merged-object count overflow".to_string())
                })?;
            }
            dropped_signatures.extend(admitted.dropped);
            // 0.56.0 step 3c (A5): the identical operation-level decode `bundle import`'s own
            // admission already runs, before this (or any) object write -- see
            // `decode_patch_operations_for_admission`'s own doc comment for why this must be the
            // same function `archive verify` calls too, not a second copy that happens to call
            // the same lower-level decoder.
            if object_type == ObjectType::Patch {
                crate::patch_replay::decode::decode_patch_operations_for_admission(
                    &admitted.envelope.canonical_payload,
                    admitted.envelope.schema_version,
                )?;
            }
            let id = admitted.envelope.object_id();
            let already_present = object_store.contains_object(object_type, id)?;
            object_store.write_object_under_held_lock(&admitted.envelope, &_container_locks)?;
            if !already_present {
                written = written.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity(
                        "archive import written-object count overflow".to_string(),
                    )
                })?;
            }
            #[cfg(test)]
            if !already_present {
                total_objects_written_so_far = total_objects_written_so_far.saturating_add(1);
                if let Some(callback) = RUN_AFTER_FIRST_OBJECT_WRITE_FOR_TEST.with(|cell| {
                    if total_objects_written_so_far == 1 {
                        cell.borrow_mut().take()
                    } else {
                        None
                    }
                }) {
                    callback();
                }
                if kill_mid_objects_after_n == Some(total_objects_written_so_far) {
                    return Err(PrikkError::Integrity(
                        "import stopped for test: simulated kill mid-objects".to_string(),
                    ));
                }
                if fail_mid_objects_after_n == Some(total_objects_written_so_far) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::StorageFull,
                        "simulated full disk for test",
                    )
                    .into());
                }
            }
            #[cfg(feature = "test-support")]
            if !already_present {
                test_support_objects_written_so_far =
                    test_support_objects_written_so_far.saturating_add(1);
                block_after_n_objects_for_real_kill_test(test_support_objects_written_so_far);
            }
            Ok(())
        })?;
        written_object_counts.push((object_type, written));
        merged_object_counts.push((object_type, type_merged_count));
    }

    // Test seam, unreachable from production: a kill right after stage 1's own last write, before
    // stage 2 or the commit point -- the case table's own "kill after the objects" row.
    #[cfg(test)]
    if STOP_AFTER_STAGE_1_FOR_TEST.with(std::cell::Cell::take) {
        return Err(PrikkError::Integrity(
            "import stopped for test: simulated kill after stage 1's objects".to_string(),
        ));
    }

    // Stage 2: carried author keys, through the unchanged DC-78 path -- idempotent (matches
    // `import_bundle`'s own counting: every carried entry attempted, whether or not it was already
    // on file), so a resume simply re-confirms what an earlier attempt may have already recorded.
    let mut recorded_author_key_count = 0usize;
    for entries in author_material.values() {
        for entry in entries {
            record_author_key_material(layout, &entry.key_id, entry.public_key, &active_lock)?;
            recorded_author_key_count =
                recorded_author_key_count.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity(
                        "archive import recorded-author-key count overflow".to_string(),
                    )
                })?;
        }
    }

    // Test seam, unreachable from production: a kill right after stage 2's own last write, before
    // the commit point -- needed to test IA2 (the author-key container is journaled and cut back
    // on cancel): a kill before this point never calls `record_author_key_material` at all, so it
    // cannot exercise the cut-back the review's own finding is about.
    #[cfg(test)]
    if STOP_AFTER_STAGE_2_FOR_TEST.with(std::cell::Cell::take) {
        return Err(PrikkError::Integrity(
            "import stopped for test: simulated kill after stage 2's author keys".to_string(),
        ));
    }

    // Stage 3: land every ref the archive carries (§9.6/§9.7), under the same four container
    // locks held continuously since the journal (see this function's own doc note above) --
    // through `publish_with_object_store_under_held_lock`/`write_received_pointer_under_held_lock`,
    // never the ordinary per-call paths `branch create`/`seal`/`bundle import` use, which would
    // conflict with this function's own continuous hold.
    //
    // `--adopt`'s own chain replay needs the archive's carried ref log, decoded once (not per ref)
    // and only when there is a `LandingOrigin::Local` entry to even consider adopting -- a
    // received-only archive, or a run without `--adopt`, never touches it.
    let needs_ref_log = options.adopt
        && journal
            .landing
            .iter()
            .any(|landing| matches!(landing.origin, LandingOrigin::Local));
    let ref_log_records: Vec<crate::refs::RefLogRecord> = if needs_ref_log {
        let ref_log_entry = find_entry(&manifest, ArchiveSectionKind::RefLog)?;
        let ref_log_bytes = read_section_body_fully(source, ref_log_entry)?;
        let (records, damaged) = decode_ref_log_records_from_bytes(&ref_log_bytes)?;
        if damaged {
            return Err(PrikkError::Integrity(
                "archive's carried ref log decodes with interior damage -- refusing to adopt \
                 any ref from it; retry without --adopt to land every ref under remotes/ instead"
                    .to_string(),
            ));
        }
        records
    } else {
        Vec::new()
    };
    let ref_store = RefStore::new(layout.clone());
    let adopted_policy = load_maintainer_trust_policy_or_empty(layout)?;
    // Test seam, unreachable from production: run once, right before stage 3's own first landing,
    // while the four container locks above are genuinely held -- IB1(a)'s own test (a concurrent,
    // ordinary `publish` must refuse with `LockConflict`).
    #[cfg(test)]
    if let Some(callback) = RUN_BEFORE_STAGE_3_FOR_TEST.with(|cell| cell.borrow_mut().take()) {
        callback();
    }
    // External review 023: a received pointer can carry the exporter's own `remotes/<ref>` name
    // verbatim, if the source repository ever `bundle import`ed something for that same ref name.
    // Landing every entry in plan order, unconditionally, let whichever one ran last silently
    // overwrite the other -- the exporter's own real history could disappear behind a received
    // pointer. RFC 155 §9.6 (amended 2026-10-12) rules it: the exporter's own local ref wins the
    // name. Every `Local` landing runs first, so its own *actual* destination (the bare adopted
    // name, which never collides, or the `remotes/<ref>` fallback, which can) is known before any
    // `Received` landing is decided -- each outcome is written into its own planned index, so the
    // final order still matches `journal.landing`'s own order, regardless of which origin a given
    // index holds.
    #[cfg(test)]
    let collision_rule_disabled =
        DISABLE_LANDING_COLLISION_RULE_FOR_TEST.with(std::cell::Cell::take);
    #[cfg(not(test))]
    let collision_rule_disabled = false;

    let mut landing_outcomes: Vec<Option<LandingOutcome>> = vec![None; journal.landing.len()];
    let mut any_landing_changed = false;
    let mut local_destinations: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (index, landing) in journal.landing.iter().enumerate() {
        if landing.origin != LandingOrigin::Local {
            continue;
        }
        let (outcome, changed) = land_one_ref(
            layout,
            &ref_store,
            &adopted_policy,
            landing,
            options,
            &mut object_store,
            &ref_log_records,
            &_container_locks,
        )?;
        any_landing_changed = any_landing_changed || changed;
        local_destinations.insert(outcome.landed_as.clone());
        if let Some(slot) = landing_outcomes.get_mut(index) {
            *slot = Some(outcome);
        }
    }
    for (index, landing) in journal.landing.iter().enumerate() {
        if landing.origin != LandingOrigin::Received {
            continue;
        }
        if !collision_rule_disabled && local_destinations.contains(&landing.ref_name) {
            // The exporter's own local ref already landed at this exact name -- its objects were
            // already imported by stage 1 (unaffected by any stage-3 landing decision); only the
            // pointer write itself is skipped, and named, not silently dropped.
            //
            // I-F item 1: a collided (not-landed) entry's own trust still matters -- stage 1 wrote
            // its object regardless of stage 3's landing decision, so the destination's own
            // `verify` checks it too, even though no visible ref names it. Computed the same way
            // `land_one_ref` does, against this object store and the destination's own adopted
            // policy, not left as a hardcoded `false` that would silently hide an untrusted
            // collided pointer from the CLI's own trust note.
            let ref_state_envelope = object_store.read_object(landing.ref_state_id)?;
            let publication_trusted = match &ref_state_envelope {
                Some(envelope) => {
                    verify_trusted_publication_envelope(&adopted_policy, envelope).is_ok()
                }
                None => false,
            };
            let signing_key_ids = ref_state_envelope
                .as_ref()
                .map(maintainer_signer_key_ids)
                .unwrap_or_default();
            // 0.56.0 step 3c2 (R2): a collided entry's own class/chain against the *local*
            // destination (the exporter's own ref, which won the name) -- the same comparison
            // the exporter's own `Local` landing, processed just above in this same loop, would
            // itself classify against had it been a `Received` entry instead.
            let (class, ref_state_chain) =
                match crate::received::read_received_pointer(layout, &landing.ref_name)? {
                    None => (BundleConnectivityClass::NoLocalHistory, None),
                    Some(pointer) => {
                        let (class, chain) = classify_landing_connectivity(
                            &object_store,
                            landing.ref_state_id,
                            pointer.ref_state_id,
                        )?;
                        (class, Some(chain))
                    }
                };
            if let Some(slot) = landing_outcomes.get_mut(index) {
                *slot = Some(LandingOutcome {
                    ref_name: landing.ref_name.clone(),
                    landed_as: landing.ref_name.clone(),
                    adopted: false,
                    remote_reason: Some(
                        "collides with the exporter's own ref at this name; the exporter's ref \
                         wins"
                            .to_string(),
                    ),
                    landed: false,
                    collision_target: Some(landing.ref_state_id),
                    origin: landing.origin,
                    publication_trusted,
                    signing_key_ids,
                    class,
                    ref_state_chain,
                });
            }
            continue;
        }
        let (outcome, changed) = land_one_ref(
            layout,
            &ref_store,
            &adopted_policy,
            landing,
            options,
            &mut object_store,
            &ref_log_records,
            &_container_locks,
        )?;
        any_landing_changed = any_landing_changed || changed;
        if let Some(slot) = landing_outcomes.get_mut(index) {
            *slot = Some(outcome);
        }
    }
    let landing_outcomes: Vec<LandingOutcome> = landing_outcomes
        .into_iter()
        .map(|outcome| {
            outcome.ok_or_else(|| {
                PrikkError::Integrity(
                    "archive import: a planned landing's own origin matched neither Local nor \
                     Received -- refusing"
                        .to_string(),
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // The commit point: the journal is emptied, its name kept (RFC 168 §6's conservative choice on
    // Windows).
    empty_journal(layout)?;

    // §9.7: "re-importing the same archive reports 'already imported'" -- computed from this same
    // report's own other fields (see `ArchiveImportReport::already_imported`'s own doc note on why
    // no separate ledger exists), not before the fields themselves are final.
    let any_object_changed = written_object_counts.iter().any(|(_, count)| *count > 0)
        || merged_object_counts.iter().any(|(_, count)| *count > 0);
    let already_imported = !any_object_changed && !any_new_author_key && !any_landing_changed;

    Ok(ArchiveImportReport {
        resumed,
        written_object_counts,
        merged_object_counts,
        dropped_signatures,
        recorded_author_key_count,
        landing_outcomes,
        already_imported,
    })
}

/// One planned landing's real outcome at stage 3, and whether it actually changed anything (the
/// caller's own tally for [`ArchiveImportReport::already_imported`]).
#[allow(clippy::too_many_arguments)]
fn land_one_ref(
    layout: &RepositoryLayout,
    ref_store: &RefStore,
    adopted_policy: &crate::trust::MaintainerTrustPolicy,
    landing: &PlannedRefLanding,
    options: &ArchiveImportOptions,
    object_store: &mut ObjectWriteSession,
    ref_log_records: &[crate::refs::RefLogRecord],
    container_locks: &ContainerLockGuard,
) -> Result<(LandingOutcome, bool)> {
    // I-E item 4: whether this landing's own tip carries a maintainer signature the *destination*
    // already trusts -- independent of whether `--adopt` applied, computed once and reused for
    // every branch below, through the exact same `verify_trusted_publication_envelope` check
    // `archive verify` and `try_adopt_local_ref`'s own conditions 2/3 already use, against the
    // destination's own `adopted_policy` rather than the archive's own carried one.
    let ref_state_envelope = object_store.read_object(landing.ref_state_id)?;
    let publication_trusted = match &ref_state_envelope {
        Some(envelope) => verify_trusted_publication_envelope(adopted_policy, envelope).is_ok(),
        None => false,
    };
    // I-F item 1: the CLI's own trust note names a concrete signing key, not just "untrusted" --
    // `verify`'s own lines name only object ids, never a key id.
    let signing_key_ids = ref_state_envelope
        .as_ref()
        .map(maintainer_signer_key_ids)
        .unwrap_or_default();
    match landing.origin {
        // §9.6: the archive's own received pointers stay received, under their own
        // already-`remotes/`-shaped name -- never re-wrapped a second time (review
        // `rfc155-impl-IA-review-v1`, finding A3).
        LandingOrigin::Received => {
            let (changed, class, ref_state_chain) = land_as_received_pointer(
                layout,
                &landing.ref_name,
                landing.ref_state_id,
                object_store,
                options,
                container_locks,
            )?;
            Ok((
                LandingOutcome {
                    ref_name: landing.ref_name.clone(),
                    landed_as: landing.ref_name.clone(),
                    adopted: false,
                    remote_reason: None,
                    landed: true,
                    collision_target: None,
                    origin: landing.origin,
                    publication_trusted,
                    signing_key_ids: signing_key_ids.clone(),
                    class,
                    ref_state_chain,
                },
                changed,
            ))
        }
        LandingOrigin::Local if options.adopt => {
            match try_adopt_local_ref(
                layout,
                ref_store,
                adopted_policy,
                landing,
                options,
                object_store,
                ref_log_records,
                container_locks,
            )? {
                AdoptOutcome::Adopted {
                    remote_reason,
                    class,
                    chain,
                } => Ok((
                    LandingOutcome {
                        ref_name: landing.ref_name.clone(),
                        landed_as: landing.ref_name.clone(),
                        adopted: true,
                        remote_reason,
                        landed: true,
                        collision_target: None,
                        origin: landing.origin,
                        publication_trusted,
                        signing_key_ids: signing_key_ids.clone(),
                        class,
                        ref_state_chain: chain,
                    },
                    true,
                )),
                AdoptOutcome::FellThrough { reason } => {
                    let (changed, class, ref_state_chain) = land_as_received_pointer(
                        layout,
                        &landing.destination,
                        landing.ref_state_id,
                        object_store,
                        options,
                        container_locks,
                    )?;
                    Ok((
                        LandingOutcome {
                            ref_name: landing.ref_name.clone(),
                            landed_as: landing.destination.clone(),
                            adopted: false,
                            remote_reason: Some(reason),
                            landed: true,
                            collision_target: None,
                            origin: landing.origin,
                            publication_trusted,
                            signing_key_ids: signing_key_ids.clone(),
                            class,
                            ref_state_chain,
                        },
                        changed,
                    ))
                }
            }
        }
        LandingOrigin::Local => {
            let (changed, class, ref_state_chain) = land_as_received_pointer(
                layout,
                &landing.destination,
                landing.ref_state_id,
                object_store,
                options,
                container_locks,
            )?;
            Ok((
                LandingOutcome {
                    ref_name: landing.ref_name.clone(),
                    landed_as: landing.destination.clone(),
                    adopted: false,
                    remote_reason: None,
                    landed: true,
                    collision_target: None,
                    origin: landing.origin,
                    publication_trusted,
                    class,
                    ref_state_chain,
                    signing_key_ids,
                },
                changed,
            ))
        }
    }
}

/// Write (or idempotently re-write) a received pointer, returning whether the pointer's own
/// target actually changed -- `write_received_pointer` itself is unconditional (last-entry-wins,
/// no CAS: see its own doc note), so this reads the prior value first purely to report
/// [`ArchiveImportReport::already_imported`] accurately, and -- 0.56.0 step 3c (A2/A3) -- to
/// classify this landing against it before deciding whether to write at all.
///
/// **Refuses** a `rollback`, `diverged`, or `does-not-connect` landing against an existing
/// pointer of this name, unless `options.allow_non_fast_forward` is set, the identical rule 3a
/// built for `bundle import`. `object_store` resolves both sides: by the time stage 3 runs this
/// (after stage 1's own unconditional object writes), the delivered tip and the pointer it would
/// replace are both genuinely local (`classify_landing_connectivity`'s own doc comment) -- this
/// refusal is specifically about *this landing's own pointer write*, the one write it is still
/// possible to withhold at this point in the run; it is not a claim that nothing was written by
/// this import at all (stage 1's own object writes already happened, unconditionally, before any
/// landing is even planned to run -- see `import_archive`'s own doc comment on why that is this
/// module's own pre-existing, unchanged shape, not something this round restructures).
///
/// An `already-at-tip` landing with an `older` `RefState` chain writes nothing here either (A2):
/// the content is unchanged, and writing would move the pointer's own record backward.
///
/// `container_locks` must already hold `ReceivedIndex` -- checked, not assumed, by
/// [`crate::received::write_received_pointer_under_held_lock`].
/// Returns `(changed, class, chain)` -- 0.56.0 step 3c2 (R2): `class`/`chain` are returned, not
/// just acted on, so `land_one_ref` can carry them into [`LandingOutcome`] additively, the same
/// way `BundleImportReport` already carries `import_bundle`'s own classification.
fn land_as_received_pointer(
    layout: &RepositoryLayout,
    ref_name: &str,
    ref_state_id: ObjectId,
    object_store: &ObjectWriteSession,
    options: &ArchiveImportOptions,
    container_locks: &ContainerLockGuard,
) -> Result<(bool, BundleConnectivityClass, Option<RefStateChainRelation>)> {
    let before = crate::received::read_received_pointer(layout, ref_name)?;
    let (class, chain) = match &before {
        None => (BundleConnectivityClass::NoLocalHistory, None),
        Some(pointer) => {
            let (class, chain) =
                classify_landing_connectivity(object_store, ref_state_id, pointer.ref_state_id)?;
            (class, Some(chain))
        }
    };
    if before.is_some()
        && matches!(
            class,
            BundleConnectivityClass::Rollback
                | BundleConnectivityClass::Diverged
                | BundleConnectivityClass::DoesNotConnect
        )
        && !options.allow_non_fast_forward
    {
        return Err(PrikkError::BundleNonFastForward {
            command: "archive-import",
            class: class.as_str(),
            message: format!(
                "archive import: landing {ref_name} is a {} against its current received \
                 pointer -- refusing rather than silently rewriting received history; pass \
                 --allow-non-fast-forward to accept a landing that does not simply extend the \
                 pointer it would replace",
                class.as_str()
            ),
        });
    }
    let pointer_updated = !matches!(
        (class, chain),
        (
            BundleConnectivityClass::AlreadyAtTip,
            Some(RefStateChainRelation::Older)
        )
    );
    if pointer_updated {
        crate::received::write_received_pointer_under_held_lock(
            layout,
            ref_name,
            ref_state_id,
            container_locks,
        )?;
    }
    let changed = before.map(|pointer| pointer.ref_state_id) != Some(ref_state_id);
    Ok((changed, class, chain))
}

enum AdoptOutcome {
    /// 0.56.0 step 3c (A3): `remote_reason` is `Some` only when a received pointer already held
    /// at this entry's own destination classified as `rollback`/`diverged`/`does-not-connect`
    /// against the delivered tip, and `--allow-non-fast-forward` let adoption proceed despite it
    /// -- see [`LandingOutcome::remote_reason`]'s own doc comment for why the ordinary,
    /// fast-forward/already-at-tip case still carries `None` here.
    ///
    /// 0.56.0 step 3c2 (R2): `class`/`chain` are the classification against the received pointer
    /// already held at this entry's own destination, if one exists (`NoLocalHistory`/`None`
    /// otherwise) -- carried into [`LandingOutcome`] the same way `land_as_received_pointer`'s own
    /// return value is.
    Adopted {
        remote_reason: Option<String>,
        class: BundleConnectivityClass,
        chain: Option<RefStateChainRelation>,
    },
    FellThrough {
        reason: String,
    },
}

/// §9.6's narrowed `--adopt`: land `landing` (a [`LandingOrigin::Local`] ref) as a real local ref
/// only when every one of the four conditions holds; otherwise names why it falls through to the
/// default `remotes/` landing instead, writing nothing here either way until every condition is
/// confirmed.
///
/// **The chain, not only the tip.** A ref's current state may be several updates past its own
/// genesis (more than one `seal` advanced it before this export) -- [`RefStore::publish`]'s own
/// CAS only accepts a `RefState` whose `previous_ref_state_id` matches what the caller expects,
/// so landing the tip alone into an absent ref would be refused by that same CAS (the tip's own
/// `previous_ref_state_id` is not `None`, but a ref genuinely absent locally has no current state
/// to match it). The whole chain, from genesis to tip, is replayed instead, each step through the
/// same already-signed `(RefState, RefUpdate)` pair the archive itself carries -- this never
/// re-signs or re-derives anything, only replays what the archive's own maintainer already
/// produced. Idempotent on retry: a step already landed reads as [`PublicationState::Complete`]
/// inside `publish` itself (unchanged, pre-existing behavior this function does not touch).
#[allow(clippy::too_many_arguments)]
fn try_adopt_local_ref(
    layout: &RepositoryLayout,
    ref_store: &RefStore,
    adopted_policy: &crate::trust::MaintainerTrustPolicy,
    landing: &PlannedRefLanding,
    options: &ArchiveImportOptions,
    object_store: &mut ObjectWriteSession,
    ref_log_records: &[crate::refs::RefLogRecord],
    container_locks: &ContainerLockGuard,
) -> Result<AdoptOutcome> {
    // Condition 1, checked first and cheaply (a second, atomic check happens inside `publish`
    // itself, under the ref lock, for the chain's own first step -- this is the lock-free,
    // fail-fast read, not the enforcement). While this import's own journal is present, every
    // other session's writer refuses too (RFC 155 Part I-A2), so nothing else can create this ref
    // between this read and the first `publish` call below.
    if ref_store
        .read_current_ref_state_id(&landing.ref_name)?
        .is_some()
    {
        return Ok(AdoptOutcome::FellThrough {
            reason: "a ref by this name already exists locally; adopting onto an existing ref \
                     is RFC 154's"
                .to_string(),
        });
    }

    // 0.56.0 step 3c (A3): `try_adopt_local_ref` used to read nothing from the received index at
    // all (2A v2's own measured R1 finding) -- classify the archive's own delivered tip against
    // the received pointer already held at this entry's own destination (`remotes/<ref_name>`,
    // the name it would land under absent adoption), if one exists. A `rollback`/`diverged`/
    // `does-not-connect` landing against it refuses, under the same `--allow-non-fast-forward`
    // flag 3a built for `bundle import` and this round extended to received landings
    // (`land_as_received_pointer`, above) -- adopting a real local ref must not silently erase
    // what this repository had already received at that name.
    let received_at_destination =
        crate::received::read_received_pointer(layout, &landing.destination)?;
    let mut adopt_remote_reason: Option<String> = None;
    let mut adopt_class = BundleConnectivityClass::NoLocalHistory;
    let mut adopt_chain: Option<RefStateChainRelation> = None;
    if let Some(pointer) = &received_at_destination {
        let (class, destination_chain) = classify_landing_connectivity(
            &*object_store,
            landing.ref_state_id,
            pointer.ref_state_id,
        )?;
        adopt_class = class;
        adopt_chain = Some(destination_chain);
        if matches!(
            class,
            BundleConnectivityClass::Rollback
                | BundleConnectivityClass::Diverged
                | BundleConnectivityClass::DoesNotConnect
        ) {
            if !options.allow_non_fast_forward {
                return Err(PrikkError::BundleNonFastForward {
                    command: "archive-import",
                    class: class.as_str(),
                    message: format!(
                        "archive import --adopt: adopting {} would be a {} against the received \
                         pointer already held at {} -- refusing rather than silently rewriting \
                         received history; pass --allow-non-fast-forward to adopt it anyway",
                        landing.ref_name,
                        class.as_str(),
                        landing.destination
                    ),
                });
            }
            // A genuine fact worth naming even though adoption proceeds (A3: "the class in
            // remote_reason") -- never set for the ordinary fast-forward/already-at-tip case,
            // see `LandingOutcome::remote_reason`'s own doc comment.
            adopt_remote_reason = Some(format!(
                "adopted despite a {} against the received pointer already held at {} \
                 (--allow-non-fast-forward)",
                class.as_str(),
                landing.destination
            ));
        }
    }

    // This ref's own subsequence, in order -- filtered from the archive's whole, interleaved
    // carried ref log (decoded once by the caller, not per ref).
    let mut chain: Vec<(RefUpdatePayload, ObjectEnvelope)> = Vec::new();
    for record in ref_log_records {
        let update = RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
        if update.ref_name == landing.ref_name {
            chain.push((update, record.envelope.clone()));
        }
    }
    chain.sort_by_key(|(update, _)| update.update_seq);
    if chain.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "archive's carried ref log has no record for {}, but its resolved ref-pointer-index \
             names it as a live local ref -- refusing to adopt it",
            landing.ref_name
        )));
    }

    // Conditions 2 and 3, together: every step's `RefUpdate` must carry a MAINTAINER signature by
    // a key this repository has adopted, and that signature must verify (`verify_trusted_
    // publication_envelope` is exactly this combined check -- see its own doc comment). Checked
    // for the *whole* chain before any step is published, so a mid-chain trust failure never
    // leaves a partially-adopted ref behind.
    for (_, ref_update_envelope) in &chain {
        if let Err(issue) = verify_trusted_publication_envelope(adopted_policy, ref_update_envelope)
        {
            return Ok(AdoptOutcome::FellThrough {
                reason: issue.message,
            });
        }
    }

    // Condition 4 (CAS against absent) for the first step, and the chain's own internal CAS for
    // every step after it -- both enforced by `publish_with_object_store_under_held_lock` itself,
    // never re-implemented here.
    let mut expected_previous: Option<ObjectId> = None;
    for (update, ref_update_envelope) in &chain {
        let ref_state_envelope = object_store
            .read_object(update.new_ref_state_id)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "archive's carried ref log names RefState {} for {}, but stage 1 never wrote \
                     it -- refusing to adopt",
                    update.new_ref_state_id, landing.ref_name
                ))
            })?;
        ref_store.publish_with_object_store_under_held_lock(
            object_store,
            &RefPublication {
                ref_name: landing.ref_name.clone(),
                expected_previous_ref_state_id: expected_previous,
                ref_state: ref_state_envelope,
                ref_update: ref_update_envelope.clone(),
            },
            container_locks,
        )?;
        expected_previous = Some(update.new_ref_state_id);
    }
    Ok(AdoptOutcome::Adopted {
        remote_reason: adopt_remote_reason,
        class: adopt_class,
        chain: adopt_chain,
    })
}

/// 0.56.0 step 3c2 (R1): the archive's own carried `Block`/`RefState`/`Tag` ancestry and chain
/// data, scanned once directly from the archive -- stage 1 has not admitted any of the archive's
/// own objects into the local store yet at the point this is built, so this is the archive-side
/// counterpart of `bundle::import_bundle`'s own pass-1 `closure_by_id` metadata map, built the
/// same way (one streamed decode of each record, only the few fields each type needs for
/// connectivity, nothing held past that).
struct ArchiveRefGraph {
    parent_block_ids: BTreeMap<ObjectId, Vec<ObjectId>>,
    ref_state_previous: BTreeMap<ObjectId, Option<ObjectId>>,
    ref_state_target: BTreeMap<ObjectId, (RefKind, ObjectId)>,
    tag_target_block: BTreeMap<ObjectId, ObjectId>,
}

fn build_archive_ref_graph<R: Read + Seek>(
    source: &mut R,
    manifest: &super::verify::ManifestView,
) -> Result<ArchiveRefGraph> {
    let mut graph = ArchiveRefGraph {
        parent_block_ids: BTreeMap::new(),
        ref_state_previous: BTreeMap::new(),
        ref_state_target: BTreeMap::new(),
        tag_target_block: BTreeMap::new(),
    };
    let block_entry = find_entry(
        manifest,
        ArchiveSectionKind::ObjectContainer(ObjectType::Block),
    )?;
    for_each_object_record(source, block_entry, ObjectType::Block, |envelope| {
        let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
        graph
            .parent_block_ids
            .insert(envelope.object_id(), payload.parent_block_ids);
        Ok(())
    })?;
    let ref_state_entry = find_entry(
        manifest,
        ArchiveSectionKind::ObjectContainer(ObjectType::RefState),
    )?;
    for_each_object_record(source, ref_state_entry, ObjectType::RefState, |envelope| {
        let payload = RefStatePayload::decode_canonical(
            &envelope.canonical_payload,
            envelope.schema_version,
        )?;
        let id = envelope.object_id();
        graph
            .ref_state_previous
            .insert(id, payload.previous_ref_state_id);
        graph
            .ref_state_target
            .insert(id, (payload.kind, payload.target_object_id));
        Ok(())
    })?;
    let tag_entry = find_entry(
        manifest,
        ArchiveSectionKind::ObjectContainer(ObjectType::Tag),
    )?;
    for_each_object_record(source, tag_entry, ObjectType::Tag, |envelope| {
        let payload = TagPayload::decode_canonical(&envelope.canonical_payload)?;
        graph
            .tag_target_block
            .insert(envelope.object_id(), payload.target_block_id);
        Ok(())
    })?;
    Ok(graph)
}

/// Resolve `ref_state_id`'s own target block, through the archive's own carried `Tag` if its kind
/// is `RefKind::Tag` -- the archive-side counterpart of `crate::refs::resolve_ref_tip_block`, read
/// from `graph` instead of an `ObjectReader` (nothing here is locally admitted yet).
fn archive_resolve_tip_block(graph: &ArchiveRefGraph, ref_state_id: ObjectId) -> Result<ObjectId> {
    let (kind, target) = graph
        .ref_state_target
        .get(&ref_state_id)
        .copied()
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "archive import: RefState {ref_state_id} is not among this archive's own carried \
             objects"
            ))
        })?;
    match kind {
        RefKind::Branch => Ok(target),
        RefKind::Tag => graph.tag_target_block.get(&target).copied().ok_or_else(|| {
            PrikkError::Integrity(format!(
                "archive import: Tag {target} is not among this archive's own carried objects"
            ))
        }),
    }
}

/// Every ancestor block id reachable from `target` (`target` included), walked purely through
/// `graph` -- the archive-side counterpart of `crate::merge::evidence::ancestors_inclusive`, over
/// the archive's own carried `Block` records rather than ones already in the local store. Sound
/// only because stage 0's own `verify_archive` has already confirmed this archive's reference
/// closure is sound before this is ever called.
fn archive_block_ancestor_ids(
    graph: &ArchiveRefGraph,
    target: ObjectId,
) -> Result<BTreeSet<ObjectId>> {
    let mut seen: BTreeSet<ObjectId> = BTreeSet::new();
    let mut stack = vec![target];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let parents = graph.parent_block_ids.get(&id).ok_or_else(|| {
            PrikkError::Integrity(format!(
                "archive import: block {id} is not among this archive's own carried objects"
            ))
        })?;
        stack.extend(parents.iter().copied());
    }
    Ok(seen)
}

/// Does `start`'s own `previous_ref_state_id` chain, walked purely through `graph`, contain
/// `target`? `target` is only ever compared by id, never dereferenced, so it need not itself be
/// one of the archive's own carried `RefState`s (it is ordinarily the *local* prior pointer's own
/// id) -- the archive-side counterpart of `bundle::ref_state_chain_contains`.
fn archive_ref_state_chain_contains(
    graph: &ArchiveRefGraph,
    start: ObjectId,
    target: ObjectId,
) -> Result<bool> {
    let mut current = Some(start);
    let mut seen: BTreeSet<ObjectId> = BTreeSet::new();
    while let Some(id) = current {
        if id == target {
            return Ok(true);
        }
        if !seen.insert(id) {
            return Err(PrikkError::Integrity(format!(
                "archive import: RefState chain contains a cycle at {id}"
            )));
        }
        current = *graph.ref_state_previous.get(&id).ok_or_else(|| {
            PrikkError::Integrity(format!(
                "archive import: RefState {id} is not among this archive's own carried objects"
            ))
        })?;
    }
    Ok(false)
}

/// Classify one planned landing's delivered tip (read from `graph`, the archive itself) against
/// a local `RefState` id (read from `local_reader`, the local store) -- the one classification
/// `classify_every_planned_landing_or_refuse` calls, before stage 1 ever runs.
fn classify_archive_landing_against_local(
    graph: &ArchiveRefGraph,
    local_reader: &impl ObjectReader,
    delivered_ref_state_id: ObjectId,
    prior_ref_state_id: ObjectId,
) -> Result<(BundleConnectivityClass, RefStateChainRelation)> {
    let delivered_target = archive_resolve_tip_block(graph, delivered_ref_state_id)?;
    let delivered_ancestors = archive_block_ancestor_ids(graph, delivered_target)?;

    let prior_envelope = local_reader
        .read_object(prior_ref_state_id)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "received pointer names missing RefState {prior_ref_state_id}"
            ))
        })?;
    let prior_payload = RefStatePayload::decode_canonical(
        &prior_envelope.canonical_payload,
        prior_envelope.schema_version,
    )?;
    let (prior_target, _tag_envelope) =
        crate::refs::resolve_ref_tip_block(local_reader, &prior_payload)?;
    let local_ancestors: BTreeSet<ObjectId> =
        crate::merge::evidence::ancestors_inclusive(local_reader, prior_target)?
            .keys()
            .copied()
            .collect();

    let delivered_chain_contains_local =
        archive_ref_state_chain_contains(graph, delivered_ref_state_id, prior_ref_state_id)?;
    let local_chain_contains_delivered =
        ref_state_chain_contains(local_reader, prior_ref_state_id, delivered_ref_state_id)?;

    Ok(classify_connectivity_from_sets(
        &local_ancestors,
        &delivered_ancestors,
        prior_target,
        delivered_target,
        delivered_chain_contains_local,
        local_chain_contains_delivered,
    ))
}

/// 0.56.0 step 3c2 (R1): classify every planned landing against the local store, before stage 1
/// ever writes an object and before the journal that would let a resume start partway through
/// this import is ever created -- a refusal here writes nothing at all: no object, no journal,
/// no half-landed pointer. For each landing, the *prior* pointer this landing would replace is
/// read under the exact same name `land_as_received_pointer`/`try_adopt_local_ref` compare against
/// at stage 3 (a `LandingOrigin::Received` entry's own `ref_name`, already `remotes/`-shaped; a
/// `LandingOrigin::Local` entry's own `destination`, `remotes/<ref_name>` -- the same name whether
/// or not `--adopt` ultimately applies, since `try_adopt_local_ref`'s own fall-through lands at
/// that identical destination too). The stage-3 check inside `land_as_received_pointer` and
/// `try_adopt_local_ref` is kept, unchanged, for the one case this early check cannot cover: a
/// resumed import, whose journal (and therefore whose landing plan) was fixed by an earlier,
/// interrupted run, where the local pointer may have moved again in the gap before the resume.
fn classify_every_planned_landing_or_refuse<R: Read + Seek>(
    layout: &RepositoryLayout,
    source: &mut R,
    manifest: &super::verify::ManifestView,
    landings: &[PlannedRefLanding],
    options: &ArchiveImportOptions,
    local_reader: &impl ObjectReader,
) -> Result<()> {
    if landings.is_empty() {
        return Ok(());
    }
    let graph = build_archive_ref_graph(source, manifest)?;
    for landing in landings {
        let prior_name: &str = match landing.origin {
            LandingOrigin::Received => &landing.ref_name,
            LandingOrigin::Local => &landing.destination,
        };
        let Some(prior_pointer) = crate::received::read_received_pointer(layout, prior_name)?
        else {
            continue;
        };
        let (class, _chain) = classify_archive_landing_against_local(
            &graph,
            local_reader,
            landing.ref_state_id,
            prior_pointer.ref_state_id,
        )?;
        if matches!(
            class,
            BundleConnectivityClass::Rollback
                | BundleConnectivityClass::Diverged
                | BundleConnectivityClass::DoesNotConnect
        ) && !options.allow_non_fast_forward
        {
            return Err(PrikkError::BundleNonFastForward {
                command: "archive-import",
                class: class.as_str(),
                message: format!(
                    "archive import: landing {} is a {} against its current received pointer \
                     -- refusing before writing anything for this import at all; pass \
                     --allow-non-fast-forward to accept a landing that does not simply extend \
                     the pointer it would replace",
                    landing.ref_name,
                    class.as_str()
                ),
            });
        }
    }
    Ok(())
}

fn compute_landing_plan(
    source: &mut (impl Read + Seek),
    manifest: &super::verify::ManifestView,
) -> Result<Vec<PlannedRefLanding>> {
    let mut landings = Vec::new();
    let pointer_entry = find_entry(manifest, ArchiveSectionKind::RefPointerIndex)?;
    let pointer_bytes = read_section_body_fully(source, pointer_entry)?;
    let decoded_pointers = decode_pointer_index_entries_for_resolver(&pointer_bytes)?;
    if decoded_pointers.damaged {
        return Err(PrikkError::Integrity(
            "archive's resolved ref-pointer-index section decodes with interior damage -- \
             refusing the import"
                .to_string(),
        ));
    }
    // External review 023: the exporter's own live container is append-only, so a ref that was
    // advanced more than once carries one entry per advance here, not one per ref -- the same
    // superseded-entry shape `reduce_pointer_index_entries` already exists to fold away for the
    // real compactor (last entry per `ref_name_key` survives). Reused verbatim, not re-derived,
    // so a ref is planned once, from its own latest entry (RFC 155 §9.6, amended 2026-10-12).
    for entry in reduce_pointer_index_entries(decoded_pointers.entries) {
        landings.push(PlannedRefLanding {
            origin: LandingOrigin::Local,
            destination: format!("remotes/{}", entry.ref_name),
            ref_state_id: entry.ref_state_id,
            ref_name: entry.ref_name,
        });
    }

    let received_entry = find_entry(manifest, ArchiveSectionKind::ReceivedIndex)?;
    let received_bytes = read_section_body_fully(source, received_entry)?;
    let decoded_received = decode_received_index_entries_for_resolver(&received_bytes)?;
    if decoded_received.damaged {
        return Err(PrikkError::Integrity(
            "archive's resolved received-index section decodes with interior damage -- refusing \
             the import"
                .to_string(),
        ));
    }
    // Same reasoning as the pointer index, just above: one entry per ref, from its own latest.
    for entry in reduce_received_index_entries(decoded_received.entries) {
        // RFC 155 Part I-B (review `rfc155-impl-IA-review-v1`, finding A3): a received-index
        // entry's own `ref_name` is already `remotes/`-shaped (`received.rs`'s own
        // `ReceivedPointer` doc: "Local logical name, always `remotes/<origin ref name>`") --
        // landed verbatim, under its own name, never wrapped in a second `remotes/` prefix the
        // way Part I-A's own first cut did.
        landings.push(PlannedRefLanding {
            origin: LandingOrigin::Received,
            destination: entry.ref_name.clone(),
            ref_state_id: entry.ref_state_id,
            ref_name: entry.ref_name,
        });
    }
    Ok(landings)
}

fn relative_string(layout: &RepositoryLayout, path: &Path) -> Result<String> {
    Ok(layout
        .repository_relative(path)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn compute_container_starting_lengths(
    layout: &RepositoryLayout,
) -> Result<Vec<JournaledContainer>> {
    let mut containers = Vec::new();
    for object_type in persisted_object_types() {
        let path = layout.container_slot_path(object_type, ContainerSlot::A);
        containers.push(JournaledContainer {
            label: format!("{object_type} container"),
            relative_path: relative_string(layout, &path)?,
            starting_length: super::stat_len(&path)?,
            kind: JournaledContainerKind::ObjectStore,
        });
    }
    // `LockableContainer::ObjectStore` is every object container *and* the single shared
    // `index.container` beside them, taken together as one resource (`layout.rs`'s own doc note).
    // A write through the ordinary path appends to both; cancel must cut both back, or the index
    // keeps naming an object whose own container bytes were just truncated away.
    let index_path = layout.container_index_path();
    containers.push(JournaledContainer {
        label: "object index".to_string(),
        relative_path: relative_string(layout, &index_path)?,
        starting_length: super::stat_len(&index_path)?,
        kind: JournaledContainerKind::ObjectStore,
    });
    let ref_log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    containers.push(JournaledContainer {
        label: "ref log".to_string(),
        relative_path: relative_string(layout, &ref_log_path)?,
        starting_length: super::stat_len(&ref_log_path)?,
        kind: JournaledContainerKind::RefLog,
    });
    let (pointer_path, _) = super::resolve_pointer_index_source(layout)?;
    containers.push(JournaledContainer {
        label: "ref pointer index".to_string(),
        relative_path: relative_string(layout, &pointer_path)?,
        starting_length: super::stat_len(&pointer_path)?,
        kind: JournaledContainerKind::RefPointerIndex,
    });
    let (received_path, _) = super::resolve_received_index_source(layout)?;
    containers.push(JournaledContainer {
        label: "received index".to_string(),
        relative_path: relative_string(layout, &received_path)?,
        starting_length: super::stat_len(&received_path)?,
        kind: JournaledContainerKind::ReceivedIndex,
    });
    // RFC 155 Part I-A2 (review finding IA2): journaled too, now that §9.5 is corrected to list
    // it. The file may not exist yet on a repository that has never authored anything --
    // `stat_len` already reads that as length 0, matching `ensure_author_key_container_exists`'s
    // own lazy-create-on-first-write behavior, so a cancel's truncate-back-to-0 is exactly right
    // either way.
    // RFC 155 Part I-A2 (review finding IA2): journaled too, now that §9.5 is corrected to list
    // it. The file may not exist yet on a repository that has never authored anything --
    // `stat_len` already reads that as length 0, matching `ensure_author_key_container_exists`'s
    // own lazy-create-on-first-write behavior, so a cancel's truncate-back-to-0 is exactly right
    // either way.
    let author_key_path = layout.author_key_container_path();
    containers.push(JournaledContainer {
        label: "author key container".to_string(),
        relative_path: relative_string(layout, &author_key_path)?,
        starting_length: super::stat_len(&author_key_path)?,
        kind: JournaledContainerKind::AuthorKey,
    });
    Ok(containers)
}

fn encode_journal(journal: &ImportJournal) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    body.extend_from_slice(JOURNAL_MAGIC);
    body.extend_from_slice(&JOURNAL_VERSION.to_be_bytes());
    body.extend_from_slice(&journal.archive_identity);
    let container_count = u16::try_from(journal.containers.len()).map_err(|_| {
        PrikkError::Integrity("import journal container count overflow".to_string())
    })?;
    body.extend_from_slice(&container_count.to_be_bytes());
    for container in &journal.containers {
        push_string(&mut body, &container.label)?;
        push_string(&mut body, &container.relative_path)?;
        body.extend_from_slice(&container.starting_length.to_be_bytes());
        body.push(container.kind.to_wire());
    }
    let landing_count = u32::try_from(journal.landing.len())
        .map_err(|_| PrikkError::Integrity("import journal landing count overflow".to_string()))?;
    body.extend_from_slice(&landing_count.to_be_bytes());
    for landing in &journal.landing {
        body.push(landing.origin.to_wire());
        push_string(&mut body, &landing.ref_name)?;
        body.extend_from_slice(landing.ref_state_id.as_bytes());
        push_string(&mut body, &landing.destination)?;
    }
    let checksum = prikk_hash::sha256(&body);
    body.extend_from_slice(&checksum);
    Ok(body)
}

fn push_string(body: &mut Vec<u8>, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let len = u16::try_from(bytes.len())
        .map_err(|_| PrikkError::Integrity("import journal string too long".to_string()))?;
    body.extend_from_slice(&len.to_be_bytes());
    body.extend_from_slice(bytes);
    Ok(())
}

struct ByteReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| PrikkError::Integrity("import journal offset overflow".to_string()))?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| PrikkError::Integrity("import journal is truncated".to_string()))?;
        self.offset = end;
        Ok(slice)
    }

    fn take_u8(&mut self) -> Result<u8> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| PrikkError::Integrity("import journal is truncated".to_string()))
    }

    fn take_u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().unwrap_or([0u8; 2]),
        ))
    }

    fn take_u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().unwrap_or([0u8; 4]),
        ))
    }

    fn take_u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().unwrap_or([0u8; 8]),
        ))
    }

    fn take_string(&mut self) -> Result<String> {
        let len = usize::from(self.take_u16()?);
        if len > MAX_JOURNAL_STRING_BYTES {
            return Err(PrikkError::ObjectOverBound {
                declared_bytes: len as u64,
                bound_bytes: MAX_JOURNAL_STRING_BYTES as u64,
            });
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| {
            PrikkError::Integrity("import journal string is not valid UTF-8".to_string())
        })
    }
}

fn decode_journal(bytes: &[u8]) -> Result<ImportJournal> {
    if bytes.len() < 8 + 2 + 32 + 2 + 32 {
        return Err(PrikkError::Integrity(
            "import journal is too short to contain its own header and checksum".to_string(),
        ));
    }
    let checksum_offset = bytes.len() - 32;
    let (Some(body), Some(declared_checksum)) =
        (bytes.get(..checksum_offset), bytes.get(checksum_offset..))
    else {
        return Err(PrikkError::Integrity(
            "import journal checksum offset overflow".to_string(),
        ));
    };
    let actual_checksum = prikk_hash::sha256(body);
    if actual_checksum != declared_checksum {
        return Err(PrikkError::Integrity(
            "import journal failed its own checksum -- refusing to trust it; preserve the \
             repository and inspect it"
                .to_string(),
        ));
    }

    let mut reader = ByteReader::new(body);
    let magic = reader.take(8)?;
    if magic != JOURNAL_MAGIC {
        return Err(PrikkError::Integrity(
            "import journal has the wrong magic".to_string(),
        ));
    }
    let version = reader.take_u16()?;
    if version != JOURNAL_VERSION {
        return Err(PrikkError::Integrity(format!(
            "import journal version {version} is not one this build knows how to read"
        )));
    }
    let archive_identity: [u8; 32] = reader.take(32)?.try_into().unwrap_or([0u8; 32]);
    let container_count = usize::from(reader.take_u16()?);
    if container_count > MAX_JOURNAL_CONTAINER_COUNT {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes: container_count as u64,
            bound_bytes: MAX_JOURNAL_CONTAINER_COUNT as u64,
        });
    }
    let mut containers = Vec::with_capacity(container_count);
    for _ in 0..container_count {
        let label = reader.take_string()?;
        let relative_path = reader.take_string()?;
        let starting_length = reader.take_u64()?;
        let kind = JournaledContainerKind::from_wire(reader.take_u8()?)?;
        containers.push(JournaledContainer {
            label,
            relative_path,
            starting_length,
            kind,
        });
    }
    let landing_count = reader.take_u32()? as usize;
    if landing_count > MAX_JOURNAL_LANDING_COUNT {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes: landing_count as u64,
            bound_bytes: MAX_JOURNAL_LANDING_COUNT as u64,
        });
    }
    let mut landing = Vec::with_capacity(landing_count);
    for _ in 0..landing_count {
        let origin = LandingOrigin::from_wire(reader.take_u8()?)?;
        let ref_name = reader.take_string()?;
        let ref_state_id_bytes: [u8; 32] = reader.take(32)?.try_into().unwrap_or([0u8; 32]);
        let destination = reader.take_string()?;
        landing.push(PlannedRefLanding {
            origin,
            ref_name,
            ref_state_id: ObjectId::from_bytes(ref_state_id_bytes),
            destination,
        });
    }
    Ok(ImportJournal {
        archive_identity,
        containers,
        landing,
    })
}

fn read_journal(layout: &RepositoryLayout) -> Result<Option<ImportJournal>> {
    let root = layout.repository_mutation_root();
    let relative = layout.repository_relative(&layout.import_journal_path())?;
    let Some(bytes) = read_file_if_exists(root, &relative)? else {
        return Ok(None);
    };
    if bytes.is_empty() {
        // The commit point and cancel both empty the journal, keeping its name (RFC 168 §6) --
        // an empty file reads identically to "no import in progress".
        return Ok(None);
    }
    Ok(Some(decode_journal(&bytes)?))
}

fn write_journal(layout: &RepositoryLayout, journal: &ImportJournal) -> Result<()> {
    let root = layout.repository_mutation_root();
    let dir_relative = layout.repository_relative(&layout.prikk_dir().join("import"))?;
    ensure_directory_required(root, &dir_relative)?;
    let relative = layout.repository_relative(&layout.import_journal_path())?;
    let bytes = encode_journal(journal)?;
    // The commit point and `cancel_import` both empty the journal rather than removing it (RFC
    // 168 §6's conservative choice on Windows) -- `read_journal` already treats an empty file
    // identically to an absent one, so by the time this function is reached (only from the `None`
    // match arm above, where `read_journal` just confirmed exactly that), any file here is either
    // absent or empty. Removing it first, best-effort, before the real exclusive create: a crash
    // between the two leaves "no file", which reads the same as "empty file" either way.
    remove_file_if_present_required(root, &relative)?;
    create_new_file_required(root, &relative, &bytes).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            PrikkError::LockConflict(
                "an import journal already exists -- a concurrent import is starting, or one \
                 crashed between creating it and this check; run `prikk doctor` to see which"
                    .to_string(),
            )
        } else {
            PrikkError::from(err)
        }
    })
}

fn empty_journal(layout: &RepositoryLayout) -> Result<()> {
    let root = layout.repository_mutation_root();
    let relative = layout.repository_relative(&layout.import_journal_path())?;
    truncate_existing_file_required(root, &relative, 0)
}

/// `prikk doctor --cancel-import <id> [--plan-only]` (§9.5): for every container the journal
/// names, save the bytes past its journaled starting length (one RFC 168 recovery run, before any
/// truncate), truncate back, then empty the journal. Refuses if no journal is present, or if `id`
/// does not match the one present (naming the real id) -- the same "named, not guessed" discipline
/// `--recovery-restore <id>` already uses.
pub fn cancel_import(
    layout: &RepositoryLayout,
    id: &str,
    plan_only: bool,
) -> Result<CancelImportReport> {
    let Some(journal) = read_journal(layout)? else {
        return Err(PrikkError::Integrity(
            "no archive import journal is present -- nothing to cancel".to_string(),
        ));
    };
    let real_id = journal_short_id(&journal.archive_identity);
    if id != real_id {
        return Err(PrikkError::Integrity(format!(
            "no import with id {id} is present; the one present has id {real_id}"
        )));
    }

    // The ref-side container locks, held for the whole cancel run -- **not** `ObjectStore`, taken
    // instead per object-type container just below (see `import_archive`'s own doc note on why
    // holding it continuously would conflict with `write_object`'s own per-append acquisition;
    // cancel does not call `write_object`, but holding it here for the whole run is still
    // needlessly wide when a scoped acquisition per cut is just as correct and cheap).
    let _container_locks = acquire_container_locks_allowing_import_journal(
        layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
        ],
    )?;
    let _active_lock = ActiveLock::acquire_for_write_allowing_import_journal(
        layout,
        crate::foundation::layout::DEFAULT_ACTIVE_NAME,
    )?;

    let run = if plan_only {
        None
    } else {
        Some(crate::recovery_log::begin_run())
    };
    let root = layout.repository_mutation_root();
    let mut steps = Vec::with_capacity(journal.containers.len());
    for container in &journal.containers {
        // Only `ObjectStore` needs its own lock acquired here: the other three
        // `LockableContainer` kinds are already held continuously, above, for this whole cancel
        // run, and re-acquiring an already-held lock would conflict with itself. `AuthorKey` has
        // no `LockableContainer` at all (see `JournaledContainerKind`'s own doc note) -- it is
        // protected solely by the `_active_lock` already held above, the same way
        // `record_author_key_material` always was.
        let _object_store_lock = if container.kind == JournaledContainerKind::ObjectStore {
            Some(acquire_container_locks_allowing_import_journal(
                layout,
                &[LockableContainer::ObjectStore],
            )?)
        } else {
            None
        };
        let relative = Path::new(&container.relative_path);
        // RFC 160 §3.1's whole-read guard: an object container or the ref log container is
        // store-growing, and this cancel's own save-before-truncate genuinely needs the whole
        // file (the saved copy is the way back) -- see `whole_read_guard::SCOPES`'s own
        // `archive-import-cancel-save` row, the same reasoning `pointer-index-rebuild-recovery-
        // save` already has for the pointer index.
        #[cfg(test)]
        let _whole_read_scope =
            crate::foundation::fsutil::whole_read_guard::declare("archive-import-cancel-save");
        let Some(bytes) = read_file_if_exists(root, relative)? else {
            steps.push(CancelImportStep {
                label: container.label.clone(),
                cut_bytes: 0,
                recovery: None,
            });
            continue;
        };
        let current_len = bytes.len() as u64;
        if current_len <= container.starting_length {
            steps.push(CancelImportStep {
                label: container.label.clone(),
                cut_bytes: 0,
                recovery: None,
            });
            continue;
        }
        let cut_bytes = current_len - container.starting_length;
        if plan_only {
            steps.push(CancelImportStep {
                label: container.label.clone(),
                cut_bytes,
                recovery: None,
            });
            continue;
        }
        let entry = crate::recovery_log::entry_for(
            layout,
            &container.relative_path,
            &bytes,
            container.starting_length,
            "cancel-import",
        )?;
        let recovery = crate::recovery_log::append(root, &entry)?;
        truncate_existing_file_required(root, relative, container.starting_length)?;
        steps.push(CancelImportStep {
            label: container.label.clone(),
            cut_bytes,
            recovery: Some(recovery),
        });
    }
    drop(run);

    if !plan_only {
        empty_journal(layout)?;
    }

    Ok(CancelImportReport {
        id: real_id,
        plan_only,
        steps,
    })
}

/// One container's own cancel outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CancelImportStep {
    /// The container's own stable label (matches `ArchiveImportReport`'s own labeling, and the
    /// journal's own).
    pub label: String,
    /// Bytes this step cut (or would cut, under `--plan-only`). `0` when the container was never
    /// appended to.
    pub cut_bytes: u64,
    /// The recovery-log entry this step wrote, before truncating -- `None` under `--plan-only`, or
    /// when there was nothing to cut.
    pub recovery: Option<crate::recovery_log::RecoveryRef>,
}

/// `prikk doctor --cancel-import`'s own report.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CancelImportReport {
    /// The import's own id, as confirmed against the journal.
    pub id: String,
    /// Whether this was a preview (`--plan-only`): every step above describes what cancel would
    /// do, but wrote nothing.
    pub plan_only: bool,
    /// One outcome per journaled container, in the journal's own order.
    pub steps: Vec<CancelImportStep>,
}

#[cfg(test)]
thread_local! {
    static STOP_AFTER_JOURNAL_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static STOP_AFTER_STAGE_1_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static STOP_AFTER_STAGE_2_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static KILL_MID_OBJECTS_AFTER_N_FOR_TEST: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static FAIL_MID_OBJECTS_AFTER_N_FOR_TEST: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static RUN_AFTER_FIRST_OBJECT_WRITE_FOR_TEST: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static RUN_BEFORE_STAGE_3_FOR_TEST: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static DISABLE_LANDING_COLLISION_RULE_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test seam, unreachable from production: the next call to [`import_archive`] on this thread
/// lands every planned ref unconditionally, in plan order, with no collision check -- reproducing
/// the exact external-review-023 bug (RFC 155 §9.6, amended 2026-10-12) for the control this
/// part's own test requires. Consumed (reset to `false`) by that one call.
#[cfg(test)]
pub(crate) fn disable_landing_collision_rule_for_test() {
    DISABLE_LANDING_COLLISION_RULE_FOR_TEST.with(|cell| cell.set(true));
}

/// Test seam, unreachable from production: the next call to [`import_archive`] on this thread
/// stops right after the journal's own first write (the case table's own "kill right after the
/// journal" row), leaving the journal present and every stage after it un-run.
#[cfg(test)]
pub(crate) fn stop_after_journal_for_test() {
    STOP_AFTER_JOURNAL_FOR_TEST.with(|cell| cell.set(true));
}

/// Test seam, unreachable from production: the next call to [`import_archive`] on this thread
/// stops right after stage 1's own last object write (the case table's own "kill after the
/// objects" row), leaving the journal present and stage 2/the commit point un-run.
#[cfg(test)]
pub(crate) fn stop_after_stage_1_for_test() {
    STOP_AFTER_STAGE_1_FOR_TEST.with(|cell| cell.set(true));
}

/// Test seam, unreachable from production: the next call to [`import_archive`] on this thread
/// stops right after stage 2's own last write (every carried author key recorded), before the
/// commit point -- needed to exercise IA2's own fix (the author-key container is journaled and
/// cut back on cancel), which a kill before stage 2 runs cannot reach.
#[cfg(test)]
pub(crate) fn stop_after_stage_2_for_test() {
    STOP_AFTER_STAGE_2_FOR_TEST.with(|cell| cell.set(true));
}

/// Test seam, unreachable from production: the next call to [`import_archive`] on this thread
/// stops with a clean, named error right after its `n`th object write overall (counting across
/// every object type, in the order stage 1 processes them) -- the case table's own "kill
/// mid-objects" row, which (unlike [`stop_after_stage_1_for_test`]) fires strictly *before* stage
/// 1 finishes, not after.
#[cfg(test)]
pub(crate) fn kill_mid_objects_after_n_for_test(n: usize) {
    KILL_MID_OBJECTS_AFTER_N_FOR_TEST.with(|cell| cell.set(Some(n)));
}

/// As [`kill_mid_objects_after_n_for_test`], but the error is a real `std::io::Error` of kind
/// [`std::io::ErrorKind::StorageFull`] rather than a clean "simulated kill" -- the case table's
/// own "full disk" row, distinguishing "the process was killed" (a clean stop, nothing more to
/// learn from it) from "the write itself failed" (import must behave the same way either way, and
/// this is what proves it rather than assumes it).
#[cfg(test)]
pub(crate) fn fail_mid_objects_after_n_for_test(n: usize) {
    FAIL_MID_OBJECTS_AFTER_N_FOR_TEST.with(|cell| cell.set(Some(n)));
}

/// Test seam, unreachable from production: run `callback` once, right after stage 1's own first
/// object write -- while this import's own `ObjectStore` container lock is genuinely held, so a
/// callback that attempts a *second*, concurrent object append (through an ordinary, unrelated
/// `ObjectWriteSession`/`FileObjectStore` handle, never through this import's own) can observe
/// whether that append refuses. This is finding IA1(a)'s own test: `ObjectStore` held through
/// stage 1 is a structural guarantee only if a real concurrent append genuinely cannot land while
/// it holds.
#[cfg(test)]
pub(crate) fn run_after_first_object_write_for_test(callback: impl FnOnce() + 'static) {
    RUN_AFTER_FIRST_OBJECT_WRITE_FOR_TEST
        .with(|cell| *cell.borrow_mut() = Some(Box::new(callback)));
}

/// Test seam, unreachable from production: run `callback` once, right before stage 3's own first
/// landing -- while all four container locks are genuinely held, so a callback that attempts a
/// concurrent, ordinary `publish` (through an unrelated `RefStore` handle, never through this
/// import's own) can observe whether that publish refuses. This is finding IB1(a)'s own test.
#[cfg(test)]
pub(crate) fn run_before_stage_3_for_test(callback: impl FnOnce() + 'static) {
    RUN_BEFORE_STAGE_3_FOR_TEST.with(|cell| *cell.borrow_mut() = Some(Box::new(callback)));
}

/// RFC 155 Part I-B2, review finding IB2: the one seam reachable from a real, separately
/// compiled `prikk` process, gated by the `test-support` feature (never the shipped binary --
/// see this crate's own `Cargo.toml` doc note on `test-support`, and `prikk-cli`'s identical one)
/// rather than `#[cfg(test)]` (which only ever compiles into this crate's own unit-test binary).
///
/// A no-op unless `PRIKK_ARCHIVE_IMPORT_BLOCK_AFTER_N_FOR_TEST` is set to `total_objects_written`'s
/// own value -- read fresh on every call rather than cached once, since this is cheap (one env
/// lookup) and avoids any state this function would otherwise need to keep across calls. When it
/// matches: writes a marker file at the path named by `PRIKK_ARCHIVE_IMPORT_BLOCKED_MARKER_FOR_TEST`
/// (so the harness that spawned this process knows it is now genuinely blocked, not merely slow),
/// then blocks forever -- there is no unblock path, by design: the harness's own next step is a
/// real `SIGKILL`, and the whole point is to leave real lock files on disk the way an actual crash
/// would, which no `Err`-returning, `Drop`-respecting seam ever could.
#[cfg(feature = "test-support")]
fn block_after_n_objects_for_real_kill_test(total_objects_written: usize) {
    let Ok(block_after) = std::env::var("PRIKK_ARCHIVE_IMPORT_BLOCK_AFTER_N_FOR_TEST") else {
        return;
    };
    let Ok(block_after) = block_after.parse::<usize>() else {
        return;
    };
    if block_after != total_objects_written {
        return;
    }
    if let Ok(marker_path) = std::env::var("PRIKK_ARCHIVE_IMPORT_BLOCKED_MARKER_FOR_TEST") {
        let _ = std::fs::write(marker_path, b"blocked\n");
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Test-only: a SHA-256 of each container the journal would name (or does name, if one is
/// present), labeled, in `compute_container_starting_lengths`'s own order -- for asserting that a
/// cancel restores every journaled container to a state byte-identical to before the import ever
/// started (the case table's own "kill mid-objects"/"full disk" rows: a hash of every journaled
/// file before the import and after the cancel must match). Reads whatever bytes are on disk
/// *now*; a missing file hashes the same as an empty one, matching `stat_len`'s own "absent reads
/// as 0" convention used throughout this module.
///
/// Reads through plain `std::fs::read`, not the anchored, mutation-root-relative primitive --
/// deliberately outside RFC 160 §3.1's whole-read guard, which polices *production* whole reads
/// of a store-growing file (`whole_read_guard::SCOPES` is a ledger of production cost decisions,
/// each with its own RFC/ROADMAP reference). This function is `#[cfg(test)]`-only scaffolding, not
/// a production path, so it has no cost decision to declare there.
#[cfg(test)]
pub(crate) fn hash_journaled_containers_for_test(
    layout: &RepositoryLayout,
) -> Result<Vec<(String, [u8; 32])>> {
    let containers = compute_container_starting_lengths(layout)?;
    let mut hashes = Vec::with_capacity(containers.len());
    for container in &containers {
        let absolute = layout.prikk_dir().join(&container.relative_path);
        let bytes = std::fs::read(&absolute).unwrap_or_default();
        hashes.push((container.label.clone(), prikk_hash::sha256(&bytes)));
    }
    Ok(hashes)
}

#[cfg(test)]
mod tests;
