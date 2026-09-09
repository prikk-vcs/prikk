# RFC 133 §6d.3 — re-measure the resident index with the masking defeated

**Ruled:** `rfcs/proposed/133-performance-cost-and-its-evidence.md` **§6d.3**. Follows `6aa67c32`, which
is **accepted as an instrument and an honest report** — this does not reopen it. **Small round.**

## 1. Why a re-measurement, in one paragraph

The probe's numbers below N=64,000 are not measurements of the index. **A structure decoded wholesale
into a `Vec` cannot occupy less RAM than its own serialized form**, and against the attribution round's
file sizes the probe reported **0.42x / 0.35x / 0.12x** at N=8,000 / 16,000 / 32,000. `IndexEntry` is
>=88 bytes (`ObjectId` 32 + `container_checksum` 32 + `offset` 8 + `length` 8 + type/slot), so 32,003
entries need **>=2,750 KiB** against **500 KiB** measured — 0.18x of a hard floor.

The worker's **11,380 KiB baseline** carries enough already-resident heap that a multi-megabyte
allocation reuses touched pages without raising peak RSS. Only at N=64,000 (1.37x the floor, credible)
does the allocation clear the slack.

**The conclusion is not in dispute** — at the credible point the index is 8.6% of the residual, not
dominant. **The evidence is**: it rests on one point, not seven.

## 2. What to change — either fix is sufficient, both are cheap

**(a) Shrink the floor.** 11,380 KiB is a large baseline for a worker whose entire job is to open one
index. Strip the probe worker to the minimum: resolve the layout, `ObjectWriteSession::open`, hold it,
report. No tree generation, no `support` helpers, nothing else linked into that process if it can be
avoided. A floor small enough that a 4 MiB structure clears it is the goal.

**(b) Extend the ladder.** 64,000 already clears the slack; add **128,000 and 256,000** so there is a real
slope inside the credible region rather than a single point.

**Do both if the cost is acceptable** — (a) makes the small-N points meaningful, (b) makes the large-N
trend real. If only one fits, **(a) is the more valuable**: it is the one that rescues the range.

## 3. REQUIRED — the sanity check becomes a standing control

**Assert, at every measured point, that resident cost >= the serialized size of the structure being
measured.** Fail the run when it does not hold. §6d.3 rules this a standing control for every future
memory probe in this project, not a one-off for this round.

Applied to the last run it would have failed four points automatically and pointed straight at the cause.
It costs one comparison against a number the instrument already has.

Report which points pass and which fail under the new method. **If points still fail after (a) and (b),
say so** — that means the masking has another source and is a finding in its own right.

## 4. What this round must NOT do

- **No production code change.** Not `IndexSnapshot`, not `IndexEntry`, not their visibility.
- **Do not execute AUD-01** or build its completion-condition map (§6d.2 still stands).
- **No `seen_ids` removal**, no optimisation, no gate, no threshold.
- **Do not re-run the other two rounds' series.** Their conclusions are not in question; only this
  probe's range is.
- `MILESTONES.md` untouched; `NFR-PERF-01` status untouched.

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 against your final commit, plus the
instrument's own deliberate run. Keep it in the same file as the other drivers, `#[ignore]`d.

Report to `.git-exclude/review-request/`. **Lead with the resident-vs-serialized ratio at every point** —
that is the number that says whether the measurement is trustworthy — then the share of the residual.
Units as computed, as last round correctly did.
