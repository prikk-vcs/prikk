# RFC 168 — A way back from every repair: one durable recovery log, a restore, and no rename on the durable path (D6, D5)

**Status.** **ACCEPTED 2026-10-06 by the owner** (*"RFC 168 is accepted."*).
- **History:** proposed 2026-10-06 by the architect; rewritten as a design the same day; revised after the architect's
  review against the owner's philosophy (§8).
- **The architect's reading of that acceptance:** the whole RFC as committed at `9c0c2b53`. That is §3 (the design,
  with §8's revisions) and §6 item 2, the three disclosed Windows residuals (a), (b) and (c).
- **Implementation:** `rfcs/handoffs/168-a-way-back-from-every-repair/implementation-handoff-v1.md`.
- **One constraint, added at handoff, that narrows §3.2 and does not widen it:** a restore writes only to a file on a
  fixed list of repairable files (the WAL, the pointer index, and the ten `repair_tails` files). Whatever source path a
  log entry names, a tampered log cannot point a restore at `FORMAT`, a ref, or any other file.
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
- **Repairs stop writing `.bytes` files.** The log is the one place a repair saves to, so there is one place to look.
- **Nothing ever truncates the log.** An entry is appended at its end, even after a torn or damaged region. The reader
  resynchronises past damage under RFC 167's budget and counts the damaged regions.
- **The log is never authority.** No classification reads it.
- **`verify` reports damage in the log on its own line, and it is not repository damage** (R8, revised by the §8
  review).
  - **The line:** *"recovery log: 1 damaged region; a save there cannot be restored (`prikk doctor --recovery-list`)"*.
  - **`verify`'s exit status is unchanged by it.** The log is not history. A torn tail (a repair interrupted mid-save,
    which then truncated nothing) is harmless.
  - **Otherwise** `verify` would fail forever on a file that no command repairs.
- **No compaction, and the user decides what is kept:**
  - **`prikk doctor --recovery-clear [--plan-only]`** lists what it will remove, then empties the log in place;
  - **the name is kept,** so later saves stay durable on Windows;
  - removed bytes can include file contents a user wants gone, so there must be a way to remove them;
  - **old `.bytes` files are not touched:** the listing says they can be deleted by hand.
  - The design round measured 222 bytes of overhead per entry.
- **Creating the log** (R3):
  - **`init` creates it** (the `init` exemption: an interrupted `init` loses nothing).
  - **In an existing repository, the first 0.49.0 command that writes to it creates the log,** not the first repair.
    - So a repair almost never creates it. Repairs mostly follow a crash, and a crash means a later boot.
    - On Linux and macOS the creation is durable (the directory is synced).
  - **On Windows, a repair in the same boot as the log's creation** carries the new-name residual (§6, item 2b).
    - It is disclosed in `platform-support.md`, **not printed by the repair.**
    - *Revised by the §8 review:* a warning the user can do nothing about is noise.
  - Every later repair appends to an existing name, which is durable.

### 3.2 A way back (R2, R6, R7)

- **`prikk doctor --recovery-list`:**
  - prints each entry: its id (the first 16 hex characters of its frame checksum), the source, offset, length, repair
    and version;
  - **it does not judge whether an entry can be restored.** That needs a whole-file hash of the source, which would make
    a listing slow. The restore's `--plan-only` checks it, and the listing says so in its last line;
  - lists the old `.bytes` files separately, as *"older format: list only; read or delete by hand"*.
- **`prikk doctor --recovery-restore <id> [--plan-only]`** prints its plan first: each condition with its result, then
  what it will write, then what follows.
  - **What follows, stated in the plan:** *"After this, the file holds what it held before the repair, damage included.
    `verify` will report that damage again, and commands that refused before the repair will refuse again."*
  - **Who it is for:** someone who believes a repair removed something it should not have, or who wants a newer prikk to
    judge the same bytes.
  - **The id is the full 16-character id.** No prefixes, so no ambiguity.
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
| the current-branch pointer | truncate, then append (RFC 166 D1's writer), inside the switch marker | a pointer without its trailing newline, which `refs.rs:1094-1096` already rejects. With the marker set, the refusal names the switch's target (below), and running that switch completes it |
| `FORMAT` | `6\n` → `7\n`, one byte overwritten, then flushed | none: a one-byte write does not tear. A later format-7 write can no longer outlive its marker |
| the witness | overwritten from offset 0, then its length set, then flushed; **never truncated first** | its checksum fails: a damaged witness over a sound WAL, RFC 166 row 8, rebuilt. It never reads as the empty "cleared" state |
| `ref-name`'s restore | D1's truncate-then-append writer | as D1 |
| the recovery saves | the log's append (§3.1) | a torn tail of the log, which the reader resynchronises past |

- **The marker names what it covers.**
  - **Today** the worktree marker holds only a sentinel (`worktree_marker.rs:44`), so a refusal over an unreadable
    pointer can say only "switch to the current branch". The user cannot know which branch that is.
  - **Under this RFC,** the switch appends one more line after the sentinel, `target heads/<name>`, and a checkout
    appends its ref.
  - **Then a refusal says exactly what to type:** *"a branch switch to heads/other was interrupted; run `prikk branch
    switch heads/other` to finish it."*
  - **Older binaries are unaffected:** they treat any non-empty marker as set (`worktree_marker.rs:78-81`).
  - **A target line without its newline is ignored,** and the refusal falls back to today's text.
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
  - *if `status` shows changes you did not make right after a switch, write the branch's files again before
    committing* — **with the exact command named.**
  - **That command must work in exactly this state:** the marker is clear, and the files differ from the branch.
    - Today's checkout refuses to overwrite a file whose bytes differ (`worktree.rs:55`). So the route cannot simply be
      today's `checkout`.
    - **The implementation round runs the route before the docs quote it** (the stikk 012 lesson).
    - **If no command can write a branch's files over files that differ, stop and ask.** A route that refuses is worse
      than no route.
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
   - **(b)** a repair's save in the same Windows boot as the log's creation, in a repository created before 0.49.0
     (§3.1). The log is created early, by the first write command, so this is rare;
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
- **Commands:** `--recovery-list`, `--recovery-restore`, `--recovery-clear` and `verify`'s log line (§3.1–3.2), each
  with a text test. Also:
  - the marker's target line (§3.3): a torn pointer under a set marker, and the refusal's exact command, then that
    command run;
  - the §3.4 route, run in its state before any doc quotes it.
- **Timing:** a release build on `/home`, inside the R1 scope, the repair's cost against `main` at two file sizes.
- **Docs:**
  - `repository-layout.md`, `durability-recovery.md` and `troubleshooting.md` (no longer *"nothing reads it back"*);
  - `platform-support.md` (§3.4).
- **Each unit's start and end, as the clock shows them.**

## 8. The owner-philosophy review (2026-10-06)

The owner asked the architect to review this RFC against *"finally clean, safe and secure, and robust and sophisticated
design"* and *"users must not be confused or misunderstand"*.

### What it changed

| finding | before | after |
|---|---|---|
| **Two places to look for saved bytes** | the log, and `.bytes` files still written | repairs write only the log; old files are listed as older |
| **`verify` failing forever** | log damage counted as repository damage, and no command repairs the log | its own line, exit status unchanged |
| **No way to remove saved content** | no compaction and no delete; removed bytes may hold content the user wants gone | `--recovery-clear`, which keeps the name so later saves stay durable |
| **A warning nobody can act on** | the first Windows repair printed a durability sentence | the log is created early, by the first write command; the residual is documented, not printed |
| **A slow listing** | `--recovery-list` judged every entry's restorability, which means a whole-file hash each | the listing lists; `--plan-only` judges |
| **A restore whose result surprises** | nothing said what happens after | the plan says verify and refusals return, and says who the command is for |
| **A refusal naming nothing to type** | an unreadable pointer said "switch to the current branch" | the marker names the target; the refusal names the exact command |
| **A documented route that may refuse** | the worktree disclosure quoted `checkout`, which refuses over differing bytes | the route must be run first; if none exists, stop and ask |

### Risk per dimension, after the changes

| dimension | risk | why |
|---|---|---|
| security | Low | a restore writes one named file and offset under three conditions and the repair's locks; the log's content can be cleared by the user |
| robustness | Low (Linux, macOS); Medium (Windows) | repository state is now durable on Windows; worktree files and two first-creation cases stay residual, of the class no Windows primitive closes |
| performance | Low | one append per repair, measured at 222 bytes of overhead; a restore hashes its source once; a listing reads only the log |
| user confusion | Low | one place, one listing, a plan that says what follows, refusals that name the command; the Windows residual is in docs, with a route that must be tested |

## 9. Amendment A1 — a restore undoes a repair (proposed 2026-10-06, for the owner's reading)

**Why.** The implementation review (`rfc168-implementation-review-v1`) found that some repairs cut more than one file, or
rewrite a file without cutting it.
- **`--discard-damaged-commits`** cuts the WAL, then rewrites the commit witness. Its entry recorded the old witness, so
  **its restore is refused for ever.**
- **`--repair-tails` row 8** rewrites a damaged witness **and keeps none of its bytes,** which breaks RFC 162 rule 3.
- **One `--repair-tails` run** can cut files that are each other's meaning files. Its restores then succeed only in an
  order nobody is told.

**The amendment:**
1. **Every byte a repair replaces is saved too, not only bytes it cuts.** A *replace* entry holds the file's previous
   bytes, and the hash of the bytes the repair wrote.
2. **One id per repair the user ran.** A repair run's entries share a run id, which the repair prints and the listing
   groups by.
3. **`--recovery-restore <id>` undoes the whole run,** in reverse order.
   - **Every condition is checked first,** against the state each earlier step would leave, before anything is written.
   - **Then each step writes:** a cut is appended at its offset; a replace writes its previous bytes in place.
   - **It holds the union of the run's locks.** If any condition fails, nothing is written, and the plan names the step.
4. **The meaning-file rule (§3.2) is unchanged.** Reverse-order checking makes each meaning file match the state at that
   step's save.
5. **An interrupted restore can be finished by running it again.**
   - A step whose file already holds what that step would write counts as done: a cut already appended, or a replace
     already holding its previous bytes.
   - So a second run completes the rest, rather than refusing for ever over a half-undone repair.
6. **A run that a later run overlaps names that run.**
   - A condition fails because a later repair cut or replaced the same file. The plan then says: *"restore run <later
     id> first"*.
   - So the user is never left guessing an order.

**What users see:** the same three commands, and one id per repair they ran. **For a one-file repair, nothing changes.**
**After a discard is undone,** the WAL and the witness are byte-identical to before it, and `verify` reports what it
reported then.
