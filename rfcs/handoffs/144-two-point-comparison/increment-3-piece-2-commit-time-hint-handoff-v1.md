# RFC 144 increment 3, piece 2 — the commit-time move hint

**Design:** `rfcs/accepted/144-two-point-comparison.md` **§4o.4**, resting on §4h.5's ruling.
**Follows** `9a1b7dc2` (rename authoring) and `cabe2487` (declaration disclosure), both accepted.

**Piece 2 only.** Not the thirteenth witness (§4o.5), not the structural honesty invariant (§4o.6).

## 1. Why now, and what it fixes

`prikk mv` exists. **The person who most needs it cannot discover it**: they run `mv` in a shell, commit,
and get a delete plus a create with nothing telling them identity could have been preserved. §4h.5 names
this exactly — *"an irreversible, commit-time-invisible opportunity cost"* — and rules the fix is
**discoverability, not a mandate**.

## 2. The rule, and it has no exceptions

**When `commit` sees a delete+create pair that is a strong move candidate, it may print a hint. It
authors nothing, prompts nothing, and blocks nothing.**

> *"looks like you moved X to Y; `prikk mv` would preserve its identity"*

**The machine suggests; the human asserts.** A hint that becomes a default, a prompt, or a
`--assume-moves` flag is the coercion §4h.5 refused. **There is no follow-up round that adds one.**

**The heuristic is allowed to be wrong.** It costs a line of output, not a fact in history — which is
precisely why it may exist at all.

## 3. REQUIRED — the hint must not be mistakable for the disclosure

`cabe2487` shipped disclosure lines on this same output surface:

```
  declaration a.txt -> build/a.txt: destination is ignored; recorded as a deletion, not a rename
```

**Those are facts about an assertion the user made. A hint is a suggestion about one they did not.** If
the two read alike, a user cannot tell what prikk *recorded* from what prikk *wondered* — and the whole
value of the disclosure work is that a recorded fact is unambiguous.

**Make them visually and grammatically distinct**, and put the hint after the operations and disclosures
rather than interleaved. The disclosure lines are declarative and past-tense; keep the hint
conditional. Do not reuse the `declaration …` prefix.

## 4. The candidate signal — exact content match, and nothing cleverer

**Exact content equality between a deleted path's baseline content and a created path's new content.**
That is the whole signal. §4o.4 rules similarity scoring **out**: it buys a marginally better hint at the
cost of a threshold nobody can defend, and a threshold in a heuristic is a number that will be argued
about forever.

**Four cases the signal alone gets wrong. Handle them; do not discover them.**

1. **Ambiguous pairing.** Two deleted paths and two created paths all sharing identical content — which
   moved to which is unknowable. **Emit no hint for an ambiguous set.** A hint naming the wrong pair is
   worse than silence, because it invites the user to assert something false.
2. **Empty files.** Every zero-byte file matches every other. Real repositories have several. Under rule
   1 these are almost always ambiguous, but **check that the rule actually covers it** rather than
   assuming; if it does not, exclude zero-byte content explicitly and say so.
3. **A declared move is not a candidate.** Declarations author `RenamePath`, not delete+create, so they
   should never reach the heuristic — **assert that, do not assume it.**
4. **Many candidates at once.** A genuine mass reorganisation would print a wall of hints, and a wall of
   hints is noise. **Above a small number, print one summary line naming the count and the command**, not
   one line per pair. Pick the threshold, state it in the report, and keep it a constant with a comment
   rather than a magic number.

## 5. Controls

1. **A shell-`mv`-then-commit prints the hint**, naming both paths, and **authors delete+create exactly
   as before** — assert the operations are unchanged, not merely that a hint appeared.
2. **A declared move prints no hint** (it authored a rename; there is nothing to suggest).
3. **An ambiguous set prints no hint** — two identical deleted files, two identical created files.
4. **A mass reorganisation prints the summary, not N lines.**
5. **The hint never changes what is authored** — the same repository committed with and without the
   hint code path produces byte-identical patches. **This is the control that matters**: it is the
   machine-checkable form of "authors nothing."

**Perturb one.** Control 5 is the one whose passing would be most taken for granted — a hint that
accidentally influenced authoring would still leave every other control green.

## 6. What this round must NOT do

- **No prompt, no flag, no default, no `--assume-moves`.** Not now and not as a follow-up.
- **Do not touch the disclosure lines** shipped in `cabe2487`, or the classifier, or any outcome.
- Do not touch `patch_replay`, the seal path, or `rename_nodes_checked_batch`.
- No thirteenth witness (§4o.5), no read-type change (§4o.6).
- **No similarity scoring**, no rename detection heuristics beyond exact content equality.
- `MILESTONES.md` untouched.

## 7. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9. **State rule 9's cross-target result, or that
it does not apply and why.**

`CHANGELOG.md`: user-visible output, so it earns a line in the open `## Unreleased` section.
`docs/src/guide/patches/declared-move.md` should show the hint **beside** a disclosure example, so a
reader sees the difference between the two rather than meeting them separately.

Report to `.git-exclude/review-request/`. **Quote the exact hint text and one disclosure line together** —
if they are hard to tell apart on the page, they will be hard to tell apart in a terminal, and that is
the finding rather than a formatting nit.
