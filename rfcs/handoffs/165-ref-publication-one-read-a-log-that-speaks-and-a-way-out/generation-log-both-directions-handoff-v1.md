# The lost generation log, decided in both directions — before the 0.50.0 tag

**Live 2026-10-09, and it is next.** External review 020 found that the candidate `b345a864` blocks the tag (020
§3.1; assessment `external-review-020-assessment-v1`).

## Task title and purpose

When a compacting container's generation log is emptied or removed, readers deduce the live slot from the two slots'
contents. **The rule ruled in Part E4 asks only whether slot B is made from slot A.**
- **Where it goes wrong:** after an even number of compactions, A is the newer slot and B the stale one. When B holds
  a superseded entry, the rule picks B.
- **020 reproduced it on all three containers:**
  - a revoked maintainer key reads as trusted, and `verify` exits 0;
  - a branch disappears;
  - a received tip goes back.
- **The recommended `prikk compact` then makes the loss permanent,** with nothing saved.

The error is the architect's: the case table behind E4 covered only one parity.

## The rule, as ruled (replaces Part E4's)

Notation:
- `F(X)` is the fold of a slot's decoded entries: the running reduction `fold_entry` already computes, which equals
  what a compaction writes.
- "X is stale beside Y" holds when **either** of two relations holds:
  1. **Committed compaction:** `Y` starts with `F(X)`. That means `Y = F(X) ++ W`: Y was compacted from all of X,
     then took writes.
  2. **Crash before the log append:** `X` equals `F(P)`, or a prefix of it, for some prefix `P` of `Y`. That means X
     is a compaction of an earlier Y, maybe cut short, and Y kept taking writes. This is E4's existing loop, with the
     roles as parameters.

**The deduction** (only when the log has no record and slot B holds data, as now):

| A stale beside B | B stale beside A | answer |
|---|---|---|
| yes | no | **B** |
| no | yes | **A** |
| yes | yes | `F(A) == F(B)`: either. Return A, and report *"both slots agree"*. Otherwise **refuse** |
| no | no | **refuse**: no compaction sequence produces this state |
| either slot damaged | | **refuse**, as now |

**Every refusal names its way out:**
- **the pointer index:** `prikk doctor --rebuild-pointer-index --plan-only`, because the ref log is the authority;
- **the received index and the trust policy:** a copy of the whole `.prikk/` from a backup taken before the damage.
  For the trust policy, add *"then re-apply every trust change made since that backup"*. This is P3d's wording.

**A compaction in the deduced state saves what it overwrites.**
- `prikk compact --<container>`, when the live slot was deduced and not recorded, saves the slot it is about to
  overwrite, and the generation log, as **one recovery run** before writing. These are RFC 168 `Replace` entries, as
  the rebuild's F2 save.
- `--recovery-restore <run>` gives back both files byte-identically, and refuses after later writes.
- An ordinary compaction, with the log recorded, saves nothing, as now.

**The recommendations in the deduced state:**
- **the pointer index:** `prikk doctor --rebuild-pointer-index --plan-only`, then the rebuild. It re-derives from the
  ref log and trusts neither slot;
- **the received index and the trust policy:** `prikk compact --received-index` / `--trust-policy`. The warning says
  that the compaction saves the other slot first.

## The case table (each row is a test, on all three containers unless marked)

`k` is the number of committed compactions before the log is lost. "Superseded" means the stale slot holds an entry
that a later entry replaced. `W` is the writes to the live slot after its last switch. The answer is the slot that
holds the latest writes; "either" means both slots fold to the same state, and the test asserts that reads answer
the same.

| # | state when the log is lost | answer |
|---|---|---|
| 1 | never compacted, B empty | A (no deduction) |
| 2 | k=1, `W` empty, A compact | either (both agree) |
| 3 | k=1, `W` empty, A superseded | either (both agree) |
| 4 | k=1, `W` not empty | B |
| 5 | k=1 uncommitted (crash after the slot write, before the log append), no writes after | either |
| 6 | k=1 uncommitted, then writes to A | A |
| 7 | **k=2, B superseded, `W` not empty** (020's P, T, R) | **A** |
| 8 | k=2, B superseded, `W` empty | either |
| 9 | k=2, B compact, `W` not empty | A |
| 10 | k=2 uncommitted (A written, crash), then writes to B | B |
| 11 | k=3, A superseded, `W` not empty | B |
| 12 | k=3 uncommitted, then writes to A | A |
| 13 | the last compaction's output cut short at a record boundary, then writes to the live slot | the live slot |
| 14 | **trust policy:** a key revoked, re-trusted and revoked again, across k=1 and k=2, with `W` repeating a snapshot the stale slot holds | the live slot; a revoked key never reads as trusted |
| 15 | A empty, B with data | B |
| 16 | A and B byte-identical | either |
| 17 | neither relation (B replaced by another repository's container) | refuse, naming the way out |
| 18 | both relations, folds differ (synthetic entries, if no command sequence builds it) | refuse |
| 19 | either slot damaged | refuse |

**Controls,** each run, shown red, and reverted:
- **E4's one-directional rule:** rows 7, 9 and 14 at k=2 go red.
- **Relation 2 only (crash), in both directions:** row 4 and row 11 go red (neither holds, so it refuses).
- **Relation 1 only (committed), in both directions:** rows 6, 10 and 12 go red.
- **Without the both-agree check:** row 18 goes red.

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **Q1: the rule and its table** | the rule above in `resolve_or_deduce`; `DeductionReason` gains the cases (A stale beside B, B stale beside A, both agree); the 19 rows as tests (`foundation/generation` tests, plus CLI-level rows 7 and 14 through real commands); the four controls; **020's `probe3b.sh` on a release build: all three containers read the right slot** | 90 min | `generation-log-both-directions-Q1-report.md` |
| **Q2: the save, the recommendations, the notes** | the compaction save in the deduced state (one run; restore byte-identical; refuses after a later write); the recommendations and refusal texts above; smoke 27d gains row 7 on the trust policy (020's T shape), runs the named command, and checks that `--recovery-list` shows the save; the CHANGELOG's lost-generation-log entry restated with the two-direction rule, and its Output-changes line; the docs-debt grep (`repository-layout.md`, `troubleshooting.md`, every quote of the old rule); `matrix.py` v5 and `reproduce.sh` on the release build; the 14 gates | 75 min | `generation-log-both-directions-Q2-report.md` |

## Explicit non-change scope

- No format change.
- The generation log, slot layout and compaction output are unchanged.
- No change to the rebuild, R4, or any other recovery rule.
- The CHANGELOG heading's date is not touched. It is set at the tag.

## Prohibited shortcuts

- A test that builds the state by writing slot bytes directly, when a command sequence can build it. Rows 1–16 are
  built by commands, as 020 did; rows 17–19 may be synthetic.
- Comparing raw bytes instead of decoded entries.
- A row asserted only by the deduction's return value. The CLI rows assert what a user sees: `trust maintainer check`
  and `branch list`.

## Required evidence

- **Per part:** the change; the tests, with every row named; the controls shown red; the docs grep; and
  `scripts/gates.py`'s summary on the last commit.
- **The report lists every item with its status.**
