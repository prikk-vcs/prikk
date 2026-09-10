# RFC 131 §6c.7 — reconcile the allowlist and turn the gate green

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6c.7**. Follows `cc15e616`,
**accepted and not reopened**.

**Three rounds of "measure, rule nothing" are over.** The ruling is made; this round transcribes it.
**The gate should end this round green** — the first time in this arc.

## 1. Cycles — one rename, nothing removed

In `DECLARED_CYCLES`, replace the bare name `lifecycle_cache` with **`lifecycle_cache::replay`**
everywhere it appears — the `lifecycle_cache↔patch_replay` entry and `worktree_patch->lifecycle_cache`.

**All eight entries stand. No entry is removed. No reason text changes.** The coupling is the same
coupling; only its name moves to where it actually lives. `lifecycle_cache.rs` writes no component edge
of its own; `patch_replay.rs` reaches `crate::lifecycle_cache::replay::…`.

**If the rename does not by itself clear all 6 cycle-shaped errors, stop and report** rather than
adjusting anything else — that would mean the ruling rests on a miscount.

## 2. Hubs — remove two, declare three

**Remove** `refs` and `lifecycle_cache` from `DECLARED_HUBS`. `min(fan_in, fan_out)` is 4 and 2 against
a threshold of 6; neither is a hub. **`refs`' fan-in of 28 makes it a sink, not a middle hub**, which is
what the threshold's `min()` form is for.

**Declare** `active` (7/6), `wal` (13/6), `author::author_key_index` (6/6).

**For `active` and `wal`, recover their previous declared reasons from git history — do not write new
ones.** `coupling.rs:56-61` records that both *"dropped out of the declared set at RFC 131 §2.2a's
`foundation` grouping … a real consolidation effect rather than a code change to either module"*.
Qualified naming undoes that collapse, so the debt they describe never went away and their original
reasoning is the honest text. **Quote the commit you recovered each from.**

**For `author::author_key_index`, write a new reason and a what-would-remove-it** from its actual edge
list — it is a submodule no mechanism before `42bcab15` could name, so there is no prior text to
recover. **You have the edge list; I do not.** I will review the reason rather than supply it.

**Update `coupling.rs:50-62`'s own threshold doc comment.** It currently narrates the
`foundation`-grouping drop-out as the live state; that is now history, and the comment is the one place
this artifact was recorded when it happened — **keep the history, mark it as resolved, do not delete it.**

## 3. Controls

1. **`boundary-check` passes**, and `the_real_repository_passes_with_no_undeclared_cycle_or_hub` and
   `workspace_and_product_boundaries_hold` both pass. **The whole ten-gate set is green.**
2. **Every declared cycle entry is still matched by the graph** — the existing
   `check_declared_entries_still_exist` path, now with nothing stale.
3. **Perturb the rename**: revert `lifecycle_cache::replay` to the bare name in one entry and confirm the
   gate reports it stale. A green gate proves nothing unless it can still go red.

## 4. What this round must NOT do

- Do not change the cycle or hub **mechanism** — node identity, resolution, `subtree_depends`,
  `minimize_component`, `subtree_cycles`, `fan_in`/`fan_out`, `HUB_THRESHOLD`.
- **Do not lift §6a's prohibition**, move a module, or narrow a visibility. §6c.7 notes the lift is now
  *available*; it is RFC 131's own next decision and not part of closing the allowlist.
- Do not add or remove any cycle entry beyond the rename.
- `MILESTONES.md` untouched. No `CHANGELOG.md` entry — internal gate; say so.

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 — **all ten green, which is the deliverable.**
State rule 9's cross-target result or why it does not apply.

Report to `.git-exclude/review-request/`. Lead with the gate result, then the recovered reason text for
`active` and `wal` with their source commits, and your new reason for `author::author_key_index`.
