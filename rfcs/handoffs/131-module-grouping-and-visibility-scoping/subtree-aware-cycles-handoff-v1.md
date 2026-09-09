# RFC 131 §6c.4 — subtree-aware cycle detection

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6c.4**, correcting §6c.1.
Follows `42bcab15`, whose mechanism is **accepted and not reopened**.

**This is my ruling that was wrong, not your implementation.** §6c.1 said nothing about containment, you
implemented it exactly as written, and the result is a gate that cannot see a cycle leaving a subtree
through a child and returning to its root.

## 1. What the zero actually meant

Traced by hand at review, against the source:

| Pair | Outgoing | Return |
|---|---|---|
| `refs` ↔ `active` | `refs/evidence.rs:7` → `crate::active::{…}` | `active.rs:22` → `crate::refs::{…}` |
| `lifecycle_cache` ↔ `patch_replay` | `lifecycle_cache/replay.rs`, `replay/effect.rs` → `crate::patch_replay` | `patch_replay.rs` → `crate::lifecycle_cache` |

**Both subtrees mutually depend.** The graph misses it because nothing connects `refs` to
`refs::evidence`. **Your attribution work was right** — `refs.rs` itself genuinely never references
`crate::active`. What was wrong is treating a submodule as unrelated to its own parent.

## 2. What to build (§6c.4's four rulings)

1. **Keep qualified nodes.** 127 nodes, longest-prefix resolution, everything from `42bcab15` stays.
2. **Cycles are computed over subtrees**: A ↔ B when *any* descendant of A depends on *any* descendant
   of B and vice versa. A node is its own descendant.
3. **Report at the smallest pair of subtrees exhibiting it.** `refs ↔ active`, not
   `refs::evidence ↔ active` — because `active` depends on `refs` itself, so `refs` is the smallest
   subtree on that side. Derived, not concatenated.
4. **An edge between a node and its own ancestor or descendant contributes nothing.** This is what keeps
   the 35 files that reference their own parent absolutely (`foundation` 14, `test_gates` 7,
   `patch_algebra` 5, `refs` 4, and five more) from becoming 2-cycles.

**Do not reintroduce concatenation.** The nodes stay separate; only the cycle *question* becomes
subtree-aware.

## 3. REQUIRED — measure again, and rule nothing

Same discipline as last round, and for the same reason it mattered:

- How many subtree cycles does the gate report?
- How do they compare to the 8 `DECLARED_CYCLES` entries — same pairs, subset, superset?
- What are the hub numbers under the corrected rule?

**Populate no allowlist entry, remove none, fix no cycle, move no module.** I ruled last round on a
number that turned out to be the wrong number; I would rather be given the right one before ruling
again.

**`boundary-check` is again expected to fail**, with the failure enumerated. Report its real result.

## 4. Controls

1. **The two traced pairs are reported as cycles** — `refs ↔ active` and
   `lifecycle_cache ↔ patch_replay`, against the real repository. **This is the round's whole point**:
   `42bcab15` reports neither.
2. **Reported at the smallest exhibiting pair**, not the top-level pair by default and not the deepest
   node pair — assert the specific pair names.
3. **A child referencing its own parent absolutely is not a cycle** — synthetic, and separately assert
   the real repository reports no cycle for any of the 35 files' parent/child pairs.
4. **A cycle inside one former top-level module is still visible** — `42bcab15`'s control 1 must keep
   passing; subtree-awareness must not re-hide what qualified naming exposed.
5. **A genuinely acyclic pair stays acyclic** — a one-directional dependency between two subtrees, at any
   depth, reports nothing.

**Perturb control 3.** It is the one whose passing is most easily vacuous — an implementation that
reported *every* subtree pair as cyclic would pass 1 and 2 and fail almost nothing else obvious.

## 5. What this round must NOT do

- Do not reopen `42bcab15`'s node identity, resolution rule, or grouped-import handling.
- **Do not add `super::`/`self::`/bare-path scanning.** §6c.1 was right about that and stays.
- Do not lift §6a's prohibition, move any module, or narrow any visibility.
- Do not touch `boundary.rs`'s member allowlist or the hub *threshold*.
- `MILESTONES.md` untouched. No `CHANGELOG.md` entry — internal gate; say so.

## 6. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, with `boundary-check`'s real result reported
whatever it is. State rule 9's cross-target result, or that it does not apply and why.

Report to `.git-exclude/review-request/`. **Lead with §3's three answers and control 1.**
