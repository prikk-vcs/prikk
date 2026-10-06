# RFC 168 — A way back from every repair: one durable recovery log, a reader, and Windows replace sites classified (D6, D5)

**Status.** **PROPOSED 2026-10-06 by the architect** (0.49.0 step 6, in the owner-approved schedule: *"D6, D5, D7"*,
from external review 014).
- **This RFC sets the design's starting positions and the questions; it does not yet choose the mechanism.** A design
  round answers §5 with prototypes and no product code:
  `rfcs/handoffs/160-costs-that-follow-the-store-and-lengths-read-from-disk/recovery-log-design-round-handoff-v1.md`.
- Then the architect rules, the RFC is rewritten into a design, and the owner reads it.
- **2026-10-06, round 1 reviewed** (`rfc168-design-round-review-v1`, Corrections Required).
  - **Ruled:**
    - one append-only log, never truncated, saved before the truncate;
    - restore only on a byte-identical prefix, with the files that give the bytes meaning unchanged;
    - `init` creates the log, and the first Windows repair in an existing repository says its save is not yet durable;
    - old `.bytes` files are list-only.
  - **Round 2** builds the prototype and redoes Q5, modelling a lost rename after the later steps survive. That adds
    worktree files to the sites.
- **D7, the release key, is RFC 169.**

**Author-review independence.** RFC 162 rule 3 ("a repair keeps every byte it removes") and the recovery file's shape
are the architect's. The external architect found D5 and D6 (letter 014) and examines the 0.49.0 candidate.

## 1. What is wrong (facts, read from source 2026-10-06)

1. **D6 — nothing reads a recovery file** (014: *"no reader and no rehearsed way back … every test covers the save,
   none the restore"*).
   - **Four writers:**
     - `wal.rs:653-676`, shared by `--repair-wal-tail` and `--discard-damaged-commits`;
     - `refs/pointer_index.rs:508-516`;
     - `doctor/repair_tails.rs:315-338`, ten files;
     - `foundation/index.rs:942-960`, the object index's lost ids, which are a hex list, not bytes.
   - **Each writes `recovery/<label>-at-<offset>-<hash16>.bytes`, the raw removed bytes, with no header.**
     - The source file is named only by a label or the session name;
     - nothing records the original length, the time, or which repair wrote it.
   - The docs say *"nothing reads it back"* (`repository-layout.md:79-87`) and *"can be read back … by hand"*
     (`durability-recovery.md:93-98`, `troubleshooting.md:347-350`). No command restores, and no test rehearses it.
2. **D5 — on Windows, a recovery file's save is not durable.**
   - Every writer goes through `write_file_atomically` → `atomic_replace` (`windows.rs:285-323`): the temp file is
     flushed, then `fs::rename`, with no write-through and no directory sync.
   - **A new name's first appearance is not durable on Windows** (RFC 101's finding). The truncate that follows *is*
     durable (`platform-support.md:114-116`).
   - **So a power loss can keep the truncation and lose the saved bytes,** which breaks "a repair keeps every byte it
     removes" on Windows. `durability-recovery.md:123-129` discloses it.
3. **New, found while preparing this RFC: the same non-durable replace writes non-rebuildable state other than
   recovery files,** and neither the code comment nor `platform-support.md` names it:
   - `FORMAT` (`format_upgrade.rs:81`);
   - the current-branch pointer (`branch_switch.rs:347`, `layout.rs:298`);
   - the restored `ref-name` (`doctor/restore_queue_target.rs:213`);
   - RFC 166's witness (`commit_boundary/witness.rs:203,268,317`).
   - `platform-support.md:120` still says the only callers are *"two rebuildable caches"*. That is stale.

## 2. Constraints

- **C1 — no format version change,** as in RFCs 166 and 167. A new file in `recovery/` is additive.
- **C2 — the save is durable before the truncate, on every platform.** On Windows, that means a write to a file that
  already exists (an append plus `FlushFileBuffers`, which `platform-support.md:114-116` holds durable), never a new
  name's first appearance.
- **C3 — a restore never changes meaning silently.**
  - It writes back exactly the removed bytes, at exactly the removed offset, of exactly the file they came from, and only
    if nothing has been written to that file since.
  - It has `--plan-only` and prints its plan first (K1).
  - The restored file is then whatever it was before the repair, damage included, and `verify` says so.
- **C4 — the old `.bytes` files stay readable** by the reader, and are never deleted by it.
- **C5 — every Windows replace site is classified:** what a crash that reverts it to its old value means. Any site where
  that is unsafe is fixed or disclosed, by name.

## 3. Starting positions (to be tested, not decided)

- **One append-only recovery log,** `recovery/log`, framed like prikk's other appended files: magic, version, a header,
  a body and a checksum, and so under RFC 164's tail rules and RFC 167's budget.
  - **Each entry:** the source file (repository-relative), the offset, the length, the repair that wrote it, the binary's
    version, and the removed bytes.
  - It is created at `init`, which is covered by the init exemption (`platform-support.md:131-135`). For an existing
    repository, the first repair creates it. **How the first creation is made durable on Windows is Q3.**
- **A reader and a way back:**
  - `prikk doctor --recovery-list` lists the entries, and the old `.bytes` files;
  - `prikk doctor --recovery-restore <entry> [--plan-only]` writes the bytes back under C3.
- **A rehearsal test for every writer:** repair, then restore, then the file is byte-identical to before the repair.
- **The Windows replace sites:** a crash after the rename can leave the old value. For each site, say whether old is
  safe:
  - the witness reverting is "behind", RFC 166 row 2, harmless;
  - `ref-name` reverting to missing means the restore is rerun;
  - `FORMAT` is marker-only;
  - the branch pointer reverting after a switch may need care.

  **Measure the claims; do not argue them** (Q5).

## 4. Questions for the design round

1. **The log's frame and entry,** from the existing framed formats. Does it fit RFC 164's tail rules and RFC 167's budget
   without a new mechanism? What does `verify` say about the log itself?
2. **Restore under C3:**
   - the exact condition for "nothing written since" (the length equals the offset? a checksum of the current tail?);
   - what happens for each of the ten `repair_tails` files, the WAL, the pointer index, and the object index's lost ids
     (which are not bytes, so is there anything to restore at all?);
   - the plan's text.
3. **Windows durability of the first creation:**
   - can `init` create `recovery/log` in every new repository;
   - for an existing repository, is there a durable way to create it, or must the first repair on Windows say plainly
     that its save is not yet durable?
   - **From source and the existing rulings** (DC-87, `narrow-round-ruling-v1.md`), not new claims.
4. **Old `.bytes` files:** the reader lists them; can it restore them? They lack the source file and length, so
   possibly not, and say so.
5. **Every Windows replace site (C5):**
   - a table of site, what it holds, what reverting means, and whether it is safe;
   - failpoints that leave the old value after the rename, run on Linux to show what each reader then does.
6. **Cost:** a repair's cost with the log against today's, and the log's growth.

## 5. Out of scope

- Moving rebuildable caches off `atomic_replace`.
- A Windows primitive for new-name durability, which RFC 101 found does not exist.
- The release key (RFC 169).
