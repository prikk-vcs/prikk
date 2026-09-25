# Current State

What prikk can do today, what it cannot do yet, and the limits worth knowing before you rely on it.
**This page describes deferrals — things not built yet.** Permanent refusals, which are a different
thing, are in [Non-Goals](non-goals.md).

## What works today

The local core can initialize a repository, author signed patches, seal them into blocks, inspect
history, verify integrity, diagnose common repository issues, perform safe checkout planning and
materialization for the supported subset, display merge evidence and merge plans for explicit sealed
candidates, and **execute a merge** when the two sides are proven confluent — refusing cleanly, with no
object, WAL, or ref write, when they are not.

**Cross-platform history identity is tested, not assumed.** Prikk authors, commits, and checks out on Linux, macOS, and Windows, and CI requires a repository authored on Linux, mutated on Windows, and verified back on Linux to produce byte-identical object ids — so the claim that anyone can verify anyone's history holds across the three.

Known limits worth stating up front: merge-base discovery is manual; conflicts are detected and refused
but never resolved; sync exists between repositories, but **prikk does not move the bytes itself** —
confidentiality is the user's channel's property, not prikk's — negotiation is branch-scoped (tags
travel and are adopted separately, under the receiver's own key), and there is no discovery or
remote-tracking; `verify` cost is linear in history length; `verify` checks author signatures
repository-wide, but only as trust-on-first-use continuity — it proves the same author signed as last
time, not who that author is on first contact; and `verify` checks a locally-published tag's
maintainer signature against this repository's own trust policy, but a received, not-yet-adopted tag
is deliberately exempt — its signature is the sender's, under a key this repository has not adopted.

Next increment candidates are tracked in `ROADMAP.md`.

## Not a good fit yet

Prikk is not yet the right tool if you need:

- a production replacement for Git;
- stable repository-format compatibility;
- Git object compatibility or transparent Git interoperability;
- hosted forge workflows, or remotes;
- complete branch management, or semantic merge;
- plugin/audit execution, attestations, or automated publication controls;
- mature key lifecycle features such as revocation, rotation, hardware signing, or thresholds;
- flexible exclusion of generated files — `.prikkignore` (since 0.29.0) takes literal repo-relative
  path prefixes, one per line, with no globbing, no negation, and no per-directory files, so
  patterns like `*.log` do not work; and a file swept into history by mistake still cannot be
  removed later.

## What scale to expect

The list above is about missing features. Scale is a separate question, and worth stating on its
own: what happens as a repository's history gets deep, or its tracked file count gets large. Every
figure below was measured with a **release build** (the build you install) on Linux, on the
development state that shipped as 0.47.0 (a build that reported 0.46.0, plus the changes the CHANGELOG lists under 0.47.0), on
the RFC 139 corpus profile `prikk-self` (a real project's shape; 155 tracked files at depth 256). Each figure names the depth range it rests on. Depth is the number
of sealed blocks.

- **Sealing cost grows roughly quadratically with history depth, and per-seal cost is worse than
  linear.** `seal` derives the next state root by walking the ancestor lineage, so each seal costs
  more the deeper the history is. Three independent builds, each grown to depth 1,024 (the deepest
  measured), median of three, mean over the 16 blocks ending at each depth:

  | depth | one `seal` | cumulative time to build to it |
  |---:|---:|---:|
  | 32 | 0.09 s | 4 s |
  | 64 | 0.18 s | 11 s |
  | 128 | 0.30 s | 30 s |
  | 256 | 0.64 s | 98 s |
  | 512 | 1.4 s | 6.4 min |
  | 1,024 | 5.0 s | 34 min |

  The per-seal exponent is 1.2 over depth 32–1,024 (1.15–1.24 across the three builds) and rises
  with depth: it is about 1 up to 256, and 1.6–1.8 between 512 and 1,024. The cumulative exponent is
  2.0 over depth 128–1,024 (1.9 over 64–1,024). **Quadratic is the release build's shape too, but
  only from about depth 128 on**: over depth 32–128, the only range a debug build measured, the
  cumulative exponent is 1.4 in release against 1.96 in debug (the cause was not investigated). A release seal is 15–33 times cheaper than a debug one at
  the same depth (0.30 s against 9.9 s at depth 128). **Every 64th seal also writes a checkpoint and
  costs more than an ordinary one**: about twice as much near depth 128 (the seal of block 129 took 0.61 s against
  0.31–0.33 s for blocks 128 and 130), less at depth, because the lineage walk dominates both (1.6× at block 513, 1.4× at
  961). The per-seal figures above are means over
  windows that end at every 64th block and so never include one of these; the cumulative column does include
  them. **This is the figure that decides
  how long it takes to build, or import, a deep history**, and it has been measured only to depth
  1,024; the build to 2,048 was not attempted (the 2-hour rule stopped it, its build having been
  projected at 2.3 hours). Nothing here is projected past 1,024.
- **Sealing also needs memory that grows with the square of depth.** One `seal`'s peak resident memory, measured with
  `getrusage` on the 0.47.0 release build: about 124 MiB at depth 256 and **about 1.8 GiB at depth 1,024** (1,874,000 KiB
  sealing block 1,025, three samples). `seal` keeps a copy of the tree's state for every block of the lineage it walks. At
  depths past a few thousand blocks, memory rather than time is the first limit. A change that seals from the nearest
  verified checkpoint instead is being designed (RFC 159); it is not in any release.
- **Checkout is close to linear in depth; `merge-evidence` is a little worse, and both are cheap at
  these depths.** From depth 32 to 256: `checkout --patch-plan` 36 → 152 ms and
  `checkout --patch-materialize` 125 → 591 ms (exponents 0.70 and 0.77; the tree itself grew as
  depth<sup>0.84</sup>, 29 → 155 files), and `merge-evidence` 34 → 455 ms when the two sides only add
  files (exponent 1.28; the baseline's replay is all it does). When each side edits files that were
  edited before, `merge-evidence` costs 69 → 473 ms (0.97). Peak memory at depth 256 is about 11 MB
  for all of them (polled `VmHWM`). **Cost tracks history depth, not repository size**, but at these depths depth is
  cheap: a `commit` that has to replay the whole history (no cache) measured 69 ms at depth 33 and
  494 ms at depth 256 (exponent 0.96). *The depth<sup>1.45</sup> figures this page used to give were
  measured before 0.43.0 (checkout was not yet anchored), at checkpoints that a divergence had
  shifted, in a debug build. A debug build measured today gives nearly the same shapes as release
  over the same depths (0.68 and 1.26), and costs 8–30 times more at each depth.*
- **A `commit` that can use the cache costs about the same at any depth we measured.** Between
  seals, the second and third `commit`, and a `commit` after `worktree-status`, take 39–44 ms at
  depth 256, against about 475–507 ms before 0.47.0 (they replayed the whole history each time). A
  `commit` at a tip that edits text an earlier block edited, 487 ms → 52 ms; the same `commit`
  editing that file, 936 ms → 53 ms. The cache's independent full replay still happens every 64
  uses. Depth range 32–256; three interleaved runs each.
- **Incremental commit memory is flat up to a few thousand tracked files, then grows linearly** at
  roughly 1.7 KiB per file beyond that — measured peak around 11.6 MiB at 100 files and 113 MiB at
  64,000 (release, three samples at each of seven sizes).

A few things worth knowing about these numbers before relying on them:

- They describe *shape and order of magnitude*, not a guarantee for any particular repository.
  They were measured against a small corpus of profiles — two profiles, chosen deliberately at
  opposite ends of one axis but also differing 46x in breadth from each other, so results are
  bracketed by two shapes rather than interpolated across every shape a real repository might
  have. The timing figures above use one of them, `prikk-self`.
- The evidence is not equally deep. The sealing figures are three builds to depth 1,024; the
  checkout and `merge-evidence` figures are five depths (32 to 256), three samples each, so their
  exponents are direction, not a fitted curve; the cache figures are five cells, depth 32–256; the
  commit-memory figures reach 64,000 files.
- The machine was shared with other work (load average 1.7–13 during the sealing builds), which is
  why each sealing figure is the median of three and why absolute times can move by up to about
  two times between runs. Ratios within a table are the sturdier reading.
- All of the above was measured on Linux.
