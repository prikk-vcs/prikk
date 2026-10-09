# Release 0.50.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-10-08, and it is next.** The owner said *"Proceed"* at the end of 0.50.0's planned work.

**The architect's reading, stated so it can be corrected:**
- **this prepares a candidate, not a cut;**
- **the external re-run is the owner's decision at the candidate.** RFC 165 R5's amendment and the lost-generation-log
  deduction change designs the external architect reviewed, and they offered the emptied-log re-run;
- **no tag and no crates.io** without the owner's word.

Follow `release-prep-handoff-template.md`; this file fills in the blanks, and splits the work into parts.

| | |
|---|---|
| version | **0.50.0**: a minor (in v0 any runtime change is one) |
| theme | **"every message points the right way":** review 019's corrections, the lost-generation-log deduction, the rebuild's way back, and the small disclosed gaps |
| CHANGELOG date | the day the release commit is made; **if that slips past midnight JST, stop and ask** |
| last tag | `0.49.0` at `ab27fc48` (the published binary is kept at `/home/nabbisen/.pgtmp/prikk-0.49.0-ab27fc48`) |
| memory ratio | against **0.49.0's 1.867×** (release-gate profile; report `node-count-memory-measurement-release-gate-621ff60d818f-1791333706.md`) |

## The working rule

- **One part per sitting,** in order, with one final report each.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **`date` at each part's start and end.**
- **Every run inside an R1 scope.**

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep** | **0.** CI on `origin/main`, every job; the Windows job time, read against the last runs (46m26s was a uniform runner slowdown; above 40 min again, say so). **1.** `--help` against 0.49.0 (`--repair-tails [--plan-only]`, the rebuild's way back, every changed description). **2.** **The `### Security` candidates: a list to the architect, in this report, and no block written yet.** **3.** The docs currency and the docs-debt grep for every message 0.50.0 changed; `current-state.md`'s limitations (the generation-log item fixed; M6, M7, F2, F4 unchanged). **4.** The public-API diff from 0.49.0, with `#[non_exhaustive]` on new report types. **5.** `cargo package --list -p prikk`. **6.** The absence claims | 90 min | `release-0.50.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.867×. **2.** `smoke-0.50.0.sh`, extending 0.49.0's: `--repair-tails --plan-only`; a rebuild, then `--recovery-restore` of its run (three files byte-identical); `verify`'s current-branch lines; and the lost-generation-log state (warning, then `compact` clears it). Run it on a release build. **No `rm -rf` on a variable** | 60 min | `release-0.50.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's Security ruling: **1.** the CHANGELOG in RFC 161's shape (Security, Upgrading, one Output-changes block, Fixed, Added). **2.** The three-file release commit, with the date. **3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh` and **`matrix.py` v5** (from receive/019) on the release build **and** on 0.49.0's binary, both outputs kept in `.git-exclude/review-request/` | 60 min | `release-0.50.0-prep-P3-report.md` |

## Not the team's

- pushing;
- the external-review question;
- the tag;
- verifying the artifact;
- RFC 165's move to `done/` at the cut (the architect's records);
- crates.io;
- consumer letters (only if `### Output changes` reaches stikk or planeter).

**P1 ACCEPTED 2026-10-08** (`8c382e08`; review `release-0.50.0-prep-P1-review-v1`). **Security ruling: no `### Security`
block in 0.50.0**; the generation-log fix stays under Fixed (local trigger, as 0.49.0's corrupted-last-record entry) and
the Attestation check under Added. **Carried into P3:** the generation-log heading names its trust consequence; the
`--plan-only` sentence names the verbs that have it (three repair verbs do not); the long `Added` paragraph becomes
bullets; the Output-changes and Upgrading lines listed in the review. **Next: P2.**

**P2 ACCEPTED as run 2026-10-08** (ratio 1.854×; smoke 27/27, re-run by the architect; review
`release-0.50.0-prep-P2-review-v1`). **One part added before P3:**

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P2b: two messages and three smoke checks** | **F1:** `--repair-tails --plan-only` prints `would truncate`, not `truncated`. **F2:** the lost-generation-log warning and `doctor`'s recommendation name the exact `prikk compact --<container>` command; the test runs the command the message names. Smoke 27a (a torn tail), 27b (`verify` after the rebuild), 27d (compact, *then* branch, then empty the log; `branch list`; run the named command). The CHANGELOG quotes the command. Details in the review | 45 min | `release-0.50.0-prep-P2b-report.md` |

**Next: P2b, then P3.**

**P2b ACCEPTED 2026-10-08** (`cb578e87`; smoke re-run by the architect, exit 0; review
`release-0.50.0-prep-P2b-review-v1`). **Carried into P3, beside the P1 review's corrections:** remove the round-name note
from `troubleshooting.md:505`; drop the CHANGELOG's "not by itself a runnable command" parenthetical; add the
`would truncate` Output-changes line. **Next: P3.**

**P3's code ACCEPTED 2026-10-08** (`d88db979`, `7977d566`; review `release-0.50.0-prep-P3-review-v1`). The external matrix's
one unaccounted row (the ref log emptied) is A1 working: 0.49.0's rebuild dropped every published state, 0.50.0 refuses.
**One part before the push:**

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P3b: the release notes** | CHANGELOG only, one commit on top of `7977d566`: the generation-log attribution (019 §5.7 asked; the architect reproduced); quote `` `--ref` `` byte for byte; "before this round" and "RFC 151" become release numbers; the A1 entry names the emptied-ref-log shape; internal round names leave headings and body (RFC numbers stay); the "three corrections along the way" paragraph goes; the JSON paragraph is cut to one sentence. Then the 14 gates on that commit, and `git diff 7977d566 HEAD --stat` shows `CHANGELOG.md` only. Details in the review | 20 min | `release-0.50.0-prep-P3b-report.md` |

**Next: P3b.** The tag goes on P3b's commit.

**P3b ACCEPTED 2026-10-08** (`71fde661`; review `release-0.50.0-prep-P3b-review-v1`; the new A1 sentence was confirmed
by a probe on both binaries). **The dev team's part of 0.50.0 prep is complete.** The architect pushes; the tag waits
for the owner.

**Candidate pushed 2026-10-08** (`8deb0be2`, 14/14). **Before the tag, one more part. The architect found it while
checking 019's grades for the external letter.**

Over a **complete damaged pointer-index record** (019 §5.3, graded 0.50.0; corrections handoff A3), `prikk doctor` on
`8deb0be2` still does not name `--rebuild-pointer-index`:
- **three `PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE` errors** recommend *"preserve the repository and inspect the failing
  stage"*;
- **the current-branch warning** recommends `branch switch`, which is wrong here: the pointer file is fine, and the
  index is what is damaged;
- **the stage errors themselves** say *"run doctor before listing"*, so the user goes round in a circle.

`--repair-tails` names the rebuild; nothing else does. The rebuild is the right way out: its plan is correct, its
real run fixes the index, and `verify` 0 and `doctor` are clean afterwards (the architect's probe on the release build).
A3 said *"`doctor` and the tail repairs"*; Part B fixed the tail repairs, and the architect accepted it without
running `doctor` in that state.

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P3c: every message over a damaged container points to its way out** | **1. Facts from source first:** every text that says *"run doctor before …"* (`compact.rs`, `refs.rs:736`, `verify.rs:803/818`); what `doctor` then prints for each of the three containers (the pointer index, the received index, the trust policy); why `verify`'s pointer-index tail line reads *"unknown (stage did not evaluate)"* here; and what typed signal exists for the damage. **2. The pointer index:** `doctor` raises one issue naming `prikk doctor --rebuild-pointer-index --plan-only`, then the rebuild. Stage errors caused by that damage point to it, not to *"inspect"*. The current-branch warning does not recommend `branch switch` when the cause is the damaged index. The texts that sent the user to `doctor` name the rebuild directly. **Decided by a typed or structural signal, never by matching text.** The stale comment at `doctor.rs:1142` goes (the rebuild exists now). **3. The received index and the trust policy:** if a way out exists, name it; if none does, say what to copy, as row 10 does. Never *"run doctor"* when `doctor` answers nothing. **4. Tests that follow the message:** damage a complete record, then `doctor`'s recommendation, then that exact command, then `verify` 0. Controls: a stage failure from another cause still says *"inspect"*; each routing site, removed, goes red. **5.** Smoke 27e (the same flow on the release build). **6. The CHANGELOG:** the entry, Output-changes lines, and one correction to the A1 entry: *"independently found"* → *"found by review 019 (§2.2, §4.4)"*. **7.** The docs-debt grep; the 14 gates; `matrix.py` v5 on the release build (no new finding, every changed row accounted for) | 75 min | `release-0.50.0-prep-P3c-report.md` |

**If item 1 shows that a fix needs a format change, a new command, or a change to a recovery rule, stop and report it
as a question.**

**Next: P3c.** The tag goes on P3c's pushed commit.

**P3c approved in part 2026-10-08** (`b0185230`; review `release-0.50.0-prep-P3c-review-v1`).

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P3d: four findings from the P3c review** | **G1:** while a container's interior damage is set, every `VERIFY-STAGE-INCOMPLETE` recommendation says to resolve that damage issue first. Decided by the typed field; *"inspect"* stays otherwise. **G2:** the container-damage issues are printed first. **G3:** every *"no repair exists"* text says to restore **`.prikk/` as a whole**, from a backup taken before the damage, never single files; the trust-policy text adds *"then re-apply every trust change made since that backup"*. State from source whether `pointer_rebuild.rs:413` is reachable. **G4:** exact commands, no *"the message names the way out:"* prefix. Tests with a control per branch; smoke 27e; CHANGELOG; docs grep; the 14 gates. **The report lists every item with its status** | 40 min | `release-0.50.0-prep-P3d-report.md` |

**Next: P3d.**

**P3d ACCEPTED 2026-10-09** (`b3b1c891`; review `release-0.50.0-prep-P3d-review-v1`). **The dev team's part is complete.**
The architect pushes. The external re-run follows (the owner's *"Proceed"* of 2026-10-08, read as yes to the re-run).
The CHANGELOG date is the owner's question: the commit slipped past midnight, and the tag waits for the review.

**Candidate `b345a864` pushed 2026-10-09** (14/14; CI `37858112537`, 16/16, Windows 37m53s). The external letter 020 is
drafted and ready for the owner to send (`.git-exclude/upstream/external-architect/send/draft/020-…`). It was run on
this build and on 0.49.0: matrix 282/289 clean (10 better, none worse); `reproduce.sh` unchanged apart from N3's text.
**The tag waits for the external answer and the owner's word.** The CHANGELOG date is set to the tag day in the last
commit before the tag (owner to confirm).

**External review 020 (2026-10-09): one finding blocks the tag.** The lost-generation-log rule is one-directional
(020 §3.1; assessment `external-review-020-assessment-v1`). It is fixed in
`rfcs/handoffs/165-ref-publication-one-read-a-log-that-speaks-and-a-way-out/generation-log-both-directions-handoff-v1.md`
(Q1, Q2). The external architect re-runs on the commit that follows.

**The release day, the owner, 2026-10-09:** *"The actual day to release it."* The architect's reading: the CHANGELOG
heading is set to the day of the tag, in the last commit before it.

**Candidate `fdbead68` pushed 2026-10-09** (14/14; CI `37911865712`, 16/16). It carries the generation-log fix
(handoff 165 Q1–Q2b). Letter 021 is ready for the owner to send. The matrix gives 289/289 the same as 020's run of
`b345a864`; `reproduce.sh` has the same outcomes. **The tag waits for the external re-run and the owner's word.** The
heading's date is set to the tag day in the last commit before it.

**External review 021 (2026-10-09): nothing blocks the tag** (assessment `external-review-021-assessment-v1`).

| part | items | budget | report |
|---|---|---:|---|
| **P4: the date commit** | `CHANGELOG.md` only: `## 0.50.0 — 2026-10-08` → `## 0.50.0 — <today>`, with an em dash (U+2014). **Run `date` first; if it is past midnight JST, use the new day.** One-line message `Release 0.50.0: date the release`. `scripts/gates.py`, all 14, on that commit; `git show --stat` shows `CHANGELOG.md`, one line changed. **Do not push or tag** | 15 min | `release-0.50.0-prep-P4-report.md` |

**After P4,** on the owner's authorization: the architect pushes, reads CI, and tags the pushed commit (signed;
message `prikk 0.50.0`, a blank line, and the Release-notes link). Then the artifacts are verified, RFC 165 moves to
`done/`, crates.io is published on the owner's word, and the consumer letters are written.

**The owner, 2026-10-09: *"Authorized."*** The architect's reading: the cut of 0.50.0 is authorized, meaning P4's date
commit, then push, CI and the signed tag on the pushed commit. **crates.io and the consumer letters are not
included;** each still needs its own word. **Next: P4.**

**P4 ACCEPTED, and 0.50.0 RELEASED 2026-10-09.** The tag `0.50.0` is signed, on `7551c3aa` (CI `37936285139`, 16/16;
release run `37940038344`, green). The architect verified the assets: checksums 6/6, build info 4/4, smoke 297/297 on
the published Linux binary, and the notes byte-identical to the CHANGELOG section. **Still owed:** crates.io (the
owner's word), note 022, RFC 165 to `done/`, and the consumer letters.

**crates.io: PUBLISHED 2026-10-09** (the owner: *"Yes."*). All eight crates at 0.50.0, in dependency order from the tag's
worktree, each confirmed on the crates.io index API. **Note 022:** the owner sends it. **The consumer letters:** the owner
sends them, possibly after 0.51.0.
