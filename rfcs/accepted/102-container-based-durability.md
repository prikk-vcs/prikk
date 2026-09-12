# RFC (accepted) - 102 Container-Based Durability

**Status.** **ACCEPTED by the project owner 2026-08-12.** Successor to RFC 101, which closed with a
negative result the same day. **Acceptance clears §6's prerequisites only** — no design, no
implementation, no production code — and a stop-and-report on any of them ends this RFC as it ended 101.
**Independence.** Author-reviewed — the standing ceiling.
**Arises from.** RFC 101's §5.2 transition trace, which established that the obstacle is prikk's storage
model rather than its ref publication; and the owner's direction of 2026-08-12 that Windows read-only
is not an acceptable permanent state.
**Target.** Owner's call. **1.0-scale**, not 0.20.0 — see §9.
**Prerequisites §6.1–§6.7: all complete and accepted.** **Design accepted by the project owner
2026-08-13** — `handoffs/102-container-based-durability/design-v1.md`. Six stages; Stage 1 (worktree
marker + the orphaned WAL-at-`init` fix) and Stage 2 (isolate-and-continue reading) change no storage
format and stand alone. **No implementation authorized yet.**

## 1. The problem, stated correctly this time

**RFC 101 stated this problem wrongly and that is why it failed.** It framed the obstacle as DC-38's
step 5 versus step 6 — a symptom — and proposed a fix that reached only ref publication. An
independently derived transition trace then showed that fix would have made things *worse*, by creating
a durable ref pointing at a non-durable object.

The correct statement:

> **prikk is content-addressed, so the filename *is* the content hash. Every object write therefore
> creates a name that did not previously exist, and Windows offers no primitive that makes a new name
> durable.**

Creating a file writes two independent things: the file's **contents**, and the **directory entry**
naming it. Windows has `FlushFileBuffers` for the first and nothing for the second. So a power loss can
leave an object whose bytes are intact on disk and whose name is gone.

This is not a ref-publication property, an `fsync` error-handling question, or a network-sync question.
It is a property of one-file-per-object storage.

## 2. What RFC 101 established, so this does not re-derive it

1. **No Windows primitive provides new-name durability** — documented, undocumented, or
   reverse-engineered. DC-87 Stage 2 checked the Win32 surface; RFC 101 §5.5 added Transactional NTFS
   and the NTFS `$LogFile`.
2. **Transactional NTFS did provide it and is being withdrawn**, with Microsoft warning it *"may not be
   available in future versions."* Ruled unusable: if TxF is removed, a repository written under
   TxF-backed durability is indistinguishable from one that always had the guarantee — silent loss of a
   guarantee, which is the failure prikk exists to prevent.
3. **The complete new-name surface is mapped** — RFC 101 §5.2's fifteen transitions and 31-site call
   index, derived independently. **That table is this RFC's primary input**, and its retention was
   ordered regardless of 101's fate.
4. **No per-ref file shape avoids first-appearance at ref creation** (DC-91 §3).

## 3. The direction

**Move durability-bearing repository state into a bounded set of fixed-name container files.**

Appending to or updating a file that already has a name requires only content durability, which Windows
provides. If no new directory entry sits on the durability path, the gap closes — with **no
vendor-specific primitive, no deprecated API, and no weakened invariant.**

> **CORRECTED 2026-08-13 by §6.5. The sentence above conflates two primitives and, read literally,
> permits an unsound construction.**
>
> - **`atomic_replace` / `write_file_atomically` is NOT sufficient.** `fsutil/anchored/linux.rs:30-46`:
>   it calls `open_new_regular` on a temp name, writes, fsyncs, then `renameat`s onto the destination —
>   **a new-name event plus a rename, unconditionally, even when the destination already exists.** Rename
>   durability on Windows is DC-87 §3.4's still-open question, and RFC 101 §5.5 already refuted
>   `ReplaceFile` and ruled out TxF, which are what would have covered it.
> - **`durable_append` / `durable_truncate` / `durable_truncate_to_empty` are sufficient.** They open the
>   existing file directly and contain **no rename call at all** — the WAL's and ref log's own idiom.
>
> **Ruled: containers use the append/truncate idiom. A container whose updates go through
> `atomic_replace` does not satisfy this RFC**, and would silently reintroduce the exact Windows question
> the RFC exists to close.
>
> **This reaches past the containers.** Seven production sites use `atomic_replace` today — including
> **the ref pointer itself** (`refs/pointer.rs:51`), which is precisely what §6.3 proposes to
> containerize. Any design that "containerizes" them while keeping the rename-based update mechanism has
> changed the file layout and not the durability property.

It is uniform across Linux, macOS and Windows, so it **satisfies** the one-mechanism constraint rather
than straining it. Packed object storage is well-trodden; this is not a novel storage idea.

**This is a hypothesis, not a design.** RFC 101's hypothesis was equally plausible and died on contact
with §5.2. §6 exists to find out whether this one survives the same treatment.

### 3.1 What this claims, stated precisely — amended 2026-08-12

§6.1 established that every comparable system (SQLite/Fossil, Git, Mercurial) converged on a
bounded-container shape, and that **none of them closes the gap for the containers' own first
creation.** This RFC originally implied it did, by analogy. It does not:

> The container model does **not** find a Windows primitive for new-name durability. It reduces
> new-name events to a **fixed, enumerable set created once at `init`** — and `init` is idempotent and
> retry-safe, so a crash there loses no history and the remedy is to run it again.

That is RFC 101 §5.3's T1 finding applied to a small set of names instead of one. **It closes the gap
only if the set is genuinely fixed.**

### 3.2 Fixed names, and no rotation — amended 2026-08-12

§6.2 found *"bounded set"* ambiguous between a fixed set of names each unbounded in size
(Fossil/SQLite) and periodically-rotated size-capped segments (Git packfiles), where **each rotation is
a new-name event** — rarer, not absent.

**Ruled: fixed set of names, each unbounded in size. Rotation is forbidden.** Rarer is not never, and
prikk's standard is invariants rather than probabilities; a design whose durability degrades every N
megabytes fails eventually and unpredictably.

**Consequently, compaction must target a pre-created alternate slot** — fixed A/B names allocated at
`init` like every other container — because compaction that writes a new container reintroduces the
problem at the worst moment.

**Every container name is created at `init`, or the design is wrong.** This is §6.3's acceptance test.

## 4. The worktree, which cannot be containerized

Worktree files are the user's real files. Materializing them creates new names, always, and no container
format changes that.

**But the danger is not the lost file — it is the inference drawn from its absence.** Per RFC 101's T12,
the commit-authoring path treats any baseline path missing from the worktree as a user deletion, so a
file whose name failed to become durable is re-authored and **signed** as a deletion the user never made.

**Remedy, evaluated in §6.5 and confirmed sound 2026-08-13 — with one construction requirement:** a
fixed-name unclean-shutdown marker, **created at `init` and updated by append/truncate, never by
`atomic_replace`** (see §3's correction — the marker is the first thing that would have been built the
unsound way). After it is set, prikk refuses to infer deletion until the worktree has been re-verified
against its baseline. §6.5 confirmed the deletion inference has exactly one choke point
(`worktree_patch/node_authoring.rs:441-446`) and that the marker's own failure modes fall toward
*"still dirty"* — a spurious refusal, never a missed dirty state. That converts silent signed data loss into a
detected condition requiring explicit user action.

**Note the asymmetry that makes this tractable:** the worktree is rebuildable from sealed history; the
repository is not. Losing a worktree file is recoverable. Signing a false deletion is not.

## 5. Non-negotiable constraints

1. **One storage mechanism across all platforms.** A Windows-only container format is a worse outcome
   than not shipping Windows mutation.
2. **B′ adoption semantics unchanged** — a merge seals the other side's patches verbatim: same bytes,
   same `ObjectId`, same author signature. A container format must not disturb object identity.
3. **Object-trust and ref-authority stay separate** (DC-78 §D2).
4. **No conversion of format-2's *rejection* of the ahead-log state into *recovery*.**
5. **Recoverability does not regress below today's audited ceiling** — DC-41 Stage 1's 24/24 reachable
   states — and the audit is re-earned rather than assumed. **Amended 2026-08-12, after §6.2 showed
   this was incomplete:** state count is a *coverage* measure and does not catch *severity*. Today every
   object is independently content-hash validated, so corruption is confined to one object and `verify`
   names exactly which; Git's packfile experience is the contrast — one corruption, hundreds of objects
   lost. **So: corruption isolation must not regress either.** A single corruption event must remain
   attributable to, and confined to, a single object, and **per-entry checksums or an equivalent
   isolation mechanism is a requirement of any proposed container format — demonstrated, not asserted.**
6. ~~**A format migration must exist** for repositories already written in the current format.~~
   **WITHDRAWN 2026-08-13 by owner ruling.** Asked whether "without concern about migration" extended
   from RFC 103's retired format to this RFC's *current* one, the owner answered that it does: *"We are
   in early development stage. The risk is accepted."* **No migration is required for repositories
   written in the current format.** This removes the single largest cost item in the container redesign
   and changes what §6.3 onward must account for. The corresponding acceptance criterion 5 (migration
   demonstrated on an existing repository) is withdrawn with it.

## 6. Blocking prerequisites

1. **What have comparable systems done, and what did it cost them?** Packed and container storage in
   content-addressed and version-control systems — what is packed, what stays loose, how durability is
   claimed, and specifically **how each behaves on Windows**. If the field universally accepts weaker
   Windows durability, that is a finding the owner needs before authorising a storage redesign.
2. **Re-derive §5.2's fifteen transitions against a container model.** For each: does it leave the
   durability path, and what remains? **Derive independently** — do not assume the table transfers.
   A transition that cannot leave the path is a stop-and-report.
3. **What is the bounded fixed-name set**, and what creates each name? Any file created outside `init`
   reintroduces the problem it was meant to remove.
4. **Read-path and concurrency consequences.** Read-only commands take no lock and run concurrently with
   mutation today. Does a container preserve that, and at what cost to lookup?
5. **The worktree question of §4** — is the unclean-shutdown marker sound, and does it close T12 without
   requiring new-name durability? Answer from the commit-authoring code, not from the sketch above.
6. **Cost.** The proof surface to be re-earned, and what a container does to DC-41's
   failpoint matrix.

### 6.3a Two findings promoted from §6.3's answer — 2026-08-13

**The object container's read path is a blast-radius requirement with no precedent in this codebase.**
The WAL and ref log already implement per-record SHA-256 framing, which is the mechanism amended
constraint 5 asks for — but both hard-`Err` on a mid-stream checksum mismatch. **Correct for a
single-purpose queue; a regression for a container holding many unrelated objects**, where today's
one-file-per-object layout lets `verify` name the bad object and keep scanning. The object container must
name the failed record *and continue*. No existing read path does this.

**An accepted fix is orphaned — ASSIGNED 2026-08-13 to this RFC's own implementation scope.** RFC 101
§5.1's WAL-at-`init` change carries its own already-accepted reasoning (behaviour-neutral: every reader
treats a missing WAL and an empty WAL identically), and it is the one place today's code fails §6.3's
acceptance test. It lands with this RFC's implementation rather than as a standalone increment, since it
is meaningless on its own and load-bearing here. **Its acceptance evidence is RFC 101 §5.1's, not
re-derived** — but confirm the reader-equivalence claim still holds before relying on it.

Original finding, retained: RFC 101 §5.1 established that the active WAL is created
lazily on first append rather than at `init`, and that moving it is behaviour-neutral. That fix was
accepted — and RFC 101 then closed with a negative result, so it never had an implementation vehicle.
**It is independently correct and blocks this RFC's own acceptance test** (every container name created
at `init`). Assign it explicitly rather than letting it ride on a closed RFC.

**Minor, recorded so it is not carried forward as real:** `quarantine/` is created at `init` and never
read or written anywhere in the workspace. Verified — the only non-`layout.rs` matches are an unrelated
doc comment and an unrelated enum variant. Do not enumerate it as a container.

### 6.7 The index question — added 2026-08-13, after the lookup decision

The owner's choice of indexed lookup creates one question the first six prerequisites could not have
asked. **It precedes the design, not the implementation.**

1. **Is the index durability-bearing, or rebuildable?** If it can be regenerated by scanning the
   container — as Git's `.idx` is regenerated from a packfile — it stays **off the durability path
   entirely**, and only concurrency remains. If it cannot, it is another container and inherits §3's
   append/truncate requirement. **This changes the size of the design materially and is not answered.**
2. ~~**Which publication shape?** … append-only index … or A/B slots with a single-field publish …~~
   **ANSWERED 2026-08-13: append-only is the only buildable shape, and my offering two was wrong.**
   `DurabilityContract`'s **eleven** methods contain **no primitive that overwrites bytes at an offset
   inside a file** — verified exhaustively, and `pwrite`/`write_at`/`seek` appear nowhere in `fsutil/`.
   A single-field in-place publish is therefore not buildable. Forced through the real primitives, "A/B"
   becomes *"append a generation → slot record, readers take the last complete one"* — **append-only
   wearing an A/B costume, not a second option.**

   Note this does **not** disturb §3.2's compaction ruling, which requires pre-created alternate
   *names* — that stands. What is unbuildable is publishing the switch by overwriting a field.
3. **ANSWERED 2026-08-13 — one property, in two parts, because there are two files:**

   > A reader must never observe an index entry as complete unless **(a)** the entry's own framing is
   > fully present — no torn or in-progress append attributed to a real entry, mirroring
   > `trailing_partial_bytes` treatment — **and (b)** the object bytes its `(offset, length)` names are
   > already durably present in the container at read time.

   **(b) is the part no framing enforces.** There is no cross-file atomic primitive, so **the write
   protocol carries it**: the container append must be durable before the index append. A design that
   appends the index entry first lets a reader see a complete, checksummed entry pointing at bytes that
   are not there.

**A stop-and-report applies.** If neither publication shape can be made sound without a primitive this
codebase lacks, that is the finding, and it returns the lookup decision to the owner.

## 7. Acceptance criteria

1. **Parity stated as a property of the design**, not as a platform list that happens to pass.
2. **A negative control** per DC-95's standing method: disable the durability-bearing step, demonstrate
   the specific failure `verify` reports, restore, confirm no residual diff.
3. **Green three-platform CI**, macOS included.
4. **DC-41-grade recoverability audit re-earned** at the new design's own state count.
5. ~~The migration demonstrated on a repository written in the current format.~~ **Withdrawn 2026-08-13** with constraint 6.

## 8. Non-goals

- **G1 path anchoring on Windows.** No Win32 component-by-component no-follow walk exists; out of scope,
  as it was for 101.
- **Windows read-only support**, which works today and is CI-gated.
- **Performance work.** If a container improves object-write throughput that is welcome, not a
  justification, and not a criterion. **Clarified 2026-08-13 after §6.4:** this non-goal must **not** be
  read as having pre-settled object-lookup cost. §6.4 established that preserving lock-free concurrent
  reads costs either **linear-scan lookup (O(1) → O(container size) per read)** or an **indexed lookup
  requiring a fencing-read primitive with no precedent in this codebase.** An O(1)→O(n) change to every
  object read is a shape change, not tuning, and filing it under "performance" would under-weight it.
  **It is an owner-level cost to accept explicitly, not a non-goal.**

  **DECIDED by the project owner 2026-08-13: indexed lookup.** Linear scan was rejected as bad for
  maintenance and unfit for a project of this size; a self-validating advisory index was rejected for
  lacking correctness and therefore reliability. **Object lookup stays sub-linear; the design owes a
  publication scheme for the index.**

  **The architect's stated cost for this option was overstated and is corrected here.** It was framed as
  *"requires inventing a lock-free reader/writer coordination primitive with zero precedent in this
  codebase."* No precedent *here* is not the same as unprecedented: SQLite's WAL-index and LMDB's
  dual-meta-page solve this exact problem, and §6.1's survey already found those systems on container
  storage. **Two known shapes fit constraints this RFC already imposes** — an **append-only index** (the
  WAL idiom §3 mandates: no in-place mutation, so no torn read) and **A/B slots with a single-field
  publish** (§3.2 already requires pre-created alternate slots for compaction). **The real cost is design
  plus a correctness proof at DC-41 grade — that readers can never observe a partial index — not the
  invention of a primitive.**
- **Changing what a ref log is** — DC-38's append-only audit trail.

## 9. The cost, and the staging consequence

This is a storage-format change: object layout, `verify` and `doctor` state derivation, the durability
contract's platform layer, and DC-41's recoverability audit. **No migration for existing
repositories** — constraint 6 withdrawn 2026-08-13, which removes what had been the largest single cost
item here.
**It is 1.0-scale work and should not be forced into 0.20.0.**

**What it changes immediately, before any of it is built:** Windows read-only stops being a verdict and
becomes a staging decision — *not yet*, rather than *never*. That is a different thing to ship and a
different thing to document, and it is the honest position while this RFC runs.

## MEASURED and RULED 2026-09-12 — concurrent object appends corrupt the index, and no command repairs it

**Every object append passes through one funnel** — `ObjectWriteSession::write_object` →
`foundation/index.rs::append_object_to_container` — which reads the container length, appends under
`O_APPEND`, and records the pre-read length as the index offset. **No object container is lockable**
(`LockableContainer` has four variants, none of them the object store), and of the eleven production
callers only some hold `ActiveLock`; `tag create`, `merge`, `sync build` and ref publication hold no lock
that any other respects. The implementing team mapped this at the checkpoint their handoff required.

**Measured by the architect: twelve concurrent `prikk tag create` in one repository, four fresh runs,
four repositories left failing `verify`** — `index entry for … resolves to an envelope with computed id
…` — with `prikk tag` itself erroring. `O_APPEND` keeps the container bytes intact; the **index offsets
collide**. And **`rebuild_index_from_containers` (`index.rs:559`) has no caller outside tests**; its doc
records that a `doctor` repair was "not assumed into scope." A user hit by this has no way out.

**RULED:** a fifth `LockableContainer` for the object store, **acquired and released inside
`append_object_to_container`** across the length read through the index append — a **leaf** lock,
never held while acquiring another, closing all eleven callers at the funnel; fail-fast like every
existing lock, so overlapping mutators now refuse visibly instead of corrupting silently. **And `prikk
doctor --repair-index`**, wiring the rebuild that already exists, as its own increment. Both first in
0.40.0. Handoffs: `object-container-write-locking-handoff-v1.md` (v2 ruling appended),
`doctor-repair-index-handoff-v1.md`.
