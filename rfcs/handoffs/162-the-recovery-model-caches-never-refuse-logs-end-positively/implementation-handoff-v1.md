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

## Addendum 1 — 2026-09-27: M1's other half, nothing silent, and M5 no worse

Report `rfc162-recovery-model-report-v1.md`; review `.git-exclude/reviewed/rfc162-recovery-model-review-v1.md`. **M2 and M3
are fixed.** The architect re-ran the gates (14/14 on `f74401a4`) and ran the external `reproduce.sh` on a release build
of `f74401a4`. **Nothing is pushed until this lands.** These items complete RFC 162 as written; they add no new semantics.

1. **`doctor` fails where `verify` fails on connectivity.** After M1's repair, `verify` exits 1 and `doctor` exits 0
   with `errors=0`. I2 holds for `doctor` too.
   - **Control:** the M1 pair asserts `doctor` non-zero after the repair.
   - **Perturb:** drop connectivity from `doctor`. It goes red.
2. **No frame is called harmless while a referenced object is missing.** After the repair, the damaged blob's frame is
   still printed as `interrupted append … (unreferenced, not damage)` / `no index entry names it`, beside the
   connectivity error for the same blob.
   - While connectivity reports any missing or unreadable referenced object, an unparseable frame is reported as
     possibly holding it, naming the object. It is never "unreferenced" or "not damage".
   - The index wording goes from every message.
   - **Control:** M1 after the repair prints neither phrase.
3. **Nothing is silent.**
   - A trailing partial and interior garbage in the **object index** get a warning or info line in `verify` and
     `doctor`, as the WAL does. The exit stays 0, because it is a cache.
   - The pointer index's tail and interior damage are reported too.
   - **The matrix asserts a report line per cell.** **Perturb:** silence the index line. It goes red.
4. **M5 no worse after rule 3.**
   - The architect measured, with `reproduce.sh` on hostile input, 0.48 / 1.91 / 7.18 / 28.64 s at 256 KiB → 2 MiB,
     against `bb81b0fb`'s 0.25 / 0.88 / 3.49 / 14.02 s. That is **2.0×**, because the WAL's `Invalid` arm now scans too.
   - **Remove the duplicate work.** "None found" ends the decode as a tail; "found" resumes there, and a rejected
     candidate is never re-examined.
   - **Reset shape B's ceiling to `bb81b0fb`'s measured cost** at 32 and 64 KiB, not to the doubled figure.
5. **Run X2:** `reproduce.sh` against your release build of the final commit and of `bb81b0fb` (a worktree at that
   commit). Attach both outputs. Acceptance: every M1 cell non-zero after the repair, M2 and M3 as now, and M5 at or
   below `bb81b0fb`.

Every run in R1's scope, perturbations capped and timed. Gates on the exact final commit. Report:
`.git-exclude/review-request/rfc162-recovery-model-report-v2.md`.

**ACCEPTED 2026-09-27** (`5ac71e96` … `11d50cc4`; reviews `rfc162-recovery-model-review-v1`, `-v2`; 14/14 gates re-run by
the architect on `11d50cc4` in R1's scope, 2445 / 0 / 56). The architect ran the external `reproduce.sh` on their own
release build:
- **M1, M2 and M3 are closed**; after M1's repair, `verify` and `doctor` both exit 1;
- **M5 is at `bb81b0fb`'s level.** The architect's own diagnosis of its doubling was wrong; the team found the real
  cause, a second WAL decode in the connectivity stage;
- nothing is silent.

**CLOSED 2026-09-27:** pushed at `d89b5d7d`, CI run `36322845875` 16/16, the Windows mutation suite green.
