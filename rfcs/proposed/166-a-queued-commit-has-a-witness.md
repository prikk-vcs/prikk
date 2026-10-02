# RFC 166 — A queued commit has a witness: a damaged acknowledged commit is never read as a crash tail

**Status.** **PROPOSED 2026-10-03 by the architect** (0.49.0 step 3, in the owner-approved schedule: *"N6's commit
witness"*).
- **This RFC sets the questions; it does not yet choose the mechanism.** A design round answers §5 from source and
  measurement, with prototypes and no product code: `rfcs/handoffs/163-a-write-never-buries-a-crash-state/
  commit-witness-design-round-handoff-v1.md`.
- **Then the architect rules on each option, this RFC is rewritten into a design, and the owner reads it** before any
  implementation handoff.

**Author-review independence.** The architect wrote RFC 162 rule 3, the rule that produces N6. The external architect
found N6 (letter 015), and the architect's own §9 needed two late corrections (RFC 164 §9, §9.2). So:
- every cell of §5 Q2 is reproduced by the dev team and checked by the architect's own crash probes;
- the external architect has said letter 018 will look for "N6's witness" in the 0.49.0 candidate, and `matrix.py`
  gains rows for it. That review is the independent check.

**Carefulness.** RFC 165's K1–K7 were bound by the owner's *"we had better be careful around such design"*. This RFC
changes what `verify` says about queued commits and adds a refusal to a repair verb. **The architect proposes that
K1–K7 bind it in the same way:**
- K1 plan-only and a printed plan;
- K2 fail closed;
- K3 every condition testable, with controls;
- K4 the recovery verbs raced;
- K5 nothing implicit;
- K6 the ways out last, with external review;
- K7 a second addendum goes back to the owner.

## 1. What is left: N6

1. **The defect** (external review 015, `matrix.py`, confirmed again in 016):
   - **One flipped byte in the body of the last queued commit.** `verify` exits 0, `status` shows trailing partial WAL
     bytes and one queued commit fewer than the user made, and `doctor --repair-wal-tail` removes the record.
   - **The bytes are kept** in `recovery/wal-<session>-at-<offset>-<hash16>.bytes`, so nothing is destroyed. But the
     user was told the commit succeeded, and it leaves the queue.
   - **Five cells are graded I1q on 0.48.0:** last record body flipped, length 2^62, length one less, file emptied, and
     file removed. The last two say "nothing is queued, the user was told 2 commits had succeeded".
2. **The cause is RFC 162 rule 3.** For the WAL, the tail is everything after the last sound record when nothing sound
   follows, "whatever its shape". A log that ends at its last sound record cannot tell an interrupted commit from a
   damaged one (015).
3. **Why the WAL kept rule 3.**
   - RFC 164 §9 and §9.2 made every other appended file call a complete record damage, decided by the checksum.
   - The WAL was the one exception. Damage there would block every `commit`, and the only way out would be a copy.
   - RFC 164 recorded that "N6's witness (0.49.0 step 3) replaces the trade-off with an exact answer".
4. **What 0.48.0 does instead:**
   - it discloses N6 (`current-state.md`, `durability-recovery.md`);
   - the repair counts *complete-looking* records among what it removes (`complete_records_removed`, `wal.rs`) and says
     so;
   - that count is a guess from the shape. A claimed length past the end of the file, such as 2^62, is not counted.
5. **Stale text:** `durability-recovery.md`'s claims table still says the WAL-tail repair "refuses complete-record
   integrity failures". It has not done so since RFC 162.

## 2. Facts from source

- **The frame** (`wal.rs`):
  - a 58-byte header: magic `PWALR001`, version, seq, body length, and a SHA-256 over all of them and the body;
  - the body is the signed Patch envelope;
  - seq counts from 1 and restarts after a drain.
- **The session directory** is `.prikk/active/<name>/`, holding `queue.wal`, `active.lock`, `ref-name` and
  `declarations`. `init` creates them, and they stay.
- **A commit, in order** (`node_authoring.rs`, under the active lock):
  - content blobs through the object store;
  - author key material;
  - `ref-name` (truncate, then a durable append);
  - the WAL append, durable (file and directory synced);
  - `declarations` cleared;
  - the report, "committed". **A commit is acknowledged only after its record is durable.**
  - A retry whose envelope equals the last record's appends nothing and returns the last seq.
- **A seal drains the queue last:** after the pointer and the log agree, `finish_active_publication_cleanup` truncates
  the WAL to empty, then clears `ref-name`.
- **Nothing durable today identifies an acknowledged commit outside the WAL itself:**
  - `cache/commit-index.v1` is a stat cache;
  - the object index is a pure cache (RFC 162);
  - blobs are shared and content-addressed;
  - the Patch envelope reaches the store only at seal.
- **The format is 7,** shared with every release since format 7. `require_current_format` accepts any format-7
  repository, so **an older binary and a newer one can both write the same repository.**

## 3. Constraints every option must meet

- **C1 — meaning (I6).** A record the user was told had committed is never removed silently, and never read as a crash
  tail.
  - A record that was never acknowledged may still be removed as a tail.
  - The witness decides which is which; it never decides what a record *means*.
- **C2 — the witness grants nothing.** It is unsigned local metadata, so it can only *add* refusals:
  - it never makes a record, a patch, a ref or a signature accepted that would be refused without it;
  - a forged or stale witness can at worst make a crash tail look like damage, which refuses and names the way out;
  - **this is shown from source, call site by call site, not argued.**
- **C3 — fail closed, and nothing silent.**
  - A witness that is damaged, absent, or disagrees with the WAL is reported.
  - No repair removes a record the witness covers without an explicit, named acknowledgement of the loss (K5).
  - §9.2's rule holds for the witness itself, if it is an appended file.
- **C4 — the format.** The starting position is **no format version change in 0.49.0**: an additive file or field in
  format 7.
  - If the round shows that format 7 cannot carry the witness safely, in particular **against an older binary writing
    the same repository** (§2), say so.
  - The owner then decides between a format change and a narrower design, before any implementation handoff.
- **C5 — cost:**
  - a commit gains at most one durable write;
  - `verify` and `status` stay linear in the WAL's size;
  - each bound is shown by measurement on `/home` (LUKS), release build, three samples and their spread.
- **C6 — Rule D.** Every writer of the session (commit, the idempotent retry, seal's drain, the repair) checks the
  witness against the WAL before its first write. A refusal writes nothing.

## 4. Starting positions (to be tested, not decided)

- **Acknowledge after durable.** The witness is written after the WAL append and before the report.
  - A crash between the two leaves a sound record the witness does not cover. The user was not told, so it is not
    acknowledged, and it stays exactly as today.
- **Three shapes of witness, from least to most:**
  - **W1:** the count or end offset of acknowledged records, as RFC 163 §5 named;
  - **W2:** W1, plus the identity of the last acknowledged record: its seq, its Patch id, and its frame's hash;
  - **W3:** W1, plus a running hash over every acknowledged frame, updated in O(1) per commit and checked by `verify`
    in O(WAL).
- **Three places for it:**
  - **P1:** a small file in the session directory, replaced atomically each commit;
  - **P2:** a fixed-size appended witness log, where a torn last entry is exactly a crash before acknowledgement;
  - **P3:** folded into `ref-name`, which every commit already rewrites (today *before* the append).
- **A drained queue must not read as a lost one.** If the WAL is shorter than the witness, the witnessed Patch is either
  sealed and reachable from the published ref (a drain, so the witness is stale) or not (a loss).
  - That is RFC 162's "commitment proven by connectivity". It may also make an older binary's drain safe.
  - It needs W2 or W3: W1 alone cannot tell a drain from a loss.
- **An acknowledged damaged record has a way out, never an implicit one:**
  - an explicit flag that names the loss, with `--plan-only` (K1);
  - the bytes are kept as today;
  - the commit's content is often still in the working tree, so committing again recovers it. The text should say so
    only where that is true.

## 5. Questions for the design round

1. **Write order:** every write ordinal of `commit`, the idempotent retry, seal's drain, and `--repair-wal-tail`, with
   the witness prototyped (each of P1–P3 where they differ). For each prefix of those writes:
   - the state a crash leaves;
   - what `verify`, `status`, `doctor`, the repair, a retried `commit` and a `seal` do.

   **The states come from failpoints, not reading.** One table.
2. **The decision table:** (the WAL's end: sound, a crash-shaped tail, a complete damaged record, shorter than the
   witness, empty, missing) × (the witness: agrees, behind, ahead, absent, damaged).
   - For each cell: the verdict (tail, acknowledged damage, a drain, a loss), what `verify` reports, and the safe way
     out.
   - **Every cell is reproduced, not reasoned.**
   - **List first any cell where no option meets C1–C3.** The architect rules on those before anything else.
3. **W1, W2 or W3:** which of the five I1q cells each closes, and which needs connectivity (§4). Include multi-field
   corruption of the last record: what W2 and W3 catch that W1 and RFC 164 §9.2 cannot.
4. **The witness's own integrity:**
   - checksum or frame;
   - a damaged witness;
   - **an absent witness:** a repository made by 0.48.0 (legacy) against one whose witness was removed. Can they be
     told apart? If not, what does each reader do?
   - C2, call site by call site: every reader of the witness, and the proof that it only adds refusals.
5. **Format 7 and older binaries (C4):** run the 0.48.0 binary (`.pgtmp/prikk-1c0d5b18`) on a repository carrying the
   prototype's witness, and the prototype on a repository made by 0.48.0:
   - a 0.48.0 commit after a witnessed one;
   - a 0.48.0 seal, which drains the WAL without touching the witness;
   - a 0.48.0 `--repair-wal-tail`.

   For each: what the newer binary then reports, and whether it is ever a false loss or a missed one. **If no shape is
   safe in format 7, say so plainly. That is the owner's decision.**
6. **Cost:** release builds, `/home` (LUKS), three samples and the spread:
   - `commit` time before and with each prototype, at 1, 64 and 1,024 queued commits;
   - `seal` at 64 and 1,024;
   - `verify` with W3 at 1,024.
7. **Text and the matrix:**
   - the five I1q cells: what each would read as under each option;
   - the stale claims-table row;
   - the repair's line, which today counts complete-looking records, against what an exact witness would print.

## 6. Out of scope

- **The per-file witness for the other appended files** (RFC 164 §9.1, alternative 3), and multi-field corruption
  elsewhere: format-8 input.
- **M5's structural fix** (0.49.0 step 4).
- **Non-default active sessions,** beyond whatever the witness's placement implies for them; name it if it implies
  anything.
- Network transport, and any change to what a signature or a ref means.
