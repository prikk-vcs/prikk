# RFC 168 — A way back from every repair: one durable recovery log, a restore, and no rename on the durable path (D6, D5)

**Status.** **PROPOSED 2026-10-06 by the architect; rewritten as a design 2026-10-06, for the owner's reading.**
- **Source:** 0.49.0 step 6, in the owner-approved schedule (*"D6, D5, D7"*), from external review 014. D7, the release
  key, is RFC 169.
- **The design round is closed:**
  - handoff `rfcs/handoffs/160-costs-that-follow-the-store-and-lengths-read-from-disk/recovery-log-design-round-handoff-v1.md`
    and its Addendum 1;
  - reviews `rfc168-design-round-review-v1` (rulings R1–R4) and `-v2` (R5–R8, Approved).
  - **Two sites where a lost rename silently changes meaning were found by running them, not argued.**
- **Owner decisions are in §6.** There is no implementation handoff until the owner accepts this RFC.
- **The evidence the design round did not deliver is required evidence in the implementation round** (§7). It is not
  counted as delivered.

**Author-review independence.** The rulings are the architect's. The external architect found D5 and D6 (letter 014),
and examines the 0.49.0 candidate (K6).

## 1. What is wrong (facts, read from source and run)

1. **D6 — nothing reads a recovery file.**
   - **Four repairs save what they remove:**
     - `wal.rs:350` (WAL repair and `--discard-damaged-commits`);
     - `refs/pointer_index.rs:514`;
     - `doctor/repair_tails.rs:336` (ten files);
     - `foundation/index.rs:958` (the object index's lost ids, a hex list).
   - **Each writes a headerless `recovery/<label>-at-<offset>-<hash16>.bytes`.** It records neither the source file's
     path nor the length.
   - **No command reads them back, and no test rehearses a way back.**
2. **D5 — on Windows, that save is not durable.**
   - **Every save goes through `atomic_replace`,** which on Windows is a rename with no durability lever. The lever was
     investigated and retired: `narrow-round-ruling-v1.md` §1; `windows.rs:286-290`.
   - **The truncate that follows is durable** (`platform-support.md`, the contract table).
   - **So a power loss can keep the truncate and lose the save.**
3. **The same weakness writes other state that cannot be rebuilt,** and the docs call `atomic_replace`'s callers *"two
   rebuildable caches"* (stale). The design round ran each site as a **lost rename after the later steps survived**:

   | site | what ran | result |
   |---|---|---|
   | the current-branch pointer (`branch_switch.rs:347`) | a switch completes; the pointer reverts; the marker clear (a durable truncate) survives | **`verify` passes.** `status` shows the new branch's whole worktree as changes against the old branch, and the refusal advice says *"commit it"*. **Meaning changes silently** |
   | worktree files (`branch_switch.rs:215`, `worktree.rs:218`) | a switch completes; one file reverts to the old branch's bytes | **`verify` passes.** `status` shows a user edit; the next commit records it |
   | `FORMAT` (`format_upgrade.rs:81`) | an upgrade completes; the marker reverts | empty repository: a second upgrade succeeds. The multi-signer case was not run (§3 makes it moot) |
   | the witness clear (`witness.rs:203`) | the drain completes; the witness reverts | **a found drain, not damage** (`classification/tests.rs:158`) |
   | the witness's first creation; `ref-name`'s restore | not run | owed (§7) |
   | four caches (`verified_blocks.rs:66`, `lifecycle_cache/incremental.rs:288`, `commit_index.rs:80`, `foundation/index.rs:920`) | from source | rebuildable, each by its module's own rule |

   - **Today's switch already writes the pointer before clearing the marker** (`branch_switch.rs:230-231`). The order
     cannot help: the later durable truncate survives, and the earlier rename is lost.
   - **`platform-support.md`'s `durable_directory_entry` row is wrong for the same reason.** It argues safety from a
     crash *before* the marker clear.
4. **Windows has no durable rename and no durable first appearance of a new name** (RFC 101, DC-87). **The one durable
   write to a name that already exists is in place, then flushed.**

## 2. Constraints

- **C1 — no format version change.** A new file in `recovery/` is additive, and every in-place write keeps its file's
  bytes as today.
- **C2 — the save is durable before the truncate,** on every platform.
- **C3 — a restore never changes meaning silently.**
  - It writes back exactly the removed bytes, at exactly the removed offset, of exactly the file they came from.
  - It writes only if that file is byte-identical to its state after the repair, and the files that give those bytes
    their meaning are unchanged.
  - It plans first. Afterwards the repository is whatever it was before the repair, damage included, and `verify` says
    so.
- **C4 — old `.bytes` files stay,** listed and never deleted.
- **C5 — every `atomic_replace` site is classified.** Each unsafe one is either fixed, or disclosed by name with the
  owner's acceptance.

## 3. The design

### 3.1 One recovery log (R1)

- **`recovery/log`:** an append-only file in the container frame (magic `PRECLOG1`, version, body length, checksum,
  body). **Each entry records:**
  - the source file (relative to the repository root);
  - the offset and the length;
  - the repair's label and the binary's version;
  - the **prefix hash** (SHA-256 of the source's bytes `[0, offset)`);
  - the **meaning files' identities**;
  - the removed bytes.
- **The save comes first:** the entry is appended and flushed, then the source is truncated. If the append fails, the
  repair refuses, as today.
- **Nothing ever truncates the log.** An entry is appended at its end, even after a torn or damaged region. The reader
  resynchronises past damage under RFC 167's budget and counts the damaged regions.
- **The log is never authority.** No classification reads it.
- **`verify` reports damage in the log as damage, naming the log, and never blocks a writer on it** (R8).
- **No compaction.** The log is the only copy of what a repair removed. The design round measured 222 bytes of overhead
  per entry.
- **Creating the log** (R3):
  - **`init` creates it** (the `init` exemption: an interrupted `init` loses nothing).
  - **In an existing repository, the first repair creates it.** On Linux and macOS that creation is durable (the
    directory is synced).
  - **On Windows it is not,** and the repair says so in one sentence: *"This repository's recovery log was just created;
    on Windows, its first save is not guaranteed to survive a power loss."*
  - Every later repair appends to an existing name, which is durable.

### 3.2 A way back (R2, R6, R7)

- **`prikk doctor --recovery-list`:**
  - prints each entry: its id (the first 16 hex characters of its frame checksum), the source, offset, length, repair
    and version, and **whether it can be restored now, with the reason if not**;
  - lists the old `.bytes` files separately, as *"older format, read by hand"*.
- **`prikk doctor --recovery-restore <id> [--plan-only]`** prints its plan first: each condition with its result, then
  what it will write, then what `verify` will report.
  - **It writes only when all of these hold:**
    1. the source's length equals the entry's offset;
    2. the source's bytes `[0, offset)` hash to the recorded prefix hash;
    3. every meaning file's identity is unchanged.
  - **It takes the same locks as the repair it undoes.**
- **Meaning files**, the files whose state the removed bytes depend on:
  - **the WAL:** `ref-name` and the witness;
  - **the pointer index:** the ref-log container. Its bytes are a publication, so any later publish blocks the restore.
    That is conservative, and the plan says why it refused;
  - **each `repair_tails` file:** listed from source in the implementation round.
  - **Without them, a WAL repaired at offset 0** would match again after a later session drains, and a byte restore
    would bring the old records back under a different session. The design round's control holds this.
- **The object index's lost ids are listed, never restored.** The index is rebuilt from its containers, and the plan
  names that rebuild.
- **Old `.bytes` files are never restored** (R4). They record no source path and no length, so condition 2 cannot be
  checked.
- **Rehearsed for every writer:** repair, restore, then a byte-identical file, then `verify`.
  - **With controls:** the source written since; a same-length change to the prefix; a meaning file changed; a damaged
    region in the log, with the entry after it still listed.
  - **The architect perturbed the prefix control and the meaning control; each went red.**

### 3.3 No rename on the durable path for repository state (R5)

**One rule: state that cannot be rebuilt, under a name that already exists, is written in place and flushed.** Atomicity
comes from the record's own check, or from the marker that covers the write, never from a rename. **One code path on
every platform,** so that the Linux failpoint suites exercise the same torn-write handling that Windows relies on.

| site | how it is written | what a torn write means |
|---|---|---|
| the current-branch pointer | truncate, then append (RFC 166 D1's writer), inside the switch marker | an empty or unresolvable pointer with the marker set: today's refusal, and running the switch again completes it |
| `FORMAT` | `6\n` → `7\n`, one byte overwritten, then flushed | none: a one-byte write does not tear. A later format-7 write can no longer outlive its marker |
| the witness | overwritten from offset 0, then its length set, then flushed; **never truncated first** | its checksum fails: a damaged witness over a sound WAL, RFC 166 row 8, rebuilt. It never reads as the empty "cleared" state |
| `ref-name`'s restore | D1's truncate-then-append writer | as D1 |
| the recovery saves | the log's append (§3.1) | a torn tail of the log, which the reader resynchronises past |

- **`init` also creates the default session's empty witness.** In a new repository, every witness write is then to an
  existing name.
- **The four caches keep `atomic_replace`.** A lost write there loses a cache, which is rebuilt.

### 3.4 Worktree files keep their atomic replace, and the Windows residual is disclosed (R5a)

- **Why the same rule is not applied to worktree files:**
  - **Writing a user's file in place changes what users see:**
    - every hard link to the file shares the new bytes;
    - a running executable refuses the write;
    - another open handle sees a partial file.
  - **Created files and deleted files are new-name events,** which no Windows primitive makes durable. They would stay
    open anyway.
  - **So a partial fix adds costs and leaves the class open.**
- **The disclosure** (`platform-support.md`, the switch and checkout guide, `troubleshooting.md`):
  - *on Windows, a power loss shortly after a completed `branch switch` or `checkout` can bring back old contents of
    rewritten files, bring back deleted files, or lose created files;*
  - *nothing in the repository is damaged;*
  - *if `status` shows changes you did not make right after a switch, run `prikk checkout --patch-materialize --ref
    <current branch>` before committing.*
- **`platform-support.md` is corrected** for `atomic_replace` (which no longer writes only caches) and for
  `durable_directory_entry` (§1 item 3).

## 4. What this RFC does not do

- It adds no Windows primitive for new names (RFC 101 found none).
- It does not move the caches.
- It does not touch `release-signers.toml` or the release key (RFC 169).

## 5. Security and compatibility

- **A restore is a writer.**
  - It writes only the exact file and offset an entry names, under §3.2's three conditions, and with the repair's locks.
  - **A tampered entry cannot redirect it:** the source path is relative to the repository root and resolved through the
    same anchored mutation root as every writer, and the frame's checksum must hold.
- **The log holds removed bytes, which may include commit contents.** Its permissions are those of the rest of
  `.prikk/`.
- **Compatibility:**
  - a repository from 0.48.0 needs no migration;
  - older binaries ignore `recovery/log`;
  - every in-place write keeps its file's bytes exactly as today.

## 6. Decisions for the owner

1. **Accept this design** (§3): one log, the restore, and in-place writes for repository state.
2. **Accept three disclosed Windows residuals,** all of the new-name or rename class that RFC 101 found no Windows
   primitive for:
   - **(a)** worktree files after a completed switch or checkout (§3.4);
   - **(b)** the first repair's save, in a repository created before 0.49.0 (§3.1);
   - **(c)** the witness's first creation, in a repository created before 0.49.0. A lost creation falls back to the
     older classification (RFC 166 rule 3), the one a session written by an older binary gets today.

   *Recommended: accept all three, each disclosed by name.* The alternative for (a) is §3.4's in-place write, with its
   costs. (b) and (c) have no alternative short of refusing repairs, or refusing commits, on Windows.

## 7. Required in the implementation round

- **Rehearsals:**
  - the ten `repair_tails` files, each rehearsed, with §3.2's meaning-file table and a control per file;
  - **stop and ask** if any file's meaning cannot be captured by file identity.
- **Failpoints and lost-rename runs:**
  - a torn-write failpoint at each §3.3 site;
  - the witness's first creation, and `ref-name`'s restore, each run as a lost rename.
- **Commands:** the commands (§3.2) and `verify`'s view of the log (§3.1), with text tests.
- **Timing:** a release build on `/home`, inside the R1 scope, the repair's cost against `main` at two file sizes.
- **Docs:**
  - `repository-layout.md`, `durability-recovery.md` and `troubleshooting.md` (no longer *"nothing reads it back"*);
  - `platform-support.md` (§3.4).
- **Each unit's start and end, as the clock shows them.**
