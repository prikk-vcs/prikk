# The RFC 133 instrument costs a release its afternoon — handoff v1

**Live 2026-09-22, and it is next.** Approved by the owner 2026-09-22 (*"Approved."*, to the self-reviewed
design; *"Proceed on the next."* for issuing it now). **Test-harness only: no runtime source, no schema, no
docs claim about the product.**

**Next after this, in order: RFC 158 Stage A** (the import/verify size bound), then RFC 136 increment 2c, then
the gate plan. Those handoffs go live when they are next, not before — this one said "Live" while claiming not
to be next once already, and the round ahead of it was done out of order as a result.

## Why

The release-prep template §1.6 says to run the memory instrument *"at two repository sizes"*. **That cannot be
done** — `rfc133_node_count_memory.rs` has no such knob:

```rust
const NODE_COUNTS: [usize; 7] = [100, 1_000, 4_000, 8_000, 16_000, 32_000, 64_000];
const SAMPLES_PER_POINT: usize = 3;
```

So the only way to obey the instruction is the full sweep: seven sizes × three samples × every series, including
the genesis series the report itself calls *"secondary — for continuity, not the in-scope property"*. Your own
diff-round report timed that at **77 minutes**, and it has gained two columns since. **The gate is one number** —
the incremental-commit peak-memory ratio against the previous release — and two points give it. **The
architect wrote the instruction that cannot be followed; this is not a process you chose.**

## 1. A second entry point, not a knob

**Reviewed and changed before issue** (`.git-exclude/reviewed/measurement-and-gate-plan-self-review-v1.md` §B):
environment variables were the wrong mechanism — a stale export silently shrinks a release gate and leaves no
trace in the command the report quotes.

Add `rfc133_node_count_memory_release_gate`, `#[ignore]`d beside the existing driver and sharing all of its
code, with its scope **fixed in the source**:

- the **incremental** series only (the gate is the incremental-commit peak-RSS ratio; genesis is secondary by
  the report's own words, and the `tree`/`diff` columns are context);
- node counts **100, 32,000, 64,000** — one small point because a *constant* overhead shows up there as a large
  ratio (50 MB on a 12 MB baseline is 5x at N=100 and 1.4x at N=64,000), and the two largest because a
  *per-node* overhead shows up there. The small point costs almost nothing;
- `SAMPLES_PER_POINT` unchanged at 3.

Its report says, in the header, that it is the release-gate profile and which series and sizes it ran, so a
reader can never mistake it for the full sweep. Same output directory, same `…-<revision>.md` shape.

**The existing driver is untouched**, so every report already on disk stays reproducible by the command that
made it.

## 2. The controls

- the release-gate entry point runs exactly the series and sizes named above, asserted on its report's header
  and row count, **not** on timing;
- the full driver's defaults are pinned by a test, so a later edit that changes them has to say so;
- the two entry points share one implementation — *perturb the sharing*: give the release-gate profile its own
  copy of the measurement step and the control must go red.

These are ordinary `#[test]`s over the profile and the header. **Do not run the instrument to test the
instrument.**

## 2a. What the round must measure, and not inherit

The architect's first estimate for this saving (~80 minutes to ~10) is **withdrawn**: the cost is dominated by
the incremental block's per-sample setup at the two largest sizes, so the honest expectation is **roughly half**.
**Measure both, once each, on the same idle machine, and report the two numbers** — that measurement is the
point of the round.

Recorded for you to **measure, not assume**: generating the repository once per size and copying it per sample
instead of regenerating. It trades content variation across samples for wall time; whether that is acceptable
is a measurement question. Report what you find; do not adopt it silently.

## 3. The scheduling note, in the template

Also update `rfcs/handoffs/119-release-policy-reset/release-prep-handoff-template.md` §1.6:

- give the **exact command** for the prep gate (the release-gate entry point of §1), so nobody re-derives it;
- **nothing is conditional at a release**: the release commit gets the full, unconditional gate set whatever its
  diff touches, and a round that adds a replay-shaped reader gets the **full** memory sweep, because that round
  measures the shape and shape needs every point;
- say plainly: **start it first, and do the reading and grepping steps while it runs** — the CHANGELOG pass, the
  docs currency pass, the absence-claims grep. **Do not start a build or a gate run alongside it**: the peak-RSS
  figures are taken from fresh child processes and survive load, but `/tmp` is a 30 GB tmpfs that a concurrent
  gate run fills. In the 0.46.0 prep the release commit was already made and everything then waited on one
  process.
- keep the full sweep where it belongs: a round that adds a replay-shaped reader (a `tree` row, the two `diff`
  rows) measures the **shape**, and shape needs every point.

## 4. Report

`.git-exclude/review-request/rfc133-measurement-cost-report-v1.md`. State the prep-gate command's wall time
against the full sweep's, measured once each on the same machine — that number is the point of the round.


## Addendum 1 2026-09-23 — the code is accepted; the wall-clock figure is refused, and costs less to get right

**Accepted** (review `rfc133-measurement-cost-review-v1`): `cad7abfc`, `423a8a62`. Gates 14/14 re-run by the
architect on the exact final commit, 2,265 / 0 / 31 per toolchain, all three controls green by name. The shared
`measure_incremental_point`, the source-fixed `RELEASE_GATE_NODE_COUNTS`, the self-naming report header and the
hand-perturbed sharing control are all the shape asked for. **The RSS agrees with the full sweep at every
point** (11,572 / 63,836 / 123,508 against 11,840 / 63,852 / 123,240) — which is the half that had to be
established, and it is.

### 1. The wall-clock number cannot be true, and that was the place to stop

- full sweep at `d6d21df0`, `--exact --test-threads=1`: **4,504 s ≈ 75 min**, 7 genesis points **and** 7
  incremental points;
- release-gate profile: **5,260 s** and **5,616 s**, 3 incremental points and no genesis.

The profile's work is a **strict subset** — the architect read both loops and `measure_incremental_point` to
confirm it, and the RSS table says the per-point work is unchanged. **A subset cannot cost more than its
superset on one machine.** The figure measures the machine's state during those runs, not the profile.

The report's *"trimming five of seven points did not trim the wall time by anything close to five-sevenths"*
reads an impossibility as a result. **When arithmetic and a measurement disagree, the measurement is the
suspect** — the same discipline that made you rebuild the queued-patch control that could not fail.

**Part of the cause is the architect's:** full 14-gate sets were running on the same machine through that
period. From here, **the measurement round owns the machine while it runs** and the architect does not gate
during it.

### 2. Ruled, instead of a third 90-minute attempt (your §5 question)

1. **Both entry points print per-point elapsed time**, and both reports carry an elapsed column. Two lines.
2. Then **one run of each** gives comparable per-point costs, a machine-drift check (the same N must agree
   across the two runs), and **partial information that survives a reboot**, because each point reports as it
   finishes.
3. **Until that exists, the template carries no cost claim.** Replace the "~88-94 minutes" paragraph with one
   sentence: the cost is being measured; "~10 min" and "roughly half" are both withdrawn; the number is recorded
   when a clean run produces it. **A wrong number in the template is worse than none** — the next release reads
   it as the budget.
4. **Give the report filename a run-distinguishing suffix.** Your two clean runs overwrote each other, so run
   1's evidence is gone.
5. **Measure a committed revision.** Both runs were taken on a dirty tree; the filename's `-dirty` is the
   instrument doing its job, but a figure the release template cites must be re-derivable.

### 3. Then

**RFC 158 Stage A is next and is not blocked by this** — this is instrumentation; Stage A closes a measured
out-of-memory hole against untrusted input. Do this round's items, then Stage A's handoff goes live.

**Report:** `.git-exclude/review-request/rfc133-measurement-cost-follow-up-report-v1.md`.

## Addendum 2 2026-09-23 — accepted; one explanation corrected; two small controls owed

**Accepted** (review `rfc133-measurement-cost-follow-up-review-v1`): `ef120c58`, `f10d2a89`. Gates 14/14 re-run by
the architect on the exact final commit, including both cross-target clippy runs, 2,265 / 0 / 31 per toolchain.
The arithmetic holds: the profile (5,345.5 s) is a strict subset of the sweep (9,633.1 s), about **55 %** within
one session; the points that carry the cost agree within 5 % and 0.2 % across the two runs, and RSS within noise.
You ran nothing else while they ran and named what you could not stop — that is the honest report of a shared
machine.

**One explanation is corrected, and it decides what the template may say.** The report calls the old 75 minutes
*"stale, from before the sweep gained its tree/diff columns."* **It was not:** the `d6d21df0` report already has
the same five sections as today's. **The same sweep took 4,504 s on 2026-09-22 and 9,633 s on 2026-09-23 —
2.14× for identical work.** Absolute wall time on this machine moves about 2× with load; ratios and per-point
agreement within one session do not. **So the template records no absolute minutes** — as it now correctly says —
and may record the **ratio**, with the instruction to compare runs by their per-point elapsed column.

**Owed, fold into the next round you open (no design needed):**

1. **A control for the run-distinguishing filename** — two `RunRevision` captures at different `started_at` give
   different `file_name()`s, and the suffix is present. It guards the defect that actually lost a run's evidence.
   *Perturb: drop the suffix — red.*
2. **`assert_eq!(rss.len(), elapsed.len())` at the top of both render functions**, and the rendering control
   asserts the `elapsed (s)` header. The rows are joined with `zip`, which silently drops a row if a future caller
   passes one elapsed value too few.

**Still open, not pressed:** the original §2a's *generate once per size, copy per sample* measurement. With
N = 64,000 at 78 % of the profile, it is the one lever left on the prep gate's cost.

**Next: RFC 158 Stage A** — its handoff goes live when the architect publishes it, not before.

## Addendum 3 2026-09-23 — the handoff is closed

**Accepted** (review `rfc133-measurement-cost-controls-review-v1`): `9bcdc956`. Gates 14/14 re-run by the
architect, 2,266 / 0 / 31 per toolchain; the filename control reddens when the `started_at` suffix is dropped.
Both Addendum 2 items are done and nothing else changed. **This handoff is closed.** The *generate once per size,
copy per sample* measurement stays open as a note, not a task.

**RFC 158 Stage A is next; its handoff goes live when the architect publishes it.**
