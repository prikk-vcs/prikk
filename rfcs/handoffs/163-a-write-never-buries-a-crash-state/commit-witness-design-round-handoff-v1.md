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
