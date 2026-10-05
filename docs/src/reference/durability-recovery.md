# Durability and Crash Recovery

This page is the authoritative current-state reference for Prikk's local persistence and
crash-recovery model. It describes the current implementation behavior without adding storage,
verification, doctor, or command semantics.

For related concepts, see the [repository layout and authority](./repository-layout.md) reference, the
[data model](./data-model.md), the [trust and threat model](./trust-threat-model.md), and the command
guides for `verify` and `doctor` through the
[integrity and recovery diagnostics](./integrity-recovery.md) reference.
Release-transaction durability, artifact identity, and evidence limits are documented separately in
[release, versioning, and compatibility](./release-compatibility.md).

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- Durability and recovery claims are supported by current unit and integration tests, not by a
  completed crash-matrix or fuzzing campaign.
- Repository mutation currently requires Linux, macOS, or Windows (DC-87 Stage 2) anchored relative
  no-follow path resolution, strict regular file sync, and the required install primitives — with one
  stated exception: Windows' anchoring is not handle-scoped between path components the way
  Linux/macOS's is, so it does not close the same inter-component race (see
  [platform support](./platform-support.md) for the exact gap and the full guarantee-by-guarantee
  table). Filesystems without any of those proved capabilities remain read-only/diagnostic targets —
  see [platform support](./platform-support.md) for exactly which commands that covers and how it is
  CI-verified.
- `.prikk/` is not a stable repository format and there is no stable migration policy yet.
- The ref pointer is a mutable convenience pointer (an entry in a shared, append-only container, not a
  file of its own), not a root of trust.
- `doctor` repairs are opt-in and narrow; they do not synthesize missing objects, signatures, trust
  policy, or key material.
- Stale `active.lock` cleanup after a crash is manual today; the current lock/CAS boundary is covered
  by the [concurrency and locking](./concurrency-locking.md) reference.

## Commit Persistence Boundary

A successful `commit` appends an exact signed Patch envelope to the active WAL. The WAL append path
rejects non-Patch envelopes and unsigned Patch envelopes, writes a checksummed record, required-syncs
the WAL file, and required-syncs the parent directory after every append. Any required
file or directory sync failure returns an operation failure while retaining written state for replay.

That is the active-session persistence boundary. It does not mean the Patch is sealed into a Block, a
RefState has been published, a ref pointer moved, or the active WAL has been drained. Sealed history is
created later by `seal`.

## WAL Replay and Tail Handling

WAL replay reads valid records from the start of the file. Each complete record carries magic,
version, sequence, body length, checksum, and the encoded signed envelope bytes.

Incomplete trailing bytes are reported separately as trailing partial bytes. **For the WAL, the tail is
everything after the last sound record, when no sound record follows it — whatever its shape** (RFC 162
rule 3): a genuine interrupted-append prefix, zeros, garbage, or a last record whose header parses but
whose own checksum does not match. If a sound frame (magic, a valid header, a body that passes its
checksum) starts anywhere in the remainder, the frame at that offset is not a tail; it is **interior
damage**, reported as a failed record at its offset, and the sound records after it are still read. **The
only question that decides tail from damage is whether a sound record follows the fault** — not the
fault's own shape, and not whether it merely looks parseable: a sound record anywhere behind it is always
damage, and nothing behind it is always a tail, including a last record whose own checksum fails (above).
A true tail is the only case `doctor --repair-wal-tail` truncates. (Before 0.48.0 the tail was the narrower, shape-defined case only —
a structurally incomplete frame, too few bytes for its own header or claimed body. A complete-but-invalid
last record — a checksum mismatch, an unsupported version, a malformed envelope — was refused as damage
even with nothing sound behind it, the same as interior damage still is. RFC 162 rule 3 widened the tail
from a question of shape to a question of position: not "does this parse as a legitimate partial frame"
but "is this the last thing in the file, with no sound record after it." Widening loses nothing, because a
repair keeps every byte it removes, below — including, now, a record that was in fact a real write, torn
in a way this file cannot tell apart from a crash.)

**Fixed in 0.49.0 (N6): a damaged last record used to read as a tail, and the repair could remove one the
user was told had succeeded.** A record whose own bytes are all present but whose checksum fails (bit rot, a
partial write the storage layer itself reordered, and similar) is, by rule 3, indistinguishable from a
genuine crash-torn prefix once nothing sound follows it, so before 0.49.0 `verify` exited 0 and
`doctor --repair-wal-tail` truncated it either way, even when it had already been acknowledged. 0.49.0
writes a small local witness with each commit — the count, Patch id and frame hash of the last record it
acknowledges, checked against the WAL independently of the WAL's own content — so the two cases are told
apart: a genuine, never-acknowledged crash tail is still removed exactly as before; an *acknowledged* record
that is now damaged or missing refuses instead (`verify` exits 1; `commit`/`seal`/`rollback-draft` refuse,
naming it), and `prikk doctor --discard-damaged-commits [--plan-only]` (RFC 166 D5) is its own way out — the
removed bytes are still saved to `.prikk/recovery/`, exactly as `--repair-wal-tail` saves them, before the
witness is rewritten to cover the resulting sound prefix.

**A repair keeps every byte it removes.** A record whose only fault is a damaged length, with nothing sound behind it, is
indistinguishable from an interrupted append, so `--repair-wal-tail` truncates it. Before it does, it writes exactly the bytes it will remove to
`.prikk/recovery/wal-<session>-at-<offset>-<hash>.bytes` (durably, under the same lock), and only then truncates; its output names the file. The
file is the raw WAL bytes, so a record that was removed by mistake can be read back from it: the removed region starts at the named offset of the
old WAL, with the same framing. If saving the file fails, nothing is truncated. The file is never authority: `verify` ignores it, and it can be
deleted once it is not needed. It also means a repair can be wrong about what it removed without anything being lost, torn tail or damage.
**What counts as a tail is not one rule for every framed file** (RFC 162):

- **The WAL and the pointer index** end at their last sound record — the position rule above — because the
  pointer index leads the log by design and gets the same repair as the WAL.
- **The object index** is a pure cache (rule 1): a damaged tail or interior record there is never refused.
  A writer rebuilds it from the containers before appending; a reader falls back to scanning them.
- **The object containers** are classified by connectivity, not by position (rule 2, above): an unparseable
  frame is a harmless remnant unless something still committed — a sealed block's state, a queued patch, a
  ref tip — still needs the object it might have held.
- **The ref log** keeps its own positive rule, unchanged: it truncates only a suffix that is a prefix of the
  record it expected to write next.
- **The rest** — the received, author-key and trust-key indexes, the trust-policy container, and the
  generation file — still use the shape rule this section described before 0.48.0: a torn tail is a prefix
  of one well-formed frame, and nothing else; a complete record that fails its own checksum there is damage,
  refused, not truncated.

**The WAL repair, the pointer index's own `--repair-pointer-index-tail`** (mirroring the WAL exactly),
**the object index's own rebuild** (its lost ids, when it cannot re-derive an entry), **and
`--repair-tails`'s own coverage of every file it truncates** all write a recovery file. **The one
exception: a ref-log tail attributable to one ref's own pending completion**, truncated directly with
no recovery file, as part of `prikk ref complete <ref>`'s own write (RFC 165 R4 — `seal`'s own DC-38
retry is one instance of the same mechanism, not a separate path). A lead-*free* tail (RFC 165 R5's
own M4) goes through `--repair-tails` instead, saving what it removes like every other file it covers.

**On Windows, the recovery file's own save is not claimed durable.** It writes through the same platform
durability contract as every other atomic replace on this repository (`foundation/fsutil/anchored/windows.rs`), and that
contract's own Windows implementation does not assert `std::fs::rename`'s durability on return — a documented gap, not
an oversight, since `MOVEFILE_WRITE_THROUGH`'s same-volume guarantee could not be established from primary sources.
Nothing is ever truncated without the save call returning success first, so a repair still never *drops* bytes silently
on any platform; what is weaker on Windows is only the recovery file's own guarantee of surviving a crash between that
return and the next durable point. "A repair keeps every byte it removes" holds as written on Linux and macOS.
One consequence to know: a crash-torn append of a blob **whose content is itself a prikk container file** leaves a partial frame with a sound frame in
its payload; by the rule that is damage, and `doctor --repair-index` indexes the embedded frame as an object. That object is content-addressed and
nothing references it, so it changes no state root and no signed output.

**An interrupted append in an object container is not damage unless something still needs it.** A container record is made durable before its index
entry is appended, so a frame that fails to parse (a genuine prefix, not a complete-but-invalid record) is what a crash between the two leaves.
**The object index is never the witness of this** (it is a pure cache; see above) — `verify` checks connectivity instead: every object a sealed
Block's state, a queued Patch in any active session's WAL, or a ref tip references must exist and read. An unparseable frame is reported as
*possibly holding* a missing object, and named as one, only while connectivity reports something of that container's own type still missing;
otherwise it is a harmless remnant. `verify` and `doctor` report it as a warning (`interrupted appends: N`, `PRIKK-DOCTOR-OBJECT-INTERRUPTED-APPEND`)
naming the offset, and exit 0 unless connectivity itself fails — which fails `doctor` too, not only `verify`; every later object is still scanned
and read. A frame an index entry names, that the containers cannot actually produce, stays a failed item. The active WAL has no such case (an
append refuses past a tail, so an interior partial frame is always damage), and the ref log keeps a failed item for it today, because nothing at
the reader names a ref-log record.

**RFC 164 round 2 Rule E (0.49.0) generalizes the same damage-vs-remnant question to a complete, decodable object that itself names a missing
reference** — not an unparseable frame (the case above), but a fully sound Block whose own `parent_block_ids`, `patch_ids`, or `snapshot_blob_ref`
names an object that does not exist. Before this round that was always a hard failure, regardless of whether anything still needed the Block making
the claim. Now: `verify`/`doctor` compute, fresh on every run, every object id reachable from committed state (a ref, a received pointer, a queued
patch, or a sealed block reached from them — the same connectivity roots the paragraph above already uses, walked further: a ref's RefState reaches
its own target Block or Tag and its prior RefState lineage; a Tag reaches its target Block; a Block reaches its own parents, patches, snapshot blob,
and merge lineage). A Block with a dangling reference is damage, exactly as before, only if the Block itself is reachable this way; otherwise it is an
**unreferenced remnant** — a warning naming the object and what it lacks ("re-run the import if you still have the bundle; otherwise it is harmless"),
and `verify` exits `0` over it, same as the frame-level case above. No command removes a remnant in 0.49.0. A remnant made reachable afterward (a new
branch created over it, say) is reclassified as damage on the very next run, since reachability is never cached. **Scope, this round**: only a Block's
own three reference fields get this treatment — `RefState`'s and `Tag`'s own reference fields are not existence-checked at all today, independent of
Rule E, and extending that is separate, larger scope this round does not cover; `RecognitionClaim`'s own references stay untouched by design (never
trust-conferring, never existence-checked).

## Active Ref Metadata

The active WAL is paired with active ref metadata that records which local branch ref owns the
non-empty WAL. A non-empty active WAL with missing or malformed active ref metadata is an
active-session integrity issue. Seal refuses that state rather than guessing which ref should receive
the WAL records. Since RFC 166 (0.49.0), the same issue is also raised when the metadata is present and
well-formed but disagrees with the session's own commit witness (D6) — a session cannot have two
owners, so the two are checked against each other, not only the metadata's own shape. Either shape
refuses with the same text, naming `prikk doctor --restore-queue-target --ref <ref>
[--not-current-branch] [--plan-only]` (RFC 166 D5, as amended by §14) as the way out: the ref always
comes from the caller, never from the witness, and the write is one atomic replace of the metadata
file, never the truncate-then-append a commit itself uses only once, for a session's first write
(RFC 166 D1) — a tear in *this* verb's own write would recreate the exact stranding it exists to
repair, since the WAL here is, by construction, already non-empty.

**Which branch the verb will accept (RFC 166 §14, 2026-10-05).** D5's original condition —
refuse unless the queue "validates against `<ref>`'s current tip by the same check `seal` makes" —
turned out not to exist: `seal` checks no such thing (a Patch names neither its own ref nor its own
base), so a queue built on one branch could be silently attached to, and then published onto, a
completely different one. §14 replaces it: with a readable commit record, `<ref>` must equal the
record's own ref, unchanged; without one, `<ref>` must be the caller's own current branch (the same
resolver `commit` uses for its default) unless `--not-current-branch` is given, and the same flag is
required when the current branch cannot be resolved at all. A restored owner is final for the verb —
a second restore refuses, the same as any healthy session.

**Residual, accepted by the owner.** A queue made with an explicit `commit --ref X` (or
`rollback-draft` on `X`) while the caller's current branch was `Y` can still be restored to `Y`
without `--not-current-branch`, if the caller does not recognize their own commits in the plan (which
names each one's own message and paths) and skips `--plan-only`. This reaches only a queue with no
readable commit record — a readable one already pins `<ref>` to itself — and is no worse than the
0.20.0-0.48.0 hand-edit this verb replaces.

An empty active WAL with leftover active ref metadata is local debris. Verification and doctor report
that distinction so empty-WAL cleanup does not get confused with sealed-history corruption.

## Seal Publication Flow

`seal --allow-no-audit` publishes the active WAL through the repository storage layers in a fixed
order:

1. Acquire the active lock.
2. Replay the active WAL and reject trailing partial bytes.
3. Reject an empty active WAL, or clean empty-WAL metadata debris where the command path permits it.
4. Require active ref metadata to match the requested local branch ref.
5. Verify the configured MAINTAINER signer against the repository-local trust policy.
6. Persist the signed Patch envelopes from the WAL into the object store.
7. Create a signed Block envelope.
8. Create a signed RefState envelope.
9. Construct the deterministic signed RefUpdate.
10. Append and required-sync the new pointer entry to the shared pointer-index container — the
    publication commit point. An append-only entry has no candidate value to stage first.
11. Append and required-sync exactly one signed RefUpdate record to the shared log container.
12. Confirm pointer/log agreement, then drain the active WAL and remove active ref metadata.

The implementation is designed so interruption recovery lands on a checkable previous ref state or a
checkable new published state. That statement is bounded by the current evidence: unit/integration
tests, no completed crash-matrix or fuzzing campaign, and gates exercised on Linux, macOS, and Windows
(the `macOS mutation test suite` and `Windows mutation test suite` CI jobs run the suite that compiles
there natively on `macos-latest`/`windows-latest` — 2,211 and 2,152 tests respectively in CI on
`914f959d`, the last commit before 0.48.0's release commit, against 2,445 on Linux's `stable` job,
since some tests are Linux-only) — with the caveat
that DC-76's negative controls (eight remain; G5 retired in DC-98) are only partly demonstrated on
Windows: G1, G2, G4, and G9 are, but G3 and G8 still rely on a failpoint injection mechanism that
exists only on Linux/macOS, and G6/G7 have no Windows analogue at all. See
[platform support](./platform-support.md) for the per-guarantee table.

If the active WAL's Patch IDs already match the current published tip, seal reconstructs the expected
no-clock RefUpdate and finishes any exact one-record pointer lead before cleanup. An existing complete
matching record is not duplicated. If the already-published transition cannot be checked exactly,
seal fails closed.

## Required Filesystem Boundaries

Authoritative directories are traversed through anchored no-follow handles on the supported Linux and
macOS mutation paths. Missing directories are created one component at a time, and each new name is
established by syncing its parent before descent. Retry also re-syncs the parent of an observed
component instead of treating presence as proof of earlier durability.

Reads, metadata checks, and directory listings that authorize a mutation use the same retained root
as the mutation. Replacing the visible worktree or `.prikk` path therefore cannot redirect a
check-then-mutate workflow to a different tree. Append retries classify an exact retained complete
record without duplicating it and re-sync the file and parent; required removal re-syncs its retained
parent even when the final entry is already absent.

Mutable metadata publication uses a unique same-directory exclusive temp, complete file sync, atomic
replace rename, and required parent sync. An error after rename leaves the final name in place and
returns failure for verification or retry; it does not blindly roll back visible state.

Immutable object publication uses a separate no-clobber operation. It syncs a unique same-shard temp,
installs the final name without replacement, syncs the shard, removes only its invocation-owned temp,
and syncs the shard again. If another publisher wins, success requires same-handle validation and
exact persisted-byte equality; malformed, wrong-identity, wrong-type, or byte-different winners fail
without replacement. Crash-left object temps are warning-only debris and are never object authority.

Worktree writes and removals use separately named strict operations. Their errors propagate and may
leave partial worktree effects, but the worktree does not become repository authority. Lock removal
from a guard destructor remains explicitly best-effort because destruction cannot return an error.

## Ref Pointer and Ref Log Recovery

Ref publication uses a signed RefState object, a signed inline RefUpdate log record, and a mutable ref
pointer entry in a shared, append-only container. The pointer is useful for fast lookup, but it is not
trusted by itself.

The ref store validates branch ref names, holds a ref-specific lock, rechecks the expected current
RefState ID, then durably appends the new pointer entry — the publication commit point — before
appending the committed log record. An append-only entry has no candidate value to stage first, so
there is no separate write-then-promote step and no candidate-cleanup diagnostic anymore: the append
either lands durably or it does not.

Verification jointly classifies pointer and log state. A pointer leading the log by exactly one
expected transition is an interrupted publication and makes `verify` fail. If the final log frame is
structurally incomplete (a genuine tail, never a complete damaged record), the completion path below
may truncate and sync only the container's own trailing incomplete suffix before the append; a torn
tail belonging to one ref never enters any other ref's own filtered record sequence, so it cannot
block a different ref's own publish or repair. Fully framed checksum-invalid or malformed records are
never truncation-safe — `--repair-tails` refuses outright on them (RFC 164 §9.2).

Pointer/log agreement with the matching active WAL and metadata still retained is incomplete cleanup,
not a healthy repository state. Verification returns non-zero and unrelated mutation remains blocked
until the completion path below revalidates the transition, appends nothing new, and removes active
state.

### `prikk ref complete <ref>` (RFC 165 R4)

The general completion verb — `seal`'s own DC-38 retry is now one instance of the same mechanism,
not a separate path. Every condition evaluated before any write, in order: (a) the leading `RefState`
verifies under the current trust policy, signed by an adopted maintainer key — **any** adopted key,
not only the one that started the publication; (b) it chains — its ref name, its previous state equal
to the log's own tip, the next sequence; (c) its target exists, with the kind the ref requires; (d)
for a publication that consumed the active WAL (`seal`, `sync seal`), the retained WAL evidence still
matches; (e) no complete damage anywhere in the ref log or the pointer index. Any one failing refuses,
writing nothing.

**K1:** `--plan-only` prints the ref, the leading `RefState` id, its signer key id, its target and
kind, the log tip it chains to, the sequence, the completing key, and any partial tail it would
remove — the identical plan a real run prints before writing, both from the same one computation.

**The write:** one signed ref-log record, by the completing key, through the exact same write path
`publish_locked`'s own completion branch already uses — never a new code path for the append itself.

### `prikk doctor --rebuild-pointer-index` (RFC 165 R5)

The way out when the *pointer index* — not the ref log — is what is damaged or untrustworthy: a
complete damaged pointer-index record, or a stale pointer reading behind the log (the common cause of
the latter: the damaged record is the ref's own *newest* one, so the damage-tolerant reader falls back
to an older, already-sound entry). Re-derives every ref's own state from the ref log directly,
structural and **never trust-filtered** — no signature is re-checked for a record already durable in
the log, only a *current* lead re-enters trust (the same R4 rule above, reused, not re-derived).

**It refuses, writing nothing**, if the ref log itself has damage or a tail (repair that first — the
rebuild's own source of truth must be trustworthy before anything is derived from it), or if **any**
lead anywhere in the repository is completable: completing it is the correct fix, and a rebuild would
otherwise drop an authorized transition, the one outcome this verb must never produce. A lead that
fails R4's rule is **dropped** and named; a merely stale pointer (behind the log) is **restored**, a
distinct outcome from a dropped lead — nothing authorized is discarded restoring one.

**K1:** `--plan-only` prints, per ref, its state before and after, and every dropped lead or restored
ref — the identical plan a real run writes from.

**The write:** truncates the ref-pointer index's other slot, writes the rebuilt records, then switches
the generation log to it — atomic, as `compact` already does for the same container; the old slot
survives until the next compaction.

## A Write Never Buries a Crash State (RFC 163)

**The rule, at six files: before an append, the writer confirms under its lock that the file ends at
its last sound record. If it does not, it refuses before appending to that file**, naming the file, the
byte offset where the sound content ends, how many bytes follow, and the way out. For four of the six
(the trust-key and trust-policy containers together, the received index, and every generation log) this
is also a whole-operation guarantee: the command writes nothing at all when it refuses. For the other two
(the pointer index and the author-key container) it is narrower: only the guarded file itself is left
untouched — see each bullet below for what else the command may already have written, and why that is
harmless. Before this round, a torn
tail that `verify` already accepted as harmless (the pointer index, under RFC 162 rule 3 above) or said
nothing about at all (the other five, still under the pre-0.48.0 shape rule, N2) was invisible to the
*next ordinary write* at these files: the write appended behind it, blind, and turned an accepted crash
state into permanent damage — `verify` failing for good, and on some of these files a `seal` or `commit`
refused too.

**RFC 164 Rule A (0.49.0) extends the same tail-by-position rule to all six of these files' own read
and repair paths, not only the write-side refusal above.** Before this: a torn *prefix* at these files
already read as a repairable-looking tail, but 100 or more zero or random bytes at the same position
read as *damage* under their own pre-0.49.0 shape rule -- readers refused ("\<container\> has a damaged
entry"), and no repair verb existed for any of the six (or the three generation logs, listed together
with them below). Now: a tail is a record whose header or body is incomplete, or bytes that are not a
record header at all (no magic, or an unknown version), when nothing sound follows -- a torn prefix,
zeros, or garbage all count, the same rule RFC 162 rule 3 already gave the WAL and the pointer index --
and readers tolerate it. `verify`/`doctor` report a tail (a warning, naming the file, the offset, the
byte count, and the repair) and interior damage (a sound record following bad bytes) separately, and
`prikk doctor --repair-tails` truncates every tail across all nine covered files (these six, the three
generation logs, the WAL, and the pointer index) in one run, saving what it removes first, refusing
before touching anything if any covered file has interior damage -- see "Doctor Repair Boundary" below
for the full mechanism. The bullets immediately following describe the write-side refusal RFC 163
already gave each file, which RFC 164 does not change.

**RFC 164 §9 (Addendum 1, 0.49.0): a complete record is never a tail, even when it is last.** Rule A's
first accepted text said "whatever its shape," which let a *complete* record (its own header valid,
its whole claimed body present) whose checksum or envelope fails be truncated as if it were a crash's
torn remnant. It is not one: a crash can only interrupt a write, so everything a crash leaves is
*incomplete*; a complete record was fully written, and a failing checksum on it is corruption *after*
the fact. For files whose last record carries a decision -- the trust policy's latest snapshot, a
generation log's live slot, the pointer index's newest pointer -- silently removing it re-asserts the
*previous* decision: measured on a real build, a single flipped byte in the trust policy's newest
snapshot, followed by the pre-§9 repair, silently re-trusted a key that snapshot had just revoked, with
`verify` exiting 0 throughout. §9 corrects this for the seven Rule-A files and the pointer index (the
WAL keeps RFC 162 rule 3 exactly as it was -- see below): a complete record whose checksum or envelope
fails is damage, even when last. `verify` fails and names it; `--repair-tails` and
`--repair-pointer-index-tail` refuse and change nothing; readers fail closed on it exactly as they do
on any other interior damage, never silently resolving to an older, undamaged state. The cost, accepted
knowingly: a single corrupted byte in the last complete record of one of these files now stops the
commands that read that file (for the pointer index's own generation log, that is nearly every
command), where before it was quietly repaired into a rollback. The way out is restoring the file from
a copy, until the F1 round (0.49.0 step 2) can rebuild the pointer index from the ref log. **The WAL is
deliberately not part of this correction**: removing its own damaged last record loses a queued commit,
which is saved to `.prikk/recovery/` and disclosed (N6) -- a loss of work in progress, not a rollback of
already-committed trust or ref state, and N6's own witness (0.49.0 step 3) will let a genuine crash be
told apart from later damage there without this trade-off at all.

**RFC 164 §9.2 (Addendum 2, 0.49.0): the checksum decides whether a record is complete, not its header.**
§9 decided completeness from the header's own magic, version, and length fields -- but any one of those
three can itself be the single corrupted byte a complete write left behind, no more a crash's own
signature than a flipped body byte is. A flipped **header** field read as a tail under §9's own rule, and
every reader rolled back to the state before the last record *before any repair even ran* -- measured on
a release build: a single flipped magic, version, or length byte in the trust policy's last snapshot, or
in the pointer index's or its generation log's, and `trust maintainer check`/`branch list` showed the
previous state immediately, with no `--repair-tails` involved at all. Corrected: at a tail candidate, the
checksum is recomputed with the format's own real magic and version constants -- never the stored,
possibly-corrupted bytes at that offset -- over two candidates: the body the stored length claims, and
the body that runs to the end of the file (catching a corrupted length field itself). A match either way
means the record is complete, whatever its header says; only genuine corruption of the checksum or body
itself still resolves as a tail when nothing sound follows. One shared helper
(`frame_resync::complete_by_checksum`), called by all six decoders (trust keys, trust policy, author
keys, the received index, the three generation logs, the pointer index) and their write-side tail scans
alike, before any of those three header fields is trusted. **Verified exhaustively, not sampled**: a
store-level test per decoder flips every single byte offset of one complete record (decode only, cheap)
and asserts none of them decodes to a tail; on a release build, flipping every offset of a real last
record across the trust policy, the pointer index, and its generation log (263 offsets total) produced
zero rollbacks, before or after the repair, in every case. **Still open:** corruption spanning more than
one field of the last record at once (a zeroed sector, say) can still read as a tail -- this rule
resolves a single field's own corruption against an otherwise-intact record, not multiple fields
corrupted together; a per-record witness (format 8) is what would tell that shape apart reliably, and is
not yet built. The WAL is untouched by §9.2 too, for the same reason §9 leaves it alone.

**RFC 164 Rule D (0.49.0): a publishing command's own content object is now written only after the
pointer index's own tail/damage check, not before.** Before this round, the pointer-index bullet below
described a real gap: `seal`, `tag create`, `merge`, and `sync seal` each wrote their own new Patch and
Block objects first, and only reached the pointer index's own compare-and-swap check -- the same read
that now also checks the tail -- immediately before the RefState object that publication itself writes.
A refusal there left the Patch and Block already durably written, unreferenced by anything until the
pointer index moved; harmless (nothing references them, `verify` exits 0), but unnecessary work repeated
on every retry until the tail is repaired. Each of these four commands now calls the same tail/damage
check once more, at the very top of the command, before that earlier content write -- `branch create`
needed no change, since it writes no content object of its own before reaching the pointer index at all
(see below). The author-key container gets the same treatment for a new author's first commit: the
check that used to run immediately before the WAL append now also runs at the top of
`author_worktree_patch`, before the commit's blob, object-index, commit-index, and lifecycle-cache
writes. In every case, the retry after `prikk doctor --repair-pointer-index-tail`/`--repair-tails` still
reuses whatever was already durably written rather than writing it again -- that half of each bullet
below is unchanged; what moved is only how much gets written before the file's own tail is ever checked.

**Refuses only when that write would actually append.** An operation that turns out to be a no-op for
one of these files — re-adding a maintainer key already adopted under the same public key, removing one
that was never adopted, a commit by an author whose key material this repository already recorded —
appends nothing, and is unaffected by a tail on the file it would otherwise have written to. The check
still runs, and still reads what the operation already reads; it is the *refusal* that is conditioned on
whether an append is actually about to happen, not the read.

**The six files, and the way out:**

- **The pointer index.** `seal`, `tag create` (and `sync adopt-tag`), `merge`, and `sync seal` each call
  the pointer index's own tail/damage check at the very start of the command (RFC 164 Rule D, 0.49.0) —
  before writing their own Patch or Block objects — naming `prikk doctor --repair-pointer-index-tail` on
  an unclean tail; the repair already exists (RFC 162). The same check still runs again, redundantly and
  harmlessly, at the compare-and-swap step every publication performs immediately before its own RefState
  write. `branch create` writes no content object of its own before that compare-and-swap step, so for it
  this one check was already both the first and the only content write in the command — it carries no
  separate early call. **This is a claim about the pointer index file only, not about the whole
  command**: a refusal over the tail itself now precedes every content-object write these commands make,
  so a retry after `prikk doctor --repair-pointer-index-tail` starts clean rather than reusing objects a
  refused attempt left behind.
- **Trust keys and trust policy, together.** `trust maintainer add`/`remove` refuse the same way,
  **entirely before either container's first write** — a fix in this same round, after review: the first
  shape checked and appended to the trust-key container, then checked the trust-policy container, so a
  torn *policy* tail alone (the key container clean) left the key recorded but the policy refused, a
  half-applied write. Both containers' tails are now decided before either is appended to, so the whole
  operation writes nothing at all when it refuses, whichever container's tail caused it (RFC 163 §2
  Addendum 2). **The way out (0.49.0): `prikk doctor --repair-tails`**, then retry `trust maintainer
  add`/`remove`.
- **The received index.** `bundle import` (never `sync accept`, which does not touch the received
  namespace) refuses the same way, **entirely before its first write** — before even the bundle's own
  objects are written, the same pre-write phase the author-key check already ran in (0.44.0,
  GHSA-px5q-233r-6hq5: a refused import must write nothing at all). **Also refuses on a damaged entry, not
  only a torn tail** (external review 016, N9, fixed the same round the guard's tail-only walk was added):
  before the fix, the same walk resynced silently past 100 or more zero or random bytes and appended
  behind them, burying damage `verify` had already reported — the one guarded writer where that held,
  since the four sibling files below already refuse through a lookup that fails on any damaged entry.
  **The way out for a tail (0.49.0): `prikk doctor --repair-tails`**, then retry the import; genuine
  interior damage (a sound record follows the bad bytes) still gives no truncation advice, the same as
  the sibling files' own damaged-entry case below.
- **The author-key container.** A commit by an author key id not yet recorded now refuses on this check
  at the very start of `author_worktree_patch` (RFC 164 Rule D, 0.49.0), before the commit writes its own
  new blob content, an object-index entry, or updates the commit-index and lifecycle caches. The same
  check still runs again, redundantly and harmlessly, immediately before the WAL append that would queue
  the commit — exactly where it ran before this round, and where a commit by an already-recorded author
  key id still only ever reaches (see "Refuses only when that write would actually append" above). **The
  way out (0.49.0): `prikk doctor --repair-tails`**, then retry the commit.
- **The generation log, at each of the three compacting containers** (the pointer index, the received
  index, the trust policy container). `compact` refuses the same way, **before its first write** — before
  the retired slot is truncated, not only before the generation record itself — and only in `--execute`
  mode: a `--plan-only` preview writes nothing and is unaffected by a tail on a log it will never write
  behind. **The way out (0.49.0): `prikk doctor --repair-tails`**, then retry `compact`.

**Where each check reads from.** No new whole read was added where an existing one could carry the
answer: the pointer index's guard rides the same replay `ensure_current_matches`'s own compare-and-swap
check already performs; the trust-key and trust-policy guards ride the same replay
`add_trusted_maintainer`/`remove_trusted_maintainer` already perform to compute the current key id list
and look up the key being added; the author-key guard rides the same replay
`check_author_key_conflict` already performs; the generation-log guard rides the same replay
`resolve_live_slot` already performs to pick the live slot. Only the received index's guard is a new read, moved to
`bundle import`'s own pre-write phase: `write_received_pointer` reads nothing before appending today
(there is no CAS to enforce), so there was no existing read to build the check on. It grows with every
import that records a pointer, not only with the number of distinct remote refs (`compact` is what
reclaims stale entries) — measured on a release build at 1,000 and 10,000 entries, a lean tail-only walk
(no entry decoded, no allocation per record) against a full decode: 1,000 entries (159 KB), 182 µs
against 256 µs; 10,000 entries (1.6 MB), 1.65 ms against 2.01 ms. Both are still linear in the file's own
size — checksumming every byte is unavoidable for telling a sound record from damage — so this is a
bounded saving, not a change of complexity class; at these sizes the absolute cost is small either way.

**Not covered, on purpose.** The object index keeps RFC 162 rule 1 (a writer rebuilds it before
appending, rather than refusing). The object containers keep rule 2's connectivity classification, but
now also report an aggregate tail count per persisted object type (RFC 164 Addendum 1, N7:
`verify`/`doctor` print "trailing partial \<type\> container bytes" for each of the seven persisted
object types, 0 for a clean container) — reporting only, no repair; a frame that does not parse is
still classified as before (a warning if its own checksum never verified, connectivity-checked damage
otherwise), unchanged. The ref log keeps its own positive truncation rule (it truncates only a suffix
that is a prefix of the record it expected to write next) — **the ref log is not in this round's
scope**: a crash inside `branch create`/`tag create` leaving its own ref-log record torn, and a later
`seal` of a *different* ref appending behind it, is disclosed, not fixed, in `current-state.md`'s known
limitations (N3), alongside F1 in 0.49.0.

## Doctor Repair Boundary

`doctor --repair-wal-tail` acquires the active lock and truncates incomplete trailing active-WAL bytes
after an under-lock publication guard and verification have accepted the preceding WAL prefix.
`doctor --repair-index` rebuilds the object index from the containers under the object-store lock — the
index is a pure cache, never touched by any other repair. `doctor --repair-pointer-index-tail` truncates
an incomplete trailing pointer-index record under the pointer-index lock, mirroring `--repair-wal-tail`
exactly. **`doctor --repair-tails` (RFC 164 Rule C, 0.49.0)** truncates every tail across all nine
covered files (the WAL, the pointer index, and the seven RFC 164 Rule A files) in one run: `ActiveLock`
first (covering the WAL, trust keys, and author keys, none of which has its own dedicated container
lock), then the pointer-index, received-index, and trust-policy container locks together (each also
covering its own generation log) — reusing `--repair-wal-tail`'s and `--repair-pointer-index-tail`'s
own repair functions for the WAL and the pointer index, unchanged. It reads all nine files once, before
touching any of them: if any one has interior damage, it refuses immediately, naming every such file,
and truncates nothing anywhere. Otherwise each file with a tail is truncated under its own lock, saving
the removed bytes to `.prikk/recovery/` first; a file with no tail is reported clean. Mutually exclusive
with the other four repair flags in one invocation (it already covers the WAL and the pointer index).
**None of the four repair verbs ever removes a sound record**: each truncates or rebuilds only what a
true torn tail or a pure-cache rebuild covers (see above), and on genuine damage each refuses and changes
nothing. **This held exactly to the letter of "whatever its shape" until RFC 164 §9** (Addendum 1,
0.49.0): a complete record whose checksum failed used to satisfy "whatever its shape" too, and both
`--repair-tails` and `--repair-pointer-index-tail` would truncate it -- removing a record that was, in
fact, sound in every way except its own checksum, and rolling back the decision it carried. §9 closed
that for a corrupted body byte; **§9.2 (Addendum 2) closed the matching gap for a corrupted header
field** -- a flipped magic, version, or length byte rolled readers back *before either repair verb even
ran*, so completeness is now decided by the checksum (above), not by the header. Doctor diagnoses
ref-publication states but does not sign, append, promote, or reconstruct ref authority.

The [integrity and recovery diagnostics](./integrity-recovery.md) reference owns the full diagnostic
catalog: verification checks, `DoctorIssue` codes, severities, and diagnostic interpretation. This
page intentionally does not duplicate that catalog.

Doctor repair refuses to modify the repository when verification has error-severity issues. It also
does not auto-trust keys, repair signatures, repair checksum mismatches, rebuild missing objects,
recover missing key material, or clear unsafe active sessions.

## Stale Locks and Manual Repair

`active.lock` is acquired with exclusive file creation. If a process dies while holding it, stale lock
cleanup is manual today. DC-28 does not define lock stealing, lock expiry, process ownership checks, or
automatic stale-lock repair. The current lock and compare-and-swap behavior is covered by the
[concurrency and locking](./concurrency-locking.md) reference.

## Deferred Work

Still deferred: the broad crash-matrix campaign, fuzzing for WAL/ref-log recovery,
macOS and Windows filesystem validation, stale-lock policy, broad active-session recovery, ref-log
repair, missing-object recovery, object quarantine or garbage collection, multi-ref backup export,
a rehearsed repository-format-migration restore, stable repository-format migration, and
production-readiness claims. Single-ref backup/restore tooling is no longer deferred —
`prikk bundle export`/`verify`/`import`, see [Backup and Restore](../guide/backup-restore.md).

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| Commit persistence appends exact signed Patch envelopes to the active WAL, required-syncs the WAL file, and required-syncs the parent directory after every append. | [`wal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/wal.rs), [DC-37](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-37-REQUIRED-FILESYSTEM-DURABILITY.md) |
| WAL replay reports incomplete trailing bytes separately from complete-record checksum failures. | [`wal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/wal.rs), [PR-004](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-004-WAL-HANDOFF.md), [PR-006](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-006-VERIFY-HANDOFF.md) |
| WAL-tail repair truncates only a trailing prefix of the last frame, preserving every complete, sound record behind it; a damaged record with sound records behind it refuses rather than truncating past it (unchanged since RFC 162), and since RFC 166 it also refuses rather than remove a record the active session's own commit witness already names as acknowledged. | [`wal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/wal.rs), [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [`commit_boundary/classification.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/commit_boundary/classification.rs), [PR-012](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-012-DOCTOR-REPAIR-HANDOFF.md), [RFC 166](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/166-a-queued-commit-has-a-witness.md) |
| `--discard-damaged-commits` removes an acknowledged commit the active WAL no longer holds soundly, or declares it lost, saving the removed bytes first, all-or-nothing, the same way `--repair-wal-tail` does; `--restore-queue-target --ref <ref> [--not-current-branch]` gives an owned queue its owner back by one atomic replace of the active ref metadata, with the ref always supplied by the caller, never read from the witness, and accepted only when a readable commit record agrees or (absent one) it is the caller's own current branch or `--not-current-branch` is given (RFC 166 §14). Both are `--plan-only`-previewable and refuse, writing nothing, over every row they do not act on. | [`doctor/discard_damaged_commits.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor/discard_damaged_commits.rs), [`doctor/restore_queue_target.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor/restore_queue_target.rs), [RFC 166 D5/§14](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/166-a-queued-commit-has-a-witness.md) |
| Non-empty active WALs require valid active-ref ownership metadata; empty-WAL metadata debris is separate local debris. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [DC-15](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) |
| Seal rejects trailing partial WAL bytes, missing/malformed active ref metadata, and mismatched active ref ownership before publication. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [DC-15](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) |
| Seal persists WAL Patches, creates signed Block and RefState objects, durably appends the pointer commit point, appends exactly one signed RefUpdate, confirms agreement, then drains active state. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [`refs/publication.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/publication.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) |
| Seal verifies the configured MAINTAINER signer against repository-local trust before publication. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [DC-11](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-11-MAINTAINER-TRUST-STORE.md) |
| Ref publication uses ref-specific locking, compare-and-swap checks, signed RefState/RefUpdate envelopes, pointer-first commit, and an idempotent exact log append. | [`refs/publication.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/publication.rs), [`refs/pointer_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/pointer_index.rs), [`refs/container.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/container.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) |
| Immutable object publication never replaces an existing final name; existing or concurrent winners require valid identity/type and exact persisted-byte equality, while recognized crash-left temps remain warning-only debris. | [`object_store.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/object_store.rs), [`immutable.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/fsutil/anchored/immutable.rs), [DC-36](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-36-EXISTING-OBJECT-PUBLICATION-INTEGRITY.md) |
| Doctor's `--repair-main-ref` input is recognized but always refused and performs no repair; `prikk doctor` itself signs and appends nothing. | [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) |
| Interrupted ref publication completion (`prikk ref complete <ref>`) requires an adopted maintainer signer — any adopted key, not only the one that started the publication — and, only for a WAL-consuming publication (`seal`, `sync seal`), matching retained WAL evidence. | [`ref_completion.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/ref_completion.rs), [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [RFC 165](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/165-ref-publication-one-read-a-log-that-speaks-and-a-way-out.md) |
| The ref-pointer index rebuild (`prikk doctor --rebuild-pointer-index`) re-derives it from the ref log alone, structural and never trust-filtered; it signs nothing. | [`pointer_rebuild.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/pointer_rebuild.rs), [RFC 165](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/165-ref-publication-one-read-a-log-that-speaks-and-a-way-out.md) |
| Doctor began as read-only diagnostics, and current mutating repairs remain opt-in and narrow. | [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [PR-011](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-011-DOCTOR-HANDOFF.md), [PR-012](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-012-DOCTOR-REPAIR-HANDOFF.md), [PR-013](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-013-REF-RECOVERY-HANDOFF.md) |
| Ref pointers are mutable, not roots of trust. | [`refs.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs.rs), [`refs/pointer_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/pointer_index.rs), [data model](./data-model.md) |
| Durability/platform claims remain limited by current test evidence and gates exercised on Linux, macOS, and Windows. | [DC-24 baseline recap](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-24-data-model-trust-threat-docs/baseline-recap.md), [DC-24](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-24-DATA-MODEL-TRUST-THREAT-DOCS.md), [DC-28](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-28-DURABILITY-CRASH-RECOVERY-REFERENCE.md) |

## Provenance

This reference follows the DC-26 documentation-home model: current-state references live in the
published mdBook, not under `rfcs/fdds/`. Its required-sync and ref-publication sections were last
updated for the DC-37 and DC-38 implementations, reviewed as part of the combined 0.18.0 release.
