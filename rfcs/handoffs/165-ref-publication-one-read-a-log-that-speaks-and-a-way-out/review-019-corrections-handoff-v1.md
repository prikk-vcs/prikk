# 0.50.0 step 1 — review 019's corrections: every message points the right way

**Live 2026-10-07, and it is next.** The owner approved the 0.50.0 plan (ROADMAP, "SCHEDULE for 0.50.0").

## Task title and purpose

**External review 019 cleared 0.49.0, and graded these for 0.50.0.** Fix them so that, in every repair state, the
message a user reads names a command that works in that state. And so that `verify` and `doctor` never disagree.

## Background and governing RFCs

- **Read first:** `.git-exclude/upstream/external-architect/receive/019-review-of-the-0-49-0-candidate-ab27fc48/019-review.md`
  §4 and §5. Its probes (`reproduce/probe.sh`, `probe2.sh`) build every state below on the matrix base.
- **RFC 165:** R4, the four conditions (a)–(d) in `ref_completion.rs:10-17`; R5, the rebuild, in `pointer_rebuild.rs`.
- **RFC 162 for the object index; RFC 151 for `current-branch`; RFC 166 for the witness.**
- **The architect's matrix finding:** `.git-exclude/reviewed/release-0.49.0-candidate-matrix-review-v1.md` (A5).

## Change scope

**A1. The rebuild drops a lead only when it is not authorized** (019 §5.1; an RFC 165 R5 amendment, ruled by the
architect):
- **A lead that fails (a) verification or (c) target is dropped,** as today.
- **A lead that fails only (b), because it is more than one transition deep while every RefState verifies and chains,
  is refused.** The refusal names the ref and the depth, and says how to recover: restore the ref log from a copy, then
  run the rebuild again.
- **The plan says the same before it writes.**
- **Test:** 019 §4.4's emptied-log state. `heads/main` with two signed, chained transitions is refused, not dropped.
  **Control:** remove the depth check, and the test goes red.

**A2. In the N3 state, the refusals name `ref complete`** (019 §5.2):
- **`verify`'s warning, `seal`, `tag create`, `--repair-tails`'s refusal, `commit`'s refusal and the rebuild's refusal**
  name `prikk ref complete <ref>` when the tail is a lead's torn record.
- **`commit`'s "signer-backed seal retry" text** (`refs.rs:483`; its own comment at `:491` says it is wrong here) gives
  way to the same route.
- **The rebuild names the tail's real offset** (019 saw "byte offset 0" for a tail at 1615).
- **Test:** follow each message in that state, and the command it names succeeds.

**A3. Over a complete damaged pointer-index record, `doctor` and the tail repairs name `--rebuild-pointer-index`**
(019 §5.3; K5).

**A4. `current-branch`:** `verify` reads it and **warns** when it is malformed or missing, naming `prikk branch switch
<branch>`. Exit status unchanged (019 §5.8: the external architect's condition for deferring it).

**A5. `verify` and `doctor` agree on a torn object-index tail.**
- **The bug:** `verify` exits 1 with *"found at least one failed object, block, or ref"* while every count is 0;
  `doctor` warns and exits 0.
- **Make `verify` warn and exit 0,** because the index is a pure cache (RFC 162).
- **Its closing error line never names a failure kind that did not occur.**
- **Reproduction:** `.pgtmp/ext019/agree2.sh <binary>`.
- **Control:** a damaged complete index record still fails as now.

**A6. The noted items** (019 §5.4–5.7, 5.10, 5.11):
1. **The N6 state** (an acknowledged record damaged) is no longer also reported as a trailing partial, and `doctor`'s
   first recommendation is `--discard-damaged-commits`, not a repair that then skips.
2. **Row 10's "a copy is the way out" says what to copy:** the repository's `.prikk/`, from a backup taken before the
   damage. Say what happens to the sound record behind the damaged one.
3. **A removed witness reads** *"acknowledged commits: none recorded (the classification of a session written before
   0.49.0 applies)"*.
4. **`--repair-tails --plan-only`,** as the other repairs have. It is the one repair that may cut ten files in one run.
5. **The rebuild:**
   - over a torn pointer-index tail, it refuses and names `--repair-tails`, as the generation-log case does since
     `0db67cc4`;
   - on a pointer index that already matches the log, it writes nothing and says so.
6. **The emptied or removed generation logs (019 §5.7): which slot is read then?** Answer from source, and run it. If a
   wrong slot is read silently, **stop and report**: that is a finding, not text.

**The rebuild's way back (019 §5.4: it saves nothing to the recovery log) is a design question.** Lay out the options
(a replace entry for the retired slot, or a saved copy), with what `--recovery-restore` would then do. **Choose none.**

## Explicit non-change scope

- No format change.
- No change to RFC 165 R4's conditions.
- No new repair verb, except `--repair-tails --plan-only`.
- No JSON schema version change. New `verify` lines are prose; any JSON addition is additive and listed.

## Prohibited shortcuts

- A message naming a command that was not run in that state.
- Weakening a refusal to make a message true.
- Text-matching paths in tests.
- Asserting only that a message appears, rather than that the command it names then succeeds.

## Compatibility and security constraints

- **A1 makes the rebuild refuse more, never drop more.**
- **A4 and A5 change `verify`'s report** (a new warning; an exit status). Each is one `### Output changes` line, and the
  stikk letter's §3 caveat applies.
- **Docs:** grep `docs/src` for each changed message and for every behaviour being replaced (the docs-debt rule), and
  fix each copy.

## Known risks

- **A1:** a lead that fails both (b) and (a) must still drop. Test that combination.
- **A2:** the N3 state and an ordinary lead-free tail must stay distinguishable. A lead-free tail still names
  `--repair-tails`.

## Required evidence and review request

- **For every item:** the state, the message before and after, and the named command run.
- **The controls,** each shown red.
- **The answer to A6.6.**
- **The rebuild way-back options.**
- **`scripts/gates.py`'s summary;** each unit's `date` start and end.
- **Report once:** `.git-exclude/review-request/review-019-corrections-report-v1.md`.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | A1 and A2 | 120 min |
| U2 | A3, A4, A5 | 90 min |
| U3 | A6, including the 6.6 answer and the way-back options | 90 min |

**Interim, seen 2026-10-07** (`67c66606`; report v1). Accepted on reading:
- **A1:** the chain walk ends, because each step's `update_seq` strictly decreases, and a missing or unverifiable link
  drops as before. The control was shown, and the revoked-link case still drops.
- **A2's offset fix:** accepted.
- **The `refs::rebuild_discovery` split:** accepted.

**Continue with A2's remaining sites, A3–A6, and the docs grep.**
- **For A2's generic refusal** (`incomplete_publication_refusal`), name `ref complete` only when the mismatch is a
  genuine N3 lead (one coherent transition, per R4). Every other case keeps its text.
- **Print `date` at each unit's start and end.** It has been asked for every round.
- **Report once, at the end:** `review-019-corrections-report-v2.md`.

## Addendum 1 — 2026-10-07: the rest of step 1, in three parts, each finished in one sitting

**Why:** this handoff was budgeted at about five hours, more than one sitting. Each stop became an interim review
request, which costs the owner a hand-off. **From now on:**
- **A review request is written only when a part below is complete.**
- **If you must stop before a part is complete, write nothing in `.git-exclude/review-request/`.** End your turn with
  *"Continuing — not ready for review"*. The owner then replies "continue" to you, not to the architect.
- **Do the parts in order.** Each part's report is final for that part, and ends with *"From dev team: <path>"*.

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **B** | A2's remaining sites (name `ref complete` only for a genuine N3 lead, one coherent transition per R4; every other case keeps its text), A3, and the docs grep for every message changed so far | 90 min | `review-019-corrections-B-report.md` |
| **C** | A4 (`current-branch` warning) and A5 (`verify`/`doctor` agree on a torn index tail), with their Output-changes lines; **plus Part B's two carried items** (review `review-019-corrections-B-review-v1`): the incomplete-publication refusal becomes a typed error the CLI matches by type, not by text; and `branch create`, `branch close`, `merge` and `sync adopt-tag` go through the same one mapping, each tested by running the command it names | 90 min | `review-019-corrections-C-report.md` |
| **D1** | **Part C's two carried items** (review `review-019-corrections-C-review-v1`): the object-index fallback scan memoized once per store handle, with a scan-counting command test, its control, and a `log` measurement at two sizes; an absent `current-branch` prints an informational line in `verify` | 60 min | `review-019-corrections-D1-report.md` |
| **D** | A6, including the answer to 6.6 and the rebuild way-back options | 90 min | `review-019-corrections-D-report.md` |

**Each report holds:**
- the items with their before/after messages and the named command run;
- the controls, each shown red;
- `scripts/gates.py`'s summary on the part's last commit;
- `date` at the part's start and end.

**Part B ACCEPTED 2026-10-07** (`8c8b4a22`; review `review-019-corrections-B-review-v1`). Two items carried into Part C (in its row above).

**Part C ACCEPTED 2026-10-07** (`199f0fb6`; review `review-019-corrections-C-review-v1`). **Next: Part D1, then Part D.**

**Part D1 ACCEPTED 2026-10-07** (`1c4093c7`; review `review-019-corrections-D1-review-v1`). **Next: Part D (A6).**

**Part D ACCEPTED 2026-10-07** for items 1–5 (`0e64499e`; review `review-019-corrections-D-review-v1`). Item 6 is a real
defect, reproduced by the architect. Two more one-sitting parts follow; the same stop rule applies.

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **E** | **Fail closed on a lost generation log.** (1) Confirm from source that only compaction writes slot B; if not, stop and report. (2) For all three compacting containers (the pointer index, the received index, the trust policy): when the generation log names no slot and slot B holds data, every reader and writer refuses, naming the container and its way out. (3) The pointer index's way out is `--rebuild-pointer-index` from the ref log, without reading either slot as live; test it from this state. The other two say to restore the generation log from a backup. (4) A test per container: the state, the refusal, and, for the pointer index, the rebuild, then `branch list` shows every branch. **Control:** restore the slot-A fallback, and each test goes red. (5) A `### Fixed` entry naming the consequence, and the first affected version from history. (6) The docs grep | 90 min | `review-019-corrections-E-report.md` |
| **F** | **The rebuild's way back, Option B** (the review): before the rebuild flips away from the live slot, save a full copy of it to the recovery log under the rebuild's run. `--recovery-restore <run id>` writes it into the then-retired slot and records a generation pointing at it. Test: rebuild, restore, then the pointer index and generation state are byte-identical to before; control shown red | 60 min | `review-019-corrections-F-report.md` |

**Part E: Corrections Required 2026-10-07** (`c4fec333`; review `review-019-corrections-E-review-v1`). The team was right: a crash before the first generation record is file-identical to a lost log, and "refuse" turns a routine compaction crash into a block. The architect's ruling is corrected.

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **E2** | **Deduce the live slot from content** (the review). (1) Confirm from source how each container's compaction copies entries (byte-identical, or re-encoded: then compare by key and value). (2) When the log names no slot and slot B holds data: every B entry found among A's entries resolves to A; a B entry A never had resolves to B; an unreadable slot refuses. (3) `verify` and `doctor` warn and name `prikk compact --<container>`; `compact` resolves the same way and writes its record. (4) Tests: the crash window (a bare compaction retry heals); the lost log after writes to B (newer entries read, for all three containers); the lost log with no write since; the unreadable slot; and two controls, "always A" and "always refuse". (5) Restore the crash test's original meaning. (6) The `### Fixed` entry and docs follow the deduction | 90 min | `review-019-corrections-E2-report.md` |

**Then Part F** as written.

**Part E2: Corrections Required 2026-10-07** (`d5380986`; review `review-019-corrections-E2-review-v1`).

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **E3** | (1) **The exact deduction rule:** `C = compaction(A)` by `compact`'s own keep-live logic; B's decoded entries equal to `C` or a prefix of it resolve to A; anything else resolves to B; damage refuses. It replaces the membership rule for all three containers. Tests: the trust-policy un-revocation sequence (the review) resolves to B and L is not trusted; the crash window and a partly written B resolve to A. **Control:** restore the membership rule, and the un-revocation test goes red. (2) **`meaning_paths_for` fails closed:** a restore whose meaning file's container is ambiguous refuses, plan and run, naming `prikk compact --<container>`. A test and its control | 60 min | `review-019-corrections-E3-report.md` |

**Then Part F.**

**Part E3: Corrections Required 2026-10-08** (`8e75e613`; review `review-019-corrections-E3-review-v1`). The implementation was right; the architect's rule missed row 4 of the case table: a compaction crash, then ordinary writes to A.

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **E4** | **B is derived from A if B equals `compaction(P)`, or a prefix of it, for some prefix P of A** (resolve to A); otherwise B; damage refuses. Computed in one pass with the running reduction (linear). **Test every row of the review's case table (rows 1–8),** for the pointer index and the trust policy at least (and the received index where it applies), each asserting the live slot and the user-visible result (branches listed; a revoked key untrusted). **Controls:** E3's rule turns row 4 red; E2's membership rule turns row 7 red; "always A" turns row 6 red | 60 min | `review-019-corrections-E4-report.md` |

**Then Part F.**

**Part E4 ACCEPTED 2026-10-08** (`940b88fd`; review `review-019-corrections-E4-review-v1`). **Review 019 item 6 is closed.** Next: **Part F** as written, plus one wording fix: wherever a comment or doc calls the deduction "linear", say O(|A|·|B|) entry comparisons, run only in the ambiguous state.

