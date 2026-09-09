# RFC 144 §4p.2 — say what a declaration became

**Ruled:** `rfcs/accepted/144-two-point-comparison.md` **§4p.2**. Follows `9a1b7dc2`, which is
**accepted** — this does not reopen it. **Small round: output, not behaviour.**

## 1. The gap, and what it is not

§4o.2 required refusing one case — declared destination gone, source back — and justified it as *"a
machine discarding a human's assertion quietly is the exact failure §4g exists to prevent."* **The rule
was narrower than the reason.** Three other resolutions discard the assertion just as quietly, and your
round implemented them exactly as specified. Verified against the built binary:

```
$ prikk mv a.txt build/a.txt        # build/ is ignore-matched
moved a.txt -> build/a.txt
$ prikk commit -m moved
  delete-file a.txt                 # no mention of the declaration at all
```

**Do not change any outcome.** Moving a tracked file into an ignored path genuinely removes it from the
tracked set, and a deletion is the honest record — **refusing it would be wrong.** Every resolution this
round produces is correct. **Only the silence is the defect.**

## 2. What to add

**Every live declaration that resolves to anything other than the rename it asserted must be named on
the commit that consumes it, with what it became.** One line each, in the commit's own output, beside
the operations it already prints.

The four resolutions needing a line, from your own classifier:

| Resolution | Why it is not a rename |
|---|---|
| Vacuous round trip (`mv a b`, `mv b a`) | nets to no move |
| Nets to deletion (destination deleted, or **ignore-matched**) | the node left the tracked set |
| `old_path` was never a tracked baseline node | there is no node to rename |
| Contradiction (destination gone, source back) | **already refuses — leave it exactly as is** |

**Say what happened, not what the user should have done.** *"declaration a.txt -> build/a.txt: destination
is ignored, recorded as a deletion"* is the shape. No advice, no suggestion to use a different command, no
warning tone — the outcome is correct and the line exists so the user can see their assertion was heard
and where it went.

**`--format json`, if `commit` has one, carries the same as a machine-branchable field**, not prose to
parse. Check whether it does; if not, do not add one in this round — say so in the report.

## 3. What this round must NOT do

- **No outcome changes.** Not the classifier, not the four resolutions, not the contradiction refusal.
- **No new refusals.** Disclosure only.
- Not the commit-time hint (§4o.4) — that is a different feature and a separate piece. This is disclosure
  of what already happened; the hint is a suggestion about what could have happened. **Do not let them
  merge.**
- No thirteenth witness (§4o.5), no read-type change (§4o.6).
- Do not touch `patch_replay`, the seal path, or `rename_nodes_checked_batch`.
- `MILESTONES.md` untouched.

## 4. Controls

Extend the existing controls rather than adding a parallel suite — controls 3 and 4 already produce two
of these resolutions and currently assert only the authored operations.

1. **Round trip** — assert the disclosure line names the declaration and says it nets to no move.
2. **Nets to deletion via ignore-match** — the case above; assert both the `delete-file` operation **and**
   the line explaining it. This one is new; the existing controls do not cover an ignored destination.
3. **`old_path` never tracked** — assert the line.
4. **Contradiction still refuses**, unchanged.

**Perturb one**: delete a disclosure line's emission and confirm its control fails. A test asserting only
"commit succeeded" would pass with every line removed.

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9. **State rule 9's cross-target result, or that
it does not apply and why.**

`CHANGELOG.md`: this changes user-visible output, so it earns a line in the open `## Unreleased` section.
The guide page you added should show the disclosure too — a reader who sees it in the terminal should find
it documented.

Report to `.git-exclude/review-request/`. Quote the exact new output lines; the wording is the deliverable.
