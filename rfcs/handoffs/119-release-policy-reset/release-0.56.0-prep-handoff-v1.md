# Release 0.56.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-11-07, and it is next.** The owner authorized the cut on 2026-11-07 (*"Yes"*). 0.56.0's planned work is
complete (theme *"what a delivery may change, and what it leaves"*, owner-approved 2026-10-29, extended 2026-10-31 and
2026-11-04). Every part's history is in `rfcs/handoffs/155-repository-complete-artifact/0.56.0-step-3-implementation-handoff-v1.md`:
- **step 0:**
  - `log --ref <block id>` refuses instead of answering an empty history;
  - stale locks are named in the lock-conflict error and in `doctor`;
- **step 1:** `sync pending` (`sync-pending-v1`), and `state` plus pending edit paths in `show`;
- **step 3:**
  - **2A:** delivery classification, with an older or diverged delivery refused by default, in both importers;
  - **2B:** `extract --claim` over a chain of pending claims;
  - **W–W3c-5c:** the worktree follows the current branch, and every in-the-way refusal comes before the first write;
  - **F/F2:** a forged maintainer signature on any admitted Block or RefState is refused, and `bundle preview` reports
    each sealer's outcome;
  - **3g:** what "pending" means, and `unclaimed` in `show`;
  - **3h:** unreached patches and blobs are counted;
  - **3i/3i-b:** `archive verify`'s four status strings;
  - **U–U4:** `uninstall.sh --prefix`, and the PowerShell uninstaller's argument guard;
  - **T:** `trust maintainer add` tells the truth for a removed id, and gains `--format json`;
  - **D:** docs corrections (authority limits, related projects, stale locks).

**The architect's reading, stated so it can be corrected:**
- **This prepares a candidate, not a cut.**
- **The external review is planned, at the candidate** (owner-approved plan, 2026-10-29). Two of this release's
  changes are admission rules over untrusted input (2A and F/F2). The architect drafts the letter after P3.
- **No tag and no crates.io** without the owner's word.
- **`prikk unlock`'s behaviour does not change in 0.56.0.** The owner ruled it into 0.57.0 (part L, on the ROADMAP's
  0.57.0 row). This release's docs describe today's behaviour: the liveness line is advice, and `--yes`/`--force`
  clears.

Follow `release-prep-handoff-template.md`. This file fills in the blanks and splits the work into parts.

| | |
|---|---|
| version | **0.56.0**: a minor, because there are runtime changes (v0 policy). P1 item 4 says whether the public API also breaks |
| theme | **"what a delivery may change, and what it leaves"** |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.55.0` on commit `511d6dab`. **The published 0.55.0 binary is no longer on this machine** (`~/.pgtmp` was wiped). Download the Linux asset from the `0.55.0` GitHub Release into a fresh, empty scratch directory, check it against the release's checksum file, and paste both the check and `--version` |
| memory ratio | against **0.55.0's candidate runs, 1.8688× and 1.853×** (release-gate profile, `release-0.55.0-prep-P2-review-v1`) |
| smoke base | `.git-exclude/review-request/smoke-0.55.0.sh` (447 ok on the published 0.55.0 binary) |

## The working rule

- **One part per sitting,** in order, with one final report each. The report lists every item with its status.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **Paste the `date` output when the work starts and when it ends.**
- **Every run inside an R1 scope.**
- **Keep every output the report cites,** gate logs included (`| tee`).
- **Build before running anything that reports on a commit.**
- **Every timing is alternated** (before and after in turn, order reversed each round). State the filesystem.
- **If a worktree is used,** paste `git worktree list` after removing it. **No `rm -rf` on a variable.**
- **`df` the scratch filesystem before the gates.** If it is full, never delete another session's files.

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job by name, the Windows and macOS jobs included.<br>**1.** `--help` against 0.55.0's binary: every changed line, named.<br>**2. The `### Security` question: a list to the architect, and no block written yet.** Measure each candidate **on 0.55.0's published binary** and say what it did. The architect expects two entries:<br>• (a) `bundle import` and `archive import` admitted a Block or RefState whose maintainer signature fails under an adopted key (`verify` reported it only afterwards);<br>• (b) an older or diverged delivery moved what the receiver holds, if 0.55.0 shows that on a received pointer or an adopted branch.<br>Say whether you agree, and whether anything else in this release is reachable by untrusted input (a received bundle or an archive).<br>**3. Docs currency and the docs-debt grep:**<br>• **the amendment fold:** the W3b–W3c-5c, F, 3g–3i and U paragraphs in `commands.md` and `guide/patches/worktree-patch.md` become one present-tense statement each;<br>• no step labels or function names in user text;<br>• **the three items the D report found:** `concurrency-locking.md` "a positive result … is reliable evidence to refuse" becomes "for the operator to refuse"; `trust-threat-model.md` (~line 171) "permanently, until an operator removes it out-of-band" is false now that `trust maintainer remove` exists; `worktree-patch.md:7` "Key material is supplied via the environment" (setup writes the seeds to the key directory, and prikk finds them with no variable);<br>• every message this release changes is quoted correctly in `troubleshooting.md`, `commands.md` and the guide pages.<br>**4. The public-API diff from 0.55.0** (`cargo public-api diff 0.55.0..HEAD`, pasted). Name every change, including `PrikkError::MaintainerKeyIdBound` and every new report or refusal type. Put `#[non_exhaustive]` on each new or changed public enum and report type, or write one doc line saying why not. A break goes in `### Upgrading` with what to call instead.<br>**5.** `cargo package --list -p prikk`.<br>**6. The CHANGELOG read through as a user.** `## Unreleased` grew entry by entry (about 495 lines, about 20 of them carrying "step …", "W3c", "3g" or "design review" labels). Fold it into one story per command:<br>• no step labels, round names or function names;<br>• no intermediate behaviour that never shipped (a fix to an unreleased feature folds into the feature's own entry);<br>• each refusal id said once, with its command;<br>• the `### Output changes` block complete. The consumer letters (planeter, stikk, inklkunn) are drafted from it.<br>**8.** Absence claims for every `### Added` entry | 90 min | `release-0.56.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.8688×/1.853×.<br>**2. Re-confirm every figure the CHANGELOG states on the candidate,** by the method that produced it, alternated against 0.55.0's binary. If the CHANGELOG states none, say so. Then measure the two costs this release added on a release build:<br>• the admission signature check, per 20,000 objects, for `bundle import` and `archive import`;<br>• `archive import --adopt`'s early in-the-way check, at 10,000 and 20,000 objects.<br>State them in the CHANGELOG only if P1's fold asks for them.<br>**3. `smoke-0.56.0.sh`,** extending `smoke-0.55.0.sh`, with a section per user-visible change:<br>• every new `refusal-v1` id reachable from the CLI (`forged-publication-signature`, the delivery-class refusals, `maintainer-key-id-bound-to-removed-key`, `maintainer-key-id-adopted-with-different-key`, and the rest P1 finds). Name any that is not reachable;<br>• each in-the-way refusal (`merge --into`, `branch create --from`, `sync seal`, `archive import --adopt`) leaves `.prikk` and the worktree byte-identical, and the retry after moving the file succeeds;<br>• the worktree after `merge --into` the current branch, then an ordinary `commit`, records no deletion;<br>• `trust-add-v1`; `archive verify`'s four statuses; `sync pending` and `show`'s `unclaimed`; `verify`'s unreached patch and blob count;<br>• `uninstall.sh --prefix` refusing an empty value, a missing value and a disagreement with `PRIKK_INSTALL_DIR`, run against a decoy prefix only.<br>Run it on a release build. Then run the new lines on 0.55.0's binary and name each that passes there, and why | 90 min | `release-0.56.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's rulings on P1 and P2:<br>**1.** The CHANGELOG in RFC 161's shape: Security only as ruled, Upgrading, one Output-changes block, Fixed, Changed, Added.<br>**2.** The three-file release commit: version, lock, and the CHANGELOG heading dated today (re-dated at the tag if needed).<br>**3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh`; and `matrix.py` v6 at `--compactions 1` and `2`, on the release build **and** on 0.55.0's binary, compared by row. **Name every row that differs, and why.** 2A and F refuse deliveries that 0.55.0 admitted, and the in-the-way checks moved refusals before the first write, so a row may now refuse earlier or write less. Each difference must be explained as that, and never as a write 0.55.0 did not make | 75 min | `release-0.56.0-prep-P3-report.md` |

## Not the team's

- pushing;
- **the external-review letter** (the architect drafts it against the candidate; the owner sends it);
- the tag; re-running any CI job on the tagged commit that was cancelled for runner capacity, until the run is fully
  green;
- verifying the artifact;
- **the release run's jobs, which the architect reads:**
  - the AUR jobs, then both AUR pages at 0.56.0;
  - `windows-install-from-release`, all four cells. The uninstaller guard changed this release;
- crates.io;
- **the consumer letters,** which the owner sends after the release:
  - **planeter:** what 0.56.0 changes for a forge, confirming the 2026-10-31 letter against the shipped binary;
  - **stikk:** the new refusal ids and `trust-add-v1`;
  - **inklkunn:** that the Unicode paths implementation and their two asks are scheduled in 0.57.0.
