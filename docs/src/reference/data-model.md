# Data Model

This page is the authoritative current-state reference for Prikk's data model. It describes what has
shipped on `main` — not necessarily the latest tagged release, see [`README.md`'s Current
Status](https://github.com/prikk-vcs/prikk/blob/main/README.md#current-status) for that boundary — and
is grounded in the code, released RFCs, and implementation status records listed in the anchor table at
the foot of the page.

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- `.prikk/` is Prikk's native repository format and is not Git-compatible storage.
- Ref pointers are mutable, for convenience and recovery, not roots of trust.
- Durability and recovery claims are supported by current unit and integration tests, not by a
  completed crash-matrix or fuzzing campaign.
- Repository *mutation* is exercised by project gates on Linux, macOS, and Windows (DC-87 Stage 2).
  Windows' anchoring guarantee is weaker than Linux/macOS in one stated way — see
  [platform support](./platform-support.md) for the exact gap and which of the nine durability
  guarantees are held, weaker, or documented no-ops there. Read-only commands are CI-gated on macOS
  and Windows too — see [platform support](./platform-support.md).
- Stable repository-format migration, complete branch management, remote-tracking, hosted forge
  trust, and plugin execution remain deferred. `prikk sync` (RFC 116) and tag travel/adoption (RFC
  117) and `prikk merge` (DC-74) have since shipped — see the [sync](../guide/sync.md) and
  [merge](../guide/merge.md) guides.

Trust, signature, and threat-boundary caveats live in the
[trust and threat model](./trust-threat-model.md). The local persistence and crash-recovery boundary
lives in the [durability and crash recovery](./durability-recovery.md) reference. The physical
`.prikk/` layout and authority-vs-pointer/cache boundary lives in the
[repository layout and authority](./repository-layout.md) reference. Local lock and ref
compare-and-swap behavior lives in the [concurrency and locking](./concurrency-locking.md) reference.

## Object Identity

Prikk objects are typed, versioned envelopes. An object id is SHA-256 over a domain-separated preimage
containing the object type, schema version, payload length, and unsigned canonical payload bytes.
Signatures live outside that identity preimage, so adding or sorting signatures does not change the
object id.

New envelope serialization and repository writes require a strict signature sequence. Ed25519
signatures must be 64 bytes, duplicate signature tuples are rejected, and signatures are ordered by
key-id bytes, signer-role code, algorithm code, then signature bytes. Advisory signature timestamps
do not affect that order.

The current object model includes persistent Patch, Block, RefState, Blob, Tag, and RecognitionClaim
object directories. **Tag objects are produced by `prikk tag create` and `sync adopt-tag` (RFC 117).**
**Attestation remains genuinely unconstructed**: the object type and directory are defined, but no
production code path builds one. RefUpdate is an object-envelope type stored inline in ref logs rather
than as a persistent object-store directory. BlockSummaryCache and RecoveryNote are explicitly not
roots of trust.

## Patch and Operation Model

A Patch is the identity-bearing unit of logical change. Its payload contains one or more ordered
operations, sorted parent Patch ids, optional intent, optional preconditions, and an identity-bearing
purpose. `PatchPurpose::Normal` is the default by omission. `PatchPurpose::RollbackDraft` is encoded
explicitly and survives WAL-to-object persistence for rollback classification.

Current production authoring creates node-addressed patches from the worktree. It derives the baseline
from authoritative replay of the published branch tip, or from an empty genesis baseline for an unborn
branch ref. It rejects snapshot-only baselines without node identity for worktree authoring.

## Blocks

A Block is an immutable sealed history unit. Its payload records sorted parent Block ids, Block kind,
Patch ids in canonical Block order, a state Merkle root, and an optional snapshot Blob reference.
Seal creates schema-2 Root Blocks with zero parents for unborn refs and schema-2 Normal Blocks with
exactly one schema-2 parent for refs with an existing published tip. **Merge Blocks with exactly two
parents are supported** (DC-75) and record both parents, a mainline pointer, and the merge baseline,
which `verify` re-derives rather than trusts. **Repair and Import Blocks remain unauthorized** until a
later design defines their state derivation.

The state root commits to the complete replay-derived live-node set in canonical path order.
Each leaf binds the exact repository path, nonzero NodeId, node kind, normalized mode, and either the
file Blob ObjectId or opaque UTF-8 symlink target. Binary Merkle reduction promotes an odd final hash
unchanged. Patch ids, tombstones, implicit directories, snapshots, and caches are not state entries.
Verification replays every Block from empty state or its one parent and rejects a root mismatch,
missing evidence, invalid path/mode/kind/content state, or mixed Block schema lineage. Snapshots and
caches may be used only as checked auxiliary data; they cannot override replay.

## Checkpoints and snapshots

A **checkpoint** is a Block that carries a snapshot. `seal`, `merge` and `sync seal` write one at a
ref's first Block and at every 64th Block after it (RFC 136 §10.2); every other Block's snapshot
reference is empty.

- **What a snapshot is.** A Blob of kind `Snapshot` holding a v2 manifest: the Block's own state, entry
  for entry — the exact leaves its state root hashes (path, NodeId, kind, mode, and the file's content
  Blob id or the symlink target). A text file whose content only ever arrived through `EditText` has no
  stored content Blob (edits carry spans, not whole files), so the checkpoint stores that content as an
  ordinary Text Blob first, and only then the manifest. A bundle carries the manifest and those content
  Blobs with the history.
- **What `verify` does with it.** Every snapshot is checked: the manifest must decode, recompute to its
  own Block's state root, and name only content Blobs that are present. A failure is an integrity
  finding naming the Block. `verify` never replays less because a snapshot exists.
- **A checkpoint changes cost, never output** (RFC 136 §10.3a). Every command gives byte-identical
  output with or without checkpoints; a snapshot only lets a command start part-way along the chain.
- **Who may start from one.** Read-only reports (`checkout --patch-plan`, `--patch-delete-plan`,
  `bundle preview`) may start at the nearest snapshot that passes validation. Commands that write the
  worktree (`checkout --patch-materialize`, `--patch-materialize-delete`, `branch switch`) start only at a
  snapshot whose Block this repository has itself confirmed by replay **and an adopted maintainer signed**, within
  63 blocks of the tip; otherwise they replay from genesis. `seal`, `merge` and `sync seal` derive the state they
  sign from the same kind of snapshot, under the same rule.
  `rollback-preview`, `commit`'s baseline and `merge-evidence` always replay the whole chain.
- **The replay-verified record and the provisional marker.** Which Blocks count as confirmed is kept in
  `.prikk/cache/replay-verified-blocks.v1`, a cache that is always safe to delete. A worktree written
  directly from an unconfirmed snapshot is marked provisional until `prikk verify`. Both are described in
  [snapshot materialization](../guide/checkout/snapshot-materialization.md).

**What a checkpoint costs.** Measured on the RFC 139 corpus profile (`profiles/prikk-self.toml`), against
the same history sealed by a build from before checkpoints existed:

| history depth | checkpoints | repository bytes without | with | checkpoints add | growth over the repository without them |
|---:|---:|---:|---:|---:|---:|
| 64 | 1 | 628,486 | 629,944 | 1,458 | +0.2 % |
| 128 | 2 | 1,621,001 | 1,829,897 | 208,896 | +12.9 % |
| 256 | 4 | 3,069,495 | 4,425,512 | 1,356,017 | +44.2 % |

The last column is growth: what checkpoints add, as a share of the same repository sealed without them.
Read as a share of the repository that *has* them, the same bytes are 11.4 % at depth 128 and 30.6 % at
256.

The manifest itself is small — 177 bytes at block 1, 12,268 bytes at block 193, where it lists 114 entries
— and is under 2 % of what a checkpoint adds. The rest is the content Blobs a checkpoint stores for text
files whose content had only ever arrived as edits. A repository whose files are mostly edited rather than
created therefore pays close to one full copy of its text content per checkpoint, and one whose files are
mostly created pays almost nothing, because those Blobs are already stored.

## Tags

A Tag is a named, signed pointer into history, created by `prikk tag create` or `sync adopt-tag`
(RFC 117). Its payload carries a **local pointer half** and a **portable identity half**, and the
distinction is the point of the object:

- `target_block_id` — the local pointer: a Block this repository can resolve directly.
- `patch_set_digest` — the digest of `target_block_id`'s own patch closure
  (`compute_patch_set_digest_from_block`), computed at creation time. Two repositories holding the same
  patches produce the same `patch_set_digest` independently, by construction — this is the value a tag's
  portability across repositories depends on, since `target_block_id` itself does not survive a move:
  blocks diverge between repositories by design even when the underlying history is identical.
- `patch_count` — the number of distinct patch ids in the closure `patch_set_digest` covers. Not new
  information (the digest's own preimage already hashes `DOMAIN ‖ count ‖ sorted ids`), exposed as a
  separate field so a resolver can prune a candidate by size before hashing it. **A hint that narrows,
  never an authority** — a wrong `patch_count` can only cause a right candidate to be skipped or extra
  candidates hashed; it can never produce a wrong resolution, because the digest still has to match.

`TagPayload` also carries `name`, an optional `message`, the same no-clock `created_at` sentinel every
other current-write payload uses, and `author_key_id`. All seven fields are admitted at `schema_version`
1 — the owner ruled (2026-08-23) that `Tag`'s schema window stays closed rather than minting a schema 2
for `patch_set_digest`/`patch_count`, on the standing premise that no production repository holds a tag
yet. **This is a two-way, permanent incompatibility**: a `Tag` written before `patch_set_digest`/
`patch_count` existed will not decode against the current 7-field reader, and the reverse is also true —
see the [`0.23.0` CHANGELOG entry](https://github.com/prikk-vcs/prikk/blob/main/CHANGELOG.md) for the
full consequence, since it reaches `prikk verify`, not only `prikk tag list`.

## Recognition Claims

A RecognitionClaim is a signed assertion, under the signer's key, that specific patches were sealed
into a specific block — nothing more. It exists so a sender can tell a receiver what a block contains
**before** the receiver holds that block: the claim is deliberately never existence-checked against the
objects it names, unlike every other reference in this data model. Its payload is minimal by design
(RFC 115 Stage 2 D3): `block_id`, `patch_ids` (the block's own order, verbatim — not sorted, not
deduplicated, non-empty), and `parent_block_ids` (the block's own parents, verbatim — not sorted, not
deduplicated, may be empty). It carries no signer `key_id` (the signature preimage already binds it), no
timestamp, and no project/genesis binding — each omission is deliberate, not an oversight. See
[Recognition claims and sync relations](./data-model-lifecycle.md#recognition-claims-and-sync-relations)
for how it relates to Block and Patch.

## Refs and Publication

RefState is the content-addressed state for a branch or tag ref. A ref pointer entry, in a shared
append-only container holding every ref's current pointer, stores the current RefState id for
convenience and recovery, but the pointer is not itself the root of trust. RefUpdate records are
signed envelope entries in a shared append-only ref-log container and link old and new RefState ids,
target Block id, update sequence, a schema-1 no-clock sentinel, and maintainer key id. The `created_at`
field is exactly zero for current writes and is not a trusted creation or event timestamp.

Publication is guarded by ref-specific locking and compare-and-swap checks. The
[concurrency and locking](./concurrency-locking.md) reference owns the detailed lock/CAS behavior.
Seal persists WAL Patch envelopes, creates a signed Block and RefState, durably appends the
authoritative ref pointer as the publication commit point, appends exactly one signed RefUpdate log
entry, confirms pointer/log agreement, then drains the active WAL and active ref metadata.

## Received Namespace

Imported history (`bundle import`, `sync accept`) lands under a **received pointer**, always named
`remotes/<origin ref name>` — a distinct index from `refs/by-id/`'s ordinary ref-pointer container, kept
in its own small append-only format. A received `RefState` keeps its *origin's own* embedded `ref_name`
(rewriting it would invalidate the object's content-addressed identity and signature), so a pointer
declared as `remotes/heads/main` could never agree with a payload that still says `heads/main` under the
ordinary pointer container's own consistency check — storing received refs separately sidesteps that
conflict rather than special-casing the check to allow it.

**Import never advances a local ref.** A received pointer is discoverable by name and nothing more;
turning received history into local history is an ordinary `merge`, using machinery that already
exists. The received-pointer index is **never read by `verify_repository`** — every object a received
pointer leads to (RefState, Block, Patch, Blob, Attestation) is an ordinary object-store entry, checked
exactly like any other by the existing type-based object scan, so accepting received history adds no
new verification path, only a new way to discover a receiver's own object graph by name.

## Sync and Exchange Artifacts

Three wire formats move information between repositories. None is a persistent object type or a root
of trust — each is **representational, not frozen** (RFC 114 §3): every object it carries has identity
already frozen elsewhere, and the artifact itself carries none of its own.

- **`PSYNCSU1`** (sync summary) — one message per repository: every local `heads/*` ref, each with its
  own patch-set digest and patch count. Answers "are we the same?" without moving a single patch id.
  Branches only; `remotes/*` and `tags/*` are excluded deliberately, not by oversight.
- **`PSYNCHV1`** (have-list) — one ref, its declared patch-set digest, and the full patch-id list the
  digest is over. Sent receiver → sender so the sender can compute the delta. The digest is always
  recomputed over the decoded list and checked, never trusted from the wire.
- **`PEXCH002`** (exchange artifact) — the patch-level payload itself, built by `sync build` and
  consumed by `sync accept`. Six sections in order: the declared patch-set digest; the ordered Patch
  list in the sender's own application order; every Blob any carried patch references; author key
  material (continuity only, never a trust decision); recognition claims (may be empty); and Tag
  objects (may be empty — every tag whose `target_block_id` lies within the synced ref's ancestry). A
  carried Tag is reported on accept, **never adopted** — adoption is `sync adopt-tag`, a separate,
  explicit, receiver-signed act. `PEXCH002` superseded `PEXCH001` (RFC 117 stage 3, adding the Tag
  section) as a **format revision, not a migration**: a `PEXCH001` byte stream is refused outright on
  read, since the artifact is transient in-flight data that never becomes repository history, so there
  is nothing to preserve across the bump.

Every declared count in `PEXCH002` (patches, blobs, author keys, claims, tags) is checked against a
caller-supplied ceiling at the moment it is read, before that section's loop runs — not after decoding
everything and counting.

## Active WAL and Recovery Boundary

The active WAL stores exact signed Patch envelopes before sealing. WAL append requires a Patch
envelope with at least one signature, writes a checksummed record, and fsyncs the WAL file. WAL replay
reads valid records from the start and reports incomplete trailing bytes separately from checksum
failures.

The detailed persistence, seal-publication, and recovery framing lives in the
[durability and crash recovery](./durability-recovery.md) reference.

The current active-session model is single-commit-per-active-WAL. Active ref metadata records which
branch ref owns a non-empty active WAL. Missing or malformed active ref metadata on a non-empty WAL is
an integrity issue; stale metadata on an empty WAL is local debris.

Doctor repair is intentionally narrow. It can truncate an incomplete trailing active-WAL record after
the preceding records verify. It does not reconstruct missing ref pointers, sign or append RefUpdates,
synthesize missing objects, repair malformed logs, or prove crash behavior beyond current test
evidence. Exact interrupted ref publication completion belongs to signer-backed `seal` retry.

## Replay, Checkout, Verify, and Doctor

Replay and lifecycle semantics live in the internally scoped `prikk-replay` crate, while `prikk-store`
remains the repository integration crate for layout, refs, WAL, active sessions, object storage,
verification, doctor, and worktree integration. `prikk-replay` is not a stable external Rust API.

Repository verification is read-only. It checks object placement, envelope decoding, object identity,
Block references, ref pointer and log consistency, active WAL checksums, active WAL metadata health,
sealed rollback Patch classification, and publication trust for publication envelopes. Doctor converts
verification results into actionable diagnostics and exposes only the narrow repairs described above.
The diagnostic catalog lives in the
[integrity and recovery diagnostics](./integrity-recovery.md) reference.

## Deferred

**`prikk sync` (RFC 116, RFC 117) and `prikk merge` (DC-74) have since shipped** — see the
[sync](../guide/sync.md) and [merge](../guide/merge.md) guides. Still deferred: stable
repository-format migration, complete branch management, remote-tracking, hosted forge trust,
audit/plugin execution, persisted proof or witness objects, general rollback authorization,
multi-maintainer publication policy, and full cross-platform filesystem validation.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| Object ids derive from type, schema version, payload length, and unsigned canonical payload. | [`id.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/id.rs), [`envelope.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/envelope.rs), [DC-09](https://github.com/prikk-vcs/prikk/blob/main/rfcs/archive/DC-09-PHASE-4-NODE-MODEL.md) |
| Signatures are outside object identity; strict new envelopes enforce Ed25519 shape, tuple uniqueness, and canonical order. | [`envelope.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/envelope.rs), [`signature.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/signature.rs), [DC-39](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-39-SIGNATURE-ENVELOPE-AUTHORITY.md) |
| Current persistent object directories exclude RefUpdate. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [`id.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/id.rs) |
| Patch payloads require non-empty contiguous operations and carry identity-bearing purpose. | [`patch.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/payload/patch.rs), [DC-10](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-10-ROLLBACK-DRAFT-SIGNING.md) |
| Worktree authoring derives baselines from authoritative replay or valid genesis. | [`node_authoring.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_patch/node_authoring.rs), [DC-13](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-13-NONDEFAULT-REF-GENESIS.md), [implementation status](https://github.com/prikk-vcs/prikk/blob/main/rfcs/IMPLEMENTATION-STATUS.md) |
| Blocks contain parent ids, kind, Patch ids, state root, and optional snapshot Blob ref. | [`block.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/payload/block.rs), [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs) |
| Tag carries `target_block_id` (local) and `patch_set_digest`/`patch_count` (portable), both fields amended in place at schema 1, no schema 2. | [`payload/tag.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/payload/tag.rs), [`tag.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/tag.rs), [RFC 117](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/117-tag-sync.md) |
| RecognitionClaim carries a block's own `patch_ids`/`parent_block_ids` verbatim and is never existence-checked against them at decode time. | [`payload/recognition_claim.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/payload/recognition_claim.rs), [RFC 115](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/115-sync-investigation.md) Stage 2 (D3), [RFC 116](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/116-sync-negotiation-and-transport.md) (N3) |
| RefState is content-addressed state and ref pointers are mutable entries in a shared container. | [`refs.rs` payload](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/payload/refs.rs), [`refs/pointer_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/pointer_index.rs), [DC-11](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-11-MAINTAINER-TRUST-STORE.md) |
| RefUpdate is append-only publication evidence stored inline in a shared ref-log container; schema-1 writes use zero as a no-clock sentinel. | [`refs.rs` payload](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/payload/refs.rs), [`refs/container.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/container.rs), [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [DC-39](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-39-SIGNATURE-ENVELOPE-AUTHORITY.md) |
| Received refs are stored under `remotes/<name>` in their own index, never read by `verify_repository`; import never advances a local ref. | [`received.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/received.rs), [`received_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/received_index.rs), [DC-78](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-78-HISTORY-EXCHANGE.md) §D4 |
| `PSYNCSU1`/`PSYNCHV1` negotiate; `PEXCH002` (formerly `PEXCH001`) carries patches, blobs, author keys, claims, and tags — representational, not frozen. | [`sync_negotiation/summary.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/sync_negotiation/summary.rs), [`sync_negotiation/have_list.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/sync_negotiation/have_list.rs), [`patch_exchange/artifact.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_exchange/artifact.rs), [RFC 116](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/116-sync-negotiation-and-transport.md), [RFC 117](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/117-tag-sync.md) stage 3 |
| Active WAL records exact signed Patch envelopes and detects trailing partial bytes. | [`wal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/wal.rs), [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [DC-15](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) |
| `prikk compact` reclaims dead records from the ref-pointer, received, and trust-key containers only; never the ref log or sealed objects. | [`compact.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/compact.rs), [RFC 102](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/102-container-based-durability.md) Stage 6 Step 2 |
| Verification is read-only and bounded to structural, WAL, ref, rollback, and publication-trust checks. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [implementation status](https://github.com/prikk-vcs/prikk/blob/main/rfcs/IMPLEMENTATION-STATUS.md) |
| `prikk-replay` is internally scoped and not a stable external API. | [DC-19](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-19-REPLAY-LIFECYCLE-CRATE-BOUNDARY.md), [DC-20](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-20-REPLAY-BOUNDARY-STABILIZATION.md), [implementation status](https://github.com/prikk-vcs/prikk/blob/main/rfcs/IMPLEMENTATION-STATUS.md) |
| Durability and platform claims remain limited by current test evidence. | [DC-24 baseline recap](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-24-data-model-trust-threat-docs/baseline-recap.md), [DC-24](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-24-DATA-MODEL-TRUST-THREAT-DOCS.md) |

## Provenance

This reference consolidates released records through DC-23 and DC-24. It uses
[`baseline-recap.md`](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-24-data-model-trust-threat-docs/baseline-recap.md)
only as a tracked recap of older non-VCS baseline inputs; current code, released RFCs, and
[`IMPLEMENTATION-STATUS.md`](https://github.com/prikk-vcs/prikk/blob/main/rfcs/IMPLEMENTATION-STATUS.md)
remain the durable authorities. DC-26 moved this current-state reference from `rfcs/fdds/` into the
published book without changing code, schema, trust, or CLI behavior.
