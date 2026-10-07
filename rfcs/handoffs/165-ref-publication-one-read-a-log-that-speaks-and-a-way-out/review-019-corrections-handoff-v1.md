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
