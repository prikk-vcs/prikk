# RFC 131 §6d.5 — narrow the re-exports my last handoff put off-limits

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6d.5**. Follows `a1ae661a`,
accepted.

**This round exists because my §6d.4 handoff was wrong**, and your round is what proved it. You narrowed
6 of 23 and reported 10 blocked by a restriction I wrote; the restriction was over-broad. The correction
is §6d.5.

## 1. What I got wrong, stated plainly

My handoff said *"do not narrow anything declared directly in `lifecycle_cache.rs` — that one needs
grouping and is deferred."*

**A `pub(crate) use` re-export is declared there and does not need grouping.** Grouping is required when
an item's *callers* sit in a sibling module — it exists to create a common ancestor. A re-export needs no
ancestor; it needs only that its *consumers* sit inside `lifecycle_cache`. **I conflated where a
declaration sits with the reason it was blocked**, and that put 10 of 23 out of reach for nothing.

**§6b.2's constraint still binds every other item declared directly in that file.** This round lifts the
restriction for **re-export lines only**.

## 2. Two facts I verified after accepting your round, before you start

**All ten blocked items have zero consumers outside `lifecycle_cache`.** I audited all ten by name across
`crates/` and `tools/` rather than sampling. The single hit anywhere is `DecodedLifecycleCache` at
`wal/tests/proptest_framing.rs:6`, inside a `//!` doc comment. **So the ceiling is 16 of 23, not an
estimate — but confirm it yourself at the compiler, not on my grep.**

**Nine of the ten are `#[cfg(test)]`, which neither your report nor my first draft of the review said:**

```
32:#[cfg(test)]
33:mod cache_ladder;
35:#[cfg(test)]
36:pub(crate) use cache_ladder::{ …9 items… };
58:mod store_resolvers;               ← no cfg
61:pub(crate) use store_resolvers::StoreBackedResolver;
```

`cache_ladder.rs`'s own header says *"Test-only lifecycle-cache codec and trust-ladder scaffolding."* So
**nine narrowings are test-scope hygiene and one is a production access reduction** —
`StoreBackedResolver`, which the code annotates as the crate's **E1 store-access boundary**, making it the
single most worthwhile item in the set. Do that one first, on its own, before the nine.

## 3. What to do

**Both ends move together.** For each eligible item, narrow the **origin declaration** in
`cache_ladder.rs` / `store_resolvers.rs` *and* the **re-export line** in `lifecycle_cache.rs` to
`pub(in crate::lifecycle_cache)` in the same edit. Narrowing one end alone is the `E0364`/`E0365` you
already measured.

Order:

1. **`StoreBackedResolver` alone.** Narrow both ends, run the gate, confirm. This is the production win;
   it should be visible as its own step and not buried in a batch of nine.
2. **The nine `cache_ladder` items.** Same treatment. Where a narrower honest ceiling exists, use it.
3. **Do not revisit the 7 ineligible** items from your last round. They were compiler-confirmed.

**Consider whether the re-export should exist at all.** If an item's only consumers are inside
`cache_ladder` and `replay`, a `use` at the consuming site may be better than a republished name at the
module root. **Report that judgement; do not act on it this round** — removing a re-export is a different
change from narrowing one, and I want to rule on it separately.

## 4. Report, in these terms

Per §6d.5's ruling, **report production and test-only yields separately and never as one number.** A
`#[cfg(test)]` item's narrowing is not an access-surface reduction in the shipped artifact, and a single
combined figure overstates the production win ninefold here.

| | narrowed | eligible | total |
|---|---|---|---|
| production items | | | 14 |
| test-only items | | | 9 |

State the running `lifecycle_cache` total against 23 as well, so §6d.3/§6d.4/§6d.5 remain comparable.

## 5. Controls

1. **An `E0603` probe per narrowed item** from a module outside `crate::lifecycle_cache`, removed after
   each, `git diff` confirmed empty. For a `#[cfg(test)]` item the probe must itself be in a test build or
   it proves nothing — **say which build each probe ran in.**
2. **Negative control:** after narrowing, re-widening is not the test. Instead add a real `use
   crate::lifecycle_cache::StoreBackedResolver;` in a module outside the scope and confirm it fails; that
   distinguishes *narrowed* from *invisible*.
3. **`boundary-check` cycle and hub findings unchanged** — and **perturb it again**. Your last round's
   perturbation of this control is the reason this one can proceed without touching the allowlist; do not
   let it decay back into an unperturbed pass.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim.
5. **Rule 9 cross-target addendum.** `lifecycle_cache.rs` contains `#[cfg(test)]` gating but the addendum
   is about `#[cfg(target_os)]`. **Check this round's own diff** and state the outcome or state why the
   rule does not apply — do not infer it from this sentence.

## 6. What this round must NOT do

- **No grouping, no file moves, no allowlist changes.**
- **No `mod` visibility change.** `mod cache_ladder;` stays as declared; this round is about what the
  re-export republishes, not about the module gate.
- **No removal of a re-export** (§3).
- **No other module.** `patch_replay`'s 19 submodule items are the next round, not this one.
