# RFC 131 §6f — grouping increment 2: `merge_{evidence,execute}` → `merge/`

**Scheduled:** 0.41.0. **Live** after `rollback/` (done, `05ac6b29`).
**Ruled by the census** (candidate C): 2 modules, 7 files, 1,944 lines, 11 rewrites in `prikk-store`
and 3 in `prikk-cli`, **one allowlist rename** — `DECLARED_HUBS`'s `merge_evidence` becomes
`merge::merge_evidence` (or `merge::evidence` if you rename the child; say which and why) — no cycle
gained or lost. The gate reports both the stale entry and the re-detected hub in one run; that is it
working, and the two-line allowlist edit is the whole cost.

## 1. The change

`merge_evidence` → `merge::evidence`, `merge_execute` → `merge::execute`, under `merge.rs` with a
two-sentence family statement. Root-export names unchanged. The `DECLARED_HUBS` entry follows the new
qualified name, with its existing justification comment moved, not rewritten. The module-count pin moves
128+1 → 130 with a shape assertion beside it, as increment 1 did.

## 2. Controls

1. `boundary-check --graph` before/after: nodes +1, edges equal, **hub set equal after the rename**
   (`merge::evidence` in, `merge_evidence` out), `subtree_cycles` identical — paste the diff.
2. Root-export name diff `0.39.0` → this commit: additions are the two RFC 102 names only; nothing
   removed or renamed.
3. Perturb: leave the allowlist entry stale → `boundary-check` must report both errors; restore.
4. Full gate set, verbatim; cross-target stated from the diff and run.
5. RFC 131 §6f row for C: report the numbers; the architect writes the row.

## 3. Not in this round

`worktree/` — refused by measurement, §6f. Nothing else grouped; the census lists no further candidate.
