# RFC 131 §6d.4 — narrow one module's submodule items, no grouping

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6d.4/§6d.4a**. Follows
`fc38e853`, accepted.

**No grouping. No file moves. No allowlist renames.** This round narrows visibility inside one module's
existing structure, and measures how far that gets.

## 1. Why this comes before more grouping

§6b.2's constraint binds items declared **directly** in one of the constrained seven. **A submodule item
already has an ancestor**, so `pub(in crate::<module>)` is expressible today with no structural change at
all. Measured:

| module | top-level | **submodule** | already `pub(in …)` |
|---|---|---|---|
| `lifecycle_cache` | 9 | **23** | 0 |
| `patch_replay` | 15 | **19** | 0 |
| `refs` | 10 | 14 | 32 |

**The submodule half is bigger than the top-level half and costs nothing structural.** Grouping one pair
cost 7 file moves, 21 reference rewrites, 6 allowlist renames and surfaced a latent gate bug — to reach
the smaller half. `refs`' 32 existing `pub(in …)` are §6b.3's own work, so **the method is proven and was
simply never extended.**

## 2. What to do — `lifecycle_cache` only

**23 submodule-declared `pub(crate)` items, zero narrowed.** Audit each for eligibility and narrow what
qualifies to `pub(in crate::lifecycle_cache)` (or deeper where a nested submodule is the true ceiling —
**narrower is better where it is honest**).

**Use the method §6d.3 established, in that order:**

1. Read every grep hit individually — **a raw match count cannot tell a call from a prose mention.** This
   error has now produced a wrong number twice in this arc, once in a report and once in my own review.
2. **Then let the compiler decide**: narrow the candidate, run `cargo check --workspace --all-targets
   --locked`, and trust that over the read.

**Report both yields separately, per §6d.4**, and do not conflate them:

- **Declared-ceiling tightening** — how many narrowed.
- **Actual access reduction** — how many were not *already* walled by a private submodule. Check whether
  each item's own module is `mod x;` or `pub(crate) mod x;`; the first already limits reach, so narrowing
  such an item is honest bookkeeping with preventive value, not an access reduction today.

## 3. Controls

1. **Every narrowed item is unreachable from outside its declared scope** — an `E0603` probe per item from
   a module outside that scope, removed after each, with `git diff` confirmed empty.
2. **Ineligible items stay `pub(crate)`, and narrowing one fails to compile** — the negative control.
   §6d.3 ran this properly; keep the standard.
3. **Gate green and the full suite passes.**
4. **`boundary-check`'s cycle and hub findings are unchanged** — visibility is not coupling, and this
   round must not move a single edge. If any does move, **stop and report**: that would mean a narrowing
   changed what the gate sees, which nothing in the design predicts.

**Perturb control 4.** It is the one nobody would think to doubt, and it is the claim that lets this round
proceed without touching the allowlist at all.

## 4. What this round must NOT do

- **No grouping, no module moves, no `mod` visibility changes.** Narrowing items is in scope; making a
  private submodule `pub(crate)` (or the reverse) is not — that changes reach rather than declaring it.
- Do not touch the gate mechanism, `HUB_THRESHOLD`, or any allowlist entry.
- Do not narrow anything in the other six modules, or any item declared directly in `lifecycle_cache.rs`
  — that one needs grouping and is deferred.
- `MILESTONES.md` untouched. No `CHANGELOG.md` entry — internal visibility; say so.

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, all green. State rule 9's cross-target result
or why it does not apply.

Report to `.git-exclude/review-request/`. **Lead with the two yields, separately**, and with control 4.
