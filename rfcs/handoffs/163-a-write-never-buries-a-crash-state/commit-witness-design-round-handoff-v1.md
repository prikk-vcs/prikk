# The commit witness — RFC 166 design round, handoff v1

**Live 2026-10-03, and it is next.** 0.49.0 step 2 (RFC 165) is closed: review
`.git-exclude/reviewed/rfc165-round-2-review-v1.md`.
- **Why it is filed here:** it settles what RFC 163 §5 deferred: *"The witness (the count or end offset of committed
  records, written with each commit) is 0.49.0."* RFC 166 is proposed, and a proposed RFC carries no handoffs.
- **This is a design round: measure, prototype, report, then stop. No product code lands.**

**Read first:**
- `rfcs/proposed/166-a-queued-commit-has-a-witness.md`, all of it, and above all C1–C6;
- RFC 162 rule 3;
- RFC 163 §5;
- RFC 164 §9 to §9.2, the WAL's exception;
- external review 015's N6 section: `.git-exclude/upstream/external-architect/receive/015-review-of-the-0-48-0-candidate/015-review.md`.

**The rules carried from RFC 164's and RFC 165's rounds:**
- **A cut is a question to the architect before delivery.** At ×2 of a unit's budget, file a question in
  `.git-exclude/review-request/` and wait for the answer.
- **Sweep the region:** every write ordinal, or the reached state asserted before anything is asserted about it. Never a
  bare failpoint ordinal.
- **Every measurement names its binary**: release, `cargo build --release -p prikk --locked`, with its sha256. Name the
  filesystem with every timing.
- **Check a calibration before reading a single sample.** An uninterrupted run's exit status and duration come first
  (the merge-kill lesson, review `rfc165-round-2-review-v1`).
- **Every run inside an R1 scope**, with a memory ceiling and a timeout.

## 1. What to answer

**RFC 166 §5, all seven questions.** Each is answered from source, and backed by a measurement where it can be measured.
1. **Write order (Q1):**
   - **one table:** writer × write ordinal → file written, durable state after it, and the verdict of `verify`,
     `status`, `doctor`, the repair, a retried `commit` and a `seal`;
   - the writers: `commit`, the idempotent retry, seal's drain (`finish_active_publication_cleanup`), and
     `--repair-wal-tail`;
   - **the crash states come from failpoints, not reading,** with the witness prototyped. Where P1, P2 and P3 differ,
     each gets its rows.
2. **The decision table (Q2):** the WAL's end (6 shapes) × the witness (5 states), as RFC 166 lays out.
   - **Every cell is reproduced, not reasoned.**
   - **List first any cell where no option meets C1–C3.** The architect rules on those before anything else.
3. **W1, W2 or W3 (Q3):**
   - which of the five I1q cells each closes, run on the prototype with `matrix.py`'s own constructions:
     `.git-exclude/upstream/external-architect/receive/017-review-of-the-candidate-5e50a661/reproduce/matrix.py`;
   - the multi-field case: the last record's length and checksum both rewritten so the frame verifies at a different
     end.
4. **The witness's integrity (Q4):**
   - damaged, absent, and absent-because-removed;
   - **C2, call site by call site:** every reader of the prototype's witness, and for each, what it can make accepted
     (the answer must be "nothing"). **A control:** make one reader accept on the witness's word, and show a test going
     red.
5. **Format 7 and older binaries (Q5):**
   - **0.48.0 is `/home/nabbisen/.pgtmp/prikk-1c0d5b18`.**
   - Run each of the three cross-version sequences RFC 166 names, both directions.
   - **A false loss or a missed one is a row, not a footnote.** If no shape is safe in format 7, say so plainly. That
     is the owner's decision (C4), and the architect takes it to the owner before ruling.
6. **Cost (Q6):**
   - release builds, `/home` (LUKS), three samples per point and the spread;
   - `commit` at 1, 64 and 1,024 queued commits, before and with each prototype;
   - `seal` at 64 and 1,024;
   - `verify` with W3 at 1,024.
7. **Text (Q7):** each of the five I1q cells as it would read under each option, quoted from the prototype's binary;
   the stale claims-table row in `durability-recovery.md`; the repair's line, today against exact.

## 2. Prototypes and measurement

- **Prototypes live in a scratch worktree with their own `CARGO_TARGET_DIR`.** Nothing lands on `main`.
- **Not every combination needs a prototype.** W2 in P1 is the minimum; add others where a question cannot be answered
  without one, and say which you skipped and why.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | the prototype; Q1 failpoint sweep and the Q2 table | 120 min |
| U2 | Q3 and Q4: the I1q cells, multi-field, integrity, C2 by call site and its control | 60 min |
| U3 | Q5: both directions against 0.48.0 | 45 min |
| U4 | Q6: timings | 45 min |
| U5 | Q7: text, from the binary | 15 min |

**Nothing else runs while U4's timings do.**

## 3. Report

- **Answers to Q1–Q7,** with the tables, each option laid out, and **no option chosen**: the architect rules.
- **Each unit's real start and end time,** against its budget.
- **Before proposing:**
  - the 14 gates on `main`'s tip are not needed (no product code);
  - state that the primary tree is clean.
- **Report:** `.git-exclude/review-request/rfc166-design-round-report-v1.md`.

## Addendum 1 — 2026-10-03: finish the round (review `rfc166-design-round-review-v1`)

Report `rfc166-design-round-report-v1.md`. **Not accepted: the round is incomplete, and its timings ran on tmpfs.** Read
review v1, all of it.
- **Still a design round:** measure, prototype, report, stop. No product code lands.
- **Ruled already** (review v1 §4):
  - the prototype is kept;
  - an absent witness means "no witness", reported, falling back to rule 3;
  - P3 is prototyped beside W2 in P1.
- **RFC 166 changed:** §1.6 (the torn `ref-name`), Q1's scope, and the depth 1,000 (the queue limit), not 1,024.

0. **Keep the prototype.** `/home/nabbisen/Desktop/prikk/scratch-166/proto` and its target directory stay until RFC
   166 is accepted. Review paths point into it.
1. **Readers, in the prototype,** under RFC 166 §4 and review v1 §4:
   - `verify`, `status`, `doctor`, `--repair-wal-tail`, and `commit`'s and `seal`'s checks before their first write;
   - connectivity for "WAL shorter or empty × witness ahead": the witnessed Patch reachable from the published ref
     means a drain, and otherwise a loss;
   - **two prototypes:** W2 in P1, and P3 (one atomically replaced session file with the ref name and the witness);
   - **W3:** prototype it, or file a question first. Not a silent skip.
2. **Q1, by failpoints, with the readers' verdicts:**
   - every write ordinal of a **second** commit (blobs, author key, `ref-name`, WAL, witness, `declarations`), of the
     idempotent retry, of seal's drain (**both orders** of WAL truncate and witness clear), and of the repair;
   - **the kill probe,** `/home/nabbisen/.pgtmp/arch-166/commit_kill_probe.py <binary> <workdir> 300`, on each
     prototype's release build: no state without a way out, or each such state classified and named;
   - **§1.6, the torn `ref-name`:** each option that closes it (no rewrite when the WAL is non-empty and the name is
     unchanged; an atomic replace; P3), measured by the same probe.
3. **Q2:** every reachable cell built, with the verdict and the way out from the prototype's readers. Each impossible
   cell, with the reason.
4. **Q3:** W1 as an end offset, too, against the same substitution and against one of a different length.
5. **`matrix.py`:**
   - run it on each prototype:
     `.git-exclude/upstream/external-architect/receive/017-review-of-the-candidate-5e50a661/reproduce/matrix.py`,
     against `matrix-5e50a661.txt` with `compare_matrix.py`;
   - every changed cell explained;
   - what the five I1q cells now read.
6. **Q5, with the readers:**
   - the four sequences;
   - **0.48.0 seal, then 0.48.0 commit** (same seq, different Patch);
   - the prototype draining and repairing a 0.48.0-made repository.
7. **Q6, on `/home`:**
   - **the filesystem printed** (`stat -f -c %T`), and the calibration (exit status, duration) first;
   - `commit` at depths 1, 64 and 1,000, baseline against each prototype, 3 samples, the mean of 5 commits each;
   - `seal` at 64 and 1,000, exit status shown;
   - `verify` with W3 at 1,000, if W3 is built;
   - **your baseline must agree with the architect's** (20.4–22.2 ms, review v1 §1), or say why not;
   - **the "seal @ 1,024" number in report v1:** how it was produced, or withdrawn.
8. **Q7:** the text, quoted from each prototype's binary.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | readers, P3, connectivity (and W3 or a question) | 120 min |
| U2 | Q1 sweep, the kill probe, §1.6's options | 90 min |
| U3 | Q2 table, Q3, `matrix.py` | 90 min |
| U4 | Q5 and Q6 | 60 min |
| U5 | Q7 | 15 min |

**Nothing else runs while U4's timings do.** Each unit's real start and end time.

**Report:** `.git-exclude/review-request/rfc166-design-round-report-v2.md`.

**Design round ACCEPTED 2026-10-03** (reviews `rfc166-design-round-review-v1`, `-v2`). RFC 166 is rewritten as a
design for the owner's reading. **No implementation handoff until the owner accepts it.**
