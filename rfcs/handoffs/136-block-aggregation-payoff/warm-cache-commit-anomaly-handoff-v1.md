# The warm-cache `commit` anomaly — measurement handoff v1

**Live 2026-09-17.** 0.45.0, measure-only. **Do this first among the 0.45.0 items:** the answer decides whether 0.47.0
(RFC 136 increment 2c) is a performance round or a cache-defect fix that moves ahead of 0.46.0 (ROADMAP schedule row).
**No product code changes in this round.**

## 1. What is known

The RFC 136 increment 3 report (`.git-exclude/review-request/rfc136-increment-3-report-v1.md`, §1.3) measured `commit`
with a warm DC-64 lifecycle cache on RFC 139's corpus:

| depth | 32 | 64 | 128 | 192 | 256 |
|---|---:|---:|---:|---:|---:|
| median ms | 11 | 2,141 | 5,690 | 50 | 13,581 |

- The ranges within each cell were tight, and the before and after binaries agreed at every depth.
- The instrument copies `.prikk`, cache included, and materializes the worktree before timing. So the cache was
  present in every cell.
- Two depths behave as if the cache was used and three as if it was cold. **Nothing so far says what decides
  that.** The cold-cache column reads ~1.35 as a growth exponent, which is §9.3.2's second chain walk.

## 2. The question

**At each depth, which path does `commit` take to its baseline, and why?** Is the variation:
- a **cache defect**: a cache that should certify is rejected, rebuilt, or bypassed;
- a **legitimate miss**: the cache's key or certificate does not cover this tip by design;
- an **instrument artifact**: something the copy or materialize step changes?

## 3. Method

1. **Reuse the increment 3 instrument** (`tools/corpus/…`, `#[ignore]`d). Same corpus, same five depths, at least
   three interleaved samples per cell. Build every binary in its own worktree and target directory. Never compare a
   shared `target/debug` build with a worktree build.
2. **Name the path taken, per cell.** Add test-only instrumentation, gated like existing test seams and absent from
   release builds. It records which rung `lifecycle_cache` takes (`cache_ladder.rs`, `incremental`, `replay`) and,
   on a miss, the **exact reason**: which certification check failed (`CacheCertificationError` variant or
   equivalent) or why it was skipped. **Timing alone does not answer §2.**
3. **Separate the instrument from the subject.** For at least one fast depth and one slow depth, also run `commit`
   in the corpus repository itself, without the copy and materialize steps, and say whether the path agrees.
4. **Check the obvious structural candidates** and report each as confirmed or ruled out, with evidence:
   - the checkpoint cadence (a ref's first block and every 64th);
   - what the last command before timing wrote to the cache, and at which tip;
   - whether the copy preserves every file the certificate compares, such as mtimes or inode-derived fields;
   - whether depth parity with the corpus's branch layout matters.

## 4. Stop and report

`.git-exclude/review-request/warm-cache-commit-anomaly-report-v1.md`:
- a table per depth: median, range, rung taken, and the miss reason;
- the cause, **classified as one of §2's three**, with the code site;
- if it is a defect, **the smallest correct fix described, not implemented**, plus the control that would fail
  without it;
- anything else the run surfaced, reported and held.

**Stop there.** No product code; the architect rules on the classification before any fix is handed off.
