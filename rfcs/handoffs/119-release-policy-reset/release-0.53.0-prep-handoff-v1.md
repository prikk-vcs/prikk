# Release 0.53.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-10-17, and it is next.** 0.53.0's planned work is complete (owner-approved 2026-10-15, theme *"a commit
costs what it changes"*):
- **step 1:**
  - `trust maintainer check`'s not-trusted exit travels through `run()`;
  - `gates_test.py` runs in CI;
  - `checkout`'s refusing modes on a never-sealed branch say what is true;
- **step 3A (F4):** a cold `commit` derives its baseline from the nearest trusted checkpoint (RFC 159 stack B), reading
  about 8 KB at any store size, with the owner's F4-2 exception;
- **step 3B (M6):** a commit reads the queue once fewer, keeping the full check.

**The architect's reading, stated so it can be corrected:**
- **this prepares a candidate, not a cut;**
- **the external review is the owner's decision at the candidate.** I recommend offering it. F4 changes where a
  commit's baseline state comes from, kinds included, and that state reaches the next signed root. F4-2 gives up one
  refusal;
- **no tag and no crates.io** without the owner's word.

Follow `release-prep-handoff-template.md`. This file fills in the blanks and splits the work into parts.

| | |
|---|---|
| version | **0.53.0**: a minor |
| theme | **"a commit costs what it changes"**: a cold commit no longer reads every stored file (F4); one queue read fewer per commit (M6); with step 1's three small items |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.52.0` at `6e5337d0`. The published Linux binary is kept at `/home/nabbisen/.pgtmp/rel-0.52.0-prikk` (`prikk 0.52.0`) |
| memory ratio | against **0.52.0's 1.891×** (release-gate profile, `release-0.52.0-prep-P2-review-v1`) |

## The working rule

- **One part per sitting,** in order, with one final report each. The report lists every item with its status.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **`date` at each part's start and end. Every run inside an R1 scope.** Keep every output the report cites, gate logs
  included (`| tee`). Build before running anything that reports on a commit.
- **If a worktree is used,** paste `git worktree list` after removing it.
- **No `rm -rf` on a variable.**

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job, and the Windows suite's time. **1.** `--help` against 0.52.0's binary: every changed description. Expect `trust maintainer check`'s exit status and the `checkout` modes; anything else is named. **2. The `### Security` question: a list to the architect, and no block written yet.** Does any 0.52.0 behaviour that 0.53.0 changes amount to a security fix? The architect's expectation is none. F4-2 is a refusal given up, not a fix, and stays under `### Output changes`. Say so, or name what you find. **3.** The docs currency and the docs-debt grep: `current-state.md` (the F4 and M6 entries, and DC-64's *"every 64 uses"* line), `durability-recovery.md` (the cold commit and the queue), `trust-threat-model.md` (whether F4-2 or the anchored baseline needs a sentence: checkpoint kinds are authenticated by the signed root), and `guide/` pages naming a commit's cost. **4.** The public-API diff from 0.52.0, with `#[non_exhaustive]` on every new or changed public report type and enum (`BaselineCacheRung` gained `Anchored` and lost `FullReplayHorizonMismatch`: say whether it is public outside `test-support`). **5.** `cargo package --list -p prikk`. **6.** The CHANGELOG read through as a user: the F4 `### Changed` entry, the M6 entry, and the F4-2 `### Output changes` bullet. No process words ("round", "step", "R-1") in user text; the RFC references stay | 75 min | `release-0.53.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.891×. **2. The cold commit, on the CLI,** release build, idle machine, 3 samples. At 1, 16 and 64 MiB stored, after one `seal` and with `.prikk/cache/` deleted, one one-file `commit`: wall time and peak RSS, on the candidate **and** on 0.52.0's binary. Expect the candidate flat and 0.52.0 following the store. **3. The queued commit, on the CLI:** at 100, 800 and 3,200 queued (one fixed file rewritten, `PRIKK_ACTIVE_PATCH_LIMIT` raised), one more `commit`: time, both binaries. **4.** `smoke-0.53.0.sh`, extending `smoke-0.52.0.sh`. Add: **F4-2 on the binary:** an untouched blob damaged below a checkpoint means a cold `commit` succeeds and `verify` reports it; the same damage with the commit editing that file refuses; the same damage with the checkpoint's signer no longer adopted refuses. **M6:** a damaged interior queued record makes `commit` refuse, naming `doctor` and `--discard-damaged-commits`. **Step 1:** `trust maintainer check` exits 0, 1 and 2 (with stderr empty on 1); `checkout --patch-plan` on a never-sealed branch with a queued commit says it has no sealed block yet, and a branch never created still says "does not exist". Run it on a release build, and run the new sections on 0.52.0's binary: name every line that passes there and why | 90 min | `release-0.53.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's Security ruling: **1.** the CHANGELOG in RFC 161's shape: Security only if ruled, Upgrading if anything needs it, **one** Output-changes block, Fixed, Changed, Added. **2.** The three-file release commit: version, lock, and the CHANGELOG heading with today's date (re-dated at the tag if needed). **3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh`; and `matrix.py` v6 (`receive/025-…/reproduce/`) at `--compactions 1` and `2`, on the release build **and** on 0.52.0's binary, compared by row. **Name every row that differs, and why.** 0.53.0 should change none: say so if true. All outputs kept in `.git-exclude/review-request/release-0.53.0-prep-*` | 75 min | `release-0.53.0-prep-P3-report.md` |

## Not the team's

- pushing;
- the external-review question;
- the tag;
- verifying the artifact;
- crates.io;
- the consumer letters. The owner sends them after the release:
  - stikk: the `checkout` wording (already announced), `trust maintainer check`, and a faster cold commit;
  - planeter and brygge: the cold commit, if relevant to their measurements.
