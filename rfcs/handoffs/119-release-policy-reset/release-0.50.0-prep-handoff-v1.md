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
