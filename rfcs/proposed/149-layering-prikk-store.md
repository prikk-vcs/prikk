# RFC 149 — Layering `prikk-store`: a core that stops growing, and a surfaces crate that may

**Status.** **PROPOSED 2026-09-12**, on the project owner's instruction to reconsider the bloatedness
direction *"not only now but also for the future."* Measured first; the design follows the measurement.

**Author-review independence gap:** the architect authored this and will review its implementation.

---

## 1. What RFC 130 §6 answered, and what it did not

RFC 130 §6 ruled out **extracting a module into its own crate**: `fsutil` is the one seam with zero
production out-edges, and extracting it removes no coupling while adding a crate to the publish order.
That ruling stands on its evidence. **But "can a clean module be extracted?" is not "does a
one-directional layer cut exist?"** — and the second question is the one that decides where growth
goes.

RFC 130 §8 measured the growth: **+11.9% in eight days, tests +23.7%, production ≈ +9%**, eight declared
cycles unchanged. Every feature since 0.36.0 — RFC 142, 143, 144, 146, 147 — landed in `prikk-store`,
and almost all of it landed in **read and operation surfaces**, not in the store.

## 2. The measurement — three layers on today's import graph

Every top-level module of `prikk-store` (52), classified by what it imports and what imports it,
production files only, `crate::<module>` references:

| layer | modules | count |
|---|---|---|
| **core** — members of `DECLARED_CYCLES` | `commit_boundary`, `lifecycle_cache`, `patch_replay`, `refs`, `trust`, `wal` | 6 |
| **surfaces** — nothing in the core imports them | `show`, `history`, `verify`, `doctor`, `bundle`, `checkout`, `patch_checkout`, `patch_inverse`, `rollback_draft`, `rollback_preview`, `rollback_verify`, `merge_evidence`, `merge_execute`, `patch_algebra`, `patch_exchange`, `patch_set_digest`, `sync_negotiation`, `seal_from_accepted`, `tag_travel`, `recognition_claim`, `worktree_status`, `worktree`, `block_state`, `state_root`, `compact`, `memory_store` | 26 |
| **below** — imported by the core, in no cycle | `foundation` and 20 others (`object_store`, `author`, `node`, `path`, `lock`, `format`, `text_span`, `trust_index`, …) | 20 |

**The two facts that make this a design and not a diagram:**

1. **Zero edges from core to surfaces.** Not one of the six cycle members imports any of the 26. The
   31 surface→surface edges (e.g. `patch_exchange → verify`, `merge_evidence → patch_algebra`) stay
   inside the surfaces layer.
2. **Zero hidden edges through root re-exports.** `lib.rs` re-exports 264 names; a core module could
   reach a surface through `use crate::<name>` without naming the module (RFC 147 §3e's finding). Checked
   by resolving every bare import in the six core modules against the owner of each re-exported name:
   **none resolves to a surface module.**

**Caveat, stated as a caveat:** this is `crate::` text at top-level granularity, not the coupling gate's
resolved subtree graph. `boundary-check --graph` (RFC 131 §6e's step 0, already handed off) is the
instrument that confirms or breaks it. **Nothing moves until it has run.**

## 3. The design

**Cut above the core.** `prikk-store` keeps *below* + *core* — the store, its durability contract, the
WAL, refs, trust, the replay engine, and the commit boundary: the part that is genuinely one thing, and
the part that is cyclic. **The 26 surfaces move to a new crate above it**, depending on `prikk-store`
one-way. The CLI depends on both.

**Why this is the answer to "getting larger", and the gates in RFC 130 §8 are not:**

- **Growth lands where it belongs by construction.** A new read or operation surface cannot be added to
  the store crate without adding a downward dependency the gate already forbids across crates
  (DC-51's placement gate covers tool↔product; the same mechanism extends to the new crate). The store
  stops growing because there is no longer a place in it for the things that grew.
- **The cyclic core becomes small and visible.** Six modules, eight declared cycles, one crate — the
  thing RFC 130's gate watches is then the crate boundary itself.
- **The tests move with their code**, so RFC 131 §4's rule (no test relocation away from its subject)
  is satisfied, not bent: `verify/tests.rs` goes where `verify.rs` goes. **Test growth per feature
  lands in the surfaces crate** — the same 52% ratio, in the crate that should carry it.
- **It is RFC 145's shape D made structural.** The reviewer's read-only-facet question (RFC 145 §8c) is
  answered not by a feature flag but by a crate boundary: a browse view links the surfaces crate,
  which cannot reach `RefStore::publish` except through the store's own public API.

**Naming.** Plain, literal, no coinage (RFC 145 §9a). The surfaces crate is not "read" (it holds
`merge_execute`, `seal_from_accepted`, `rollback_draft`) — **the owner names it**; the architect's
candidates are `prikk-ops` and `prikk-surface`.

## 4. The costs, weighed against RFC 130 §6's list

| RFC 130 §6 named | here |
|---|---|
| publish order grows | 9 → 10 crates; one line in the fixed registries in `boundary.rs`/`placement.rs` — one-time |
| `pub(crate)` → `pub` conversions | **every item the surfaces reach in the core becomes public API of `prikk-store`.** This is the real cost and it is unmeasured: the increment's first step is a census of exactly which core items the 26 surfaces use |
| removes no coupling | **true, and not the goal.** The eight cycles stay inside the core; the goal is that growth stops compounding in the store |

**A widening of `prikk-store`'s public API is a stability question**, and RFC 132's third trigger
(*"any move toward a library stability promise"*) is close by. **This RFC does not promise stability**
— crate APIs stay explicitly unstable — but it does make the store's public surface larger and more
load-bearing. That is the trade, named.

## 5. Decision criteria — the increment stops if any fails

1. `boundary-check --graph` confirms zero core→surface edges on the resolved graph.
2. The census of core items the surfaces use is a list the owner is willing to make `pub`.
3. Every test moves with its module; no test is relocated away from its subject.
4. The eight declared cycles are unchanged in number and membership afterwards — the cut does not
   hide one (RFC 131 §6a/§6c's failure mode).

## 6. Sequencing

1. **`boundary-check --graph`** (step 0 of the census handoff, already live).
2. **The public-API census** — which core items the 26 surfaces reach.
3. **Owner rulings** (§7).
4. **One move commit per surface family**, moves separate from any content change (RFC 131 §5).
5. **RFC 130 §8's file-size gate and evidence line land regardless** — they are interim controls and
   remain useful after the cut.

## 7. Owner rulings

1. Accept the direction: cut above the core.
2. The surfaces crate's name.
3. Whether the file-size gate (RFC 130 §8 item 2) lands before or alongside.

## 8. Non-goals

No change to the format, the object model, or any command's behaviour. No stability promise. No
removal of a declared cycle — that is separate work, scheduled by the owner from the gate's
`what_would_remove_it` list.
