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

## 7. Revisit triggers

Inherited from RFC 130 §6, restated because they bound this RFC too: **watch coupling, not lines.** A
grouping that has to be redone because the code evolved differently is a cheap, reversible failure —
the external review's own assessment, and the reason this is the recommended direction rather than a
crate split.

## 8. Non-goals

No crate split. No compile-time work. No change to `prikk-object`, `prikk-cli`, or any other member.
No new dependency. No renaming of modules — grouping moves files, it does not rename what they
contain.
