# RFC 131 §6c.5 — generalise subtree cycles from pairs to SCCs

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6c.5**. Follows `74e6edc2`,
**accepted and not reopened** — this extends its rule 2 and changes nothing else.

**You named this option and declined to build it unruled. That was right.** It is now ruled.

## 1. What is missing, and why it is real

Rule 2 as written is two-variable, so a cycle closing through a third node is inexpressible. Verified at
review: `worktree_patch → patch_replay` has **no return edge** — nothing in `patch_replay` reaches
`worktree_patch` — and the cycle closes through `active`.

**A 3-node cycle is real coupling.** A→B→C→A means none of the three is independently extractable, and
RFC 130's original component was **six** modules precisely because it is a strongly connected component,
not a set of pairs. Four declared entries (5 edges) name exactly this.

## 2. What to build

**Strongly connected components over the `subtree_depends` relation**, replacing the pairwise scan.

- **Rule 3 (smallest granularity) is unchanged in intent and must survive**: report each component's
  members at the smallest subtree that still participates. Your pairwise local-minimality argument
  generalises — a member replaceable by one of its own children while the rest of the component is held
  fixed is not minimal.
- **Rule 4 is unchanged**: an edge between a node and its own ancestor or descendant contributes nothing,
  still enforced *inside* `subtree_depends` rather than as a post-filter.
- **A 2-member SCC must produce exactly what the pairwise mechanism produced** — the four current pairs
  are the regression baseline.

**Output shape**: `DECLARED_CYCLES` is a list of directed edges, so a component must reduce to edges the
gate can compare against it. Say how you chose the reduction and why — a component's *closing edges* and
its *full edge set* are different answers, and the choice determines what an allowlist entry means.

## 3. REQUIRED — measure, rule nothing, again

- How many components, of what sizes?
- Which of the 8 `DECLARED_CYCLES` entries are now recovered, and at what names?
- Do the four current pairs survive as 2-member components?

**Add, remove and rename nothing in `DECLARED_CYCLES`/`DECLARED_HUBS`.** Three rounds have now produced
a number I was asked to rule on, and twice it was the wrong number. **`boundary-check` is expected to
fail; report its real result.**

## 4. Controls

1. **A 3-node subtree cycle is reported** — synthetic A→B→C→A, asserting all three members and that no
   two of them form a 2-member component on their own.
2. **The four current pairs still report identically** — regression guard on `74e6edc2`'s own result.
3. **Smallest granularity survives at N>2** — a 3-node cycle where one member's participation is really
   its child's; assert the child is named, not the parent.
4. **Rule 4 still holds** — `74e6edc2`'s two control-3 variants must keep passing unchanged.
5. **A DAG of subtrees reports nothing**, at any depth and any width.

**Perturb control 2.** It is the one whose passing is most easily vacuous: an SCC implementation that
reported every weakly-connected blob as one component would satisfy control 1 and lose every pair
distinction — and the four pairs are the only result this arc has so far confirmed by hand.

## 5. What this round must NOT do

- Do not reopen node identity, resolution, grouped imports, `subtree_depends`'s rule-4 guard, or the hub
  computation.
- Do not add `super::`/`self::`/bare-path scanning.
- Do not lift §6a's prohibition, move a module, or narrow a visibility.
- `MILESTONES.md` untouched. No `CHANGELOG.md` entry — internal gate; say so.

## 6. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, `boundary-check`'s real result included.
State rule 9's cross-target result or why it does not apply.

**Before reporting a number smaller than last round's, hand-trace two of the vanished findings against
the source** (§6c.6). Three times in this arc a cleaner number was the gate losing sight of something,
and two greps is what distinguished resolved from invisible each time.

Report to `.git-exclude/review-request/`. Lead with §3's three answers.
