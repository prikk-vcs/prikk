# Release 0.49.0 preparation, as a candidate — the dev team's part

**Live 2026-10-07, and it is next. The owner authorized the prep:** *"Proceed to the 0.49.0 release prep."*

**The architect's reading, stated so it can be corrected:**
- this round prepares a **candidate**, not a cut. The tag follows the candidate's gates, CI and matrix;
- **the external review of the candidate** (K6, recommended in the 0.49.0 plan) is **the owner's decision**, asked
  once the candidate is green;
- **0.49.0 is not called "stable"**, as 0.48.0 was not;
- **crates.io is not authorized.** It needs the owner's own word at publication time;
- **RFC 169 is parked** (owner, 2026-10-07). Its factual verification docs stay. Its lost/compromised paragraph is
  removed here (§1.3).

Follow `release-prep-handoff-template.md`; this file fills in the blanks and adds the items carried into this prep by
0.49.0's rounds.

| | |
|---|---|
| version | **0.49.0**: a minor (in v0 any runtime change is one) |
| theme | **"a way out of every crash state"**: every appended file ends positively, with a way out of each tail; ref publication in one read; a queued commit has a witness; resynchronisation is linear; a way back from every repair; repository state no longer depends on a Windows rename |
| CHANGELOG date | the day the release commit is prepared; if it slips past midnight JST, stop and ask |
| last tag | `0.48.0` at `5e50a661` |
| memory ratio (§1.6) | against **0.48.0's 1.869×** under the trimmed release-gate profile (review `release-0.48.0-prep-review-v1`) |

## 0. Before the sweep

- **CI on `origin/main`, job by job;** the macOS and Windows mutation suites by name. The last pushed commit is
  `7001f61d`.
- **Every run in this round goes through R1's cgroup scope** (RFC 160 §9), measurements included. `scripts/gates.py` is
  the gate runner.

## 1. Readiness sweep: the template's eight steps, with these specifics

1. **`--help` (§1.1).**
   - **New since 0.48.0** (diff `commands.rs` against `0.48.0` for the full list):
     - `doctor --repair-tails`;
     - `doctor --discard-damaged-commits [--plan-only]`;
     - `doctor --restore-queue-target --ref <ref> [--not-current-branch] [--plan-only]`;
     - `doctor --recovery-list`, `--recovery-restore <run id> [--plan-only]` and `--recovery-clear [--plan-only]`;
     - each `--plan-only` it accepts.
   - Every description must still be true.
2. **CHANGELOG (§1.2), in RFC 161's shape, first in the section:**
   - **`### Security`:** list every fix since 0.48.0 that is reachable by untrusted input, and bring the list to the
     architect for a ruling **before** writing the block. **Candidates:**
     - M5: resynchronisation was quadratic on hostile content. 0.48.0's notes said "known, not fixed";
     - `bundle import` now refuses a bundle whose ref chain or required attestations are not carried;
     - `verify` checks a received tip's previous state;
     - the read budgets charging headers.
   - **`### Upgrading`** must include:
     - a repository from 0.48.0 gets `recovery/log` and the session witness at its first write;
     - older `recovery/*.bytes` files are listed, never restored or deleted;
     - a session whose `ref-name` was torn by 0.20.0–0.48.0 and the commands that restore it (RFC 166);
     - the refusal messages that changed;
     - any new `#[non_exhaustive]` types.
   - **`### Output changes`:** one consolidated list. It includes:
     - repair output now names a recovery run;
     - `verify`'s recovery-log line;
     - the refusals that name the marker's target;
     - the compact live-slot refusal and the rollback-draft output (RFC 164 carry);
     - every changed `verify` line.
   - **One spelling for a breaking change:** `### Changed — breaking once: …`.
3. **Docs (§1.3), plus the carries:**
   - **The current-state limitations** (`docs/src/reference/current-state.md`):
     - **move to fixed, each with its RFC:** M4, M5, F1, M8, N3, and the torn `ref-name`;
     - **keep, with figures:** F2, F4, M6, M7, and the index lookup (AUD-01);
     - **add, as disclosed gaps:**
       - `Attestation.target_block_id` has no existence check;
       - the LinuxDurability conformance suite runs on Linux only;
       - the snapshot before-stat gate runs on Linux only;
       - three framed formats are not fuzzed;
       - RFC 168's Windows residuals (a), (b) and (c), by name;
       - a stranded `ref-name` is refused by writers, but `verify` does not report it as an item (scheduled for
         0.50.0).
   - **The stale `active.lock`** left by a sync failure at its own creation: `troubleshooting.md:221` already gives `prikk
     unlock`. Confirm that the refusal names that route (run it once, as a probe); if not, report it. **No product
     change.**
   - **Two product text fixes, each its own small commit, with a test:**
     - **RFC 166:** the `--not-current-branch` plan says the uncertainty sentence, *"prikk cannot tell which branch
       these commits were made on; you chose <ref>, not your current branch <current>."*;
     - **RFC 167:** `verify` does not print *"unreferenced remnants: 0"* beside a remnant warning. The count line
       agrees with the warning.
   - **RFC 164 carry:** the generation-log refusal advises `--repair-tails`, and `troubleshooting.md` has its entry.
     Check both; fix whichever is missing.
   - **RFC 169 parked:** remove `SECURITY.md`'s *"If the release key is lost or compromised"* paragraph. Keep the
     fingerprint, the `VALIDSIG` check and the coverage sentences.
   - **D4 counts:** `platform-support.md`'s per-platform test counts are read from the CI logs of the candidate's run.
     The architect fills them in after the push if you cannot.
4. **Public API (§1.4):** the root-export and struct-shape diff from `0.48.0`. Every addition is named, and every new
   report type is `#[non_exhaustive]`.
5. **`cargo package --list -p prikk` (§1.5).**
6. **Memory ratio (§1.6)**, the release-gate profile, against 0.48.0's 1.869×. A move outside the profile's spread
   stops the cut until it is explained.
7. **Smoke script (§1.7)**, covering the new commands on real damaged fixtures:
   - each repair, then `--recovery-list`, then `--recovery-restore <run> --plan-only`, then the restore, then the repair
     again, then a commit;
   - `--discard-damaged-commits`, undone by its run;
   - `--restore-queue-target`;
   - `--recovery-clear`.
8. **Absence claims (§1.8).**

## 2. The candidate

- **The release commit is exactly as the template says:** the version and the internal pins, the lockfile's member
  versions, and the CHANGELOG heading. Three files.
- **Before it is proposed:**
  - the 14 gates on it, in R1's scope;
  - the RFC 162 matrix green;
  - the external `reproduce.sh` (letter 014's) run against its release build **and** 0.48.0's
    (`/home/nabbisen/.pgtmp/prikk-0.48.0-5e50a661`), both outputs attached. **M5 must now be linear.**

## 3. Not the team's

- pushing;
- the external-review question to the owner;
- the tag (signed, Release-page link only, RFC 161 §4.3);
- verifying the artifact;
- moving delivered RFCs from `accepted/` to `done/` (the architect's records);
- crates.io, on the owner's word;
- the consumer letters (stikk, planeter; from `### Output changes`; brygge waits by the owner's ruling).

## 4. Units and budgets

| unit | what | budget (stop at ×2) |
|---|---|---:|
| P1 | the release-gate memory profile (template §1.6), started first | 20 min |
| P2 | the sweep, §1.1–§1.5 and §1.7–§1.8, while P1 runs | 120 min |
| P3 | the two product text fixes and the docs | 60 min |
| P4 | the candidate: release commit, gates, matrix, `reproduce.sh` on both | 30 min |

**Report:** `.git-exclude/review-request/release-0.49.0-prep-report-v1.md`, with `date` at each unit's start and end.
**The Security list comes to the architect before the block is written:** one review request, then continue.

## Addendum 1 — 2026-10-07: the documentation debt, cleared before the candidate

**The owner's rule:** *incomplete content stays in RFCs, except plans and declarations of intent.* An independent audit
(`.git-exclude/reviewed/docs-audit-2026-10-07-v1.md`) found **47 wrong or stale statements** in `docs/src`,
`SECURITY.md` and the CHANGELOG, plus five product-text items. Many predate this cycle. The architect spot-checked the
worst, and each was confirmed. This addendum supersedes §1.3's narrower list where they overlap.

1. **Every row of the audit, 1–47, is fixed or answered.**
   - **Check each against source before changing it.** The audit is a lead, not an authority.
   - **If a row turns out to be right as written, say why** in the report.
   - **Where a page restates a behaviour another page owns, link to the owner page** rather than restate it. Fewer
     copies is less future debt.
2. **The product-text items P1–P5,** each its own small commit with a test where it is a message:
   - **P1:** `refs.rs:342` names `--repair-tails`;
   - **P2:** `doctor --help` lists the three `--recovery-*` commands, and `--repair-tails`'s ten files;
   - **P3:** `verify`'s remnant warning uses the corrected wording;
   - **P4:** the CHANGELOG entries;
   - **P5:** the stale source comments.
3. **The three items the audit could not verify:** M6's figure (measure or cite it, else say "unmeasured"),
   `--recovery-clear`'s output, and `--rebuild-pointer-index` over a damaged generation log. Run each.
4. **A guard, so that row 36 cannot come back silently:** a check in `prikk-release-policy`, run by the existing gates,
   that every repository path in `docs/src` resolves. That covers link targets to `crates/…` or `tools/…`, and
   `github.com/prikk-vcs/prikk/blob/main/<path>` URLs.
   - **Control:** rename one linked file in a scratch tree, and the check goes red.
   - It is a gate change, so state it in the report.
5. **Prohibited:**
   - a sentence describing parked or unaccepted work as fact;
   - "planned for 0.49.0" left on anything that did not ship (say the release it moved to, from `ROADMAP.md`);
   - restating a fix in prose without checking the source.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| D1 | items 1 and 3, the docs rows and the unverified items | 180 min |
| D2 | item 2, the product text | 45 min |
| D3 | item 4, the guard and its control | 45 min |

**Report:** fold into `release-0.49.0-prep-report-v1.md`, one row per audit row (fixed / answered, with the reason).
**The candidate follows these units,** so that the release commit sits on docs that are true.

**Record note, 2026-10-07:**
- **Commit `6c9bed36`'s message** (*"clear the documentation debt …"*) names Addendum 1's **scope, as an instruction**.
  It only added this text; none of that work was done in it. Where work is done is in the commits that do it. (Raised
  by the development team.)
- **Interim, seen:** `acb1b980` (P1) and `0033a7d2` (P2).
  - **P2 still needs its own test,** as item 2 asks for a message: `doctor --help` names the three `--recovery-*`
    commands, and `--repair-tails`'s line names the ref log.
  - **Make `--recovery-clear`'s help line say the removal is permanent** (*"Remove every saved entry, permanently"*),
    because it is the one destructive recovery command.

**Interim, seen 2026-10-07:** P3 `3bafeca3`, P4 `e1932e82`, P5 `4e5c4bdf`. One condition on row 14, both copies (the
CHANGELOG and `troubleshooting.md`):
- **"Run `prikk doctor` for diagnosis" is only an answer if `doctor` then names a route.**
- **Run `doctor` on a WAL whose damage exhausts the budget** (RFC 167's shape A or B), and quote what it recommends.
- **If it names an actionable command, say that command.** If none exists, say so plainly and list it in current-state's
  limitations. A user must never be sent to a command that only describes the problem.

**Working rule for the rest of this round (2026-10-07): no interim check-ins.** Each one costs the owner a hand-off.
**Report once, in `release-0.49.0-prep-report-v1.md`, when the round is done or when you are blocked.** Interim seen:
`b298457b` (the size pin follows P5's line count; fine). **Row 14's condition above (`fbdc1ace`) still applies to the
troubleshooting copy.**
