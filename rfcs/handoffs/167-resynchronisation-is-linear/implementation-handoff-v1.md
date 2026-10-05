# RFC 167 implementation — one work budget per decode, exhaustion is damage, `verify` back to its cost

**Live 2026-10-05, and it is next.** RFC 167 is ACCEPTED by the owner. **Read all of it first:**
`rfcs/accepted/167-resynchronisation-is-linear.md`, above all §4 (D1–D6) and §6 (the self-review, whose guards bind).
- Also read the design-round review, `.git-exclude/reviewed/rfc167-design-round-review-v1.md`: what it corrected,
  and the doubled-`verify` table.
- The prototype in `/home/nabbisen/Desktop/prikk/scratch-167/proto` is a reference, not a patch to copy. **D1 differs
  from it:** an explicit budget value, not a thread-local counter.

**The rules carried from RFC 166's rounds:**
- **A cut is a question to the architect before delivery,** at ×2 of a unit's budget.
- **Every site gets its own control, and a control must be able to fail.**
- **Every measurement:**
  - release build from the final commit, sha256 stated;
  - on `/home`, with `stat -f -c %T` printed;
  - the calibration line first;
  - **compared against `main`** (`/home/nabbisen/.pgtmp/prikk-606e0385`) and, for D5, the real 0.48.0
    (`/home/nabbisen/.pgtmp/prikk-0.48.0-5e50a661`).
- **Every run inside an R1 scope with a timeout:** a hostile input on `main` runs for minutes.
- **Read the code before writing a claim about it,** and cite the line.

## 1. D1 and D2: the budget and "undetermined" (U1)

1. **A budget value** (for example `ScanBudget`), created once per decode call at 8× the input length, and passed to
   both placements:
   - **narrow:** inside `sound_frame_after_partial`, between candidates;
   - **broad:** in each reader's own decode loop, charging the reader's ordinary per-frame hashing.
2. **The six affected readers:** the WAL, object containers, trust policy, received index, ref container and pointer
   index. **List every call site** and its "undetermined" arm, which is damage, never tail.
3. **The four immune readers are untouched.** Say from source why each is immune.
4. **The messages (§6 item 6):** one coherent statement per case, as RFC 167 D2 words it.
   - Where Rule E makes an object frame a remnant, the message says remnant and nothing contradictory.
   - Quote each from the binary.
5. **Tests:** for each of the six, shapes A and B, the verdict is damage (or remnant, under Rule E) and the time is
   linear. **Control:** remove either placement, and that reader's test goes red (each placement was shown
   insufficient alone).

## 2. D4: the guards (U2)

1. **The reader-level linear bound,** for all six readers and both shapes: bytes hashed at most 8× the input plus a
   stated slack, at 32 KiB, 256 KiB and 2 MiB. It replaces the 1.5× ceilings; remove the ignored ceiling test.
2. **The command-level row:** a whole `verify` on hostile input (the WAL, a blob container, the ref log), bytes hashed
   at most k× the input. **Control:** add a second decode of the WAL inside `verify`, and the row goes red.

## 3. D5: `verify` back to its cost (U3)

1. **Find the second decode.** It entered between `24ca5991` and `a222d2c2` (RFC 164 round 1's last fix, or round 2),
   and RFC 165 and 166 added about 4 s more on a 2 MiB hostile WAL.
   - **Bisect with release builds,** using `/home/nabbisen/.pgtmp/arch-166/m5_probe.sh` (2 MiB is enough), and name
     the commit and the call site.
2. **Remove it** without losing what the second reader checked. Say what it checked, and where that check now lives.
3. **Measure honest `verify`** on a 1,000-commit WAL, 3 samples, against 0.48.0: at most 1.2× its cost. Do the same
   on the hostile 2 MiB WAL against 0.48.0's time, for the record (the budget makes it fast anyway).
4. **The command-level row (§2.2) is what keeps it from returning.**

## 4. The honest margin (U4, §6 item 2)

1. **The largest bytes-hashed-to-input ratio** of any decode call over:
   - the whole test suite;
   - the RFC 133 corpus;
   - `matrix.py` (`.git-exclude/upstream/external-architect/receive/017-review-of-the-candidate-5e50a661/reproduce/`)
     against the cached pre-RFC-166 run, `/home/nabbisen/.pgtmp/ext-matrix-83a42498.txt`;
   - **a repository whose committed files contain thousands of every frame magic,** with a real crash tail (the design
     round's `SIGXFSZ` method).
2. **8× must leave at least a 4× margin over that ratio.** If it does not, stop and ask.
3. **`matrix.py`:** every changed cell explained with its own replay.

## 5. D6 and the text (U5)

- `current-state.md`: M5 closed, how, and that a self-vouching header is format-8 input.
- The 0.48.0 CHANGELOG line ("Fixed in 0.49.0") made true. `## Unreleased`: `### Fixed` (M5; the doubled `verify`),
  and `### Output changes` (the new messages).
- `troubleshooting.md` and `durability-recovery.md`: the new messages, quoted from the binary, with the way out per
  reader (RFC 167 D3).

## 6. Units, gates, report

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | §1: the budget, both placements, six readers, messages, tests and controls | 120 min |
| U2 | §2: the two guards and their controls | 60 min |
| U3 | §3: bisect, remove the second decode, honest `verify` against 0.48.0 | 90 min |
| U4 | §4: the honest margin, `matrix.py` | 60 min |
| U5 | §5: text | 45 min |

**Before proposing:**
- all 14 gates on the final commit;
- the architect's probes on the release build: `m5_probe.sh`, `container_tail_block_probe.sh`, and the `rfc163_*`,
  `rfc164_*`, `rfc165_*` probes in `/home/nabbisen/.pgtmp/arch-seal/`;
- the kill probe (`/home/nabbisen/.pgtmp/arch-166/commit_kill_probe.py`, 300 kills): unchanged, 0 stuck.

**Report:** `.git-exclude/review-request/rfc167-implementation-report-v1.md`, with each unit's real start and end.

**K7:** a second addendum, or any finding that would change D1–D6, goes back to the owner before the work continues.

**ACCEPTED and CLOSED 2026-10-05** (review `rfc167-implementation-review-v1`). Commits `2374491b`, `00674e00`, `85e2bed5`, `becaadbe`. One text note (a remnant count beside its own warning) is carried into release prep.
