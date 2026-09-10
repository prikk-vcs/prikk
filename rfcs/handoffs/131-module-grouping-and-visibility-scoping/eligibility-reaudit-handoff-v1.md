# RFC 131 §6d.3 — re-audit eligibility, comment-aware

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` **§6d.3**. Follows `ec59aba7`,
whose grouping, narrowing, `reexports()` fix and gate work are **accepted and not reopened.**

**Only the measurement is reopened.** Small round.

## 1. What went wrong, and it is a method problem not a care problem

The eligibility audit used a raw crate-wide grep per item. **A raw grep cannot tell a call from a prose
mention.** Checked at review, five of seven items were eligible, not one:

| Item | Why it is actually eligible |
|---|---|
| `commit_worktree_changes_with_generator` | used only in `commit_boundary/worktree_patch/tests.rs` |
| `next_op_seq` | used only in `commit_boundary/worktree_patch/node_authoring.rs:774` |
| `AuthorError` | zero mentions anywhere outside `commit_boundary` |
| `author_worktree_patch` | both cited callers are **comments** — `rollback_draft.rs:122`, `patch_replay.rs:429` |

Genuinely ineligible, confirmed: `read_active_ref_metadata_for` (`verify.rs:1507`) and
`prepare_empty_active_ref_for_append` (`rollback_draft.rs:177`) — both real calls.

**The gate you work on already solves this.** `coupling/graph.rs`'s `classify`/`blank` blank comments and
strings *before* any scan, precisely because source text read naively mistakes prose for code. **Use that
machinery, or the compiler, rather than a grep.**

## 2. What to do

1. **Re-audit all seven items comment-aware.** Confirm the table above independently — do not take it
   from me; I used `grep -n` and read each hit, which is better than an unexamined grep and worse than a
   tool that knows the grammar.
2. **Narrow every genuinely eligible item** to `pub(in crate::commit_boundary)`.
3. **Re-report the yield** — eligible over total, with the **denominator counted the same way**: items,
   not occurrences of the string `pub(crate)`. My own raw count of that string gives 8 across the three
   files against the report's 7, which is the same class of imprecision in the other direction.

**The compiler is the arbiter.** If an item narrows and the crate still builds, it was eligible; if it
does not, it was not. **That is a stronger audit than any grep** — consider narrowing a candidate and
letting `cargo check` answer, rather than deciding first and editing after.

## 3. Controls

1. **Every narrowed item is genuinely unreachable from outside `commit_boundary`** — one `E0603` probe
   per item, as `ec59aba7` did for the first. Remove the probes.
2. **The two ineligible items stay `pub(crate)`**, and a probe narrowing either one **fails to compile** —
   the negative control, which is what distinguishes "audited" from "narrowed everything that happened to
   work".
3. **Gate green and the full suite passes** at your final commit.
4. **The grouped pair's cycle is still reported** — `ec59aba7`'s control 1 unchanged and still passing.

**Perturb control 2.** It is the one whose passing is most easily vacuous: an audit that narrowed
everything and reverted whatever failed would pass control 1 and look identical in the report.

## 4. What this round must NOT do

- **Do not group a third module.** §6d.2's one-pair ruling still stands until the yield is re-reported.
- Do not touch the gate mechanism, the `reexports()` fix, `HUB_THRESHOLD`, or any allowlist entry.
- Do not narrow anything outside the grouped pair.
- `MILESTONES.md` untouched. No `CHANGELOG.md` entry — internal visibility, not user-visible surface; say so.

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, all green. State rule 9's cross-target result
or why it does not apply.

Report to `.git-exclude/review-request/`. **Lead with the corrected yield and the method you used to get
it** — the method is the finding here, not the number.
