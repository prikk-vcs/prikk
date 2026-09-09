# RFC 133 — commit memory against node count: establish the shape

**Owner instruction 2026-09-09:** the node-count memory cost *"should be recorded and started to be in
control now."* Recorded in **RFC 133 §6b**; this handoff is the first step of §6b.3.

**This round measures. It optimises nothing.** §6b.3 puts measurement first deliberately: the candidate
optimisation (removing `seen_ids`) is provably safe but may not be what the data points at, and
optimising against an unestablished shape is how this project got the claim it is now re-checking.

## 1. The question

**Does incremental commit memory grow with the number of nodes, and in what shape?**

RFC 133 §2 reports a genesis series against file count — 100 → 14.5 MiB, 1,000 → 14.2, 4,000 → 14.0,
**8,000 → 21.7** — captioned *"Flat. Cost follows bytes, not paths."* **§6a.3 found the fourth point
contradicts that caption**: +55% for 2x paths while content grew 1 MiB. Three flat points and one
departure, with the series ending at the departure.

## 2. REQUIRED — measure the *incremental* path, not only genesis

**§2's count series is genesis, and genesis is out of scope.** The owner ruled on 2026-07-30 that
`NFR-PERF-01` bounds **steady-state** commit cost, not the first commit. So the in-scope property —
the one §6a.1 says is true, free and worth holding — is **incremental** commit, and it has never been
measured against node count at all.

Measure **incremental commit memory against node count**: a repository already committed with N nodes,
then change **one** file and commit. Vary N. Hold per-file size constant (256 bytes, as §2 did) so the
byte axis stays still while the count axis moves.

Include genesis at the same N values as a secondary series, for continuity with §2 and because it is the
series whose caption is in question. **Report them separately; do not merge them into one table.**

## 3. Method — comparability is the whole point

**Follow §2.1's method exactly**, or the numbers cannot be placed beside the ones already recorded:
release build, Linux, worktrees under `/tmp` (tmpfs), peak RSS from
`getrusage(RUSAGE_CHILDREN).ru_maxrss`, each measurement in a **fresh process** so the running maximum
cannot contaminate the next. Record the binary's identity and the revision measured at.

**Requirements this round adds:**

- **Extend past 8,000** — at minimum 16,000 and 32,000; further if the curve is still moving and the
  build time allows. The departure begins where the old series stops, so the old range cannot answer it.
- **Re-take the existing points** (100 / 1,000 / 4,000 / 8,000). They are one sample each and one of them
  is load-bearing.
- **At least 3 samples per point.** Report the spread, not only a central value. A single number per point
  is what produced the reading now in doubt.
- If a point is too expensive to reach, **report the wall and the cost that stopped you** rather than
  extrapolating past it.

## 4. REQUIRED — one control that separates cause from correlation

Peak RSS measures the whole process. **Also measure the persisted incremental-cache file size at each N**
(`lifecycle_cache`'s ladder file). That is a **direct** measurement of materialized `NodeLifecycleState`
size, free of allocator behaviour, page granularity and RSS noise.

If cache-file size grows with N on the same shape as RSS, the state structure is implicated. If RSS
departs while the cache file does not, **the cause is somewhere else and the `seen_ids` hypothesis is
wrong** — which is exactly what this round exists to find out before anyone acts on it.

**Do not conclude a cause from the two series agreeing.** Agreement is consistent with the hypothesis; it
is not proof of it. Report what you measured and what it is consistent with.

## 5. What this round must NOT do

- **Do not remove `seen_ids`** or change any state structure. That is §6b.3 step 2 and depends on this.
- **Do not add a gate or a threshold.** RFC 126 §6 objects to hand-maintained numbers, and nothing should
  be gated before the shape is known.
- **Do not touch `MILESTONES.md`**, and do not re-open `NFR-PERF-01`'s recorded-unmet status — that is the
  owner's on evidence and this round is not that evidence.
- No optimisation of any kind, however obvious it looks while you are in there. Report it instead.

## 6. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 against your final commit. If the harness is
`#[ignore]`d like `dc59`/`dc92` — the right shape for an instrument — say so and give the exact command
to run it.

Report to `.git-exclude/review-request/`. Lead with the two tables and the spread. State plainly:
**does incremental commit memory grow with node count, from what N, and on what shape** — and if the
answer is "it does not, within the range reached", say that, because it would retire §6a.3's concern
rather than confirm it. Either result is a good result; only an unclear one is not.
