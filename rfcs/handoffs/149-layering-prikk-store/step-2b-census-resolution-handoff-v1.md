# RFC 149 — step 2b: make the census a list the owner can rule on

**Live.** Step 1 is confirmed and your corrections to the handoff are recorded in RFC 149 §4a. Step 2's
census (`0.42.0-round-1-review-request.md` §D) has 26 of 51 entries marked `not-found`; the owner rules
on visibility, and cannot rule on an unresolved name.

## 1. Resolve every entry

For each of the 65 items: the declaring file and line, the item kind (fn / struct / enum / variant /
trait / const / module), and its current visibility as written. Module-path entries (`patch_replay::decode`,
`lifecycle_cache::replay`, `commit_boundary::worktree_patch`, `patch_replay::read`, `lifecycle_cache::incremental`)
expand to the items the surfaces actually use inside them — the module itself becoming `pub` is a
different and larger decision than its three functions. Enum variants (`refs::Evaluated`, `refs::Failed`)
resolve to their enum. If compiling the cut in a scratch worktree is the fastest resolver, do that and
report the compiler's list; discard the worktree.

## 2. Classify

Three columns per item: **already public** · **would become `pub`** · **test-support only** (the seven
helpers named in §C2, plus any other `*_for_test` / `proptest_*` the resolution finds — these go behind
the existing `test-support` feature, precedent `simulate_one_seal_for_test_support`, and never appear in
the `pub` column). A fourth column: **needed only by `verify`** — so the owner can see what a first cut
without `verify` would cost.

## 3. Report

`.git-exclude/review-request/rfc149-step-2b-report-v1.md`: the table, totals per column, and the
sentence the owner will read: *"these N items become `pub`; these M are test-support; K of the N are
`verify`-only."* No commit; no move. If step 1's method or numbers changed under resolution, say so first.
