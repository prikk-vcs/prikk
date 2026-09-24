# Every timing figure again, in the build people run — handoff v1

**Live 2026-09-24, and it is next.** RFC 136 increment 2c is closed (review
`.git-exclude/reviewed/rfc136-2c-addendum-2-review-v1.md`). **Measurement and docs only: no product code.**

**Next after this:** the gate plan (held). It is not live until it is next.

## Why

Every corpus-driven timing through 2026-09-24 was a debug build, because `locate_prikk_binary` ran
`cargo build -p prikk` without `--release`. This was found in the 2c design round (review
`rfc136-2c-design-round-review-v1` §1.2) and fixed in `2926e84c`: the instrument now builds release, fixed in source,
and proves it from Cargo's `opt_level`.

- **The gap:** on the 256-block corpus, a cold full replay is 14.0 s in debug and 0.47 s in release.
- **The shapes moved too:** cold `commit`'s exponent is 1.18 in debug and 0.96 in release.

So **the public scale page** (`docs/src/reference/current-state.md`, "What scale to expect") is corrected with a
dated note but not yet re-measured. And **brygge** (letter 004, held by the owner) is waiting for a release figure
for `seal`, which decides how long a deep import takes.

## 1. Re-measure, in release

1. **RFC 139's build-cost curve** (`tools/corpus/tests/build_cost_curve.rs`): `commit` + `seal` per block, at its
   own checkpoints (`[32, 64, 128, 256, 512, 1024, 2048]`), planned to the RFC 139 §6 floor of 2,048. The debug run
   stopped at 128.
   - **Go as deep as the time allows, and stop by rule, not by feel:**
     - build to 256 first, and report its per-block `seal` time;
     - continue to each next checkpoint while the build **to** that checkpoint is projected, from the per-block
       times already measured, to finish within **2 hours**;
     - stop at the first checkpoint that would not, and say which one it was.
   - **Three samples at every depth you reach**, where the original had one: the sealing exponent was "the
     least-supported figure".
   - **Report per-seal cost against depth separately from cumulative cost**, fit both exponents, and say the depth
     range each rests on.
2. **The two-measurements ladder** (`two_measurements.rs`): checkout and merge-evidence at the five depths (32, 64,
   128, 192, 256). Now that 2c has landed, report `merge-evidence` twice:
   - with sides that carry no edits (the baseline replay only);
   - with sides that do.
3. **The 2c cells, repeated on a machine you can keep as quiet as possible** (`two_c_warm_cells`,
   `two_c_repeat_cells`): release only. This gives the single-figure numbers the CHANGELOG currently quotes as
   ranges.
4. **One debug column** beside the release column for items 1 and 2, from the same session. It is the bridge that
   lets the figures already on record be read. Debug is secondary everywhere.

## 2. Rules

- Ratios within one session only. Absolute time on this machine moves about 2× with load.
- **Every report states its binary:** release or debug, the commit, and the sha256.
- Peak RSS by `getrusage(RUSAGE_CHILDREN)`. `build_cost_curve.rs` samples `VmHWM` at 500 µs; if it reports memory,
  say which method produced each figure. Timing is the point of this round, not memory.
- Load is reported at the start of each timed step, with what you could not stop.
- **The architect runs nothing on this machine while you measure.**

## 3. Docs

Rewrite the scale section of `docs/src/reference/current-state.md` from the release numbers.

- **Each figure** carries the version, the build (release) and the depth range it rests on.
- **Drop the 2026-09-24 correction note** once every figure it names is replaced. If one is not, keep the note for
  that figure alone.
- **No projection past the deepest measured depth** without the word "projected" and the depth it rests on. **If the
  measured shape stays near-linear, say so, and say the old "quadratic" was a debug artefact.** If it does not, say
  that.
- The CHANGELOG entries for 2c keep their ranges unless item 3 gives a single figure; then use the figure, and name
  the build.

## 4. What the report states

- each table, release and debug, with its binary's commit and sha256;
- how deep the build curve reached, and the checkpoint the 2-hour rule stopped at;
- the fitted exponents, and the depth range each rests on;
- the load, and what you could not stop;
- **one paragraph the architect can send to brygge**: the release per-seal cost against depth, and the cumulative
  build time at the deepest depth reached, **measured, not projected**, with the depth stated. The architect writes
  the letter; you write the facts.

## 5. Out of scope

Any product change. `seal`'s lineage walk is theme 17's, designed together with the split walk (2c option (i)).
Changing `CHECKPOINT_CADENCE`.

**ACCEPTED 2026-09-25** (`bf25960e`, `e459ef7c`, `bb9dfbe1`, `3a627af2`; review `release-remeasurement-review-v1`; 14/14
gates re-run by the architect, and a release `seal` reproduced independently at 0.52–0.55 s for an ordinary block at
depth 258). **This handoff is closed.** The expectation above that release might be near-linear was the architect's,
and the measurement refuted it: `seal` is still roughly quadratic in release from about depth 128 on.
