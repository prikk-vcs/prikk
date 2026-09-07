# RFC 131 — grouping `prikk-store`, and scoping what may reach what

**RFC:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` — accepted 2026-09-01 for its
**direction and constraints**; §2's central design question was left open to the implementing
increment. **This handoff rules the part the data now settles and delegates the rest, deliberately.**
**Base:** `main` at `7cd06ab`.

**§2 is the part to read before planning anything.** Grouping is not neutral with respect to RFC 130's
coupling gate — done carelessly it would erase the gate's entire ledger while reporting success.

---

## 1. The numbers, re-derived at `7cd06ab` — RFC 131's own are a week stale

| | RFC 131 (2026-09-01) | today | |
|---|---:|---:|---|
| top-level entries in `src/` | 123 | **125** | |
| distinct modules | 69 | **69** | |
| `pub(crate)` | 641 | **679** | **+38 in one week** |
| `pub(super)` | 271 | **273** | |
| `pub(in crate::…)` | 0 | **0** | the lever, still unused |

**Cite these, not the RFC's.** The shape is unchanged — 11 name families covering 31 modules, 38
singletons — but the `pub(crate)` count is moving in the wrong direction while the RFC waits.

## 2. RULED — grouping must not merge two coupling-graph nodes

**The coupling gate's graph node is the top-level `mod` declared in `lib.rs`.** `walk`
(`tools/release-policy/src/boundary/coupling/graph.rs:334`) enumerates `lib.rs`'s `mod` declarations,
and `collect_production_text` (`:363`) **concatenates every descendant file's text into that same
node**. There is no qualified naming: a child of `foo` is part of `foo`.

**Consequence: moving `refs.rs` to `publication/refs.rs` and declaring `mod publication;` renames the
node from `refs` to `publication`, and merges into it every other module placed under that roof.**

**Two things break, and the second is worse than the first:**

1. **Every `DECLARED_CYCLES`/`DECLARED_HUBS` entry naming a moved module goes stale**, and RFC 130 §5's
   reverse binding makes a stale entry a **hard `boundary-check` failure**. Expected, loud, fixable.
2. **A cycle wholly inside one group disappears from the graph.** The gate would report no cycle — not
   because the coupling was resolved, but because the grouping hid it. **RFC 130's ledger of structural
   debt would be silently erased by work whose stated purpose is to reduce coupling.**

**RULED: no two of these seven may share a group.** They are the six-module strongly-connected
component plus the one hub outside it, taken from the gate itself rather than from RFC 130 §2.2's
superseded table:

```
active   lifecycle_cache   patch_replay   refs   trust   worktree_patch      (the SCC, 13 edges)
wal                                                                          (hub, outside the SCC)
```

**They may each move into a directory of their own** — that keeps one node per module and the gate's
declarations merely need renaming. **They may not be collected together.**

**Teaching the gate qualified module names is the principled alternative and is OUT OF SCOPE here.**
It changes RFC 130's gate and deserves its own decision; do not attempt it as a side effect of a file
move.

## 3. RULED — two groupings the data settles

**(a) The `#[cfg(test)]` block is already a family; make it a directory.** `lib.rs:84-99` declares
eight modules under one contiguous `#[cfg(test)]` run:

```
dc55_identity_evidence          format_stability_gate        release_compatibility_gate
rfc111_index_decode_cost_gate   rfc111_seal_decode_cost_gate  signature_contract_tests
test_support                    trust_gated_operations_binding_gate
```

**These are gates, evidence harnesses and shared fixtures — not production code**, confirmed by
reading each. `lib.rs` already treats them as one group; a directory documents a fact rather than
inventing one, and removes **eight** entries from the top-level listing with no design argument.

**This does not violate §4's "no relocation of tests away from the code they exercise."** That
constraint protects `foo/tests.rs` sitting beside `foo.rs`. These eight are standalone top-level
modules beside nothing. **Say so in your report** — it looks like a violation and is not.

**Note `rfc111_seal_simulation` is NOT in this set** — it is production (`lib.rs:63`), despite the
name family. Do not sweep it in.

**(b) The eleven name families are adopted**, per the owner's own sketch:
`patch` (6), `worktree` (4), `rfc111` (3, minus the two `cfg(test)` gates — see above), `rollback` (3),
`trust` (3), and the pairs `author`, `format`, `merge`, `node`, `received`, `signature`.

**Subject to §2**: `trust`, `patch_replay` and `worktree_patch` are constrained modules. A `patch/`
group containing `patch_replay` merges nothing else with it, so it is fine — **the rule is about two
constrained modules sharing a roof, not about grouping one.**

## 4. DELEGATED — the remaining singletons

After (a) and (b), roughly **30 singletons** remain. **Whether they group by role is yours to decide
with the code in hand**, and RFC 131 §2's constraint is the one that matters:

> do not invent a home for a module that genuinely has none. A singleton left at the top level is
> honest; a group of one, or a group whose members share nothing but the need to be somewhere, makes
> the listing longer to read rather than shorter.

RFC 130 §2.3's **foundation** set (`layout`, `fsutil`, `byte_cursor`, `file_codec`, `frame_resync`,
`container`, `index`, `generation`) is the strongest candidate — a shared role, one-directional, and
none is in §2's constrained seven. **Offered as material, not as a ruling.**

**Report what you decided and what you left alone.** A round that leaves 20 honest singletons at the
top level and says why is a better outcome than one that files them all somewhere.

## 5. §3 — the visibility scoping, which is the half that changes the property

**Grouping alone changes nothing about reach.** `pub(crate)` still spans the crate; **679 of them, and
zero `pub(in crate::…)`.**

**Start with the seven modules of §2** — they are where a change propagates in both directions.
**RFC 131 §3's "four middle-hubs" is one short**: `active` is now a declared hub in its own right, not
merely one end of the `active ↔ refs` cycle.

**Each `DECLARED_CYCLES` entry carries a `what_would_remove_it` string. That is this increment's
input** — RFC 130 wrote them so that this round would not have to re-derive the reasoning.

**Where a `pub(in ...)` cannot be narrowed without a cascade, record it and leave it wide.** RFC 131
§3: *"that is information about the true coupling"*. **Do not widen silently back to `pub(crate)`
without saying so** — the resisting cases are the ones worth knowing.

## 6. Sequencing — pure moves and content changes are different commits

**RFC 131 §5 requires it and it is not negotiable**: a move that also edits content destroys `git
blame` through the move.

1. **Commit 1 — moves only.** No content edits, no visibility changes. `git log --follow` must work
   through it.
2. **Commit 2 — the coupling gate's declarations**, renamed to the new node names. Expect
   `boundary-check` to be red between commits 1 and 2; that is the reverse binding working.
3. **Commit 3 — `pub(in ...)` scoping.**

**Do not compress these.** If the round runs short, stopping after commit 2 leaves the crate in a
coherent state; stopping mid-move does not.

## 7. Controls

1. **The public API is byte-identical.** `cargo doc -p prikk-store --no-deps` generates **276** files
   in `target/doc/prikk_store/` today. **Capture the file list before and after; it must match
   exactly.** RFC 131 §4 forbids widening the external surface by even one item, and this makes that
   checkable rather than asserted.
2. **No two of §2's seven share a group.** State the mapping explicitly in the report.
3. **The coupling gate still sees the same graph.** After commit 2, `boundary-check` is green **and
   the declared cycle count is still 8 entries over 13 edges across six modules.** A drop in cycle
   count without a corresponding `what_would_remove_it` being *acted on* means §2's failure happened.
4. **`git log --follow` works through the moves** for at least one file per group.
5. **Long provenance comments are intact** (§4). Spot-check the largest ones; a move that splits a
   comment from what it explains is a regression this RFC is answerable for.
6. **Tests still sit beside their subjects** — the `foo/tests.rs` pattern is untouched.

## 8. Gates

The full set, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

**`boundary-check` is the gate this round is really about.** Run it after commit 1 to see the stale
entries it names — that list is your rename checklist.

**Cross-target clippy if your diff carries `#[cfg(target_os)]`** — moving a file that contains one
counts.

## 9. No `CHANGELOG.md` entry

Internal restructuring; `prikk-store`'s outward API is unchanged by control 1. **Ruled here rather than
left unsaid.**

## 10. Reporting

`.git-exclude/review-request/`. Include:

- **the group mapping**, and §2's seven shown to be in distinct groups;
- **what you left as a top-level singleton, and why** (§4) — this is the judgement being delegated;
- **the before/after doc file lists** from control 1;
- **every `pub(in ...)` that resisted**, and what it would have cascaded into;
- **anything about the grouping that made the coupling gate harder to read.** You will be the first to
  see the gate and the new layout together.
