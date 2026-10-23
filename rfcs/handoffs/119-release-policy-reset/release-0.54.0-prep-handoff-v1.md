# Release 0.54.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-10-24, and it is next.** 0.54.0's planned work is complete (owner-approved 2026-10-19, theme *"what you
receive, and what it leaves"*):
- **step 1:** a commit reads the queue twice, not three times;
- **step 3, B1:** `verify` reports an object nothing reaches as a warning, not a failure;
- **step 3, A:**
  - `prikk extract`, with `extract-report-v1`;
  - `refusal-v1`;
  - the `show` fix for a second consecutive text edit;
- **step 3, B2a:** `prikk received remove`;
- **Part 6:** the Windows installer, `install.ps1` and `uninstall.ps1`;
- **Part 7:** the AUR packages, `prikk` and `prikk-bin`, pushed by the release workflow;
- **the `cargo binstall` Windows fix,** and the G1 gate work, which is not user-facing.

**The architect's reading, stated so it can be corrected:**
- **This prepares a candidate, not a cut.**
- **The external review is the owner's decision at the candidate.** I recommend offering it, for two reasons:
  - `received remove` is a new writer that changes what `verify` checks;
  - B1 changes `verify`'s trust rule for unreached objects.
- **No tag and no crates.io** without the owner's word.

Follow `release-prep-handoff-template.md`. This file fills in the blanks and splits the work into parts.

| | |
|---|---|
| version | **0.54.0**: a minor |
| theme | **"what you receive, and what it leaves"**: read a point's files without a checkout (`extract`); take a received pointer out on purpose (`received remove`); `verify` stops failing on what nothing reaches; a Windows installer and AUR packages |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.53.0` at `3f72b47f`. The published Linux binary is kept at `/home/nabbisen/.pgtmp/rel-0.53.0-prikk` (`prikk 0.53.0`) |
| memory ratio | against **0.53.0's 1.880×** (release-gate profile, `release-0.53.0-prep-P2-review-v1`) |
| smoke base | `.git-exclude/review-request/smoke-0.53.0.sh` |

## The working rule

- **One part per sitting,** in order, with one final report each. The report lists every item with its status.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **`date` at each part's start and end. Every run inside an R1 scope.** Keep every output the report cites, gate logs
  included (`| tee`). Build before running anything that reports on a commit.
- **If a worktree is used,** paste `git worktree list` after removing it.
- **No `rm -rf` on a variable.**

### The held docs: a rule for every part

`hold/0.54.0-windows-docs` holds six docs-only commits that are not on `main`, touching
`docs/src/guide/install.md` and `docs/landing/index.html`:
- `c87da024` and `92f53924` (the landing page);
- `9d34f257`, `3a39f3c2`, `1a759e0e` and `2c9fea04` (the install guide).

They describe `install.ps1` and the AUR packages, which do not exist until the 0.54.0 release run publishes. So:
- **On `main`, do not edit either file in any part.** Any change to either goes on the held branch: check it out,
  commit, then check `main` out again.
- **The architect cherry-picks the six commits, and any you add, after the release run publishes.** That happens once
  `install.ps1` returns 200 and both AUR pages show 0.54.0. It is a docs-only push after the tag, so the tag still sits
  on the commit CI ran on.

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job, and the Windows suite's time. **1.** `--help` against 0.53.0's binary: every changed description. Expect the new `extract` and `received remove`, and `verify`'s unreached-object line if it is in help. Anything else is named. **2. The `### Security` question: a list to the architect, and no block written yet.** The block today follows up 0.52.0's own Security entry. The architect's expectation: **keep it, but shorten it to the correction alone.** What 0.52.0 said that is no longer the whole picture; what now changes (an unreached object warns, and `received remove` can take the pointer out); and what still holds (the bytes are never deleted). Point at the Output-changes and Added entries rather than re-describing them. Say whether you agree, and whether anything else 0.54.0 changes is a security fix. The architect expects not: the `show` fix is a read path, and binstall is packaging. **3.** The docs currency and the docs-debt grep, on `main`: `commands.md` (`extract`, `received remove`, `refusal-v1`; line 196's "completed 0.54.0 step 3 Part 5d"); `current-state.md` (this release's entries say "0.54.0 step 3": write "0.54.0"); `troubleshooting.md` line 587's "Before this round"; and any guide page that names `verify`'s unreached objects or `show`. Older releases' wording in `current-state.md` is not this release's debt. **4.** The public-API diff from 0.53.0, with `#[non_exhaustive]` on every new or changed public report type and enum: the extract report, the refusal types, and `verify`'s report with its unreached objects. Say which are public outside `test-support`. **5.** `cargo package --list -p prikk`. **6.** The CHANGELOG read through as a user. No process words in user text: "0.54.0 step 3 Part N", "this round", "Part 1b polish", "Part 5c", "handoff v1" and the bare "B1"/"B2a" labels all go. The RFC references stay. **7. On the held branch** (no gate run; rebuild with mdBook, and check the `#verify-the-tag` and `#arch-linux-aur` anchors): the two AUR wording touches from `0.54.0-step-3-part-7b3-review-v1`, and the landing page's platform test tightened from `/win/i` to `/\bWin(dows\|32\|64)\b/` (Part 6b review) | 75 min | `release-0.54.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.880×. **2. The queue reads, on the CLI,** release build, idle machine, 3 samples: at 100 and 800 queued (one fixed file rewritten), one more one-file `commit`, with time and peak RSS on the candidate **and** on 0.53.0's binary. Re-confirm the CHANGELOG's byte figures (107,074 / 157,046 and 808,781 / 1,209,605) by the method that produced them, or correct them. **3. `extract`'s cost,** both binaries where they apply. At 1, 16 and 64 MiB stored, `extract --ref` (the tip and a point 63 blocks back), `extract --claim`, and each with `--plan-only`: time and peak RSS. Check every cost sentence `current-state.md` makes about `extract`, including "`--plan-only` costs the same as a real write", against these numbers. **4.** `smoke-0.54.0.sh`, extending `smoke-0.53.0.sh`. Add:<br>• **`extract`:** by ref, by block id and by claim; `--plan-only` writes nothing; `--allow-full-replay` and its refusal without the flag; a non-empty target refused; `extract-report-v1`'s fields;<br>• **`received remove`:** `--plan-only`; a removal, after which `verify` stops failing; the refusal while something still reaches the objects; an unknown pointer;<br>• **`verify`'s unreached objects:** the count line at 0 and above 0, exit 0, the JSON array;<br>• **`show`:** two and three consecutive text edits of one file, text and JSON;<br>• **`refusal-v1`:** one line per id reachable from the CLI. Name any that is not reachable.<br>Run it on a release build. Then run the new sections on 0.53.0's binary, and name every line that passes there and why. The `show` lines must fail there | 90 min | `release-0.54.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's Security ruling: **1.** the CHANGELOG in RFC 161's shape: Security only as ruled, Upgrading if anything needs it, **one** Output-changes block, Fixed, Changed, Added. **2.** The three-file release commit: version, lock, and the CHANGELOG heading with today's date (re-dated at the tag if needed). **3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh`; and `matrix.py` v6 at `--compactions 1` and `2`, on the release build **and** on 0.53.0's binary, compared by row. **Name every row that differs, and why.** B1 may change rows where an unreached object used to fail `verify`: if so, each is expected and named. **4. The held docs, dry run:** on a scratch branch from the candidate, cherry-pick the held branch's docs commits in order. They must apply cleanly, and the book must build. Then delete the scratch branch, and paste `git branch --list` and `git status`. All outputs kept in `.git-exclude/review-request/release-0.54.0-prep-*` | 75 min | `release-0.54.0-prep-P3-report.md` |

## Not the team's

- pushing;
- the external-review question;
- the tag; re-running any CI job on the tagged commit that was cancelled for runner capacity, until the run is fully
  green;
- verifying the artifact;
- **the release run's two AUR jobs** (`aur-build`, `aur-push`), read by the architect. Then the architect confirms both
  AUR pages show 0.54.0, and reads `paru -G prikk`'s PKGBUILD (`validpgpkeys` and `?signed`);
- **a real install of 0.54.0 from the release page under Windows PowerShell 5.1,** after the tag (Part 6a4 review). The
  method is settled at the cut;
- **`cargo binstall --dry-run … --targets x86_64-pc-windows-msvc prikk@0.54.0`,** after the tag. It confirms the
  binstall fix against the real asset;
- **the held docs' cherry-pick and push,** after the release run publishes (above);
- crates.io;
- **the consumer letters.** The owner sends them after the release:
  - **stikk:** the Windows installer now exists (letter 025 said none was scheduled); the `show` fix; `refusal-v1`; the
    binstall fix;
  - **planeter:** `extract`, their request, measured on the published binary;
  - **the external architect:** the review, if the owner offers it.
