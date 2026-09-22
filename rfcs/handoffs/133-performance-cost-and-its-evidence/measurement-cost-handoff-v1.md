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

