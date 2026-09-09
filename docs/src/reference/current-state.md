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
own: what happens as a repository's history gets deep, or its tracked file count gets large. Three
measured shapes:

- **Sealing cost grows quadratically with history depth.** Per-seal cost is roughly linear in depth,
  so cumulative cost to build a history of a given depth is roughly quadratic — a measured power-law
  exponent of 2.03.
- **Checkout and merge-evidence both cost roughly `depth^1.45`**, from two separate, uncached history
  walks — measured exponents 1.446 and 1.445. Over the same range, tree size itself grew only as
  `depth^0.859`. **Cost tracks history depth, not repository size**: a wide, shallow repository is
  cheap to work with; a deep one is not, regardless of how large its tree is.
- **Incremental commit memory is flat up to a few thousand tracked files, then grows linearly** at
  roughly 1.7 KiB per file beyond that — measured peak around 11.6 MiB at 100 files and 113 MiB at
  64,000.

A few things worth knowing about these numbers before relying on them:

- They describe *shape and order of magnitude*, not a guarantee for any particular repository.
  They were measured against synthetic history ladders and a small corpus of profiles — two
  profiles, chosen deliberately at opposite ends of one axis but also differing 46x in breadth from
  each other, so results are bracketed by two shapes rather than interpolated across every shape a
  real repository might have.
- Extrapolating the sealing curve to very deep history (a few thousand sealed blocks) lands
  somewhere between roughly 23 and 101 hours, depending on which points the extrapolation is fit
  from — that range is a **projection**, not a measurement; that depth has never actually been
  built. If you quote a number like that, say "projected" alongside it, or better, quote the shape
  (quadratic) and apply it to your own depth.
- All of the above was measured on Linux, with release builds.
