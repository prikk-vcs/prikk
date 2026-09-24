# RFC 159 — Sealing without re-walking the lineage

**Status.** **PROPOSED 2026-09-25 by the architect**, on the owner's instruction for history import's
prerequisite: *"Schedule it and start the design when ready."*

**The architect's reading of that instruction, stated so it can be corrected:**
- it authorizes a **design round** (measure and prototype, no product code), scheduled first after the 0.47.0 cut;
- **the direction this RFC settles on needs the owner's acceptance before any implementation handoff**, because it
  changes what `seal`, the one operation that signs new history, re-checks on every call.

Author-review independence: the architect proposes and will review; §6's controls compensate, each shown to fail.

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
