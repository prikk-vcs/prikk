# RFC 149 — steps 1–2: confirm the layer, then census what the surfaces reach

**RFC 149 accepted 2026-09-12**; the surfaces crate is **`prikk-operations`**; the file-size gate lands
first (`130-…/size-check-handoff-v1.md`). **Live after it.** This round moves nothing: it produces the
two pieces of evidence §5's criteria 1 and 2 need, and the owner rules on the second before any move.

## 1. Step 1 — zero core→surface edges, including through root re-exports (§5.1)

Core = the six `DECLARED_CYCLES` members (`commit_boundary`, `lifecycle_cache`, `patch_replay`, `refs`,
`trust`, `wal`) and everything beneath them; surfaces = the other top-level modules of `prikk-store`
after `rollback/` and `merge/` (currently 26 → list them by name from `lib.rs`, not from memory).

- From `boundary-check --graph`: every edge whose `from` is in the core and whose `to` is a surface.
  Expected: none. Print the count and, if any, the edges.
- **Then the edges the graph cannot see**: a core file may `use crate::<name>` where `<name>` is a root
  re-export owned by a surface (RFC 147 §3e's shape). Resolve every bare `crate::<ident>` import in the
  core against the owner of each name in `lib.rs`'s `pub use` block. Expected: none owned by a surface.
  Show the resolver's method and its count of imports examined; a zero with no count is not a result.

If either finds an edge, **stop and report** — §5 says the increment stops, and which edge it is
decides what happens next.

## 2. Step 2 — the census of core items the surfaces use (§5.2)

For each surface module: every item it reaches in the core — types, functions, constants, traits —
with the item's current visibility (`pub`, `pub(crate)`, private-via-reexport) and one line on why the
surface needs it. Group by core module; total the distinct items; mark the ones that are already `pub`
and root-exported (they cost nothing) versus the ones that would have to become `pub` for the cut to
compile. **That second list is what the owner rules on.** Derive it by attempting the compile in a
scratch worktree if that is faster than reading — say which you did; if you compiled, the error list
is the census and the worktree is discarded.

Also report: which surfaces reach *no* core item (they move for free), and which reach the most.

## 3. Report

`.git-exclude/review-request/rfc149-step-1-2-report-v1.md`. No product change; the one commit allowed
is a `boundary-check --graph` extension if the resolver of §1 belongs there permanently (it probably
does, as `--graph --resolve-reexports`; say so and gate it). Full gate set if anything is committed.
