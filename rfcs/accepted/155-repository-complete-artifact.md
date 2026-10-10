# RFC 155 — The repository-complete artifact: a whole repository, verbatim, in one file

**Status.** **ACCEPTED by the project owner 2026-09-16** (*"RFC 155 is accepted."*); proposed the same
day by the architect. This is the "own RFC" that RFC 114 §5.2 (format carry-forward), RFC 115 §6.2 and
RFC 116 §7 left open, and RFC 154 §6.2 needs it. planeter's requirements letter
(`.git-exclude/upstream/planeter/receive/2026-09-16-repository-complete-artifact-requirements.md`, R1–R6)
is folded in as input. Author-review independence: the architect proposed and will review; §8's
controls compensate, and each must be shown to fail.

**The architect's reading of the approval, stated so it can be corrected:**
- **§7.1:** the direction is §3–§6 as written.
- **§7.2:** maintainer trust material travels as inert data and is never adopted by import.
- **§7.3:** landing goes through RFC 154's rule, and only on an explicit `--adopt`; the default stays
  `remotes/`, and DC-78 §D4's letter holds.
- **§7.4:** scheduling was not ruled by the word. The architect's proposal stands, after 0.43.0:
  1. the maintainer key-id collision fix;
  2. this artifact;
  3. RFC 154's adoption act.

  ROADMAP row 18 carries it. **Nothing is handed off yet.** R4's all-or-nothing import is the largest
  piece and gets a design round (report before implementation) of its own.

## 1. Why it is needed

- **Carry-forward.** RFC 114 §5.2 (owner-ruled) requires a supported, tested carry-forward before format
  7. A one-ref bundle is not one.
- **Keyless relay** (RFC 154). A holder with no key must be able to pass a repository's signed history on
  unchanged. Today a received ref cannot be re-exported (measured 2026-09-15).
- **Backup.** `bundle export` takes one ref. A restore today gives `remotes/<ref>`, not a working
  repository (`docs/src/guide/backup-restore.md:292`).

## 2. What exists, and what does not

- **`bundle export`/`verify`/`import`** (DC-78) cover **one ref**.
  - `verify` needs no repository and writes nothing, but checks structure only; no signature is
    verified (`backup-restore.md:161-162, 291`).
  - `import` records objects and author-key material, and creates a `remotes/` pointer. It advances no
    ref and adopts no maintainer key (`:292`).
- **`PEXCH002`** (sync) is a patch-level delta, deliberately not a repository (RFC 116 §1 (a)).
- **One envelope per id.** A repository refuses a second envelope for an id it already holds with
  different signature bytes, as an Integrity error (`decide_write_outcome`, `crates/prikk-store/src/foundation/index.rs:442-470`).
- **No repository-wide read snapshot exists** across many refs (RFC 115 §3, point 3).

## 3. Scope

The artifact is a file in, a file out, and off the network. It is not sync, not a protocol, and not a
backup service. RFC 115 §6.1's "protocol or transfer" is answered as **transfer** for this artifact only.
Negotiation stays RFC 116's.

## 4. Requirements and the architect's recommendation on each

**R1 — Complete.**
- The artifact carries **every stored object** and **every ref**: branches, closed branches, tags,
  received pointers, and the author-key container.
- Accepted-but-unsealed work (what `sync pending` lists) is stored objects: patches, blobs and recognition
  claims. It therefore travels. prikk carries no review state; what makes a claim an *open change* is
  the forge's own metadata.
- **The maintainer trust policy never travels as authority.** RFC 115 §6.2 names it "the dangerous one":
  carrying adopted keys into a receiver lets a sender widen what the receiver trusts. The adopted keys'
  **public material** travels as an inert, labeled record. `import` never adopts from it; an operator
  may adopt from it by a separate, explicit `trust maintainer add`. So planeter's "trust-policy entries"
  arrive as **material to decide on, not decisions**.

**R2 — Verifiable offline, before import.** `verify` on this artifact checks structure, closure and
**every signature against the key material the artifact itself carries**. It reports the result as
*internally consistent*, never *trusted*. Trust exists only against a receiver's own adopted set, at
import or after. A signature the carried material cannot check is reported, not skipped.

**R3 — Verbatim, never re-signed.** Every envelope travels byte-identical. This is RFC 154's accepted
principle and needs no new ruling.

**R4 — Read-only export; all-or-nothing import.**
- **Export** takes one repository-wide consistent view under the repository lock and writes nothing in
  the repository. This is the mechanism RFC 115 §3 found missing.
- **Import** is all-or-nothing, **including when the process is killed**: a target is either untouched
  or complete. It is never partial, and `doctor` names any interrupted state. This is the hard part: a
  multi-ref import is WAL-scale work, not a loop over `bundle import`.

**R5 — Driven as a subprocess.**
- Every verb has a `--format json` outcome with stable fields.
- Cancelling means killing the process, which R4 makes safe.
- Bounded in memory by streaming, not by loading the whole artifact. The bound is measured, not
  asserted.

**R6 — Identity-stable, idempotent.** *(Superseded in part 2026-09-16 by RFC 156: the same id under different signatures is **merged** under RFC 156 §4's rules, not refused.)*
- An object already held with identical bytes is a no-op. Re-importing the same artifact changes nothing
  and says so.
- The same id with **different** signature bytes stays **refused and named**; never union and never
  pick (one envelope per id). Verbatim relay (R3) never creates that case. Two holders who signed the
  same content separately do.

## 5. Where a complete import lands (RFC 115 §6.3)

RFC 115 §6.3 asks how one operation can serve both a repository moving itself and foreign history. By
default, `import` lands every ref under `remotes/`, as today. DC-78 §D4's letter holds: import creates
no ref the operator did not ask for.

With an explicit `--adopt`, the operator asks. For each ref, import then applies RFC 154's adoption rule
against the **target's own** adopted set:
- a ref whose chain is signed by an adopted key, and is a fast-forward (from nothing, in a fresh
  repository), **lands as a local ref**;
- every other ref still lands under `remotes/`, and the outcome names why.

Adoption runs inside the same all-or-nothing import (R4), not as a second pass that could stop between
refs.

**Carry-forward** therefore reads as follows. On the new version:
1. run `init`;
2. adopt the maintainer keys explicitly (from the artifact's inert record);
3. run `import --adopt`.

Every branch lands as a branch, no signature changes, and trust was never automatic.

## 6. Format

The artifact gets a new magic and version, `PREPO001`, under RFC 114's stability contract. The build that
introduces format 7 must read `PREPO001` written by the last format-6 build: that is the carry-forward
test. The layout's retired-format refusal messages then point at it.

## 7. What the owner rules

1. **Direction:** build this artifact as §3–§6 describe.
2. **The trust-policy rule of R1:** public material travels as inert data and is never adopted by
   import.
3. **Landing via RFC 154** (§5), rather than a separate "migration mode".
4. **Scheduling.** The architect proposes: after 0.43.0, following the maintainer key-id collision
   fix, before RFC 154's adoption act (§6.2 there).

## 8. Controls an implementation must show failing

- A byte flipped in any envelope makes `verify` fail and names the object.
- An artifact carrying a trust record: `import` leaves the target's adopted set unchanged.
- A process killed at each write stage of import leaves the target untouched or complete; `doctor` names
  the interrupted state.
- Export during a concurrent `seal` yields either the pre- or post-seal repository, never a mix.
- Re-importing the same artifact is a no-op, reported.
- The same id with different signature bytes is refused and named.
- A ref signed by a non-adopted key lands in `remotes/`, never as a branch.
- Round trip: export, import into a fresh repository, `verify`, and the ref set and object ids are
  identical.

## 9. Design, 2026-10-10 — for the owner's reading (design round D1–D4)

**Status of this section: PROPOSED by the architect, for the owner's reading, then acceptance.** It answers *how*
for §3–§8. It came from a four-part design round: reports `rfc155-design-round-D1…D4-report.md` and reviews `…-review-v1`.
Author-review independence: the architect set the questions and rules here. The external review at the cut
compensates.

### 9.1 What the artifact carries

| content | in the artifact | on import |
|---|---|---|
| object containers (every type) | verbatim, the live slot's bytes | each object written through the ordinary path, in dependency order |
| the ref pointer index, the ref log, the received index | their **resolved** content (no slot letters, no generation log) | written as a fresh slot `a`, as `init` lays out |
| **author keys** | carried | **recorded** through DC-78's path (`check_author_key_conflict`, then `record_author_key_material`). A conflict refuses the whole import before any write |
| **maintainer keys and policy** | carried as an inert record | **never written.** The artifact's verify lists them as *"carried, not adopted"*; the operator adopts with `trust maintainer add` |
| caches, the object index, `FORMAT` | not carried | derived by the importer |
| the active WAL and witness, the recovery log, markers, locks | not carried | the importer's own, fresh |

**Export refuses before writing anything** when:
- **a session holds queued, unsealed commits.** The message names the session, the count, and `prikk seal`. A queued
  patch lives only in the WAL until `seal`, so an archive taken without it would silently lose signed work. No
  override; seal first;
- **a slotted container cannot be resolved:** an ambiguous lost generation log, or a damaged slot. It uses the
  existing texts and their ways out.

**Damage in an object container travels verbatim.** The archive's verify names it, and import refuses such an archive,
naming the object (§8). An archive is a faithful copy, damage included.

### 9.2 The format, `PREPO001`

- **Sections:** one per carried file, in the container frame (magic, length, body, checksum).
- **A trailing manifest and a fixed 24-byte end trailer,** so a reader finds the manifest without scanning.
- **Streamed on both ends,** with memory independent of the archive's size. The prototype's RSS stayed flat (2.7 MiB)
  across a 600× size range.
- **Reference closure in two passes, with an id set:** memory bounded by object count, 32 bytes an id.
- **Carry-forward:** a later `PREPO` version keeps the old magic's reader forever, with a fixture from the old encoder
  (`decode_bundle`'s precedent).

### 9.3 Export: one consistent view

Export holds **every container lock for its whole run** (`acquire_container_locks`, all five), so an export taken
during a `seal` is the repository before or after it, never a mix.
- **Measured:** 1.2 GiB in 775 ms (warm cache), RSS flat.
- **Writers fail fast** while it runs (`LockConflict`).
- **A killed export leaves its lock files.** `prikk unlock` clears them, as for any killed writer.
- **Rejected alternative:** a brief lock, then an unlocked stream to recorded lengths. It reads torn data once RFC 158's
  reclaim rewrites containers in place.

### 9.4 Verify: streaming, no scratch copy

The archive's verify checks:
- structure (the manifest);
- closure (two passes);
- every signature against the **archive's own carried** author and maintainer material, held in memory, through
  `verify_author_signatures_with`'s lookup closure.

It writes nothing anywhere, on any exit path. It reports *"internally consistent"*, never *"trusted"*, and names
every signature it cannot check.
- **Rejected alternative:** staging into `$TMPDIR` plus `verify_repository`. That costs a full copy (memory, on a tmpfs
  `/tmp`), a double write on import, and deletions in a shared temp directory.

### 9.5 Import: untouched or complete, literally (R4 as written)

- **Stage 0, checks:** the archive's verify, the author-key conflict check, the landing plan. A refusal writes nothing.
- **The journal, the first write:** the archive's identity, the landing plan, and the length of every container import
  can append to: each object container, the ref log, and the live slots of the pointer index and the received index.
  It is taken under every container lock, which import holds **from the journal to the commit point**.
- **Stage 1, objects:** in dependency order through the ordinary write path. Content-addressed and idempotent.
- **Stage 2, author keys:** recorded.
- **Stage 3, refs:** each landing written, idempotently, under the control-plane locks.
- **The commit point:** the journal is emptied. Its name is kept, the conservative choice on Windows (RFC 168 §6).

| state `doctor` sees | what it means | the way out |
|---|---|---|
| no journal | nothing started, or finished | none needed |
| journal, *N* of *M* refs landed (0 ≤ *N* ≤ *M*) | interrupted: `PRIKK-DOCTOR-INTERRUPTED-IMPORT` | run the same import again (it resumes), **or** `prikk doctor --cancel-import <id>` |

- **Cancel restores "untouched".** For every container the journal names, it saves the bytes past the journaled
  length (one RFC 168 run, before any truncate), truncates back, and empties the journal. Landed refs are appends to
  those containers, so they go back with them. `--recovery-restore` can undo the cancel.
- **While a journal is present, every repository writer refuses** except the import itself, the cancel and the
  recovery verbs. The message names *"run the same import again, or `prikk doctor --cancel-import <id>`"*. Nothing may
  be appended behind an unfinished import, because that would make the cut-back unsafe. This is RFC 163's rule: a
  write never buries a crash state.
- **The cost:** writers refuse for the import's duration (held locks fail fast). Today's `bundle import` writes 1.15 GiB in 8.2 s, an upper
  bound, because it buffers. On §5's carry-forward target, a fresh repository, that costs nothing.

### 9.6 Landing (§5), and the narrowed `--adopt`

- **The default:** every ref under `remotes/`.
- **`--adopt`** lands a ref as a local ref only when four conditions all hold:
  1. it does not yet exist locally;
  2. its chain is signed by a key in **this** repository's adopted set;
  3. it is verified;
  4. its compare-and-swap against *"absent"* holds.

  Every other ref lands under `remotes/`, and the outcome names why. One reason is *"exists locally; adopting onto an
  existing ref is RFC 154's"*.
- **This is RFC 114 §5.2's carry-forward case, exactly:** `init`; adopt the maintainer keys listed by the archive's
  verify; `import --adopt`. RFC 154's general rule replaces this narrowed one when it ships, and must re-check against
  it.

### 9.7 Identity (R6, as RFC 156 superseded it)

- **The same id with identical bytes:** a no-op, counted as *"already present"*.
- **The same id with different signatures:** RFC 156 §4's merge, through `admit_carried_signatures`, as `bundle import`
  and `sync accept` do. Signatures that do not qualify are **dropped and named**.
- **§8's control *"refused and named"*** is read as *"merged under RFC 156 §4; a dropped signature named"*.
- **Re-importing the same archive** reports *"already imported"*.

### 9.8 The commands

A new noun parallel to `bundle`, the one-ref artifact. **Proposed: `archive`.**

```
prikk archive export <file> [--format json]
prikk archive verify <file> [--format json]          lists carried maintainer keys as "carried, not adopted"
prikk archive import <file> [--adopt] [--format json]
prikk doctor --cancel-import <id> [--plan-only]
```

- **Rejected:** `prikk verify <file>`. `verify [path]` already takes a repository path.
- **Rejected:** a mode of `bundle`. A user who knows `bundle export <ref>` would expect a ref argument.
- **JSON** follows `verify-report-v1`: a schema name; `ok` split into structure, closure and signatures, so
  *"internally consistent"* never reads as *"trusted"*.

### 9.9 Implementation (0.51.0 step 3): four one-sitting parts

1. **Export:** the format writer, the refusals, the full lock. Tests:
   - §8's concurrent-`seal` control;
   - a repository whose live slot is `b`;
   - the queued-work refusal.
2. **Verify:** streaming. Tests: §8's flipped-byte control, which names the object; carried maintainer keys listed;
   nothing written.
3. **Import A:** stage 0, the journal, objects under the lock, author keys, `doctor`'s state, the cancel, and writers
   refusing behind a journal. Tests: a kill at each point, each row above; §8's *"adopted set unchanged"*.
4. **Import B:** landing, `--adopt`, identity, re-import. Tests:
   - §8's remaining controls;
   - the round trip, where `branch list` and the object ids are identical.

**One finding outside this RFC:** today's `bundle import` peaks at about 4.6 times the bundle's size in memory
(5.26 GiB for 1.15 GiB), from whole-file and whole-object buffering. It is a candidate for a later release.
