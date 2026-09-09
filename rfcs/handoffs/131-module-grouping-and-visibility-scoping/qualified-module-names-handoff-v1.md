# RFC 131 §6c — teach the coupling gate qualified module names

**Design:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6c**, amending RFC 130's
gate (cross-referenced from RFC 130 §8). Owner-authorised 2026-09-10.

**This round changes how precisely the gate names a node. It changes nothing about what an edge is,
moves no module, and fixes no cycle.**

## 1. The mechanism today, verified — check it yourself before building on it

`walk` (`tools/release-policy/src/boundary/coupling/graph.rs:377-401`) builds
`BTreeMap<String, String>` keyed by the **top-level `mod` declared in `lib.rs`**, and
`collect_production_text` **recursively concatenates every descendant file's text into that same
value**. Edges come from *"every `crate::<ident>` occurrence"* — **the first segment only**.

**Those two facts are the same fact.** The scan reads one segment, so a node can only be a top-level
module; the concatenation is what makes that self-consistent.

**Why it matters** (RFC 131 §6a): grouping merges nodes, so a cycle wholly inside a group **disappears
from the graph** — the gate would report green because the grouping hid it, not because the coupling
was resolved. That is why §6a forbids any two of the constrained seven sharing a parent, and why
RFC 131 §3's target is unreachable today.

## 2. What to build

**A node becomes every production module at every depth, keyed by its qualified path from the crate
root** — `foundation::layout` distinct from `foundation::fsutil`. A module's text is **its own file's**,
not its descendants'.

**Resolution: longest matching module path wins.** `crate::a::b::C` resolves to node `a::b` when `a::b`
is a module and to node `a` when it is not. An item name is not a node.

**Extend the grouped-import expander in step.** `use crate::{a, b::c}` currently reduces each element to
a first segment; it must preserve full paths so the same resolution applies. Its own doc notes no nested
grouping (`crate::{a::{b, c}}`) exists in this crate — **re-check that rather than trusting it**, since
the file has changed since.

## 3. REQUIRED — the edge vocabulary does not change (§6c.1)

**Do not add `super::`, `self::`, or bare-path scanning.** They are invisible to the gate today. Making
them visible changes **what an edge means**, not how precisely a node is named — and it would turn every
`foo/bar.rs` reaching back through `super::` into an edge to `foo`, making ordinary parent/child
structure read as coupling.

**If you find the change impossible without it, stop and report** rather than widening the definition.
That would be a genuine finding about the design, and it is mine to rule on.

## 4. REQUIRED — measure, do not remediate (§6c.2)

Splitting concatenated nodes **will expose cycles that exist today and are invisible**. The gate will
start failing on debt it could not previously see.

**Report the count and the shape. Populate no allowlist entry. Fix no cycle. Move no module.**

- How many cycles does the gate report after the change that it did not before?
- What shape are they — mostly within one former top-level module, or across?
- How many nodes does the graph have now, against the previous top-level count?

**The response depends on the number and the number is not knowable from reading.** A handful is a
ledger entry each, in RFC 130 §4b's existing per-cycle idiom. Dozens reshapes the work, because an
allowlist long enough to skim is a gate nobody reads. **I rule after the number.**

**So this round is expected to leave the gate failing**, with the failure enumerated in your report.
Say plainly whether `boundary-check` passes or fails at your final commit — **if it fails, that is the
expected outcome, not a broken round**, and the ten-gate set's `boundary-check` line should record it as
such rather than being worked around.

## 5. What this round must NOT do

- **Do not lift §6a's prohibition** and do not move, group, or rename any module in `prikk-store`.
  §6c.3: this round *enables* the lift; RFC 131 performs it on evidence, later.
- **Do not narrow any visibility.** No `pub(in crate::…)` anywhere. That is §3's work and it comes after.
- Do not change what an edge is (§3 above).
- Do not touch the hub-degree rule, the SCC detection algorithm, or `boundary.rs`'s member allowlist.
- `MILESTONES.md` untouched.

## 6. Controls

1. **A cycle inside one former top-level module is now visible** — construct two modules under one
   parent referencing each other by `crate::` path, and assert the gate reports the cycle. **This is the
   whole point of the round**: today that cycle is invisible.
2. **Resolution picks the deepest existing module** — `crate::a::b::C` reaches `a::b`, not `a`.
3. **A path whose deeper segments are not modules still reaches its module** — `crate::a::Item` reaches
   `a`.
4. **Grouped imports resolve per element**, each to its own deepest node.
5. **A previously-reported cycle is still reported** — a regression guard: the change must not *lose*
   an edge it used to find.

**Perturb control 5.** It is the one whose passing is most easily vacuous, and losing an existing edge
is the failure that would make the gate quietly weaker while looking more precise.

## 7. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, **with `boundary-check`'s real result
reported whatever it is** (§4). State rule 9's cross-target result, or that it does not apply and why.

The hardcoded production-module count in `graph/tests.rs` counts top-level modules today; its meaning
changes. **Update it deliberately and say what it now counts.**

No `CHANGELOG.md` entry — this is an internal gate, not user-visible surface. Say so rather than
leaving it unmentioned.

Report to `.git-exclude/review-request/`. **Lead with §4's three numbers.**
