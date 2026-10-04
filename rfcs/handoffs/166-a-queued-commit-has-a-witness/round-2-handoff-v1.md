# RFC 166 round 2 — the ways out: `--discard-damaged-commits` and `--restore-queue-target` (D5)

**Live 2026-10-04, and it is next.** Round 1 is ACCEPTED and pushed (review
`.git-exclude/reviewed/rfc166-round-1-review-v1.md`).
- **Read RFC 166 §4 D5, §5, and §13 items 10, 14 and 15.** K1–K7 bind every item.
- **This round adds two writers that can remove acknowledged commits or attach a queue to a branch.** Every item
  names the K-rule it serves, and an item without its tests is not done.

**The rules carried from round 1:**
- A cut is a question before delivery, at ×2 of a unit's budget.
- Every site gets its own control, and a control must be able to fail.
- The release build from the final commit, sha256 stated; anything timed on `/home`.
- **Every command a refusal names is run by a test,** and it does what the refusal says.
- **For 0.48.0, use `/home/nabbisen/.pgtmp/prikk-0.48.0-5e50a661`,** not `prikk-1c0d5b18`.

## 0. FIRST: the refusals say what the classification says (review v1, required fix)

1. **`commit`, `rollback-draft` and `seal` refuse with the classification's own text** for rows 4–7, 9 and 10, before
   any older tail or damage check speaks. Over acknowledged damage, no writer advises `--repair-wal-tail`.
2. **The tail advice stays for row 3 only** (a real crash tail), where `--repair-wal-tail` works.
3. **"invalid name:"** leaves the WAL-tail refusal; use the error kind the condition is.
4. **The retry text:** "a previous commit was interrupted", as `status` says.
5. **Test:** for each of those rows, `status`'s warning, `commit`'s refusal, `seal`'s and `rollback-draft`'s come from
   one source and name the same way out. **Control:** restore the older check's order, and the test goes red.

## 1. `prikk doctor --discard-damaged-commits [--plan-only]` (D5, §13 item 14; K1, K2, K5)

1. **For rows 4, 5 and 7 only.** It refuses, writing nothing:
   - when nothing acknowledged is damaged or lost;
   - for rows 6 and 10 (a substituted record), whose way out is a copy;
   - when anything else in the session cannot be evaluated (K2).
2. **K1, the plan,** printed by `--plan-only` and printed first by a real run:
   - each acknowledged record it would remove or declare lost: its seq, and the Patch id where the witness holds it;
   - the recovery file the bytes go to;
   - whether the working tree still holds uncommitted changes. Say that the content "may still be in your working
     tree" only when `status` shows uncommitted changes.
   - **Test:** `--plan-only` leaves the tree byte-identical, and its plan equals the real run's.
3. **The write:**
   - the removed bytes are saved exactly as `--repair-wal-tail` saves them, all-or-nothing;
   - then the WAL is truncated to the sound prefix;
   - then the witness is rewritten to cover exactly that prefix, its running hash recomputed.
4. **Round 1's refusals now name it** for rows 4, 5 and 7 (item 0's single source).

## 2. `prikk doctor --restore-queue-target --ref <ref> [--plan-only]` (D5, §13 items 10, 15; K1, K2, C2)

1. **For row 9 (no owner) and D6's mismatch.** **The ref comes from the user, never from the witness** (C2). `doctor`
   may show the witness's ref as a hint.
2. **It refuses, writing nothing:**
   - when a witness exists and names a different ref;
   - **when the queue does not validate against `<ref>`'s current tip by the same check `seal` makes, run without
     writing.** First show from source that such a check exists, or can be called without writing. **If it cannot,
     stop and ask:** that is a design question, not a cut;
   - when anything cannot be evaluated (K2).
3. **K1, the plan:** the ref, the queue's records, the check's result, and **every ref whose tip the queue validates
   against, when there is more than one** (§13 item 10), said before writing. **Test:** plan equality, as in §1.
4. **The write:** `ref-name`, by atomic replace.

## 3. K3, K4, K5

1. **K3:** each refusal condition of both verbs has a constructed case that fails only that condition. Each refuses
   and writes nothing, and each has a control that removes the condition and turns the test red.
2. **K4:** failpoints at every write ordinal of both verbs, each crash state classified and finished by a second run.
   Both verbs raced against `commit` and `seal` under the lock harness.
3. **K5:** no command calls either verb. `verify`, `status`, and every other `doctor` mode leave the WAL, the witness
   and `ref-name` byte-identical over each state.

## 4. Text, probes, report

- **Text:**
  - `current-state.md`: the N6 entry rewritten (closed, with its way out);
  - `troubleshooting.md`: each entry names its verb, with the binary's real lines;
  - `commands.md`, `--help` (the help-inventory gate) and `durability-recovery.md`: both verbs, their plans, and their
    refusals;
  - CHANGELOG `### Added` (both verbs), and `### Output changes`.
- **Probes,** on the release build:
  - the kill probe, 300 kills: 0 stuck;
  - `round1_probe.sh` and `n6_refusal_text.sh` from `/home/nabbisen/.pgtmp/arch-166/`: `commit` and `seal` now name
    the discard verb;
  - **`matrix.py`** against the pre-round binary (`prikk-83a42498`): the round's cells explained.
    - The two "cut short" WAL cells stay `I3` in the unmodified `matrix.py`, because its repair list is fixed.
    - **Run a local copy with `--discard-damaged-commits` added to its repair list:** both cells `ok`. Report both
      runs.
- No cost measurement: neither verb is on a hot path.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U0 | §0: one source for the refusals, the "invalid name" kind, the retry text | 45 min |
| U1 | §1: `--discard-damaged-commits`, K1 | 90 min |
| U2 | §2: `--restore-queue-target`, K1, the `seal` check | 90 min |
| U3 | §3: K3, K4, K5 | 90 min |
| U4 | §4: text and probes | 60 min |

**Report:** `.git-exclude/review-request/rfc166-round-2-report-v1.md`, with each unit's real start and end. All 14
gates on the final commit.

**K7:** a second addendum on this round, or any finding that would change D5, goes back to the owner before the work
continues.

## Addendum 1 — 2026-10-05: the restore verb's branch rule, and its text (review `rfc166-round-2-review-v1`)

Report v1: **not accepted, on one item.** §0 and `--discard-damaged-commits` are accepted. The owner has ruled on
`--restore-queue-target` (K7): **read RFC 166 §14, all of it.** It replaces D5's "same check `seal` makes", which does
not exist. **This is a fix round:** fixes only, nothing else.

1. **The rule (§14 items 1–4):**
   - **with a witness:** `<ref>` equals the witness's ref, unchanged;
   - **without a witness:** `<ref>` is the current branch, by the same resolver `commit` uses for its default, unless
     `--not-current-branch` is given. An unresolvable current branch requires the flag;
   - remove `tip_matches` and `block_patch_ids_match` (§13 item 10's list);
   - a second restore over an owned queue still refuses.
2. **The text (§14 items 6–10), quoted from the binary in the report:**
   - the refusal from `commit`, `seal`, `rollback-draft` and `status` names the concrete command, with the current
     branch filled in and `--plan-only` first, and uses no internal words;
   - **the plan shows each queued commit's message and paths,** and the branch's latest sealed commit they go on top
     of. No block hash as the primary content;
   - the plan's one sentence of uncertainty, including the `--ref` case;
   - the run ends with the next step (`prikk seal --allow-no-audit`);
   - the `--not-current-branch` refusal names both branches and the `--ref` case, and never says just "add the flag".
3. **The discard plan's working-tree note** (review v1, carried): it did not print when the discarded commit's file
   was still in the working tree. Make it work and test it, with a control.
4. **Tests:**
   - each rule condition refusing and writing nothing, with a control that turns it red;
   - **the wrong branch:** the architect's `/home/nabbisen/.pgtmp/arch-166/restore_wrong_ref_probe.sh` (against
     `/home/nabbisen/.pgtmp/prikk-0.48.0-5e50a661`). Restoring to `heads/other` or `heads/same` must refuse without
     the flag, and with the flag the plan names both branches;
   - `restore_probe.sh`: both real stranded queues are still restored to the current branch, then `seal` succeeds;
   - an unresolvable current branch refuses without the flag.
5. **Text:** `troubleshooting.md`, `commands.md`, `--help` and CHANGELOG `### Output changes` carry the new flag and
   lines. State the residual (§14) once, in `durability-recovery.md`.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| A1 | items 1, 3 and 4 | 60 min |
| A2 | items 2 and 5 | 45 min |

**Report:** `.git-exclude/review-request/rfc166-round-2-report-v2.md`, with all 14 gates on the final commit and each
unit's real start and end. **K7:** a second addendum goes back to the owner.
