# Integrity and Recovery Diagnostics

This page is the authoritative current-state reference for Prikk's repository verification and doctor
diagnostics. It describes what `prikk verify` checks, what it does not prove, how `prikk doctor`
interprets verification results, and which repair boundaries are intentionally narrow.

For the storage recovery mechanics behind WAL-tail truncation and signer-backed ref completion, see the
[durability and crash recovery](./durability-recovery.md) reference. For trust scope, see the
[trust and threat model](./trust-threat-model.md). For operator key input and local maintainer trust
setup, see the [security and signing setup](../guide/security-setup.md) guide.

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- `prikk verify` is read-only.
- `verify` checks structural integrity and current repository-local publication trust for publication
  objects; it is not a global trust proof.
- There is no repository-wide AUTHOR trust enforcement.
- MAINTAINER key revocation exists (`prikk trust maintainer remove`), but there is no historical PKI
  (temporal/point-in-time revocation semantics), key rotation, hardware signing, remote trust, sync
  trust, or stable migration policy yet.
- `prikk doctor` repairs are opt-in and narrow.
- Doctor recommendations are human guidance, not an automated recovery policy.
- Prose output fields, counters, severity labels, and issue-code names are current CLI vocabulary, not a
  stable machine-readable schema. `prikk verify --format json` emits the versioned `verify-report-v1`
  document (`args.rs:147`; `output/verification.rs:124`); doctor has no JSON output.

## Verify Scope

`prikk verify` calls the repository verification layer and prints a read-only report. Current
verification covers:

- persisted object placement by object type directory and canonical object path;
- object envelope decoding and recomputed object identity;
- Block payload decoding and references to parent Blocks, Patch objects, and optional snapshot Blobs;
- joint ref pointer, RefState-chain, and ref-log-chain consistency;
- signed RefUpdate log record decoding;
- active WAL replay, including trailing partial WAL byte reporting;
- object-index and pointer-index trailing-partial and interior-damage reporting (the object index is a
  pure cache — a reader scans the containers in memory when it is damaged, rather than refusing — so this
  is never a failure by itself; see [durability and crash recovery](./durability-recovery.md));
- connectivity: every object a sealed Block's state, a queued Patch in any active session's WAL, or a ref
  tip references must exist and be readable;
- received (`remotes/*`) pointers: each pointer's RefState, target, required attestations, and previous
  state (the `ReceivedRefs` stage, `verify.rs:1484-1489`, `verify.rs:1916-1960`);
- the active session's commit witness (acknowledged queued commits) against its WAL (the
  `CommitWitness` stage);
- whether active WAL Patch records already exist as persisted Patch objects;
- active WAL ref metadata health;
- active rollback-draft WAL record classification;
- sealed rollback Block and sealed rollback Patch classification;
- repository-local publication trust for Block, RefState, and RefUpdate envelopes;
- whether `.prikk/current-branch` resolves (0.50.0 step 1 A4) — a warning, never a failure: the
  pointer is a default `--ref` given explicitly always overrides, not an authority anything above
  checks.

Object enumeration, Block/RefState reads, active-WAL replay, active metadata, ref pointers, and ref logs
all use the same retained repository-root authority. Publication trust consumes the exact Block,
RefState, and RefUpdate envelopes returned by those anchored structural scans; it does not reopen
publication paths in a separate trust phase.

Publication trust and recognized ref-publication state issues are collected separately from hard
structural verification errors. This lets the command return command failure when trust is invalid or
a blocking interrupted-publication state exists.

## What Verify Does Not Prove

`verify` does not prove that a repository is globally trustworthy. It does not enforce
repository-wide AUTHOR trust, historical PKI semantics (temporal/point-in-time revocation tracking --
`verify` only ever checks against the current adopted-key snapshot), key rotation, remote identity,
remote trust, hosted forge policy, or thresholds beyond the current repository-local `required = 1`
maintainer policy.

`verify` also does not prove production readiness, stable repository-format migration, complete
cross-platform filesystem behavior, merge execution safety, semantic conflict resolution, backup
coverage, or successful recovery from every crash shape.

**`verify` reads the received-ref index.** Ref pointers imported by `prikk bundle import` live in
`refs/containers/received-index-{a,b}.container`. The `ReceivedRefs` stage checks each pointer's RefState,
target, required attestations, and previous state (`verify.rs:1916-1960`), and the index file's own tail
and interior damage are reported with the other appended files (`verify.rs:1273`). What `verify` cannot
see is a pointer that is absent from the index: it checks the entries the index names, and no doctor verb
rebuilds this index.

## Verify Output and Exit Behavior

The current CLI prints, among other lines: stage outcomes; object, block-state, ref-file, and ref item
counts; checked objects, Blocks, rollback Blocks, sealed rollback Patches, refs, ref-log records, WAL
records, persisted WAL Patches, and rollback draft WAL records; ref-publication issues; publication
trust records and issues; sealed Blocks; object temp warnings; connectivity issues; interrupted appends;
unreferenced remnants; trailing partial WAL, object-index, and pointer-index bytes; one line per appended
file and per object container tail; the active WAL metadata state; the acknowledged-commit (commit
witness) verdict; and commit-index, lifecycle-cache, and merge-baseline divergence counts. Object findings
are ordered by numeric object type and raw ObjectId bytes, followed by active WAL sequence, then unsigned
UTF-8 ref-name bytes and ref-log sequence.

The command exits with failure, and `--format json` reports `ok: false`, when any of these conditions
holds (`crates/prikk-cli/src/verify_verdict.rs`, `VERDICT_CONDITIONS`):

- `stage-failure`: a verification stage failed or did not complete;
- `item-failure`: an object, Block state, ref file or ref item, WAL record, or received ref failed; or a
  connectivity issue is present; or an appended file has interior damage. A connectivity issue is an
  object a sealed Block's state, a queued Patch, or a ref tip references that cannot be read. `prikk
  doctor` fails on the same condition (`PRIKK-DOCTOR-OBJECT-CONNECTIVITY`), scoped to the active session
  whose queued Patch references it when that is the source;
- `active-wal-metadata-integrity`: a non-empty active WAL has missing or malformed active ref metadata;
- `commit-witness-integrity`: an acknowledged queued commit disagrees with the WAL in a way the WAL's
  sound prefix cannot explain;
- `commit-witness-substituted-earlier-record`: the acknowledgment history disagrees with the WAL's running
  hash at a record before the last acknowledged one;
- `blocking-ref-publication`: a one-transition pointer lead, matching active state retained after
  completed publication, or an unproved pointer/log divergence;
- `publication-trust`: publication-trust issues are present;
- `commit-index-divergence`, `lifecycle-cache-divergence`, `merge-baseline-divergence`, or
  `active-wal-ordering`: the named check fails.

A structural verification error before a report can be produced also fails the command.

Trailing partial WAL bytes, trailing partial object-index bytes, object-index interior damage, trailing
partial pointer-index bytes, object container tails, interrupted appends, unreferenced remnants, object
temp files, and an unresolved `current-branch` pointer are printed as warnings. None of them fails
`verify` by itself; an appended file's interior damage does. The object and pointer indexes are caches or
repairable tails, so their damage is never a failure — including when the torn entry is the one most
recently written, which a reader now rescans the containers for rather than reporting as a missing
object (0.50.0 step 1 A5) — but it is never silent either. `current-branch` is a default, never an
authority (RFC 151 §2.1): nothing above reads it, and an explicit `--ref` always works regardless of
whether it resolves. **An absent pointer is not a warning** (every repository initialized before RFC
151, and every one of this repository's own fresh `init`s before the file was first written) — a
normal, unaffected state, reported as an informational line instead (0.50.0 step 1 Part D1):
`current-branch: not set; commands without --ref use heads/main`. The recovery mechanics and safe
truncation boundary are covered by the
[durability and crash recovery](./durability-recovery.md) reference.

## Active WAL Metadata States

`ActiveWalMetadataStatus` currently has six states:

| State | CLI meaning | Doctor issue |
|---|---|---|
| `MissingForEmptyWal` | Empty active WAL with no metadata. | Healthy; no issue by itself. |
| `ValidForEmptyWal` | Empty active WAL with stale but valid metadata. | Warning: `PRIKK-DOCTOR-ACTIVE-REF-METADATA-DEBRIS`. |
| `InvalidForEmptyWal` | Empty active WAL with malformed metadata. | Warning: `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED-DEBRIS`. |
| `ValidForNonEmptyWal` | Non-empty active WAL with valid ownership metadata. | Healthy; no issue by itself. |
| `MissingForNonEmptyWal` | Non-empty active WAL without ownership metadata. | Error: `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MISSING`. |
| `InvalidForNonEmptyWal` | Non-empty active WAL with malformed ownership metadata. | Error: `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED`. |

Only the non-empty missing/malformed states are active-session integrity issues. Empty-WAL metadata
states are local debris warnings because no WAL records need ownership for publication.

## Doctor Scope

`prikk doctor` is an actionable diagnostic layer over repository verification. When verification
completes, doctor prints the verification report, emits issue lines with severity, code, message, and
recommendation, then prints an issue summary.

When verification fails before a report can be produced, doctor emits a verification-error issue and
recommends preserving the repository before attempting repair.

Doctor output is intended for human diagnostics. The issue-code strings and severity labels are
current CLI vocabulary, not a stable JSON/API contract.

## Doctor Issue Catalog

Current doctor severities are `info`, `warning`, and `error`.

| Code | Severity | Meaning |
|---|---|---|
| `PRIKK-DOCTOR-VERIFY-OK` | `info` | The structural verification scan completed; later issue lines still determine health. |
| `PRIKK-DOCTOR-WAL-TRAILING-PARTIAL` | `warning` | Default session's WAL has trailing bytes that are a prefix of one incomplete final record (a true torn tail); `--repair-wal-tail` truncates exactly these. |
| `PRIKK-DOCTOR-ACTIVE-SESSION-WAL-TRAILING-PARTIAL` | `warning` | The same condition for a non-default active session; `--repair-wal-tail` truncates that session's bytes. |
| `PRIKK-DOCTOR-VERIFY-WAL-RECORD-INCOMPLETE` | `error` | A WAL record failed verification, including a partial frame with a sound record behind it (damage, not a tail); the message says how many sound records follow. No repair switch touches it. |
| `PRIKK-DOCTOR-OBJECT-CONNECTIVITY` | `error` | An object a sealed Block's state, a queued Patch, or a ref tip references cannot be read. Scoped to the active session whose queued Patch references it, when that is the source; no repair switch touches it. |
| `PRIKK-DOCTOR-OBJECT-INTERRUPTED-APPEND` | `warning` | An object container has a frame that does not parse. If connectivity reports a missing object of this container's type, the message names it as possibly held by that frame and says not to treat it as a harmless remnant; the connectivity finding is what needs repair. Otherwise no repair is required: an interrupted append that nothing still needs was never committed. |
| `PRIKK-DOCTOR-OBJECT-CONTAINER-TRAILING-PARTIAL` | `warning` | An object container ends in an incomplete final frame. No automated repair; connectivity reports whether anything still needs what the frame may hold. |
| `PRIKK-DOCTOR-UNREFERENCED-REMNANT` | `warning` | A stored object references a missing object, but nothing reachable from committed state needs it. The message reads `object <owner> references missing <role> <id>`. Harmless; no repair. |
| `PRIKK-DOCTOR-OBJECT-TEMP-DEBRIS` | `warning` | A non-authoritative object publication temp remains. Doctor preserves it and does not remove it. |
| `PRIKK-DOCTOR-OBJECT-INDEX-TRAILING-PARTIAL` | `warning` | The object index has trailing bytes that look like an incomplete final record. The object index is a pure cache (readers rescan the containers when it is damaged), so this is never a failure; `--repair-index` rebuilds it. |
| `PRIKK-DOCTOR-OBJECT-INDEX-INTERIOR-DAMAGE` | `warning` | The object index has an interior record that failed to decode. Same non-failure reasoning as the trailing-partial row; `--repair-index` rebuilds it. |
| `PRIKK-DOCTOR-POINTER-INDEX-TRAILING-PARTIAL` | `warning` | The pointer index has trailing bytes that look like an incomplete final record. `--repair-pointer-index-tail` truncates exactly these; unlike the object index, a *damaged* pointer-index entry (not merely a trailing partial) already fails the `Refs` stage outright rather than reaching this code. |
| `PRIKK-DOCTOR-APPENDED-FILE-TRAILING-PARTIAL` | `warning` | One of the eight appended files (the ref log is one of them) has an incomplete final record. `--repair-tails` truncates exactly these bytes. |
| `PRIKK-DOCTOR-APPENDED-FILE-INTERIOR-DAMAGE` | `error` | One of those appended files has a record that fails to decode. `--repair-tails` refuses it rather than guessing which bytes are safe to remove. |
| `PRIKK-DOCTOR-GENERATION-LOG-AMBIGUOUS` | `error` | Handoff 165 Q1b: a compacting container's generation log is lost, and its two slots fit two different, equally honest histories (the Q1 review's own H1/H2 proof) -- not damage, typed separately (`PrikkError::AmbiguousGenerationLog`). Printed first, before the `*-INTERIOR-DAMAGE` rows below, the current-branch warning, and any `VERIFY-STAGE-INCOMPLETE` error the same ambiguity causes. The recommendation names the rebuild for the ref pointer index, or a whole-`.prikk/` backup for the others. |
| `PRIKK-DOCTOR-POINTER-INDEX-INTERIOR-DAMAGE` | `error` | 0.50.0 P3c: the ref pointer index has a damaged entry, read directly rather than only through the `Refs` stage failure above. Printed first, before the current-branch warning and any `VERIFY-STAGE-INCOMPLETE` error the same damage causes (each of those names this code instead of "inspect the failing stage"). The recommendation names `prikk doctor --rebuild-pointer-index --plan-only`, then `prikk doctor --rebuild-pointer-index`. |
| `PRIKK-DOCTOR-RECEIVED-INDEX-INTERIOR-DAMAGE` | `error` | 0.50.0 P3c: the received index has a damaged entry, read directly. No repair exists; the recommendation names a copy of the whole `.prikk/` directory from a backup, never a single file. Printed first, the same as the pointer-index row above. |
| `PRIKK-DOCTOR-TRUST-POLICY-INTERIOR-DAMAGE` | `error` | 0.50.0 P3c: the trust policy container has a damaged snapshot, read directly. No repair exists; the recommendation names a copy of the whole `.prikk/` directory from a backup, then re-applying every trust change made since (an older copy could re-trust a revoked key). Printed first, the same as the pointer-index row above. |
| `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MISSING` | `error` | Default session's WAL has records but its ref metadata is missing. |
| `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED` | `error` | Default session's WAL has records but its ref metadata is malformed. |
| `PRIKK-DOCTOR-ACTIVE-REF-METADATA-DEBRIS` | `warning` | Default session's WAL is empty but stale valid ref metadata remains. |
| `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED-DEBRIS` | `warning` | Default session's WAL is empty but malformed ref metadata remains. |
| `PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-MISSING` / `-MALFORMED` | `error` | The same two conditions for a non-default active session; the message names the session. |
| `PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-DEBRIS` / `-MALFORMED-DEBRIS` | `warning` | The same two debris conditions for a non-default active session. |
| `PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-UNREADABLE` | `error` | A non-default active session's ref metadata failed to read. |
| `PRIKK-DOCTOR-ACTIVE-SESSION-WAL-UNREADABLE` | `error` | A non-default active session's WAL failed to read. |
| `PRIKK-DOCTOR-COMMIT-WITNESS-STALE` | `info` | The session's own acknowledgment history is stale (an older binary's seal drained the queue it covered). No action; the next commit or `--repair-tails` replaces it. |
| `PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-DAMAGE` | `error` | A queued commit the session was told had succeeded is damaged. The default session's message names `--discard-damaged-commits` (RFC 166 D5). |
| `PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-LOSS` | `error` | An acknowledged queued commit is no longer present. The default session's message names `--discard-damaged-commits`. |
| `PRIKK-DOCTOR-COMMIT-WITNESS-SUBSTITUTED-RECORD` | `error` | The record at an acknowledged sequence does not match the commit that was acknowledged. Not a crash shape; no verb, and a copy is the way out. |
| `PRIKK-DOCTOR-COMMIT-WITNESS-UNKNOWN` | `error` | The queue has an unexplained tail, and the acknowledgment history is unreadable, so the tail cannot be shown to be a crash leftover. The default session's message names `--discard-damaged-commits`. |
| `PRIKK-DOCTOR-COMMIT-WITNESS-DAMAGED` | `warning` | The acknowledgment history is unreadable, but the WAL it covers is wholly sound. The default session's message names `--repair-tails`. |
| `PRIKK-DOCTOR-COMMIT-WITNESS-OWNERSHIP-MISSING` | `error` | The active ref metadata names an owner the acknowledgment history does not recognize. The default session's message names `--restore-queue-target`. |
| `PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-*` (`STALE`, `ACKNOWLEDGED-DAMAGE`, `ACKNOWLEDGED-LOSS`, `SUBSTITUTED-RECORD`, `UNKNOWN`, `DAMAGED`, `OWNERSHIP-MISSING`) | as above | The same conditions for a non-default active session, with the session named in the message and the same severity. Their recommendations name no repair verb. |
| `PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-UNREADABLE` | `error` | A non-default active session's acknowledgment history failed to read. |
| `PRIKK-DOCTOR-MISSING-REQUIRED-DIRECTORY` | `error` | A directory the repository layout requires is missing. |
| `PRIKK-DOCTOR-REQUIRED-DIRECTORY-WRONG-TYPE` | `error` | A required directory name is occupied by something that is not a directory. |
| `PRIKK-DOCTOR-REQUIRED-DIRECTORY-UNREADABLE` | `error` | A required directory could not be read. |
| `PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE` | `error` | A verification stage failed, or was not attempted because an earlier stage failed under `--stop-on-first-error`. 0.50.0 P3c: when one of the three `*-INTERIOR-DAMAGE` issues above is also present, the recommendation names its code instead of the plain "preserve the repository and inspect the failing stage" — decided by that issue's own typed field, never by this stage's own message text. Handoff 165 Q1b: `GENERATION-LOG-AMBIGUOUS` above gets the same treatment. |
| `PRIKK-DOCTOR-VERIFY-OBJECT-INCOMPLETE`, `-BLOCK-STATE-INCOMPLETE`, `-REF-FILE-INCOMPLETE`, `-REF-ITEM-INCOMPLETE` | `error` | One item of that kind failed verification. |
| `PRIKK-DOCTOR-CURRENT-BRANCH` | `warning` | `.prikk/current-branch` is malformed or names a branch that does not exist or is closed; every `--ref` default refuses until it is fixed, while an explicit `--ref` still works. |
| `PRIKK-DOCTOR-PROVISIONAL-WORKTREE` | `warning` | The worktree was materialized from a snapshot and is not replay-verified. `prikk verify` clears it. |
| `PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION` | `warning` | A checkout or branch switch stopped part-way, so `commit` refuses until it is re-run or the named file is moved aside. |
| `PRIKK-DOCTOR-VERIFY-ERROR` | `error` | Repository verification failed before doctor could produce a healthy report. |

Publication-trust issues can also appear in doctor output as error-severity diagnostics using the
trust issue code and message from publication-trust verification. Ref-publication diagnostics use
their verification codes: pointer lead, retained active cleanup, and unproved divergence are errors;
candidate debris is a warning.

`MissingForEmptyWal` and `ValidForNonEmptyWal` are healthy metadata states and do not produce doctor
issues by themselves.

## Doctor Repair Boundary

Doctor repairs are opt-in: a plain `prikk doctor` only diagnoses. The switches, all parsed in
`crates/prikk-cli/src/args.rs:478-531`, are:

- `--repair-wal-tail`: truncates an incomplete final record from the default session's WAL and, for each
  non-default active session, that session's WAL (`doctor.rs:1258`).
- `--repair-tails [--plan-only]`: truncates the incomplete final bytes of the WAL, the pointer index, and
  the eight appended files, ten files in all (`doctor/repair_tails.rs:76`). It refuses interior damage in
  any of them, the identical way with or without `--plan-only` (0.50.0 step 1 A6 item 4) -- the one repair
  that may cut across ten files in one run, and the last of the five recovery verbs to gain a preview.
- `--repair-index`: rebuilds the object index from the containers.
- `--repair-pointer-index-tail`: truncates an incomplete final pointer-index record.
- `--rebuild-pointer-index [--plan-only]`: re-derives the ref-pointer index from the ref log (RFC 165 R5,
  `pointer_rebuild.rs`). Before flipping away from the live slot, a real run saves a full copy of it, of
  the rebuilt slot's own pre-rebuild bytes, and of the generation log's own before/after bytes, to the
  recovery log under one run (0.50.0 step 1 Parts F/F2) -- `--recovery-restore <id>` undoes the whole
  switch byte for byte: both slots and the generation record all go back to what they were, naming the
  old slot live again. A restore refuses, and writes nothing, if an ordinary write has landed in the
  rebuilt slot since (a new branch, a publication).
- `--discard-damaged-commits [--plan-only]`: removes an acknowledged queued commit that the commit witness
  names and the WAL no longer holds soundly (RFC 166 D5, `doctor/discard_damaged_commits.rs`).
- `--restore-queue-target --ref <ref> [--not-current-branch] [--plan-only]`: gives an active queue back its
  owner when its ref metadata is missing or does not match.
- `--recovery-list`, `--recovery-restore <id>`, and `--recovery-clear`: work with the recovery log.
- `--repair-main-ref`: a recognized input that performs no repair and is always refused
  (`doctor.rs:1263-1269`).

`prikk ref complete <ref> [--plan-only]` is a separate command, not a doctor switch. It completes an
interrupted publication that is a completable pointer lead (RFC 165 R4, `ref_completion.rs`).

Each repair that removes bytes saves them to the recovery log, `.prikk/recovery/log` (see
[durability and crash recovery](./durability-recovery.md)). `--repair-index` saves the ids it cannot
re-derive, when any, the same way.

### `--repair-pointer-index-tail`

Truncates an incomplete trailing pointer-index record — a torn tail, positionally: the last record ends
past the file's own length, and nothing sound follows it. Mirrors `--repair-wal-tail` exactly in shape
and contract, including saving the removed bytes first. Refuses, unchanged, on a genuinely damaged entry
(not merely a trailing partial). That entry is not recovered by truncation. `prikk doctor
--rebuild-pointer-index` re-derives the pointer index from the ref log, and it refuses while a pointer
lead is still completable, so `prikk ref complete <ref>` runs first (`pointer_rebuild.rs:148-192`).
Holds the pointer-index lock for its whole run, matching `--repair-index`'s own discipline.

### `--repair-index`

Rebuilds `containers/index.container` by scanning the object containers themselves. Use it when
`verify` reports

```
error: integrity error: index entry for <id> resolves to an envelope with computed id <other>
```

which means an index entry points at a different record's bytes. The containers are self-describing —
each record carries its own magic, framed length, and checksum — so everything needed to rebuild is
still on disk, and a rebuild is a recovery rather than a reconstruction.

The repair reads and writes the index only; container bytes are never touched. It is idempotent: on a
healthy repository it reports `object index: nothing to repair` and writes nothing. The new index is
installed atomically — written to a temporary file, fsynced, renamed over the old one — so an
interruption leaves one whole index, never a mix.

**The repair holds the object-store lock for its whole run** — it is a writer to the index like any
other, and it takes the same lock every other writer takes. Three consequences, all ordinary lock
behaviour:

- a writer (`commit`, `seal`, `tag create`, …) that arrives while a repair is running is refused with
  `lock conflict`, and retrying after the repair finishes succeeds;
- a repair that arrives while a write is running is refused the same way;
- a **stale** `objects.lock`, left behind by a failed acquisition, refuses the repair with the message
  naming `prikk unlock` — the one case where an operator must act before the repair can run at all.

Acquiring the lock only around the final install would not be enough: the repair scans the containers
first, and a writer appending between the scan and the install would have its object discarded by an
index rebuilt from the older state. The lock covers the scan and the install together.

This state was reachable before prikk 0.40 by running two object-writing commands concurrently; the
object-store lock now prevents it (see [concurrency and locking](./concurrency-locking.md)). This verb
exists for repositories damaged before that lock existed.

Refusals differ by verb. `--repair-wal-tail` refuses while a repository-wide error exists, and skips an
active session that has its own error (`doctor.rs:1258`, `1275`). `--repair-tails` refuses interior damage
in any file it covers. `--rebuild-pointer-index` refuses ref-log damage and any completable lead. `--repair-index`
checks neither; it rebuilds the index from the containers under the object-store lock. The detailed recovery
mechanics and safety preconditions for those repairs live in the
[durability and crash recovery](./durability-recovery.md) reference. Local lock conflicts, stale-lock
limits, and ref compare-and-swap conflicts are covered by the
[concurrency and locking](./concurrency-locking.md) reference.

Doctor does not synthesize missing objects, repair malformed ref logs, repair checksum mismatches,
repair signatures, auto-trust keys, reconstruct trust policy, or recover key material. Stale-lock cleanup
is `prikk unlock`. An interrupted publication is completed by `prikk ref complete <ref>` under RFC 165
R4's rule (`ref_completion.rs`); doctor itself does not sign or append a ref state
(`doctor.rs:1117`).

## Relationship to Rollback Verification

Repository `verify` counts active rollback-draft WAL records after classifying and decoding
rollback-marked Patch payloads under the supported replay subset. It also counts sealed rollback
Blocks and sealed rollback Patch references.

`prikk rollback-draft-verify` is a stronger selected-ref pre-seal check for one active rollback draft.
It verifies that the active WAL contains exactly one rollback draft and that the draft payload matches
the inverse Patch derived from the selected ref. See the
[rollback draft verification](../guide/rollback/rollback-draft-verify.md) guide for the command-level
boundary.

## Deferred Work

Still deferred: broader repair policy, stale-lock policy, missing-object recovery, malformed-log
repair, checksum-mismatch repair, object quarantine and garbage collection, repository-wide AUTHOR
trust policy, key rotation, hardware signing, remote trust, hosted identity, doctor JSON output, stable
diagnostic schema, stable repository-format migration, multi-ref backup export, a rehearsed
repository-format-migration restore, and production readiness. (MAINTAINER key revocation is no
longer deferred — `prikk trust maintainer remove`. Single-ref backup/restore tooling is no longer
deferred either — `prikk bundle export`/`verify`/`import`, see
[Backup and Restore](../guide/backup-restore.md).)

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| Repository verification reports counters for objects, WAL records, Blocks, refs, ref logs, rollback material, publication trust, ref-publication issues, trailing partial WAL bytes, and active WAL metadata state. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`verification.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/output/verification.rs) |
| Verification checks object placement, envelope decoding, object identity, Block references, ref pointer/log consistency, WAL replay, rollback classification, and publication trust. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`refs.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs.rs), [data model](./data-model.md) |
| Publication trust checks Block, RefState, and RefUpdate envelopes against repository-local maintainer trust and reports issues separately. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [DC-11](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-11-MAINTAINER-TRUST-STORE.md), [trust and threat model](./trust-threat-model.md) |
| `verify` command failure occurs for active-WAL metadata integrity issues, publication-trust issues, or blocking ref-publication issues after printing the report. | [`main.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/main.rs), [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) |
| Active WAL metadata has six states, with two healthy states, two empty-WAL warning states, and two non-empty-WAL integrity states. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [DC-15](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) |
| Doctor is a diagnostic layer over verification with issue severities, issue codes, messages, recommendations, and an issue summary. | [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [`output.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/output.rs), [PR-011](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-011-DOCTOR-HANDOFF.md) |
| Doctor surfaces publication-trust and ref-publication issue codes in addition to doctor-owned diagnostics. | [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [`refs/verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/verify.rs) |
| Doctor mutation is opt-in and limited to the switches listed in the Doctor Repair Boundary; `--repair-main-ref` is recognized but performs no repair and is always refused. | [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [`args.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/args.rs), [DC-38](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md), [durability and crash recovery](./durability-recovery.md) |
| Repository verification classifies rollback draft WAL records and sealed rollback material, while `rollback-draft-verify` performs a stronger selected-ref check. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`rollback_verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/rollback/verify.rs), [PR-029](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-029-ROLLBACK-DRAFT-VERIFY-HANDOFF.md), [PR-030](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/PR-030-SEALED-ROLLBACK-HISTORY-HANDOFF.md), [rollback draft verification guide](../guide/rollback/rollback-draft-verify.md) |
| Verify/doctor output is current CLI vocabulary, not a stable machine-readable schema. | [`output.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/output.rs), [DC-29](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-29-VERIFY-DOCTOR-INTEGRITY-RECOVERY-REFERENCE.md) |

## Provenance

This reference consolidates behavior through RFC 103, which retired format-1 support entirely (see
[Doctor Repair Boundary](#doctor-repair-boundary) above). It follows the DC-26 documentation-home
model: current-state references live in the published mdBook, not under `rfcs/fdds/`. DC-38 documents
pointer-first publication diagnostics and the narrower doctor boundary; DC-39 added strict
new-envelope admission and, before RFC 103, format-1 signature diagnostics.
