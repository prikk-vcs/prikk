# RFC 159 — Sealing without re-walking the lineage

**Status.** **ACCEPTED by the project owner 2026-09-26** (*"Yes. Accepted."*, answering *"Accept RFC 159's direction,
rulings included?"*). **The architect's reading, stated so it can be corrected:**
- the direction of §4 is accepted, with **all seven rulings of §8**, including C2's anchor-trust rule and its extension
  to RFC 136 2b's anchored worktree writes;
- the fix ships as **0.48.0**, cut as soon as this implementation is accepted, **ahead of RFC 158 Stages B and C**,
  and 0.47.0 is **not** re-tagged;
- **crates.io:** 0.47.0 is published together with 0.48.0, at that release (*"How about publishing it when we are
  ready for 0.48.0 release?"*).

Implementation handoff: `rfcs/handoffs/159-sealing-without-rewalking/implementation-handoff-v1.md`.

*History:* **PROPOSED 2026-09-25 by the architect**, on the owner's instruction for history import's
prerequisite: *"Schedule it and start the design when ready."*

**The architect's reading of that instruction, stated so it can be corrected:**
- it authorizes a **design round** (measure and prototype, no product code), scheduled first after the 0.47.0 cut;
- **the direction this RFC settles on needs the owner's acceptance before any implementation handoff**, because it
  changes what `seal`, the one operation that signs new history, re-checks on every call.

Author-review independence: the architect proposes and will review; §6's controls compensate, each shown to fail.

**Design round DELIVERED and reviewed 2026-09-26** (instrument `5e97cfa5`..`455eeca7`; review
`.git-exclude/reviewed/rfc159-design-round-review-v1.md`). The results and the architect's rulings are in **§8**. **Awaiting the
owner's acceptance of the direction** before any implementation handoff.

## 1. Measured, not assumed — release build, 2026-09-25

RFC 139's corpus (`prikk-self`), three independent histories built commit-then-seal to depth 1,024 (review
`.git-exclude/reviewed/release-remeasurement-review-v1.md`):

| depth | one `seal` (ordinary block) | cumulative build |
|---:|---:|---:|
| 256 | 0.64 s | 98 s |
| 512 | 1.4 s | 6.4 min |
| 1,024 | 5.0 s | 34 min |

- **Per-seal cost is about linear in depth up to 256 and steepens above it** (exponent 1.5 over 256–1,024,
  1.8 over 512–1,024).
- **Cumulative cost is quadratic from about depth 128** (exponent 2.0 over 128–1,024).
- A checkpoint block (every 64th) costs about 2.3× an ordinary one.
- The architect reproduced an ordinary seal at depth 258 at 0.52–0.55 s on a quiet machine.

**This is the cost that decides history import** (RFC 113, theme 17). An importer of 10⁴–10⁶ source commits
(brygge) writes thousands of blocks at least.

## 2. Why every seal costs O(depth) — read at source

`seal_block` (`block_state.rs`) derives the next state root through `resolved_parent_state`, which walks **every**
ancestor:
- `validate_v2_lineage` reads each block;
- `verify_v2_lineage_roots` folds each block with the ordinary fold, **recomputes `compute_state_root` over all
  entries at each block**, and memoizes a **clone of the state and the text cache per block**, never evicted on this
  path.

So a seal at depth *N* does O(*N*) folds, O(*N* × files) root hashing and O(*N*) retained clones. The last two
explain why per-seal cost steepens past linear.

**Why it does that is deliberate.** A new Block's signature asserts a state root. `seal` refuses to sign a root it
derived from a lineage whose own roots it has not confirmed, because otherwise a wrong root in history becomes a
signed one (the laundering RFC 136 §10.3c exists to prevent).

## 3. Who pays it — every writer that seals, `sync` included

Every production path that creates a Block goes through the one seal function (RFC 136 increment 0):

| path | reached by | pays the walk |
|---|---|---|
| `seal_block` | `prikk seal` (`prikk-cli/src/seal.rs`) | per block sealed |
| `seal_block` | `prikk merge` (`merge/execute.rs:176`) | per merge block |
| `seal_block_classified` | **`sync`'s seal-from-accepted** (`seal_from_accepted.rs:212`) | **per block a receiver seals from an accepted artifact** |

**So `sync` is directly in scope.** A receiver re-seals every claimed block under its own key, and each of those
seals walks the receiver's whole lineage. A long catch-up over `sync` pays the same quadratic that an import does.
`bundle import` does **not**: it stores blocks verbatim and seals nothing. `verify` walks the whole lineage by
design and is out of scope.

## 4. The direction to measure — seal from the nearest replay-verified anchor

It is the same mechanism as RFC 136 increment 2c's deferred option (i), which the 2c design round prototyped for
the cold baseline: whole-state identity with full replay at **256 of 256 blocks**, 0.31× full replay in release at
depth 256.

1. **Live state** comes from the nearest block that has a snapshot **and** is in the replay-verified record
   (`verified_blocks.rs`), and whose snapshot passes `validate_snapshot_manifest` (its root equals that block's
   signed `state_merkle_root`).
2. **History fields** (`latest_tombstone_by_id`, `seen_ids`) come from an **id-only walk** of the blocks up to the
   anchor. The 2c round showed it exact at every block compared, at 0.7 % of a full replay's time in release.
3. **The ≤ 63 blocks after the anchor** are folded and root-checked exactly as today.
4. **No anchor in the record, or any doubt:** today's full walk.

Per seal, that is O(63) folds and root checks plus an O(*N*) walk over ids, instead of O(*N*) folds and root checks.

**What the record means, checked at source.** It is written only by `seal_block` (the lineage it just verified, and
the new block) and by `verify` (every block whose state outcome is `Verified`). It is rebuildable, never
authoritative, and keyed to the crate version. A received block can enter it **only** through `verify`, and only
after replay confirmed its root. So membership means *"this root was confirmed by replay here"*, not *"this signer
is trusted"*. That is exactly what `seal` re-checks today.

## 5. What the design round must settle

1. **Exactly what today's full walk checks that the anchored walk would skip** for blocks below the anchor. Enumerate
   every check in `validate_v2_lineage` and `verify_v2_lineage_roots`: structure, parents, roots, the
   delete-preimage and restoration checks the 2c round named. For each, say whether it is covered by (a) the record
   having been written by a verification that ran it, (b) the id-only walk, or (c) nothing. **Any (c) is a finding
   the architect rules on** before implementation.
2. **Received history.** A `merge` or seal-from-accepted whose lineage includes received blocks: are they ever
   anchors today, and should they be? Answer from the record's writers, not from intent.
3. **The id-only walk is still O(*N*) per seal.** Measure it at depth 1,024 in release. If it matters at import
   scale, lay out options: a rebuildable cache of history fields keyed by verified anchor (not a format change),
   or something else. RFC 136 2c option (ii), stored history in the manifest, was rejected as a format change;
   do not reopen it.
4. **The memo's clones.** With ≤ 63 blocks folded, does the per-block clone still matter? Measure peak RSS by
   `getrusage` at depth 1,024, before and with.
5. **One mechanism, two users.** The same anchored derivation should serve the cold baseline (`incremental.rs`
   rung 3 for *no cache* only, **never the reanchor or `verify_divergence`**) and `merge-evidence`'s baseline
   (`merge/evidence.rs:56`, `:172`). Say which call sites one function covers.

## 6. Controls the implementation will need (sketched here, built later)

- **Byte-identity:** block ids, state roots and signatures identical with and without anchoring, across the corpus
  to depth 1,024, for `seal`, `merge` and seal-from-accepted.
- **A lying anchor is never used:** a snapshot that passes the loader but is not in the record changes nothing.
- **A missing or damaged record** means the full walk, and the same result.
- **Every item §5.1 marks (a) or (b)** has a perturbation that turns a control red.
- **Per call site:** remove the anchoring at each one, and its control, and only its control, goes red.

## 7. Scheduling (the architect's; the owner authorizes cuts)

- **Design round:** first after the 0.47.0 cut. It measures and prototypes, and lands no product code.
- **Implementation:** 0.48.0, **before RFC 158 Stages B and C**, if the design needs no format change, as §4
  expects. If it does need one, it joins Stage C's format 8, and this RFC says so before anything is built.
- **brygge** is told when the design round reports, with measured before-and-after figures, not before.

## 8. Design round results and rulings — 2026-09-26

**Measured (release, three samples, one session; the prototype built in a separate worktree and never landed):**

| | shipped 0.47.0 | prototype |
|---|---:|---:|
| one ordinary `seal` at depth 1,024 (mean over a 64-block cycle) | 4,086 ms | 394 ms |
| cumulative build to depth 1,024 | 1,894 s | 283 s |
| cumulative build to depth 2,048 | not reachable in 2 h | 1,409 s |
| **peak RSS of one `seal` at depth 1,024** | **1.8 GiB** | **30 MiB** |
| a 64-block `sync` catch-up sealed at depth 1,024 | 251 s, 2.1 GiB | 18.2 s, 57 MiB |

**Identity:** 0 differences across every anchored block of six 1,024-block histories, with the whole lifecycle state
compared against an independent forward replay. `seal`, `merge` and seal-from-accepted were byte-identical in all three arms
(prototype, prototype with anchoring off, baseline), including renames, restorations, mode changes and merge blocks.

**The architect reproduced the shipped figure on the downloaded 0.47.0 asset:** sealing block 1,025 takes 5.3 s and
**1,874,000 KiB** peak, three samples. **Today's release needs about 1.8 GiB to seal at depth 1,024**, and the memory grows
roughly with the square of depth (the per-block clone memo of §2). At import depth the shipped code is killed by memory
before it is slow.

**What is still not flat:**
- Past 1,024, per-seal cost follows the tree size, not the depth (exponent 1.05 over 256–2,048).
- A checkpoint seal is 4–5× an ordinary one in the prototype, and about 80 % of it is the snapshot write.
- The id-only walk is linear in depth: 42 ms at 1,025 blocks, 143 ms at 2,048.

**Rulings (the architect's):**

1. **C1, old blob content below the anchor is not re-read: accepted.** A state root commits to blob *ids*, never to their
   content, so the new block signs nothing about content no live node names. `verify` still reports damaged content. The
   implementation states this in the docs and pins it with a control: damaged content below the anchor, `seal` succeeds,
   `verify` reports it.
2. **C2, the record must not be the only gate: option (ii).** An anchor's *state* may reach a signed root only if all four
   hold, and otherwise the walk runs in full:
   - the anchor block is in the record;
   - its snapshot passes `validate_snapshot_manifest`;
   - its maintainer signature verifies against a key in this repository's adopted maintainer trust policy;
   - it is within 63 blocks.

   A rebuildable local file then only chooses *which* authenticated anchor to use; it never makes one. **Residual, stated:**
   forging requires a trusted maintainer to have signed a wrong root, **and** write access to `.prikk/cache/`.

   **The same anchor-trust function also governs RFC 136 increment 2b's anchored worktree writes**, which today trust the
   record alone. That is one rule for every snapshot *state* that reaches a signature. 2c's anchored *text* stays as it is: it
   is hash-checked, so it is self-certifying.
3. **Received history as anchors: (A)**, any recorded block, under ruling 2. A received anchor is used only when an adopted
   maintainer key signed it.
4. **The id-only walk: nothing now.** At depth 1,024 it is about 1 % of today's walk. A history-fields cache is deferred to
   the history-import work (theme 17), and **any cached history must be bound to something ruling 2 authenticates**: it must
   not become a new unauthenticated input to a signed root. The same holds for `load_verified_blocks`' O(N) read.
5. **Scope of the implementation: stack A only.** That is block-state derivation (`resolved_parent_state`), the one function
   under `seal`, `merge` and seal-from-accepted. **Stack B** is baseline derivation, meaning the cold `commit` rung and
   `merge-evidence`'s baseline; with it done, a `merge` pays one walk instead of two. Stack B follows as its own increment,
   after `incremental.rs` can say whether a call is a cold start or the reanchor. The reanchor and `verify_divergence` are
   never anchored.
6. **Included, measured: the snapshot writer's presence check.** Where the checkpoint writer reads content only to learn
   whether a blob is stored, it asks the index instead. It carries an identity control on every snapshot the corpus writes,
   and the checkpoint seal is measured before and after.
7. **Docs, owed now:** the scale page gains `seal`'s peak memory in the shipped release, and its checkpoint sentence is
   corrected. The ratio falls with depth (2.0 at block 129, 1.4 at 961), so "about twice" holds only near depth 128.

**Corrections to this RFC and its handoff, found by the round and verified:**
- `merge/evidence.rs:173` is `lifecycle_state_at`, `show`'s helper, not `merge-evidence`.
- `incremental.rs:157` does not distinguish a cold start from the reanchor.
- A merge also pays a full baseline replay first (`execute.rs:101`).
- "A checkpoint seal is about 2.3×" was one depth's probe, not the curve.
