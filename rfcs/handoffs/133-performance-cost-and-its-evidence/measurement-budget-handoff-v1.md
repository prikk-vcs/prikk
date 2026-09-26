# A time budget and a watcher for every measurement — handoff v1

**Live 2026-09-26, next after RFC 159's Addendum 2** (the Windows test-path fix; `main` must be green first).
RFC 159's implementation is accepted (review `rfc159-implementation-review-v1`).
**Next after this:** 0.48.0 release prep, which uses what this round delivers.

**Approved by the owner 2026-09-26** (*"Approved. Yes. Write the handoff."*), after the architect's account of why
measurement sessions held the owner's machine for hours across 0.46.0 and 0.47.0. **Measurement tooling only: no
product code.**

## Why — what the records show

1. **The release gate rebuilds a huge repository three times to read one number.** Release prep needs the peak RSS
   of one incremental `commit` at N = 32,000 and 64,000. Each of the three samples of `rfc133_node_count_memory.rs`
   does all of the following (`measure_incremental_point`):
   - it creates a repository and writes N files;
   - it commits and seals them;
   - it runs a whole-worktree `diff`;
   - it makes the measured commit;
   - it runs `tree`, seals again, and runs a two-point `diff`.

   **N = 64,000 alone was 4,770 of 6,008 s** at the 0.47.0 prep. How that splits between the steps is **unknown**,
   because nothing times the steps.
2. **"Generate once, copy per sample"** was noted on 2026-09-23 and never scheduled.
3. **The deep-history rounds rebuild 0.47.0's quadratic history** (~32 min per sample at depth 1,024, ×3) instead of
   reusing kept repositories.
4. **No handoff stated a time budget**, and nothing stopped a unit that ran long. A shared machine and a power-off then
   decided what a session cost.

## 1. See the cost first — one sample, every step timed

Before changing anything, time **each step** of **one** N = 64,000 sample in the release build: tree generation,
baseline commit, baseline seal, worktree diff, measured commit, `tree`, second seal, two-point diff. Report the table.
It is the before-picture, and it tells us which of §2 and §3 matters most.

## 2. Build the setup once, and reuse it

- **For each N, generate the tree and commit and seal the baseline once.** Copy that repository for each sample, and
  make each sample's one-file change from its own seed.
- **Prove the gate figure is unchanged.** In one session, run the old and the new instrument alternately at N = 32,000
  and 64,000, and report the median and spread of the measured commit's peak RSS for both. The acceptance rule:
  **each new median lies within the old run's min–max.** If it does not, stop and report; do not adjust anything to
  fit.

## 3. Trim the release gate to what it gates

The release-gate profile (`rfc133_node_count_memory_release_gate`) measures **only** what release prep gates on:
- the incremental commit's peak RSS;
- the persisted cache file-size control that its report already carries.

The `tree` and `diff` rows and the second seal stay in the **full sweep**, which the template already requires when a
round changes those commands. The profile's report header says which rows it measures.

## 4. The watcher — a budget, fixed in source

A small shared helper for the measurement harnesses. **No environment knobs**, for the same reason as RFC 133's
release-gate profile.
- **Each unit declares its budget in source**: a named constant, set from §1's and §5's measured times, with a stated
  margin.
- **Every step's start, end, elapsed time and the boot id** (`/proc/sys/kernel/random/boot_id`) go into the report as
  they happen. A unit stopped for any reason, power-off included, therefore leaves every finished step on disk.
- **At twice its budget, a unit stops itself**: it writes its partial report with a clear `STOPPED: over budget`
  line, and fails. It does not keep running silently.
- Apply it to the RFC 133 instrument (both entry points) and to `tools/corpus/tests/build_cost_curve.rs`.
- In `build_cost_curve.rs`, a baseline history at a stated commit may be **reused from a kept repository**, when its
  commit and binary sha256 match, instead of being rebuilt.

## 5. Measure the result — and quote only what is measured

Run the new release-gate profile end to end, once, on this machine. Report:
- its total wall time and the per-step table;
- the boot id and the load at start;
- the gate figure (the RSS ratio between the two largest N) against 0.47.0's 1.910×.

**This run's wall time is the only duration the release-prep template may then quote**, as "measured on <date>,
<load>", never as a promise.

## 6. Controls — each shown red

1. **The watcher stops a unit.** Give a test unit a budget smaller than its work: it stops, writes the partial report
   with `STOPPED: over budget`, and fails. **Perturb:** remove the check, and the control goes red.
2. **The partial report survives.** Kill a unit midway (a test-only failpoint), and every step finished before the
   kill is in its report.
3. **The trimmed profile measures only its rows.** Assert on its report's header and row set, not on timing.
4. **Reuse is checked.** A kept repository whose recorded commit or binary does not match is not reused; the unit
   rebuilds and says so.
5. **Copy-per-sample leaves the gate figure unchanged:** §2's comparison, with its numbers in the report.

## 7. What changes after, and whose it is

- **The release-prep template §1.6** gets the measured duration and the trimmed profile. That edit is the architect's,
  from this round's report.
- **Every future measurement handoff states its units and their budgets up front**, so the owner can see a round's
  cost before it starts. The architect writes that into the handoffs; you apply the watcher.
- Moving long units to CI (the owner's question of 2026-09-26) comes **after** this round, on the trimmed and budgeted
  units, and only if what remains is still long.

## 8. Report

`.git-exclude/review-request/measurement-budget-report-v1.md`:
- §1's step table;
- §2's gate-figure comparison;
- §5's measured run;
- each control, and the perturbation that turned it red;
- the gates on the exact final commit;
- anything in this handoff that is not true at source.

## Addendum 1 — 2026-09-26: the failed acceptance rule, ruled

Report: `.git-exclude/review-request/measurement-budget-report-v1.md`. **Stopping was right.** The rule was the architect's,
and it failed at both N. Read as the gate reads it, the failure is not a rounding matter:
- the new median at N = 32,000 is **+0.95 %** above the old one;
- the gate's own number, the 64,000 / 32,000 ratio, moves **−1.1 %** (1.936 → 1.914);
- the last two releases moved it by 1.0–1.4 %, and the release-prep template says *a ratio that moved stops the cut
  until explained*.

A method change of that size cannot be accepted blind. **Option A is refused.**

**Option B, with one hypothesis added, and a like-for-like baseline at the end.**

1. **The suspect the report did not name: timestamps.** A copy that gives every file a fresh mtime may make the
   "incremental" commit re-read or re-hash files it would otherwise skip. It would then be **a different operation**,
   not the same one faster. Add a copy that **preserves each file's timestamps** as an arm of its own.
2. **Arms, alternating, five samples each, at N = 32,000 and 64,000:**
   - (i) fresh build, with the worktree diff (the old form);
   - (ii) fresh build, without it;
   - (iii) copy with fresh mtimes, without it (the new form as delivered);
   - (iv) copy preserving mtimes, without it.

   **Budget: 75 min, stop at 150**, declared in source, as §4 requires.
3. **Decide by the result, using the rule stated here, before the run:** the chosen form's median must lie within
   **±0.5 %** of arm (i)'s median at each N, and the ratio within ±0.5 % of arm (i)'s ratio. **±0.5 % is half the
   smallest release-to-release swing on record.** The chosen form is the cheapest arm that meets the rule. If none
   does, stop and report again.
4. **Then re-baseline, whatever wins.** In one session, alternating, run the chosen trimmed profile on **0.47.0's
   release commit (`21895f46`)** and on **HEAD**. 0.48.0 prep then compares like for like. The old profile's 1.910× is
   never compared with the new profile's figure.
5. **§5's end-to-end run is that re-baseline run.** It is not run separately, so the machine is not loaded twice. Its
   wall time is the one duration the template may quote.

**On the other questions:**
- **§6.a**, the shared helper included by `#[path]` from two crates: **accepted**. It is test harness code, the boundary
  gates pass, and a workspace member would be a larger decision for no gain.
- **§6.f**, `PRIKK_BCC_REUSE_KEPT`: accepted, because a reuse run says in its own report that no curve was measured.
  It never decides a **gate's** scope.
- **The full sweep's 4-hour budget** stays a stated guess until it is next run under the watcher. That run replaces it.

Gates on the exact final commit, then the report, with §2's arm table, the chosen form, and the re-baseline figures.

**ACCEPTED 2026-09-26** (`67b6ee3c`, `8cf2f9f3`, `22242330`, `07280736`, `91ff9516`, `74e7c4ac`; review
`measurement-budget-review-v1`; 14/14 gates re-run by the architect on `74e7c4ac`, controls perturbed by the architect).
The chosen gate form is (iv), a baseline copied preserving timestamps: the commit index trusts size and mtime
(`commit_index.rs:50`), so only (iv) gives the measured commit the reference form's input. The data cannot tell the
four arms apart. **0.47.0 under the trimmed profile: 1.916×** (runs 1.922 and 1.910), measured 672–770 s per run. The
release-prep template carries both. **This handoff is closed.** Next, live:
`rfcs/handoffs/102-container-based-durability/append-length-without-reading-handoff-v1.md` (a defect this review found),
then 0.48.0 release prep.
