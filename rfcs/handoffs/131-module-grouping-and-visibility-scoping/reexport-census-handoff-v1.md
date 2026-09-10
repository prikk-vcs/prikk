# RFC 131 §6d.6 — census the crate's decorative module gates, then narrow two files

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6d.6**. Follows `da8fc40c`,
accepted — `lifecycle_cache` is closed at 16 of 23, nothing eligible left.

**Your round's finding is bigger than your round.** The private-`mod`-plus-`pub(crate) use` shape you
found in `lifecycle_cache` occurs **37 times across 8 files in `prikk-store/src`, republishing 89
names.** Every one of those 37 private `mod` declarations wallpapers over crate-wide reach.

## 1. The rule that changed, and why it matters to this round

§6d.4 told you to judge actual access reduction by whether an item's own module is private. **That test
is now retired** — it returned *zero reduction* for your last round, and the truth was ten. §6d.6:

> **Reach is what a module outside the scope can name.** Write the `use`, let the compiler answer, and
> do it at the commit *before* the change as well as after. An `E0603` after a narrowing proves
> unreachability, not reduction.

**Use the before/after pair for every reduction you claim this round.** `git worktree add --detach` at
the parent commit is how I verified yours; the same method is now the standard, not a courtesy.

## 2. Step one — the census, and it is the deliverable even if step two shrinks

For each of the 37 sites, report:

| file | private `mod` | names republished | consumers outside the declaring module | narrowable? |
|---|---|---|---|---|

**Read every consumer, do not count matches.** A doc-comment mention has now presented itself as a
reference four times in this arc, and a raw `pub(crate)` grep has produced a wrong denominator three
times — restrict to column zero and read each hit.

**Then produce the one number that matters: how many of the 89 are narrowable.** I have deliberately not
estimated it and no figure should be quoted before you measure it. A re-export exists *because* something
consumes it; expect a materially lower ratio than `lifecycle_cache`'s 10 of 10, and **report the real
number even if it is small** — a census that finds the shape mostly load-bearing is a good result and
closes the question.

Split the census by **production vs `#[cfg(test)]`**, per §6d.5. `test_gates/test_support.rs` is
presumably all test-only; confirm rather than assume.

## 3. Step two — narrow two files only

**`refs.rs` / `refs/verify.rs`** (4 sites) and **`text_span.rs`** (2 sites).

`refs` is one of **RFC 130 §2.2's four middle-hubs** — §3 of this RFC says to start where a change
propagates both ways, and this is the first round that can. `text_span` is small and adjacent; it makes
the round two data points rather than one.

Both ends together, as you did. The ceiling is the narrowest honest common ancestor of the actual
consumers — **go deeper than the top-level module where the consumers allow it**, and say so when they do
not.

**Defer** `patch_algebra` (8 sites), `foundation/fsutil` (9 sites, platform-gated — the cross-target
addendum will bite there), `test_gates` (2 sites). They are the round after, sequenced on your census.

## 4. Do not remove a re-export

**Ruled and closed in §6d.6**: a re-export that carries a boundary annotation or holds an established
import path is **kept and narrowed, not removed.** Narrowing achieves the reach reduction; removal only
moves churn into consumers. Do not re-open this per module and do not report a judgement on it again.

## 5. Controls

1. **Before/after `use` probe per claimed reduction** (§1). Detached worktree; assert it exists before
   editing and confirm the primary tree is clean after.
2. **Negative control:** for at least one site the census marks *not* narrowable, narrow it anyway and
   show the compiler rejecting it at a real consumer. **An ineligibility claim that was never tested is
   an assertion.**
3. **`boundary-check` cycle and hub findings unchanged — perturbed again.** You have now perturbed this
   twice and both times it earned its keep. **Do not cite the earlier passes**; the code it watches
   changed again.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim.
5. **Cross-target addendum.** It did not apply to your last round and I verified that independently — zero
   `cfg(target_os)` in the whole `lifecycle_cache` subtree. **`refs` and `text_span` are not obviously in
   the same position.** Check this round's own diff and state the outcome, or state why the rule does not
   apply; do not carry last round's answer forward.

## 6. What this round must NOT do

- **No grouping, no file moves, no allowlist changes, no `mod` visibility changes.**
- **No re-export removed** (§4).
- **No `patch_algebra`, `fsutil` or `test_gates` narrowing** — census them, do not touch them.
- **No revisiting `lifecycle_cache`.** It is closed.
