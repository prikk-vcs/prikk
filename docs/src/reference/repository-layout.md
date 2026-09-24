# Repository Layout and Authority

This page is the authoritative current-state reference for Prikk's on-disk repository layout and
storage authority boundaries. It describes the current implementation and is grounded
in the code, released RFCs, and implementation status records listed in the anchor table at the foot of
the page.

For logical object concepts, see the [data model](./data-model.md). For repository path and worktree
write-safety rules, see [path and worktree safety](./path-safety.md). For local persistence and
recovery behavior, see [durability and crash recovery](./durability-recovery.md). For trust and
signature scope, see the [trust and threat model](./trust-threat-model.md). For lock and ref
compare-and-swap behavior, see [concurrency and locking](./concurrency-locking.md).
For format stability, migration limits, and release identity, see
[release, versioning, and compatibility](./release-compatibility.md).

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- `.prikk/` is Prikk's native repository format and is not Git-compatible storage.
- `.prikk/FORMAT` is a current format-version gate, not a stable-format or migration guarantee.
- Ref files are mutable pointers for convenience and recovery, not roots of trust.
- Cache-like directories are initialized today, but are not current roots of trust; the quarantine
  directory is retired and no longer initialized.
- Durability and recovery claims are supported by current unit and integration tests, not by a
  completed crash-matrix or fuzzing campaign.
- Repository *mutation* is exercised by project gates on Linux, macOS, and Windows (DC-87 Stage 2).
  Windows' anchoring guarantee is weaker than Linux/macOS in one stated way — see
  [platform support](./platform-support.md) for the exact gap and which of the nine durability
  guarantees are held, weaker, or documented no-ops there. Read-only commands are CI-gated on macOS
  and Windows too — see [platform support](./platform-support.md).
- Stable repository-format migration, garbage collection, quarantine enforcement, cache rebuilding,
  hosted forge trust, and remote-tracking remain deferred. `prikk sync` (RFC 116, RFC 117) and
  `prikk merge` (DC-74) have since shipped — see the [sync](../guide/sync.md) and
  [merge](../guide/merge.md) guides.

## Initialized Layout

A fresh `prikk init` creates the repository directory, the initialized directories below, and the
format marker file. It does not create runtime leaf files such as the active WAL or active ref
metadata. Ref, received-ref, and trust storage are the exception: the ref-pointer index, ref-log,
received-ref index, and trust containers are all fixed, named files allocated by `init` itself, empty
until first use — there is no per-ref, per-received-ref, or per-key-id file or directory created
later, since none of those names exist at `init` time and a per-name file would have to be.

```text
.prikk/
  FORMAT
  worktree.marker
  containers/
    patch/{a,b}.container
    block/{a,b}.container
    ref-state/{a,b}.container
    tag/{a,b}.container
    attestation/{a,b}.container
    blob/{a,b}.container
    index.container
    generations.log
  active/
    default/
      queue.wal
      ref-name
  refs/
    containers/
      log-{a,b}.container
      pointer-index-{a,b}.container
      pointer-index-generation.log
      received-index-{a,b}.container
      received-index-generation.log
    locks/
  trust/
    keys.container
    policy-{a,b}.container
    policy-generation.log
  cache/
```

Every file above is created by `init` and is empty until first use. **No name under `.prikk/` is created
after `init`** — that is a design invariant, not an implementation detail, and it is what makes the
repository durable on filesystems that cannot make a new directory entry durable.

`init` also creates `refs/tmp/`, which is **allocated but never written to**. It is a remnant of a
retired candidate-publication mechanism, not authority for anything, but its **absence is an error**:
`verify` scans it on every run, so repository open validates it is present.

Ten further directories that were previously allocated at `init` for the same reason — `objects/` and
its six object-type subdirectories, `quarantine/`, `refs/by-id/`, `refs/logs/` — are no longer created.
Nothing has ever written into any of them (object and ref storage moved into containers years earlier),
and nothing validates their presence at open, so a repository initialized before this change keeps them
harmlessly and a newly initialized one simply lacks them. One dormant diagnostic still reads the
`objects/` tree if present — see [Object Store](#object-store) below.

New repositories contain `7` in `FORMAT`; a repository created by 0.44.0 or earlier contains `6`, still
opens, and moves to 7 in place with `prikk format upgrade` (see
[Release Compatibility](release-compatibility.md#repository-format-transitions)). **Formats 1 through 5
are rejected at open**, with an error naming the format found and directing migration through
`prikk bundle export` on a version that still supports it; so is any number this build does not know.
The format marker is a gate: a repository whose on-disk shape does not match what the code expects is
refused rather than opened and left to fail later.

`cache/` holds rebuildable, non-authoritative state — a corrupt or absent cache file is never an error,
and any operation's result is identical whether the cache is warm, cold, or missing.

There is no initialized `gc/` directory, and prikk performs no garbage collection: no object is ever
deleted or superseded once written.

## Object Store

**Objects are not stored one-file-per-object.** Every persistent object envelope is a checksum-framed
record appended into a shared, per-type container allocated at `init`:

```text
containers/<object-type>/a.container
```

with a paired `b.container` allocated at `init` and permanently unused — object containers have no data
model to compact against (see [Compaction](#compaction) below) — and one `containers/index.container`
mapping each object id to the container, slot, offset and length where its record lives.

Six object types are persisted this way — `patch`, `block`, `ref-state`, `tag`, `attestation`, `blob`.
`RefUpdate` is stored inline in ref logs, not as an object record.

**Why containers rather than files.** An object's identity *is* its content hash, so every
one-file-per-object write created a new directory entry. Making a new name durable requires an fsync on
the parent directory, which POSIX provides and Windows does not — there is no documented or undocumented
Windows primitive that makes a new directory entry durable. Appending to a file that already has a name
needs only content durability, which every supported platform provides. Moving objects into fixed,
`init`-allocated containers is what makes prikk's durability guarantee statable as a property rather
than as a list of platforms that happen to pass.

**Reading is isolate-and-continue.** A damaged record is named at its byte offset and the scan continues
to the next intact one, so corruption is confined to the records it actually damaged rather than
failing the whole container. A record is authority only when its frame checksum verifies, its envelope
decodes and validates, it has the expected type, and its computed content-addressed id matches the
requested id.

**The index is rebuildable and off the durability path.** An object record is appended and made durable
*before* its index entry is appended, so a crash between the two leaves an object present but unindexed
— recoverable by rebuilding the index from a container scan. The reverse ordering would let a reader see
a valid index entry pointing at bytes that are not there, so the ordering is load-bearing.

**Nothing deletes an object, and nothing is rewritten.** Writing an id that already exists with
identical bytes is a no-op. In format 7 an id may hold several records, and **the last one is
authoritative**: another signer's copy of a stored object — the same type, schema and payload under other
signatures — is written as one superseding record carrying the union of both signature sets, and the
index's last entry for the id points at it. Every reader resolves an id to that last record, in every
pass. The union is decided under the object-store lock against the record as stored at that moment, so
two writers merging into one id keep both signature sets. A copy whose payload differs under the same id
is corruption and is refused in every format; in format 6, any different copy is refused. Containers
therefore only ever grow.

**`objects/` and its six type subdirectories are a retired one-file-per-object layout**, replaced by the
containers above; nothing in a format-3 repository writes into it, and `init` no longer creates it. A
repository initialized before this change may still have it on disk — its presence or absence is not
validated at open — and if so, one dormant diagnostic (`scan_loose_file_temp_debris`, kept for
`.pobj.tmp.` debris a format-3 repository can no longer produce) still reads it during `verify`.

## Refs and Ref Logs

Ref pointer and ref-log storage is shared, not per-ref: every ref's own pointer entry and log records
live in containers allocated once at `init`, not in a file named for that ref. A ref name does not
exist until `branch create`/`tag create` mints it, well after `init` — a per-ref file could never be
one of `init`'s own fixed names, so pointers and logs for every ref instead interleave inside:

```text
refs/containers/pointer-index-{a,b}.container
refs/containers/log-a.container
refs/containers/log-b.container
refs/locks/<ref-name-storage-key>.lock
```

Locks remain per-ref files, one per ref name actually in use, unaffected by this: the storage key is
the hex SHA-256 digest of the ref name bytes, the same digest used internally to attribute each
container entry to its own ref.

`pointer-index-{a,b}.container` is an append-only, checksum-framed sequence of pointer entries; each
entry records one ref's human-readable name, its published RefState object id, and the SHA-256 key
derived from that name. A ref's current pointer is its *last* matching entry in the *live* slot —
republishing a ref appends a new entry rather than rewriting the old one, and which slot is live is
named by this container's own generation log, not always `a` (see [Compaction](#compaction) below).
It is useful for lookup and recovery, but is not trusted alone: verification checks pointer content
against RefState objects and ref-log evidence, the same as before.

`received-index-{a,b}.container` holds the same shape of entry for **received** refs — pointers
imported by `prikk bundle import` under the separate `remotes/<name>` namespace (DC-78 §D4), never
`refs/by-id/`: an imported RefState object keeps the origin repository's own embedded ref name, which
could never agree with a locally renamed pointer, so received refs get their own container and key
space rather than a special case in the ordinary one. Last-entry-wins, the same as the ref-pointer
index, and compacted the same way. This index is never consulted by `verify_repository` directly —
every object a received pointer leads to is checked by the ordinary object-store scan regardless of
how it was discovered.

`log-a.container`/`log-b.container` hold every ref's RefUpdate log records, interleaved by append
order; a reader filters to one ref's own subsequence by that same per-record key. Each record carries
a signed RefUpdate envelope inline with frame magic, versioning, length, and checksum. Ref logs are
publication evidence when their record chain, referenced RefState objects, target Blocks, signatures,
and trust policy checks all hold — unchanged in meaning, only in storage shape. Slot `b` is allocated
alongside slot `a` at `init` and permanently unused: the ref log is DC-38/DC-69's audit trail and must
never be compacted, so writes always target slot `a` (see [Compaction](#compaction) below).

`refs/locks/*.lock` files are local synchronization files for ref-specific publication and repair, not
a root of trust. There is no longer a temporary-candidate mechanism: an append-only pointer entry has
no candidate value to stage before becoming durable, so a publish that used to write, sync, and
promote a candidate now durably appends the pointer entry directly, in one step.

`refs/by-id/` and `refs/logs/` are no longer initialized directories — nothing has read or written
either since ref publication moved into containers, so `init` no longer creates them, and a repository
missing them behaves identically to one that still has them from before this change (their presence or
absence is not validated at open).

**`refs/tmp/` is different and is genuinely required.** `init` still allocates it, and `verify` lists it
on every run, so a repository missing it fails verification with `directory is absent: refs/tmp`.
Nothing has written into it since ref publication moved into containers, so the scan can only ever find
nothing — but the directory must exist for the scan to succeed.

## Active Session

The default active-session paths are:

```text
active/default/queue.wal
active/default/active.lock
active/default/ref-name
```

These files are runtime-written, not guaranteed members of a bare initialized repository.

`queue.wal` stores exact signed Patch envelopes before sealing. WAL records are load-bearing local
session state: they are the pending changes that `seal` replays and publishes, but they are not sealed
history until publication succeeds.

`ref-name` records which local branch ref owns a non-empty active WAL. Missing or malformed active ref
metadata on a non-empty WAL is an integrity issue because seal must not guess the publication target.

`active.lock` is a local synchronization file. It prevents concurrent active-session writers, but it is
not evidence of repository history or trust. Stale lock cleanup after a crash is manual today; see
[concurrency and locking](./concurrency-locking.md) for the current operator boundary.

## Trust Store

The repository-local MAINTAINER trust containers are:

```text
trust/keys.container
trust/policy-{a,b}.container
```

Both are allocated empty at `init`, then appended to by `prikk trust maintainer add`/`remove` — no
name is created after `init`.

`keys.container` holds one append-only entry per adopted key id: the key id and its Ed25519 public key.
It has no slot pair and is never compacted — TOFU history must persist across key removal (see
[Compaction](#compaction) below).

`policy-{a,b}.container` holds a sequence of complete policy snapshots in its *live* slot (named by
this container's own generation log, not always `a`) — each `add` or `remove` appends the *entire*
current adopted-key-id list, not an incremental change; readers take the last complete snapshot, with
`required = 1` meaning any one adopted key's signature suffices (never stored — it is a constant, not
configurable). Seal checks the configured MAINTAINER signer against this repository-local policy
before publication, and verify checks publication envelopes against the same local trust boundary.

This trust store is authority for current publication-trust checks. It is not remote trust, global
identity, key rotation, hosted forge policy, or a multi-maintainer threshold system. Key revocation is
supported (`prikk trust maintainer remove`): a removed key's material is retained internally (so a
different key presented later under the same id is still refused), but it no longer counts toward the
adopted set or reserves its case-folded id.

## Compaction

Three containers accumulate entries that are superseded the moment a newer one lands:
`pointer-index-{a,b}.container`, `received-index-{a,b}.container`, and `policy-{a,b}.container`. A
ref's or received ref's pointer is only ever its *last* matching entry — everything earlier for the
same key is dead weight from the moment it is superseded — and a trust policy snapshot supersedes every
earlier snapshot outright, since each one already carries the complete adopted-key-id list.

Each of these three containers is paired with its own generation log
(`pointer-index-generation.log`, `received-index-generation.log`, `policy-generation.log`, all under
the same directory as the container they belong to) recording which slot — `a` or `b` — is currently
live. An empty generation log means no compaction has ever run for that container, and the live slot
is `a`. A reader resolves the live slot by reading the last complete record in the generation log; it
never assumes `a`.

`prikk compact --pointer-index|--received-index|--trust-policy|--all` reclaims the dead entries for
one or all three: it reads the currently-live slot, keeps only what is still current (the last entry
per key for the two indexes, the last snapshot for the trust policy), writes that reduced set to the
*other* slot, makes it durable, and only then appends a generation record naming the new slot live —
so a crash at any point before that append leaves the previous generation fully authoritative, and
retrying the compaction from scratch is always safe. `prikk compact ... --plan-only` reports what a
real run would reclaim without writing anything. Compaction never runs automatically; nothing else in
Prikk invokes it.

Compacting one of these containers takes the same per-container lock its own writers take (see
[concurrency and locking](./concurrency-locking.md)), so a compaction run and an ordinary write to the
same container cannot interleave, and a real run and a `--plan-only` preview never report stale
numbers against each other.

**Two container families never compact, deliberately — not because rotation is merely pending.**
Object containers (`containers/<type>/{a,b}.container`) have no data model to compact against: nothing
is ever superseded or deleted (see [Object Store](#object-store) above), so there is nothing for a
second slot to reclaim. The ref log (`refs/containers/log-{a,b}.container`) is DC-38/DC-69's audit
trail, which must never be compacted. Both keep their `b` slot allocated at `init` and permanently
unused. The trust key container (`trust/keys.container`) has no slot pair at all — TOFU history must
persist across key removal, so it stays a single append-only file, never compacted.

## Authority Model

| Path or data | Classification | Current meaning |
|---|---|---|
| `.prikk/FORMAT` | Format gate | Required by repository open; `7` is the current format. Format `6` still opens and is written as format 6 until `prikk format upgrade` moves it to `7` in place (marker only; no downgrade). Formats 1-5 are rejected at open, with no bridge and no in-place migration. |
| `.prikk/config` | Local settings, optional | Written only by `prikk config set` (`unset` removes the file); absent means every key takes its default. One key today, `incoming.max-object-bytes`. Never authority for history and never read from the worktree or from anything imported, so nothing received can set a value in it; `bundle verify`, which opens no repository, does not read it, and an older `prikk` build ignores it. |
| `containers/<type>/a.container` | Content-addressed object authority | One checksum-framed record per object in format 6; in format 7 an id may hold several, and the last is authoritative. Authority when the frame checksum verifies, the envelope validates, and its computed id/type match the requested object. `objects/` is a retired remnant no longer created by `init`; authority for nothing. |
| `refs/containers/log-a.container` (`log-b.container` reserved) | Publication evidence | Shared, append-only signed RefUpdate records for every ref, interleaved; authoritative only with valid chain, object, signature, and trust checks. |
| `trust/policy-{a,b}.container` and `trust/keys.container` | Repository-local trust authority | Current local MAINTAINER trust input for seal and verify publication-trust checks. `policy-{a,b}` is compacted by `prikk compact --trust-policy` (the live slot is named by its own generation log); `keys.container` has no slot pair and is never compacted — TOFU history must persist across key removal. |
| `active/default/queue.wal` | Local active-session state | Pending signed Patch envelopes before seal; load-bearing for the active session, not sealed history. |
| `active/default/ref-name` | Local active-session metadata | Identifies which ref owns a non-empty active WAL; not sealed history. |
| `refs/containers/pointer-index-{a,b}.container` | Mutable convenience pointer | Shared, append-only, last-entry-wins RefState pointer for every ref; fast current-state lookup and recovery target; not trusted without RefState/ref-log checks. Compacted by `prikk compact --pointer-index`; the live slot is named by its own generation log, not always `a` — see [Compaction](#compaction). |
| `refs/containers/received-index-{a,b}.container` | Mutable convenience pointer | Same shape as the ref-pointer index, for imported `remotes/<name>` refs (`prikk bundle import`); not consulted by `verify_repository` directly. Compacted by `prikk compact --received-index`. |
| `*-generation.log` (`pointer-index-`, `received-index-`, `policy-`) | Compaction state | Records which slot (`a`/`b`) is currently live for its own container; empty means `a`. Not history or trust evidence — see [Compaction](#compaction). |
| `active/default/active.lock`, `refs/locks/*.lock`, and the four container locks (`pointer-index.lock`, `log.lock`, `received-index.lock`, `policy.lock`, all under `refs/containers/` or `trust/`) | Local synchronization | Prevent concurrent writers, and (for the container locks) a concurrent `prikk compact` run; not history or trust evidence. Recoverable after a crash via `prikk unlock` — see [concurrency and locking](./concurrency-locking.md). |
| `cache/` | Initialized, rebuildable, non-root | Never authority; a corrupt or absent cache file is not an error and does not change any result. |
| `refs/tmp/` | Initialized, unwritten, required | `init` still allocates it; nothing writes into it since ref publication moved into containers, but `verify` lists it on every run, so its absence fails verification. Not authority for anything. |
| `objects/` (and its six type subdirectories), `quarantine/`, `refs/by-id/`, `refs/logs/` | Retired, no longer initialized | `init` no longer creates these; nothing has written into any of them since object and ref publication state moved into containers. Not validated at open, so a repository initialized before this change keeps them harmlessly. Not authority for anything. `objects/` alone still has one dormant reader — see [Object Store](#object-store). |
| `gc/` | Deferred/not present | No current initialized directory or released behavior. |

## Deferred and Not Stable

Prikk does not provide in-place or history-preserving migration between any two formats. The
documented writable path is a newly initialized repository, under whichever format is current (see
the Authority Model table above), followed by deliberate worktree re-authoring, which creates new
NodeIds, objects, signatures, and history. Copying `.prikk/` data or editing `FORMAT` is not
migration. This explicit transition does not promise general format stability.

**`prikk sync` (RFC 116, RFC 117), `prikk merge` (DC-74), and single-ref `prikk bundle
export`/`verify`/`import` (DC-44) have since shipped** — see the [sync](../guide/sync.md),
[merge](../guide/merge.md), and [backup and restore](../guide/backup-restore.md) guides. Still
deferred: garbage collection, cache rebuild semantics, quarantine enforcement, stable
repository-format migration, multi-ref backup export, a rehearsed repository-format-migration
restore, remote trust, hosted forge semantics, complete branch management, remote-tracking, and
full cross-platform filesystem validation.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| Repository initialization creates the listed directories and writes `.prikk/FORMAT`. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [DC-31](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-31-REPOSITORY-LAYOUT-AUTHORITY-REFERENCE.md) |
| `.prikk/FORMAT` selects format 7 for new repositories; format 6 still opens and upgrades in place; formats 1-5 are rejected at open. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [DC-40](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-40-STATE-MERKLE-FORMAT-TRANSITION.md) |
| Persistent objects are checksum-framed records appended into per-type containers allocated at `init`. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [`object_store.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/object_store.rs) |
| Six object types currently have initialized persistent object directories; `RefUpdate` is inline-only in ref logs. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [`object_store.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/object_store.rs), [`refs/container.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/container.rs) |
| Ref storage keys are SHA-256 hex digests of human-readable ref names, shared by the pointer index, the log container, and per-ref lock files. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [`refs.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs.rs) |
| The ref-pointer index is a shared, append-only, last-entry-wins container holding every ref's own current-pointer entry (name, RefState id, storage key). | [`refs/pointer_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/pointer_index.rs), [`refs.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs.rs), [data model](./data-model.md) |
| The ref-log container is a shared, append-only sequence holding every ref's own signed RefUpdate envelopes, interleaved, with frame magic, checksums, and per-ref replay semantics. | [`refs/container.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/container.rs), [`refs.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs.rs), [durability and crash recovery](./durability-recovery.md) |
| Active WAL and active ref metadata are runtime active-session state, not fresh-init files. | [`active.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/active.rs), [`wal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/wal.rs), [durability and crash recovery](./durability-recovery.md) |
| Trust policy and maintainer public-key files are written by the trust command and define current repository-local MAINTAINER trust. | [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [security and signing setup](../guide/security-setup.md) |
| Verification checks object placement, ref pointer/log consistency, active WAL state, and publication trust within current limits. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [integrity and recovery diagnostics](./integrity-recovery.md), [trust and threat model](./trust-threat-model.md) |
| `cache/` is initialized but not a root of trust; `quarantine/` is retired and no longer initialized, and `gc/` is not an initialized directory. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [DC-31](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-31-REPOSITORY-LAYOUT-AUTHORITY-REFERENCE.md) |
| The received-ref index is a shared, append-only, last-entry-wins container for imported `remotes/<name>` pointers, kept separate from `refs/by-id/` because an imported RefState's own embedded ref name can never agree with a locally renamed pointer. | [`received.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/received.rs), [`received_index.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/received_index.rs) |
| Three containers (ref-pointer index, received-ref index, trust policy) each have a generation log naming which slot is live, defaulting to `a` when empty; `prikk compact` reads the live slot, writes the reduced set to the other slot durably, then appends a generation record naming it live; `--plan-only` performs the same read with no write. Object containers and the ref log allocate an unused `b` slot and never compact; the trust key container has no slot pair at all. | [`generation.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/generation.rs), [`compact.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/compact.rs), [`lock.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/lock.rs) |

## Provenance

This reference implements DC-31 as a documentation-only extension of the DC-24 current-state reference
series. It adds no code, schema, CLI behavior, repository behavior, trust behavior, verification
behavior, repair behavior, or repository-format stability guarantee.
