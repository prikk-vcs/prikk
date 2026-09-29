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

Incomplete trailing bytes are reported separately as trailing partial bytes. **A torn tail is a prefix
of one well-formed frame, and nothing else**: the remnant of an append that was interrupted, at the end
of the file. If a sound frame (magic, a valid header, a body that passes its checksum) starts anywhere in
the remainder, the partial frame is not a tail; it is damage, reported as a failed record at its offset,
and the sound records after it are still read. When the evidence is ambiguous the answer is damage, never
tail. A true torn tail is the only WAL truncation case that `doctor --repair-wal-tail` handles. A complete
record with a checksum mismatch, malformed header, unsupported version, or malformed envelope, and a
partial frame with a sound frame behind it, are integrity failures and are not safe automatic truncation
candidates: the repair refuses and leaves the file as it was, naming the damaged offset and how many sound
records follow it. (Before 0.48.0 a damaged length was read as a tail whatever followed it, and the repair
truncated the sound records away.)

**A repair keeps every byte it removes.** A record whose only fault is a damaged length, with nothing sound behind it, is
indistinguishable from an interrupted append, so `--repair-wal-tail` truncates it. Before it does, it writes exactly the bytes it will remove to
`.prikk/recovery/wal-<session>-at-<offset>-<hash>.bytes` (durably, under the same lock), and only then truncates; its output names the file. The
file is the raw WAL bytes, so a record that was removed by mistake can be read back from it: the removed region starts at the named offset of the
old WAL, with the same framing. If saving the file fails, nothing is truncated. The file is never authority: `verify` ignores it, and it can be
deleted once it is not needed. It also means a repair can be wrong about what it removed without anything being lost, torn tail or damage.
**The torn-tail-is-a-prefix-of-one-frame classification applies to every framed file** — the object containers and
index, the ref log and pointer index, the received, author-key and trust indexes, and the generation file — but
**only three repairs write a recovery file today:** the WAL repair, the pointer index's own
`--repair-pointer-index-tail` (mirroring the WAL exactly), and the object index's own rebuild (its lost ids, when it
cannot re-derive an entry). The
ref log's own tail truncation, run automatically as part of a signer-backed seal's interrupted-publication recovery
rather than as its own `doctor` verb, truncates directly and saves nothing first: a ref-log record removed by mistake
this way cannot be read back.

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

## Active Ref Metadata

The active WAL is paired with active ref metadata that records which local branch ref owns the
non-empty WAL. A non-empty active WAL with missing or malformed active ref metadata is an
active-session integrity issue. Seal refuses that state rather than guessing which ref should receive
the WAL records.

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
expected transition is an interrupted publication and makes `verify` fail. Signer-backed `seal` retry
may append the exact deterministic RefUpdate after revalidating retained WAL and trust. If the final
log frame is structurally incomplete, that same path may truncate and sync only the container's own
trailing incomplete suffix before the append; a torn tail belonging to one ref never enters any other
ref's own filtered record sequence, so it cannot block a different ref's own publish or repair. Fully
framed checksum-invalid or malformed records are never truncation-safe.

Pointer/log agreement with the matching active WAL and metadata still retained is incomplete cleanup,
not a healthy repository state. Verification returns non-zero and unrelated mutation remains blocked
until signer-backed seal revalidates the transition, appends nothing, and removes active state.

## Doctor Repair Boundary

`doctor --repair-wal-tail` acquires the active lock and truncates incomplete trailing active-WAL bytes
after an under-lock publication guard and verification have accepted the preceding WAL prefix.
`doctor --repair-index` rebuilds the object index from the containers under the object-store lock — the
index is a pure cache, never touched by any other repair. `doctor --repair-pointer-index-tail` truncates
an incomplete trailing pointer-index record under the pointer-index lock, mirroring `--repair-wal-tail`
exactly. **None of the three ever removes a sound record**: each truncates or rebuilds only what a true
torn tail or a pure-cache rebuild covers (see above), and on genuine damage each refuses and changes
nothing. Doctor diagnoses ref-publication states but does not sign, append, promote, or reconstruct ref
authority.

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
| WAL-tail repair truncates only incomplete trailing bytes and refuses complete-record integrity failures. | [`wal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/wal.rs), [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [PR-012](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-012-DOCTOR-REPAIR-HANDOFF.md) |
| Non-empty active WALs require valid active-ref ownership metadata; empty-WAL metadata debris is separate local debris. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [DC-15](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) |
| Seal rejects trailing partial WAL bytes, missing/malformed active ref metadata, and mismatched active ref ownership before publication. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [DC-15](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) |
| Seal persists WAL Patches, creates signed Block and RefState objects, durably appends the pointer commit point, appends exactly one signed RefUpdate, confirms agreement, then drains active state. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [`refs/publication.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/publication.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) |
| Seal verifies the configured MAINTAINER signer against repository-local trust before publication. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [DC-11](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-11-MAINTAINER-TRUST-STORE.md) |
| Ref publication uses ref-specific locking, compare-and-swap checks, signed RefState/RefUpdate envelopes, pointer-first commit, and an idempotent exact log append. | [`refs/publication.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/publication.rs), [`refs/pointer_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/pointer_index.rs), [`refs/container.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/container.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) |
| Immutable object publication never replaces an existing final name; existing or concurrent winners require valid identity/type and exact persisted-byte equality, while recognized crash-left temps remain warning-only debris. | [`object_store.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/object_store.rs), [`immutable.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/fsutil/anchored/immutable.rs), [DC-36](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-36-EXISTING-OBJECT-PUBLICATION-INTEGRITY.md) |
| Doctor's `--repair-main-ref` input is recognized but always refused and performs no repair; exact interrupted ref publication completion requires retained active evidence and a trusted signer. | [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) |
| Doctor began as read-only diagnostics, and current mutating repairs remain opt-in and narrow. | [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [PR-011](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-011-DOCTOR-HANDOFF.md), [PR-012](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-012-DOCTOR-REPAIR-HANDOFF.md), [PR-013](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-013-REF-RECOVERY-HANDOFF.md) |
| Ref pointers are mutable, not roots of trust. | [`refs.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs.rs), [`refs/pointer_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/pointer_index.rs), [data model](./data-model.md) |
| Durability/platform claims remain limited by current test evidence and gates exercised on Linux, macOS, and Windows. | [DC-24 baseline recap](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-24-data-model-trust-threat-docs/baseline-recap.md), [DC-24](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-24-DATA-MODEL-TRUST-THREAT-DOCS.md), [DC-28](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-28-DURABILITY-CRASH-RECOVERY-REFERENCE.md) |

## Provenance

This reference follows the DC-26 documentation-home model: current-state references live in the
published mdBook, not under `rfcs/fdds/`. Its required-sync and ref-publication sections were last
updated for the DC-37 and DC-38 implementations, reviewed as part of the combined 0.18.0 release.
