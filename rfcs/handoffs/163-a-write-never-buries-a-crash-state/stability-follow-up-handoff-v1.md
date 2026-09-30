# 0.49.0 step 0 follow-up — writes in dependency order, the disclosure, and the genesis measurement

**Live 2026-09-30, and it is next.** Step 0 (`stability-confirmation-handoff-v1.md`) is accepted as a measurement: review
`.git-exclude/reviewed/stability-confirmation-review-v1.md`. It found a crash state with no way out, and this round
closes it. **Fixes and one measurement only. No new classification:** a way out that needs no bundle is RFC 164's.

## 1. The defect, reproduced by the architect

A `bundle import` killed partway leaves a block durable while the patch or blob it names is not. `verify` then fails for
good ("object … (block) … references missing …", or "snapshot of Block … names Blob …", "state-root failed"). None of the
three repairs clears it; a commit is still accepted.
- 89 of 300 kills on the shipped 0.48.0, and 136 of 300 on 0.47.0, with an 80-file bundle.
- **Cause, confirmed by source and probe:** objects are written in the bundle's own order (`admit_carried_signatures`
  keeps it; `bundle.rs:896` writes it), not in dependency order.
- **Re-running the same import heals it:** 25 of 25 repositories went to `verify` 0.

## 2. The fix

1. **Every writer that lays down several objects in one command writes them in dependency order**: an object only after
   every object it references is durable.
   - **Derive the order from the object model and write it down:** blob → patch → block → ref-state → tag, or what the
     model says. It goes in the module doc of the one place that sorts.
   - **Classify every such writer from source**: `bundle import`, `sync accept`, and any other (seal, merge, rollback,
     format upgrade). Give its order today, and whether an interruption can leave a dangling reference. Fix those that
     can. Prove each "cannot" with a kill probe or a failpoint row, not an argument.
2. **Tests:**
   - **white-box:** extend `003fe7b4`'s scaffold (`test_gates/rfc163_stability_soak.rs`) to `bundle import` and
     `sync accept`: a failpoint at every write ordinal, seeded and randomized. Assert that `verify` reports no dangling
     reference or state-root failure caused by the interrupted write. **Control:** restore the bundle order; the rows go
     red;
   - **black-box:** re-run your SIGKILL soak for `bundle import` and `sync accept`, N = 1,500 each, with a fixture of at
     least 80 files (step 0's was too small to show the rate). **0 dangling** is the bar;
   - the architect's `import_kill_dangling_probe.py` (`/home/nabbisen/.pgtmp/arch-seal/`), 300 kills on a release build
     of the final commit: **0 dangling**.
3. **Text:**
   - `current-state.md`, known limitations: in 0.48.0 and earlier, a `bundle import` interrupted by a crash can leave
     `verify` failing, while commits still work. **Re-running the same import clears it.** Fixed in 0.49.0, so an
     interrupted import leaves only complete objects. A way out without the bundle is planned (RFC 164);
   - `troubleshooting.md`: one entry keyed on both messages, saying exactly that;
   - CHANGELOG `## Unreleased` → `### Fixed`, with affected versions (0.47.0 reproduced by the architect; find the first
     release from history).

## 3. The genesis measurement (step 0's finding 1)

Genesis peak RSS was 10–14% higher at N = 8,000, 32,000 and 64,000 than the last full sweep, from one sample each.
- **Three samples** at N = 32,000 and 64,000, on release builds of `f10d2a89` and `5e50a661`, alternating, nothing else
  running.
- **If the difference holds beyond the spread**, bisect the range, and name the commit and why. If it does not hold,
  say so, with the spread.
- Measure only; no fix in this round.

## 4. Gates, units, report

- **The 14 gates on the final commit**, in R1's scope. `003fe7b4` lands with this round.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | the white-box import and accept sweep | 30 min |
| U2 | the black-box soak, 2 × 1,500 | 90 min |
| U3 | the genesis measurement, and the bisection if needed | 60 min |

**Nothing else runs while U3 does.**

**Report:** `.git-exclude/review-request/stability-follow-up-report-v1.md`.
