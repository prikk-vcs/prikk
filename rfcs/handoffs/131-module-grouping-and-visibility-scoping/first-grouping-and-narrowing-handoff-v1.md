# RFC 131 §6d.2 — group one pair, narrow one item, report the cost

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6d** (the lift) and **§6d.2**
(prove it on one pair first). Follows the coupling-gate arc, closed green at `ba6f1e76`.

**This is the first time since RFC 131 was written that `pub(in crate::<path>)` is expressible for these
modules.** It is also a measurement round: **group one pair, narrow at least one real item, report what
it cost.** Do not group seven.

## 1. Why this is now possible, and what proves it

§6a forbade any two of the constrained seven — `active`, `lifecycle_cache`, `patch_replay`, `refs`,
`trust`, `worktree_patch`, `wal` — from sharing a group, because grouping merged graph nodes and **a
cycle inside a group vanished**. §6d lifts it: `subtree_cycles()`, the mechanism `check()` actually
calls, reports **both** directed edges between two siblings under a shared parent
(`graph::tests::subtree_control4_…`). Rule 4 does not suppress them — siblings are not
ancestor/descendant of each other.

**Re-prove it on real modules, not synthetic ones.** That is control 1.

## 2. What to do

**Group exactly two of the seven** under one new parent module. **Choose the pair and justify the
choice** — a pair with a real, narrowable dependency between them is worth more than the pair that is
easiest to move.

**Narrow at least one real item** from `pub(crate)` to `pub(in crate::<parent>)` — an item in one of the
two that only the other uses. **One is enough.** The deliverable is that it compiles, the gate stays
green, and you can say what it cost.

**Rename the allowlist entries in step** (§6d.1). Grouping `refs` under `g` makes it `g::refs`, and every
`DECLARED_CYCLES`/`DECLARED_HUBS` mention must move with it — the same rename §6c.7 just performed for
`lifecycle_cache`. **The coupling is unchanged; only its name moves.** A grouping that leaves the
allowlist alone turns the gate red, correctly.

## 3. REQUIRED — report the cost, because the rest is a decision that depends on it

- **How many files moved**, and how many `crate::<name>` references were rewritten?
- **How many items could actually be narrowed** in the pair you grouped — not how many you narrowed, how
  many were *eligible* (used only from within the new parent)?
- **What did the allowlist rename touch?**
- **Was anything about the move mechanical, or did any of it require judgement?**

**That last one decides whether grouping the remaining five is a chore or a design exercise**, and it is
the question I cannot answer from outside.

## 4. Controls

1. **The grouped pair's cycle is still reported, by name, against the real repository** — the §6d
   evidence re-proven on real modules. **This is the round's whole point**: it is what §6a's prohibition
   was protecting, and the first real test of the lift.
2. **The narrowed item still compiles and is genuinely narrower** — assert it is `pub(in crate::<parent>)`
   and not reachable from outside that parent. A narrowing that compiles because nothing outside used it
   anyway is worth recording as such.
3. **`boundary-check` is green at your final commit**, with the allowlist renamed.
4. **Nothing outside the moved pair changed meaning** — the full suite passes, and the other six
   modules' allowlist entries are untouched.

**Perturb control 1.** It is the one whose passing would be most easily assumed rather than checked —
and it is the specific claim §6d rests on. Break the grouped pair's mutual reference and confirm the
gate stops reporting the cycle.

## 5. What this round must NOT do

- **Do not group more than two.** §6d.2 rules one pair first, deliberately.
- **Do not narrow broadly.** One item proves expressibility; a sweep is a later decision informed by §3's
  numbers.
- **§4 still binds**: `foo/tests.rs` beside `foo.rs` must survive the move. And §6a.1 stands — *"a group
  whose members share only the need to be somewhere is worse than none."* **A lifted prohibition is not
  an instruction to group.**
- Do not touch the gate mechanism, `HUB_THRESHOLD`, or any allowlist entry beyond the rename the move
  forces.
- `MILESTONES.md` untouched. No `CHANGELOG.md` entry — internal structure, not user-visible surface; say
  so.

## 6. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, **all green**. State rule 9's cross-target
result, or that it does not apply and why — a module move touches many files and the trigger is worth
re-checking rather than assuming.

Report to `.git-exclude/review-request/`. **Lead with control 1 and §3's four answers.**
