# RFC 162 — The recovery model: caches never refuse, logs end positively, `verify` checks what is referenced

**Status.** **ACCEPTED by the project owner 2026-09-27** (*"Reviewed and accepted."*). It was presented for reading in
one message and accepted in a later one, as the external review advised. **The architect's reading, stated so it can be
corrected:**
- the three rules of §2 are accepted as written, with the invariants of §4 and the first matrix rows of §5;
- the implementation is **one round in 0.48.0**, live when the R2–R4 round closes, before 0.48.0 prep;
- §3 (a self-vouching header, chained frames) stays format-8 design input and is not 0.48.0 work.

Implementation handoff, **live 2026-09-27**: `rfcs/handoffs/162-the-recovery-model-caches-never-refuse-logs-end-positively/implementation-handoff-v1.md`.

*History:* **PROPOSED 2026-09-27 by the architect**, after the external review of `bb81b0fb` (letter 014).

**Author-review independence:** the architect proposes, and it was the architect's own F3 Addendum 1 ruling that
produced M1. Two things compensate:
- the external architect will re-run their reproduction script, and the matrix of §5, against the 0.48.0 candidate;
- every rule below comes from named prior art, not from our own invention.

## 1. Why a design and not another addendum

The external review measured three recovery defects that block 0.48.0. The architect reproduced each on a release build
of `bb81b0fb`, with the same numbers:
- **M1:** `doctor --repair-index` turns a detected damaged blob into a clean `verify`. The repair drops its index
  entry, and F3's rule then calls the unparseable frame "an interrupted append, nothing references it", while the
  queued commit still needs it (`seal` fails). New in this cycle: 0.46.0 keeps failing.
- **M2:** one torn object-index append, then one commit, and every command refuses. `--repair-index` answers "nothing
  to repair".
- **M3:** a tail of zeros or garbage after the last sound WAL record is fatal from 100 bytes up, and repairable below.
  The pointer index has no way out at all.

**All three come from two rules the project adopted within the last day:**
- **the index as the witness of commitment** (F3 Addendum 1, the architect's ruling);
- **a tail defined by its shape** (F3's "a torn tail is a prefix of one frame").

The F3 round needed two addenda. **The review's signal is right: the design was incomplete, and patching it a third time
would repeat the pattern.** This RFC replaces those two rules with three that each have a precedent.

## 2. The three rules

1. **The object index is a pure cache. Damage to it never makes a command refuse** (precedent: git's `.idx`, always
   regenerable and never evidence).
   - A **writer** holding the object-store lock that finds the index damaged (a failed entry, an interior partial frame,
     a trailing partial) rebuilds it from the containers **before** appending. It never appends behind damage (M2).
   - A **reader** without the lock falls back to scanning the containers, or rebuilds under the lock. It never refuses
     while the containers are sound. The implementing round chooses between the two, and reports what each costs.
   - `--repair-index` rewrites whenever the file is not byte for byte the encoding of its sound entries (M2's "nothing
     to repair").
   - **The index is never evidence of anything.**
2. **What is committed is proven by what references it** (precedent: git's `fsck` connectivity).
   - `verify` checks that every object referenced by committed or queued work exists and reads: sealed blocks' states,
     queued patches in every active WAL, and ref tips.
   - An unparseable frame in an object container is then classified as follows:
     - if any referenced object is missing or unreadable, it is **damage**, with the referencing work named;
     - if every referenced object is present, it is an **unreferenced remnant**, reported as a warning, and a crash
       never makes `verify` fail for good (incident 7 stays fixed).
   - This replaces index membership as the witness. After M1's repair, the queued patch still references the blob, so
     `verify` fails, and names the patch.
   - **`--repair-index` never forgets silently.** If an id the old index named cannot be re-derived from the containers,
     the repair writes the lost ids to a durable file under `recovery/`, names every one on stderr, names any work
     that references it, and exits non-zero.
3. **A log ends at its last sound record** (precedent: SQLite's WAL, PostgreSQL's recovery, RocksDB's recovery modes).
   - For the WAL and the pointer index, the tail is **everything after the last sound record, when no sound record
     follows**, whatever its shape: zeros, garbage, or a torn prefix.
   - The repair truncates it and **saves the removed bytes** to `recovery/`, as the WAL repair does now. Since the bytes
     are kept, widening what counts as tail loses nothing (M3).
   - **Interior damage**, where a sound record follows, stays refused, and names its offset, as F3 made it.
   - **The pointer index gets a way out.** If it is derivable from the ref log, it is a cache under rule 1. If it is
     not, it gets the same repair as the WAL. The round finds out which, and reports it.
   - **The ref log keeps its own rule.** It truncates only a suffix that is a prefix of the record it expected to write,
     a positive witness, and it is the model. Its silence over unparseable bytes (M4) is fixed in 0.49.0 with F1, under
     the principle below.

**The principle behind all three, for every framed file:** every byte is a sound frame, a reported tail, or reported
damage. Nothing is silent, and nothing a repair removes is lost.

## 3. What this RFC does not do (format 8, not 0.48.0)

The review's second and third prior-art rows need a format change. They are recorded as **design input for format 8**,
which RFC 158 opens:
- a frame header that vouches for itself (its own checksum over 60 bytes), so resync never hashes a claimed body (M5's
  structural fix);
- each frame chained to its predecessor, or to a per-file salt, so a frame embedded in a payload can never validate
  (the phantom object, and F3's ambiguous case).

## 4. The invariants the implementation must hold (the review's I1–I4)

- **I1.** `verify` exiting 0 implies that the next `seal`, and every read of committed work, succeeds.
- **I2.** No repair turns a failing `verify` into a passing one unless what was damaged is restored.
- **I3.** From every state a crash can leave, a documented command sequence reaches a repository that accepts a commit.
- **I4.** Every repair is idempotent, removes no sound record, and keeps what it removes.

## 5. The first rows of the crash-and-corruption matrix

The implementation round lands a CLI matrix, seeded from the external architect's `reproduce.sh`:
- **files:** object container, object index, WAL, pointer index;
- **faults:** a torn prefix, 30, 100 and 4,096 zero bytes, 100 random bytes, a flipped body byte, a flipped length;
- **next commands:** `verify`, `doctor`, each repair, then `commit`, `seal` and a read;
- **checks:** I1–I4 above.

M1, M2 and M3 are its first failing rows. The matrix is the start of what the review makes a gate for the word
"stable" (D1). **It runs under R1's cgroup scope, and in CI with a timeout.**

## 6. Scheduling

**Proposed:**
1. R2–R4 now (already live).
2. This RFC, **read by the owner, then accepted or changed**.
3. One implementation round.
4. 0.48.0 prep, including D2, D4, D5 and D9.
5. A candidate, which the external architect re-runs against.
6. The cut, **without the word "stable"**.

## 7. DELIVERED 2026-09-27

Accepted at `11d50cc4` (reviews `rfc162-recovery-model-review-v1`, `-v2`).
- **Rule 1:** readers scan in memory and never persist from an unlocked path; writers rebuild before appending; the
  repair rewrites unless the index is byte-identical to its sound encoding.
- **Rule 2:** a connectivity stage covers the queued patches of every active session; `doctor` fails with it; the
  repair names and saves the ids it loses; a frame is never called harmless while a referenced object of its type is
  missing.
- **Rule 3:** the WAL and the pointer index end at their last sound record, and the pointer index gets its own tail
  repair, because it leads the log by design.
- **The matrix:** four files × seven faults, with I2, I3 and I4 asserted, and a report line per cell.

The external `reproduce.sh`, run by the architect on the final commit, shows M1, M2 and M3 closed and M5 back at
`bb81b0fb`'s cost.
