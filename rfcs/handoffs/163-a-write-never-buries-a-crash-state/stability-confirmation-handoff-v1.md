# 0.49.0 step 0 — confirm 0.48.0's stability (measure only)

**Live 2026-09-30, and it is next.** 0.48.0 was released today, at `5e50a661`. The owner approved the 0.49.0 plan (theme:
"a way out of every crash state"; the ROADMAP row "SCHEDULE for 0.49.0"). The owner deferred the consumer letters **"to
confirm stability on the recent update and improvement first"**. This round is that confirmation.

**Measure only.** No product code changes. New tests and harnesses may land, as their own commits, `#[ignore]`d where
they are long. **Anything found is reported for the architect's ruling, not fixed here.**

## 1. What runs, on what

**The binary under test is the shipped 0.48.0**: the Linux x86_64 release asset, downloaded from the GitHub Release, with
its `.sha256` checked. For in-tree tests, the tree is the tag `0.48.0`. Say which binary each result came from.

1. **The full RFC 133 sweep** (the full driver, not the release-gate profile), on the tag, release build. Report the
   shape at all seven points and both series, against the last full sweep on record. Say whether anything moved beyond
   the measured spread.
2. **The external architect's `matrix.py` v4 and `reproduce.sh` v2** (`.git-exclude/upstream/external-architect/receive/
   016-…/reproduce/`), on the shipped binary. Compare them with `matrix-5e50a661.txt` (receive/017) cell for cell, using
   `compare_matrix.py`.
3. **The architect's seven probes** (`/home/nabbisen/.pgtmp/arch-seal/rfc163_*.sh`), on the shipped binary.
4. **A crash-injection soak.** This is the new part. Two modes:
   - **(a) Black-box, on the shipped binary.** Run real commands (`commit`, `seal`, `branch create`, `tag create`,
     `bundle import`, `sync accept`, `trust maintainer add`, `compact --all`, each repair) against fixture repositories,
     and **SIGKILL each one at a random moment**, over a seeded distribution. After every kill:
     - `verify` exits 0, or it names damage that a documented command or the documented manual way out clears;
     - the way out ends in a repository where `verify` exits 0 and a `commit` is accepted;
     - no command aborts on a signal of its own, or hangs past its time limit;
     - no commit whose command exited 0 is missing afterwards (I1q).

     Record the seed, the command, the kill delay and the file sizes for every failing iteration, so each one can be
     replayed.
   - **(b) White-box, in `prikk-store`'s own tests.** The failpoint seams (`foundation/fsutil/anchored/failpoints.rs`)
     are `cfg(test)`. First list which store operations already have a failpoint sweep. Then add one `#[ignore]`d,
     seeded, randomized sweep over (operation × failpoint × ordinal) for the operations above, asserting the same
     invariants through the store API.
   - **Budget and size:** state N for each mode up front. Use R1's scope on every run. Stop at twice the budget.
5. **CI:** read the latest `ci.yml` runs on `main`, the Windows and macOS mutation suites by name. The last is
   `36682213868`, 16/16.

## 2. What is a finding

- **Any failing soak iteration.** Report its replayable record and your classification: a new defect, or an instance of
  a disclosed limitation (N2's remainder, N3, N6, N7, N10, RFC 163 §10). **Disclosed means its known-limitations text
  covers exactly what you saw.** Say which sentence covers it.
- **Any matrix cell that differs from `matrix-5e50a661.txt`.**
- **Any shape change in the full sweep** outside the measured spread.

**An explained cell or iteration gets its own replay, not an argument.** 0.48.0's last external blocker (N9) sat in a set
of cells we had explained without running.

## 3. Units and budgets

| unit | what | budget (stop at ×2) |
|---|---|---:|
| S1 | the full RFC 133 sweep, release, on the tag | 60 min |
| S2 | `matrix.py` v4, `reproduce.sh` v2 and the seven probes, on the shipped binary | 45 min |
| S3 | soak (a): N iterations, N stated in the report's first line | 90 min |
| S4 | soak (b): N stated likewise | 60 min |

**Nothing else runs while S1 does,** the architect's gates included.

**Report:** `.git-exclude/review-request/stability-confirmation-report-v1.md`. It carries the binary's sha256, each unit's
N and wall time, and every finding with its replay record. Report the gates only for any commit that lands test code.

**ACCEPTED as a measurement 2026-09-30** (review `stability-confirmation-review-v1`; test code `003fe7b4`). Finding 3: a
killed `bundle import` leaves `verify` failing for good. The architect reproduced it on the shipped 0.48.0 and on 0.47.0.
The cause is objects written in bundle order, not dependency order, and re-running the import heals it. Finding 2 is N3,
reproduced by a real kill. Finding 1, genesis RSS, is to be measured. **Next, live:** `stability-follow-up-handoff-v1.md`.
