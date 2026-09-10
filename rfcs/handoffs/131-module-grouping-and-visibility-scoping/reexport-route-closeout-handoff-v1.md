# RFC 131 §6d.7 — take the last 9 names and close the re-export route

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6d.7**. Follows `da8fc40c`
and the census, accepted.

**This is a closeout, not an expansion.** Your census found **89 of 91 names load-bearing**, and both of
my headline figures were wrong — 37 was statements not sites, and my pattern silently skipped every
aliased re-export, so the name count was 91 not 89. Corrected in §6d.7. **The route has 9 names left in
it and then it is done.**

## 1. Take the two cheap ones first

`patch_algebra/report.rs`: **`analysis::analyze_pair_merge_evidence`** and **`mapping::pair_class_report`**.

I confirmed both independently — 9 and 6 total hits respectively, **zero outside `patch_algebra`**.
Production code, no `cfg`, no chained hop. Narrow both ends to `pub(in crate::patch_algebra)`, or deeper
if the real consumers allow it.

**Leave `report.rs`'s `input::sort_report_items` alone** — already `pub(super)`, and your census was
right to class it as not a pending site.

## 2. Then the seven in `fsutil`, under the amended two-hop rule

`RootDirEntry`, `LinuxDurability`, `MacosDurability`, `WindowsDurability`, `NoDurability`,
`set_directory_create_barrier_for_test`, `set_anchor_verification_barrier_for_test`.

**Your two-hop observation is right and it amends my §6d.5 rule.** §6d.5 said both ends move together;
that was stated from `lifecycle_cache`, where the outer hop existed *solely* to republish. **§6d.7 now
says: both ends move together only when the outer hop has no independent consumers.** `fsutil.rs`'s outer
hop has its own crate-wide callers, so for these seven **the inner hop in `anchored.rs` narrows alone**,
to `pub(in crate::foundation::fsutil)`, and the outer hop moves only where nothing outside `fsutil` uses
that name.

**Judge each of the seven at both hops separately and say which hop moved.** A report that says "narrowed
7" without naming the hop is not reviewable.

## 3. The cross-target addendum applies to §2, and it is the round's main cost

`anchored.rs` carries **15** `cfg(target_os)` uses and `fsutil.rs` **8** — counted at source. Four of the
seven names *are* the per-platform durability implementors, so this diff is cfg-gated code by any
reading.

**Run rule 9's addendum in full**: clippy `--target x86_64-pc-windows-gnu` and `--target
x86_64-apple-darwin`, and **state the outcome**. This is the round where the addendum earns its place;
three rounds of CI red on macOS and Windows came from exactly this file family.

**If the addendum's cost looks disproportionate to seven narrowings, say so and stop after §1.** Seven
platform-support and test-barrier names are not worth a red CI, and you are better placed than I am to
judge the ratio once you see the diff. **That is an authorized outcome, not a shortfall** — report it as
a recommendation and I will rule.

## 4. Controls

1. **Before/after `use` probe per claimed reduction**, per §6d.6 — detached worktree at the parent
   commit, both directions. This is now the standard and it is the only test that distinguishes a
   reduction from an item nothing reached.
2. **Negative control** on at least one name the census marked ineligible in the files you touch — narrow
   it, show the compiler rejecting it at a real consumer, revert, `git diff` empty.
3. **`boundary-check` unchanged, perturbed on a fresh edge again.** You have used
   `patch_replay ↔ lifecycle_cache::replay` twice and `refs ↔ trust` once; pick a third.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim, **plus the addendum** (§3).
5. **Root-relative paths in every table.** Your census wrote `verify/tests/ref_cluster.rs` where
   `refs/verify.rs` also exists; I nearly recorded a finding that was not there. Write
   `crates/prikk-store/src/verify/tests/ref_cluster.rs`.

## 5. And then the route is closed

After this round, report **the running total for the whole re-export route** — names censused, names
narrowable, names narrowed — and state that nothing eligible remains. **Do not look for more sites.**
§6d.7 rules this an accuracy finding rather than an access-surface one: 89 of 91 of those names really
are consumed crate-wide, and a private `mod` beside a `pub(crate) use` is a misleading declaration, not
reclaimable reach.

## 6. What this round must NOT do

- **No grouping, no file moves, no allowlist changes, no `mod` visibility changes.**
- **No re-export removed** — settled in §6d.6, still settled.
- **No `refs`, `text_span`, `test_gates` or `lifecycle_cache`.** Censused at 0 eligible or closed.
- **No new census.** The 91 are counted.
