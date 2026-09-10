# RFC 131 — Grouping `prikk-store`'s 123 top-level entries, and scoping what may reach what

**Status.** **ACCEPTED by the project owner 2026-09-01**, together with RFC 130 — including §6's
sequencing: **after RFC 130's gate, and between feature arcs rather than during one.** Acceptance is
of the direction and the constraints; §2's central design question (role-based versus name-based
grouping, and what to do with the 38 singletons) is explicitly still open and is the implementing
increment's work.

**Folder corrected 2026-09-05: `proposed/` → `accepted/`.** Owner-accepted 2026-09-01 with a
settled design and a stated sequencing constraint (after RFC 130, between feature arcs), so an
implementer may start. Caught by the project owner.

**SUPERSEDED 2026-09-05** — this said the RFC stays in `rfcs/proposed/` until implemented. RFC-000 makes folder location lifecycle authority and `accepted/` means an implementer may start, which is true here; see RFC 130's corrected status note for the full reasoning and for the RFC 120 gap the old arrangement was compensating for.

Same origin as RFC 130 (owner question 2026-09-01; external architect
review; architect review of it). **RFC 130 carries the shared evidence and should land first — see
that RFC's §7 and this one's §6.**

**Tracks.** Readability of `prikk-store`'s source tree, and compiler-enforced reach between its
modules. No product behaviour changes.

---

## 1. The two costs this addresses, and the one it does not

The owner named three: **side-effect predictability, readability, and compile time.**

| Cost | This RFC |
|---|---|
| **Readability** | **Directly.** 123 top-level entries in one directory is the concrete complaint, and grouping is the honest fix |
| **Side-effect predictability** | **Partly, and only with §3.** Grouping alone changes *nothing* about reach — `pub(crate)` still spans the whole crate. The lever is `pub(in ...)`, of which the crate has **zero** uses against **641 `pub(crate)`** |
| **Compile time** | **Not at all**, and it needs nothing — see RFC 130 §2.4 |

**Grouping without §3 is the failure the external review named explicitly**: work that looks like
progress on predictability while changing nothing about it.

## 2. Grouping — and why it is not a rename

The owner's sketch was `src/{patch,worktree,…}_* → src/{patch,worktree,…}/*`. Measured against the
tree: of the **69 distinct modules** (123 entries, since many exist as both `foo.rs` and `foo/`),
**31 sit in a multi-member name family**:

`patch` (6), `worktree` (4), `trust` (3), `rollback` (3), `rfc111` (3), plus six two-member pairs
(`signature`, `received`, `node`, `merge`, `format`, `author`).

**The other 38 are singletons** — `wal`, `index`, `layout`, `seal_from_accepted`, `compact`,
`text_span`, `doctor`, `container`, `snapshot`, and so on. **So a grouping derived from name prefixes
places under half the crate**, and the real work is deciding groups for the rest.

**Constraint on that decision:** do not invent a home for a module that genuinely has none. A
singleton left at the top level is honest; a group of one, or a group whose members share nothing but
the need to be somewhere, makes the listing longer to read rather than shorter.

**Two groupings the coupling graph suggests but does not settle**, offered as starting material:

- The **foundation** (`layout`, `fsutil`, `byte_cursor`, `file_codec`, `frame_resync`, `container`,
  `index`, `generation`) — RFC 130 §2.3's wide, one-directional base. These share a role, not a name.
- The **orchestrators** (`verify`, `sync_negotiation`, `patch_exchange`, `seal_from_accepted`,
  `worktree_patch`) — high fan-out, near-zero fan-in.

Whether role-based grouping beats name-based grouping is the RFC's central design question and is
**not** settled here.

## 3. Visibility scoping — the half that changes the property

Today: **641 `pub(crate)`, 271 `pub(super)`, 0 `pub(in crate::<path>)`.** The crate's default answer
to "who may reach this?" is *everyone in the crate*, 641 times.

**Introduce `pub(in crate::<group>)` where the coupling graph says reach should be narrow**, using
each group from §2 as the natural scope. This attacks side-effect predictability head-on at **zero
crate-count cost, zero published-API cost, zero release-procedure cost** — and it is reversible one
item at a time.

**Start where the cost is real, not where it is easy:** RFC 130 §2.2's four middle-hubs (`refs`,
`patch_replay`, `wal`, `lifecycle_cache`) and the `active ↔ refs` cycle. Those five are where a change
propagates in both directions; the wide foundation (`layout`, `fsutil`) is high-impact but
*one-directional*, and narrowing it would be work without a matching benefit.

**Expect the layering to resist in places.** The external review named this as the likely failure
mode and it is a mild one: where a `pub(in ...)` cannot be narrowed without a cascade, that is
information about the true coupling. **Record what resisted rather than widening the scope silently
back to `pub(crate)`** — the resisting cases are the ones RFC 130's gate exists to watch.

## 4. What this must not do

- **No crate split.** RFC 130 §6 rules it out on evidence, including `fsutil`, the one clean seam.
- **No change to the crate's outward API.** `prikk-store`'s curated `pub use` surface is what the
  2026-08-31 audit praised; this RFC narrows *internal* reach and must not widen the external surface
  by even one item.
- **No relocation of tests away from the code they exercise.** 34,883 of the crate's 74,687 lines are
  tests living beside their subject, deliberately.
- **No reflow of the long provenance comments.** The external auditor recorded that they made the
  audit *"move twice as fast"*; a file move must carry them intact, and a move that splits a comment
  from what it explains is a regression this RFC is answerable for.

## 5. Why the diff will be large and what that costs

A file move touching most of a 253-file crate is a large diff over code that is otherwise stable.
Two consequences to plan for rather than discover:

1. **It collides with in-flight work.** RFC 123 (schema-3 authoring) and RFC 125 (decoder hardening)
   edit modules this would move. **Sequencing is a real constraint, not a preference** (§6).
2. **`git blame` and provenance.** Moves should be pure moves, in their own commit, separate from any
   content change, so history stays followable through them.

## 6. Sequencing

1. **RFC 130's gate lands first** — the invariant should be checked before the layout moves, or this
   RFC is the "cheap failure" of §1.
2. **This RFC lands between feature arcs, not during one.** Band 1 of the audit's corrective program
   (RFCs 127, 121-EPIPE, 122, 128) and then RFC 125 are in flight; a whole-crate file move during
   them buys a merge conflict for every one.
3. **Grouping and visibility can be separate increments**, and probably should be: the moves are
   mechanical and reviewable by inspection, while each `pub(in ...)` narrowing is a claim about who
   should reach what.

## 6a. RULED 2026-09-08 — the constraint the coupling gate imposes, which this RFC predates

**Handoff issued:** `rfcs/handoffs/131-module-grouping-and-visibility-scoping/grouping-and-scoping-handoff-v1.md`.

**This RFC was written before RFC 130's gate existed, and grouping turns out not to be neutral with
respect to it.** The gate's graph node is the **top-level `mod` declared in `lib.rs`**
(`coupling/graph.rs:334`), and every descendant file's text is concatenated into that same node
(`:363`). There is no qualified naming.

**So grouping merges nodes** — and a cycle wholly inside one group **disappears from the graph**. The
gate would report no cycle, not because the coupling was resolved but because the grouping hid it.
**RFC 130's ledger of structural debt could be erased by work whose stated purpose is to reduce
coupling**, and the gate would call it green.

**RULED: no two of these seven may share a group** — the six-module SCC plus the one hub outside it,
taken from the gate rather than from RFC 130 §2.2's superseded table:
`active`, `lifecycle_cache`, `patch_replay`, `refs`, `trust`, `worktree_patch`, and `wal`.
Each may have its own directory; they may not be collected together. **Teaching the gate qualified
names is the principled alternative and is out of scope** — it changes RFC 130's gate and deserves its
own decision, not a side effect of a file move.

### 6a.1 §2's central question, part-ruled and part-delegated

**Two groupings the data now settles**, which it did not on 2026-09-01:

- **The eight `#[cfg(test)]` modules** (`lib.rs:84-99`) are gates, evidence harnesses and fixtures, and
  `lib.rs` already declares them as one contiguous block. **A directory documents a fact rather than
  inventing one**, and removes eight entries from the listing with no design argument. It does **not**
  violate §4 — that constraint protects `foo/tests.rs` beside `foo.rs`, and these are standalone.
  (`rfc111_seal_simulation` is production, `lib.rs:63`, and stays.)
- **The eleven name families**, per the owner's own sketch.

**The remaining ~30 singletons stay delegated**, with §2's own constraint governing: a singleton left
at the top level is honest, and a group whose members share only the need to be somewhere is worse
than none.

### 6a.2 Two corrections to this RFC's own numbers and citations

- **§2/§3's counts are a week stale.** Re-derived at `7cd06ab`: **125** top-level entries (was 123),
  **679** `pub(crate)` (was 641), 273 `pub(super)`, and still **zero** `pub(in crate::…)`. **The
  `pub(crate)` count rose by 38 in the week this RFC waited** — the problem is getting worse, not
  holding still.
- **§3's "four middle-hubs" is one short.** `active` is now a declared hub in its own right, not merely
  one end of the `active ↔ refs` cycle.

## 6b. DELIVERED 2026-09-08 (`544cc6c`, `4acd7e8`, `971e664`) — and §6a's ruling was too narrow

**125 → 100 top-level entries, 69 → 52 modules, and the first 27 `pub(in crate::…)` in the crate's
history.** §1's readability complaint is materially addressed.

### 6b.1 CORRECTED — merge-induced cycles are not limited to the constrained seven

**§6a ruled that no two of the constrained seven may share a group. That was right and insufficient.**
Merging *any* two modules collapses their distinct edge targets onto one node, and a cycle can appear
between modules none of which is constrained. Verified against the real edge set:

```
patch_exchange -> tag_travel ;  tag_travel -> patch_set_digest
   separate : three nodes, a path, no cycle
   merged   : patch -> tag_travel -> patch      == a 2-cycle, from consolidation alone
```

**Merging all seven name families produced 13 undeclared cycle-forming edges**, and four families
(`patch`, `worktree`, `rollback`, `merge`) were left ungrouped rather than declared as debt.

**RULED: the constraint generalizes. A grouping may not create a coupling-graph cycle that did not
exist before it**, whether or not a constrained module is involved. §2's own principle extends to a
case it did not have in mind: **a group whose members' combined reach is worse than their separate
reach is worse than no group.** Declaring new cycles to buy filesystem tidiness inverts what RFC 130
and this RFC exist to do.

### 6b.2 RULED — §6a's own ruling forecloses §3's stated target, and the gate is now the blocker

**`pub(in crate::<path>)` requires `path` to be an ancestor of the item.** The constrained seven sit
directly under the crate root, and **§6a forbids giving them a shared parent.** So an item declared
*directly* in `refs.rs`, `active.rs`, `wal.rs`, `patch_replay.rs` or `lifecycle_cache.rs` **has no
expressible scope narrower than `pub(crate)` at all** — and §3 named exactly those modules as where to
start.

**Neither half saw this**: §3 predates the coupling gate, and §6a predates anyone trying to narrow
against it.

**Consequence: teaching the coupling gate qualified module names is no longer "the principled
alternative" (§6a) — it is the prerequisite for §3's stated goal.** Until the gate can name
`group::module`, the SCC and hub edges §3 exists to narrow cannot be narrowed at all. **That is the
next decision this RFC needs, and it belongs to RFC 130's gate.**

### 6c. DESIGNED 2026-09-10 — qualified module names, and the two rulings that bound the change

§6b.2 posed this as *"the next decision this RFC needs"* and said it *"belongs to RFC 130's gate."*
**RFC 130 is closed, so the design is recorded here** — in the RFC the gap actually blocks — and
cross-referenced from RFC 130 §8. It amends that gate's behaviour and nothing else about RFC 130.

**The mechanism, verified at source before ruling.** `walk` (`coupling/graph.rs:377-401`) builds a
`BTreeMap<String, String>` keyed by the **top-level `mod` in `lib.rs`**, and
`collect_production_text` recursively concatenates **every descendant file's text into that same
value**. Edges come from *"every `crate::<ident>` occurrence"* — **the first segment only**, which is
precisely why a node is a top-level module and can be nothing else.

**RULED: a node becomes every production module at every depth, keyed by its qualified path from the
crate root.** `foundation::layout` is a node distinct from `foundation::fsutil`, and a module's text
is its own file's, not its descendants'.

#### 6c.1 RULED — the edge *vocabulary* does not change, only its resolution

**An edge stays "a `crate::` path reference", exactly as today.** The change is that the path resolves
to the **deepest existing module node** rather than its first segment: `crate::a::b::C` reaches node
`a::b` when `a::b` is a module, and node `a` when it is not.

**Do not add `super::`, `self::`, or bare-path scanning.** Those are invisible to the gate today, and
making them visible changes **what an edge means**, not how precisely it is named. It would also make
every `foo/bar.rs` that reaches back through `super::` an edge to `foo` — turning ordinary parent/child
structure into reported coupling, which is not what §3 is trying to narrow.

**That is a separate decision and does not ride along.** If it is ever wanted, it is its own round with
its own measurement.

The grouped-import expander (`crate::{a, b::c}`) must be extended in step: it currently reduces each
element to a first segment, and must preserve full paths for the same resolution to apply.

#### 6c.2 RULED — measure what qualified naming reveals before deciding what to do about it

**Splitting concatenated nodes will expose cycles that exist today and are invisible** — two modules
under one top-level parent that reference each other by `crate::` path are one node now, and two nodes
after. **The gate will start failing on debt it previously could not see.**

**RULED: the implementing round reports the count and shape and populates no allowlist and fixes no
cycle.** RFC 130 §4b already made acyclicity *"an allowlist ... a ledger of structural debt rather than
a list of permanent excuses"*, with **per-cycle entries**, so recording newly-visible debt is the
established idiom — **but how much there is changes what the right response is**, and that number is not
knowable from reading.

A handful is a ledger entry each. Dozens is a finding that reshapes the work, because **an allowlist
long enough to skim is a gate nobody reads.** The ruling comes after the number, not before it — the
same sequencing that paid for itself in RFC 133 §6c.2.

#### 6c.3 §6a's prohibition is NOT lifted by this round

§6a forbids any two of the constrained seven sharing a group **because grouping merges nodes and hides
cycles**. This round removes that mechanism — but the prohibition is lifted by RFC 131, on evidence that
the gate no longer hides a cycle inside a group, **not as a side effect of the gate changing**.

**This round enables the lift. It does not perform it**, and must not move any module.

### 6c.4 CORRECTED 2026-09-10 — §6c.1 made the gate blind, and the SCC did not dissolve

**Qualified naming delivered at `42bcab15`: 53 → 127 nodes, longest-prefix resolution, grouped imports
per element, edge vocabulary untouched.** The mechanism is accepted and is not reopened.

**Its measurement reported zero cycles and concluded the declared debt "was never real coupling, only
imprecise attribution." That conclusion does not stand.** Two pairs traced by hand at review:

- `refs/evidence.rs:7` → `crate::active::{…}`; `active.rs:22` → `crate::refs::{…}`
- `lifecycle_cache/replay.rs`, `replay/effect.rs` → `crate::patch_replay`; `patch_replay.rs` →
  `crate::lifecycle_cache`

**In both, the two subtrees mutually depend.** That is a cycle in the only sense RFC 130 cares about,
and nothing about it has changed. **The new graph misses it because nothing connects `refs` to
`refs::evidence`** — containment is unmodelled, so a dependency that leaves a subtree through a child
and returns to the subtree's root is invisible.

**This is §6a's own failure mode in a new form** — *"the gate would report no cycle, not because the
coupling was resolved but because the grouping hid it"* — arrived at by implementing §6c.1 faithfully.

**Naive containment edges are also wrong, measured before proposing them:** 35 files across 9 parents
reference their own parent by absolute `crate::` path (`foundation` 14, `test_gates` 7, `patch_algebra`
5, `refs` 4, …). Parent→child containment plus those child→ancestor text edges makes a 2-cycle out of
ordinary intra-subtree access written absolutely instead of with `super::`.

**RULED, superseding §6c.1's silence on this:**

1. **Keep qualified nodes** — they are what makes `pub(in crate::…)` targetable and stop grouping from
   hiding anything.
2. **Compute cycles over subtrees, not raw nodes**: a cycle exists between A and B when *any* descendant
   of A depends on *any* descendant of B and vice versa. This restores the question the old gate
   answered correctly, without its fixed top-level granularity.
3. **Report at the smallest pair of subtrees exhibiting it** — `refs ↔ active` here, but *derived*
   rather than an artifact, so a future narrowing changes the reported pair instead of silently keeping
   it.
4. **An edge between a node and its own ancestor or descendant is not coupling** and must not
   contribute. That removes the 35-file noise without losing the real cycles.

**The allowlist decision §6c.2 reserved is not made**: zero is the count of *raw node* cycles, and the
subtree count is unmeasured. **Hold all 8 `DECLARED_CYCLES` entries, both stale hubs, and the 3 new
undeclared hubs** until it is known.

### 6c.5 RULED 2026-09-10 — generalise from pairs to subtree SCCs, the third blindness in this arc

**Subtree-aware detection delivered at `74e6edc2`** and accepted. Four subtree cycles: `active↔refs`,
`active↔worktree_patch`, `refs↔trust`, `lifecycle_cache::replay↔patch_replay`. Rule 4 is enforced
*inside* `subtree_depends`, so an ancestor/descendant pair is structurally unreportable.

**§6c.4's own table was one level too shallow, and the round re-derived it.** `lifecycle_cache.rs`
never writes `crate::patch_replay` (0 occurrences); `patch_replay.rs` reaches
`crate::lifecycle_cache::replay::…` and `::incremental::…`. The smallest pair is
**`lifecycle_cache::replay ↔ patch_replay`**. §6c.4 rule 3 gave `refs`/`active` a worked derivation and
did not give this pair the same rigour; the round noticed the asymmetry rather than matching the wording.

**Rule 2's pairwise form cannot express a cycle that closes through a third node.** Verified:
`worktree_patch → patch_replay` has **no return edge** — nothing in `patch_replay` reaches
`worktree_patch` — so the cycle closes through `active`. Four declared entries (5 edges) are real
coupling the mechanism cannot name.

**A 3-node cycle is real coupling**: A→B→C→A means none is independently extractable, and RFC 130's
original component was *six* modules precisely because it is a strongly connected component, not a set
of pairs.

**RULED: generalise rule 2 from pairs to strongly connected components over `subtree_depends`**, keeping
rule 3's smallest-granularity reporting and rule 4's exclusion unchanged.

### 6c.6 The pattern: three blindnesses, each revealed by fixing the last, each reporting *fewer* cycles

| # | The gate could not see | Fixed by |
|---|---|---|
| 1 | a cycle wholly inside a group (§6a) | qualified nodes |
| 2 | a cycle leaving a subtree through a child (§6c.4) | subtree-aware `depends` |
| 3 | a cycle closing through a third node (§6c.5) | SCC over subtrees |

**Every time, the gate reported fewer cycles than reality, and the cleaner number looked like progress.**

**RULED as a standing caution for this gate: a precision change that reduces the number of findings is
suspect until two of the vanished findings are hand-traced against the source.** That trace is cheap —
two greps — and it is the only thing that has distinguished *resolved* from *invisible* on all three
occasions.

**The allowlist stays held.** 3 entries confirmed, 1 needs renaming
(`lifecycle_cache↔patch_replay` → `lifecycle_cache::replay↔patch_replay`), 4 pend §6c.5. Nothing is
added, removed or renamed until the generalisation lands.

### 6c.7 RULED 2026-09-10 — the allowlist decision, deferred three times and now made

**The SCC generalisation at `cc15e616` reproduces RFC 130's own original measurement**: one component,
**six members, thirteen directed edges**, against RFC 130's line 16 — *"8 `DECLARED_CYCLES` entries
covering 13 directed edges across a six-module"* component. **That figure predates all three of this
arc's mechanisms**, so reproducing it is independent evidence the arc has landed on the truth rather
than on a fourth blindness. The two intermediate mechanisms reported 8 edges and 0.

**CYCLES — all eight entries stand. One rename, applied everywhere the bare name appears.**

`lifecycle_cache` → **`lifecycle_cache::replay`** in every `DECLARED_CYCLES` mention (the
`lifecycle_cache↔patch_replay` entry and `worktree_patch->lifecycle_cache`). `lifecycle_cache.rs` itself
writes no component edge; `patch_replay.rs` reaches `crate::lifecycle_cache::replay::…`. **No entry is
removed, and no reason text changes** — the coupling is the same coupling, named where it actually lives.

**HUBS — the arithmetic settles all five, since `HUB_THRESHOLD` is `min(fan_in, fan_out) >= 6`.**

- **Remove `refs` (28/4, min 4) and `lifecycle_cache` (3/2, min 2).** Neither is a hub. **`refs` with
  fan-in 28 is a sink, not a middle hub**, and the threshold's `min()` form is deliberately about
  bidirectional traffic — so removal loses no information that the threshold was ever measuring.
- **Declare `active` (7/6), `wal` (13/6), `author::author_key_index` (6/6).**

**`active` and `wal` are a recorded artifact returning, not new debt.** `coupling.rs:56-61` already
states both *"dropped out of the declared set at RFC 131 §2.2a's `foundation` grouping … a real
consolidation effect rather than a code change to either module"* — `wal`'s min fell 6→2, `active`'s
6→5. **Qualified naming undoes precisely that collapse**, so each recovers its own previous declared
reason from git history rather than receiving a newly-invented one.

**`author::author_key_index` is genuinely newly visible** — a submodule no mechanism before
`42bcab15` could name. Its reason and its *what-would-remove-it* must be written from its actual edge
list, which is work for whoever has that list in front of them; I will review the reason, not supply it
from here.

**§6a's prohibition may now be lifted** — the gate can see a cycle wholly inside a group, a cycle
leaving a subtree through a child, and a cycle closing through a third node. **That is RFC 131's own
next decision and is not part of closing the allowlist.**

### 6c.8 CLOSED 2026-09-10 — the gate is green, and §6a's prohibition is now liftable

**Reconciled at `ba6f1e76`.** `boundary-check` → `{"valid": true, "errors": []}`; 1824 tests pass, zero
failures. **First green `boundary-check` in the arc** — the four preceding rounds each ended
deliberately red.

One rename (`lifecycle_cache` → `lifecycle_cache::replay`, all three edge tuples), all eight cycle
entries standing with no reason text altered, and the hub arithmetic applied exactly: `refs` (min 4) and
`lifecycle_cache` (min 2) removed, `active`/`wal`/`author::author_key_index` (min 6 each) declared.

**`active` and `wal` recovered their reasons verbatim from `4acd7e8a`'s minus side** — checked
byte-for-byte at review. That mattered: §6c.7's ruling turned on these being *the same debt returning*,
and a reworded reason would have quietly made it new.

**The edge count is machine-verified, not counted.** `check()` compares the allowlist against
`subtree_cycles()` exhaustively in both directions, so `valid: true` *is* the proof that eight entries
cover exactly thirteen edges.

### 6c.9 The arc, and what it cost to get right

| Round | Mechanism | Cycles reported |
|---|---|---|
| `42bcab15` | qualified nodes, raw-node cycles | **0** |
| `74e6edc2` | subtree-aware pairs | 8 edges |
| `cc15e616` | SCC over subtrees | **13 edges, six modules** |
| `ba6f1e76` | allowlist reconciled | gate green |

**The final figure independently reproduces RFC 130's own original measurement**, taken before any of
these mechanisms existed and fitted to by nobody.

**Three blindnesses, each revealed only by fixing the previous one, each having made the gate report
*less* than reality.** Every intermediate number looked like progress. §6c.6's standing caution exists so
a fourth is caught by the same two greps that caught the first three.

**§6a's prohibition is now liftable.** The gate sees a cycle wholly inside a group, one leaving a subtree
through a child, and one closing through a third node — the three mechanisms §6a's "no two of the
constrained seven may share a group" was protecting against. **Lifting it is this RFC's own next
decision**, on evidence, and was deliberately excluded from closing the allowlist.

**And §3's original target is unblocked.** `pub(in crate::<path>)` was inexpressible for the constrained
seven because §6a forbade them a shared parent; that prohibition now has no mechanical reason to stand.

## 6d. RULED 2026-09-10 — §6a's prohibition is LIFTED, on evidence

**§6a forbade any two of the constrained seven sharing a group** — `active`, `lifecycle_cache`,
`patch_replay`, `refs`, `trust`, `worktree_patch`, `wal` — for one stated reason:

> *"grouping merges nodes — and a cycle wholly inside one group **disappears from the graph**. The gate
> would report no cycle, not because the coupling was resolved but because the grouping hid it."*

**That mechanism no longer exists, and a production-path control proves it rather than an argument
about the code.** `graph::tests::subtree_control4_a_cycle_inside_one_former_top_level_module_is_still_
visible` builds two siblings under a shared parent that reference each other, and asserts
**`subtree_cycles()`** — the mechanism `check()` actually calls — reports **both** directed edges
`parent::child_a ↔ parent::child_b`. A companion control asserts the same through the raw graph.

**Rule 4 does not suppress it**, which is the detail worth stating: siblings under a common parent are
not ancestor/descendant of each other, so the *"an edge to a node's own ancestor or descendant
contributes nothing"* exclusion never applies to the pair grouping would create.

**RULED: §6a's prohibition is lifted.** The seven may be grouped. §6a's other clause — *"each may have
its own directory"* — was never in question and stands.

### 6d.1 Two consequences to plan for, not discover

**Grouping renames allowlist entries.** `refs` under a parent `g` becomes `g::refs`, and every
`DECLARED_CYCLES`/`DECLARED_HUBS` mention must move with it — exactly the `lifecycle_cache` →
`lifecycle_cache::replay` rename §6c.7 just performed, and for the same reason: the coupling is
unchanged, its name moves to where the module now lives. **A grouping round that leaves the allowlist
alone will turn the gate red**, correctly.

**§4's constraint still binds.** It protects `foo/tests.rs` beside `foo.rs`; grouping must not collapse
that arrangement, and §6a.1's ruling that a group whose members *"share only the need to be somewhere is
worse than none"* is not weakened by this lift. **A lifted prohibition is not an instruction to group.**

### 6d.2 §3's target is now mechanically reachable, and should be proven on one pair first

§6b.2 established that an item declared directly in one of the seven **has no expressible scope narrower
than `pub(crate)`**, because `pub(in crate::<path>)` needs an ancestor and §6a forbade them a shared one.
With the lift, a shared parent is available and `pub(in crate::<parent>)` becomes expressible.

**RULED: prove it on one pair before grouping seven.** Grouping all seven means moving seven modules and
their subtrees and rewriting every `crate::<name>` reference crate-wide — hundreds of sites — for a
payoff that is per-item and unmeasured. **One pair, one real narrowing, and the cost reported** is the
step that makes the rest a decision rather than a commitment. The same measure-first sequencing that
this RFC's own §6c arc and RFC 133 §6c.2 both paid for.

### 6d.3 DELIVERED and PARTLY CORRECTED 2026-09-10 — the lift holds on real modules; the yield was understated fivefold

**`active` and `worktree_patch` are grouped under `commit_boundary` at `ec59aba7`**, and §6d's lift is
re-proven on real code: `subtree_cycles()` reports
`commit_boundary::active ↔ commit_boundary::worktree_patch`, and breaking the one real mutual reference
makes the gate report both edges stale by name with nothing false in their place. **Gate green.**
`active_patch_limit_exceeded` is now `pub(in crate::commit_boundary)` — **the crate's first
`pub(in crate::…)` narrowing**, verified by `error[E0603]` on a probe call from outside the parent.

**A latent gate bug was found and fixed in passing:** `reexports()` captured only the first segment of a
`pub use` path, never updated when `resolve_target` gained full-path capture at `42bcab15`. Latent since
§2.2a, harmless until a multi-segment `pub use` existed — and this round created the first one. It would
have bitten every future grouping.

**CORRECTED — the eligibility measurement.** The round reported **1 of 7** items eligible and that the
other six *"each have at least one real external caller."* Checked at review: **five were eligible.**

| Item | Actual |
|---|---|
| `active_patch_limit_exceeded` | eligible (narrowed) |
| `commit_worktree_changes_with_generator` | **eligible** — used only inside `commit_boundary` |
| `next_op_seq` | **eligible** — used only inside `commit_boundary` |
| `AuthorError` | **eligible** — zero mentions outside |
| `author_worktree_patch` | **eligible** — both cited callers are **comments** (`rollback_draft.rs:122`, `patch_replay.rs:429`) |
| `read_active_ref_metadata_for` | ineligible — real call, `verify.rs:1507` |
| `prepare_empty_active_ref_for_append` | ineligible — real call, `rollback_draft.rs:177` |

**The cause is the method: a raw crate-wide grep per item cannot tell a call from a prose mention.** And
the gate's own scanner runs `classify`/`blank` to blank comments before scanning **for exactly this
reason** — the audit used the naive method the gate itself exists to avoid.

**So the yield for a two-module group is 71%, not 14%**, which strengthens the case for grouping rather
than weakening it — the opposite direction from the round's own conclusion, and the number §6d.2 exists
to produce.

**RULED: re-audit comment-aware, narrow what is genuinely eligible, and re-report the yield before any
decision about the remaining five.** The denominator deserves the same item-level rigour as the numerator.

### 6d.4 RULED 2026-09-10 — the cheap half is the bigger half: submodule narrowings need no grouping

**§6d.3's re-audit closed at `fc38e853`**: all five eligible items narrowed, compiler-verified, with both
ineligible items actually narrowed, broken at their real callers, and reverted — the negative control run
rather than asserted.

**Two corrections, one of them mine.** The 8th `pub(crate)` occurrence I flagged in the denominator is
`commit_boundary/active.rs:113`, a **doc comment** mentioning the literal string. I criticised a raw grep
for counting comments as code and then used one that did. Their item-level 7 was correct.

**And the yield splits in two, which the round volunteered against its own headline.** `node_authoring`
is declared `mod node_authoring;` — private — so `AuthorError` and `author_worktree_patch` were already
unreachable from outside `worktree_patch`:

- **Declared-ceiling tightening: 5 of 7 (71%).**
- **Actual access reduction: 3 of 7 (43%).**

Both matter. The two already-walled items gain **preventive** value — a later `pub(crate) mod
node_authoring;` would re-expose them unless pinned — but a declaration that reduces no present access is
bookkeeping, and §3's goal is coupling. **The two numbers must not be conflated.**

#### 6d.4a The measurement that redirects the work

**§6b.2's constraint was precise and I under-read my own words: it binds items declared *directly* in one
of the seven.** An item in a **submodule** already has an ancestor — `pub(in crate::worktree_patch)` is
expressible today for anything under `worktree_patch`, **with no grouping at all.**

Measured across the remaining five:

| module | top-level | **submodule** | already `pub(in …)` |
|---|---|---|---|
| `lifecycle_cache` | 9 | **23** | 0 |
| `patch_replay` | 15 | **19** | 0 |
| `refs` | 10 | 14 | 32 |
| `wal` | 1 | 0 | 0 |

**The submodule half is larger than the top-level half and costs nothing structural** — no file moves, no
reference rewrites, no allowlist renames, no gate risk. Grouping one pair cost 7 moves, 21 rewrites, 6
allowlist renames and surfaced a latent `reexports()` bug, and it buys access to the **smaller** half.

`refs`' 32 existing `pub(in …)` are §6b.3's own narrowings — **the method is already proven; it was never
extended past `refs`.**

**RULED: take the submodule narrowings first, one module at a time, and measure their eligibility rate.
Defer all further grouping until that rate is known.** Grouping remains available (§6d) and unprohibited;
it is simply no longer the first move.

### 6d.5 CORRECTED and RULED 2026-09-10 — a re-export is not a grouping problem, and my restriction said it was

**`lifecycle_cache` narrowing delivered at `a1ae661a`: 6 of 23**, with both yields identical (6 declared,
6 actual) because `replay` is `pub(crate) mod`, not private — nothing was pre-walled.

**7 are genuinely ineligible**, compiler-confirmed at their real callers. **10 were blocked by a
restriction I wrote.**

§6d.4's handoff said *"do not narrow anything declared directly in `lifecycle_cache.rs` — that one needs
grouping."* **A `pub(crate) use` re-export is declared there and does not need grouping.** Grouping is
required when an item's *callers* sit in a sibling module; a re-export requires only that its *consumers*
sit inside the module. **I conflated where a declaration lives with why it is blocked.**

**The structural fact underneath, verified:** `mod cache_ladder;` and `mod store_resolvers;` are
**private**, yet `pub(crate) use cache_ladder::{…}` and `pub(crate) use store_resolvers::…`
**republish their items crate-wide through a separate path** — so the private `mod` does **no walling**
for those ten. Narrowing an origin alone fails with `E0364`/`E0365`, quoted verbatim by the round after
narrowing all ten at once.

**This is the same class as the `reexports()` gate bug fixed at `ec59aba7`**: declared visibility
diverging from actual reachability — once in the gate's model of the crate, once in the crate itself.

**All ten are narrowable, audited by name rather than sampled**: zero have a consumer outside
`lifecycle_cache`, the one apparent hit being a `//!` doc comment at `wal/tests/proptest_framing.rs:6`.
**So the ceiling is exactly 16 of 23 (70%), not 26%** — reached by editing a re-export, not by moving a
module.

**RECORDED, and missed by both the round and this section's first draft: nine of the ten are
`#[cfg(test)]`.** `mod cache_ladder;` and its re-export both carry `#[cfg(test)]`, and the file declares
itself test-only scaffolding; `store_resolvers` does not. **So the remaining win is 9 items of test-scope
hygiene plus 1 production narrowing** — `StoreBackedResolver`, which the code annotates as the crate's E1
store-access boundary, making it the one that matters. **Against production items the figures are 7 of
14, not 16 of 23**; both belong in the record, and quoting only the larger overstates the production win
ninefold.

**RULED: narrowing a `pub(crate) use` re-export is in scope for a narrowing round**, and a narrowing
round must report production and test-only yields separately — a `#[cfg(test)]` module's items are not
an access-surface reduction in the shipped artifact. Both ends move together — the
origin item and the re-export that republishes it — and eligibility is judged on the re-export's
consumers. **Items declared directly in a module's own top-level file that are not
re-exports remain deferred to grouping**, which is what §6b.2 actually constrains.

### 6b.3 What the round delivered under those constraints

- **The eight `#[cfg(test)]` modules → `test_gates/`**, eight declarations to one, zero graph impact.
- **Five groups formed** (`foundation`, `author`, `node`, `received`, plus the test group), each
  verified to create no new cycle.
- **`active` and `wal` retired from `DECLARED_HUBS`** — dropped below threshold by `foundation`'s
  consolidation, **with the cause recorded in the constant's own doc comment.** A hub retirement caused
  by consolidation rather than decoupling could easily have been reported as an improvement; it was not.
- **The SCC is untouched** — `the_scc_has_exactly_this_edge_set` still pins 8 entries, 13 edges, six
  modules.
- **27 `pub(in crate::refs)`**, one module's internal machinery, with four resisting cases recorded
  rather than widened back.

### 6b.4 A cost this RFC now owes RFC 130

RFC 130 §6 argued against a crate split partly on `fsutil` being *"the one genuinely clean seam, 0
production out-edges"*. **After grouping, `fsutil`'s own figure is no longer separately checkable** —
only `foundation`'s aggregate. The conclusion holds; the evidence is one level less granular, and the
implementing round disclosed this against its own result.

## 7. Revisit triggers

Inherited from RFC 130 §6, restated because they bound this RFC too: **watch coupling, not lines.** A
grouping that has to be redone because the code evolved differently is a cheap, reversible failure —
the external review's own assessment, and the reason this is the recommended direction rather than a
crate split.

## 8. Non-goals

No crate split. No compile-time work. No change to `prikk-object`, `prikk-cli`, or any other member.
No new dependency. No renaming of modules — grouping moves files, it does not rename what they
contain.
