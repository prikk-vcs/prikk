# The recovery model — RFC 162 implementation, handoff v1

**Live 2026-09-27, and it is next.** The R2–R4 round closed at `87dc151f`: CI run `36307290831` went 16/16, the Windows
mutation suite green, and the new job time limits held. **In 0.48.0, and it blocks it** (owner, 2026-09-27: *"Reviewed and
accepted."*). **Next after this:** 0.48.0 prep, then the candidate, which the external architect re-runs against, then the
cut without the word "stable".

**Read first:**
- `rfcs/accepted/162-the-recovery-model-caches-never-refuse-logs-end-positively.md`, all of it;
- the external review, `.git-exclude/upstream/external-architect/receive/014-review-before-0-48-0-findings-and-answers/014-review.md`,
  §2 (M1–M3) and §5, question 3;
- its `reproduce/reproduce.sh`: your acceptance must include a clean run of it.

**This round carries RFC 162's three rules and nothing else** (RFC 152 §7: a round carries its own subject). If you find
something else, report it; do not fix it here.

## 1. What lands

1. **Rule 1 — the object index is a pure cache.**
   - A writer holding the object-store lock rebuilds a damaged index before appending, and never appends behind a
     damaged or torn tail.
   - A reader never refuses while the containers are sound. Either it rebuilds under the lock, or it scans. Measure
     both and report the choice with its cost.
   - `--repair-index` rewrites whenever the file differs from the encoding of its sound entries.
   - The index is never evidence of anything. Remove F3 Addendum 1's "no index entry names it" classification.
2. **Rule 2 — connectivity is the witness.**
   - `verify` checks that every object referenced by sealed blocks' states, queued patches (in every active WAL) and ref
     tips exists and reads.
   - An unparseable object-container frame is **damage** if any referenced object is missing or unreadable, with the
     referencing work named. Otherwise it is an **unreferenced remnant** (a warning).
   - `--repair-index` never forgets silently. Lost ids go to a durable file under `recovery/`, each one named on
     stderr with any work that references it, and the exit is non-zero.
3. **Rule 3 — a log ends at its last sound record** (the WAL and the pointer index).
   - Everything after the last sound record, when no sound record follows, is tail, whatever its shape. The repair
     truncates it and saves the removed bytes.
   - Interior damage stays refused.
   - The pointer index: find out whether it is derivable from the ref log. If it is, it is a cache under rule 1. If it
     is not, it gets the WAL's repair. Report which.
   - **The ref log is unchanged** (M4 is 0.49.0).

## 2. Controls — the matrix rows (RFC 162 §5), landed, each shown red

- **The matrix, as a CLI test file:**
  - **files:** object container, object index, WAL, pointer index;
  - **faults:** a torn prefix, 30, 100 and 4,096 zero bytes, 100 random bytes, a flipped body byte, a flipped length;
  - **next commands:** `verify`, `doctor`, each repair, then `commit`, `seal` and a read;
  - **invariants I1–I4**, each asserted per row.
- **M1, M2 and M3 are named rows.** Each fails on `bb81b0fb` (show it) and passes after.
- **Perturbations**, each reddening its rows (run under R1's scope, capped and timed):
  - restore the "no index entry names it" classification (M1's row);
  - let a writer append behind a torn index tail (M2's row);
  - restore the shape rule for the tail (M3's rows);
  - let `--repair-index` drop an id silently (I2).
- **Incident 7 stays fixed:** a crash-torn object frame that nothing references, then later writes. `verify` exits 0
  with a warning.
- **M5's second path gets its standing ceiling here, because rule 3 rewrites that path.** The architect measured both
  hostile shapes (`/home/nabbisen/.pgtmp/arch-seal/m5_two_paths_probe.py`):
  - **shape A** has every header fitting and then failing its checksum. It goes through RFC 102's old `Invalid`→resync
    loop, and is quadratic on 0.47.0 too. R2–R4's standing test guards it;
  - **shape B** is the external review's own `mkhostile`: a leading header claiming more than remains, so the tail runs
    through F3's `sound_frame_after_partial`. It is **0.00 s on 0.47.0 and 0.22 → 3.48 s (256 KiB → 1 MiB) on
    `bb81b0fb`**: new in this cycle, and not yet guarded.

  Rule 3 must still find "no sound record follows" before it calls the rest a tail, which is exactly that scan. **Add
  shape B to the standing ceiling test** at 32 and 64 KiB: bytes hashed at or below 1.5 × today, **and no worse after
  rule 3**. If rule 3 happens to make it linear, assert the 8× bound instead, and say so.

## 3. Units and budgets

| unit | what | budget (stop at ×2) |
|---|---|---:|
| X1 | the matrix, one full run, this build | 10 min |
| X2 | the external `reproduce.sh`, this build and 0.47.0 | 5 min |
| X3 | the reader-path choice under rule 1: a cold `status`/`cat` on a damaged index, scan against rebuild | 5 min |

## 4. CHANGELOG (RFC 161 shape)

- `### Fixed`, one entry per defect, M1 named as introduced and fixed within the 0.48.0 cycle;
- `### Output changes`: `verify` now fails over missing referenced objects; `--repair-index` exits non-zero when it
  loses ids; the repair's recovery file;
- `### Upgrading` if any command's refusal changes.

**Report:** `.git-exclude/review-request/rfc162-recovery-model-report-v1.md`. It covers:
- the gates on the exact final commit, **run in R1's scope**;
- the matrix table;
- the controls;
- X1–X3;
- the pointer-index answer;
- anything in RFC 162 that is not true at source.
