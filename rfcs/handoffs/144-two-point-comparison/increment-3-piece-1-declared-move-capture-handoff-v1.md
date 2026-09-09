# RFC 144 increment 3, piece 1 — `prikk mv` and rename authoring

**Design:** `rfcs/accepted/144-two-point-comparison.md` **§4o**, specifically §4o.1–§4o.3 and §4o.7.
**Ungated:** §4k.1, increment 3's only gate, closed at `bb2e932d`.

**Piece 1 only.** §4o designs four pieces; the other three — the commit-time hint (§4o.4), the
thirteenth conflict witness (§4o.5), and the structural honesty invariant (§4o.6) — are **out of scope
for this round** and must not ride along. Piece 4 in particular changes read types across four render
surfaces and would make both rounds harder to review.

## 1. What this round is, and why it is kept small

**This is the first thing in prikk that ever emits a `RenamePath`.** Until now nothing authors one, so
no repository contains one, and every rename-handling path built in increments 1 and 2 has been
precautionary. **This round makes them load-bearing** (§4o.7):

- `checkout --patch-plan --format json` **fails outright** on a `RenamePath` — that is documented
  behaviour and we have told the stikk project so;
- the queued-envelope fold site's rename routing stops being theoretical — which is exactly why §4k.1
  gated this increment on proving that site;
- `rename_nodes_checked_batch`'s fail-atomicity starts meeting real input.

**None of that is a reason to hesitate. It is the reason the round is one piece.**

## 2. `prikk mv <old> <new>` — the four worktree states (§4o.1)

It **both moves and declares**. Not one or the other.

| Worktree state | Behaviour |
|---|---|
| `<old>` exists, `<new>` does not | Perform the rename, record the declaration |
| `<old>` gone, `<new>` exists | **Record the declaration only, touch no bytes** |
| Both exist | **Refuse** — two files, and which is the node is not ours to guess |
| Neither exists | **Refuse** — nothing to move |

**Row 2 is what makes the command usable and is easy to skip.** §7 records the realistic flow: a user
runs `mv` in a shell and *then* remembers. Without row 2 the command is useless to exactly that person.

**Do not compare content to decide any of these.** Deciding what moved by looking at bytes is inference,
and §4g's declaration-only ruling forbids the machine deciding that. The four rows key on **existence
only**.

Path handling goes through the existing repository-relative path validation — this command takes two
user-supplied paths and is squarely in `DC-72` path-safety territory. Do not hand-roll it.

## 3. The declaration store (§4o.2)

Durable, alongside the queue, **cleared when the commit that consumes it is queued** — not when sealed.
A declaration is an intent about the *next* patch, and a queued patch has already captured it.

**Collapse chains to the net move.** `mv a b` then `mv b c` before one commit authors **`a→c`**, one
operation. This is not a new rule: §4j.2 already ruled *"its only honest meaning is the net move"*, and
both `patch_replay` and the seal path already **reject** a chained rename within one patch.
**Authoring must not emit what replay would reject** — that is the whole point of doing the collapse
here rather than discovering it at seal time.

**Two corollaries, to be handled rather than found later:**

- **`mv a b` then `mv b a`** nets to no move. **Drop the declaration.** Do not author `a→a`.
- **A declared move whose destination is then deleted** nets to a deletion. **Drop the declaration,
  author the plain `DeleteNode`.**

**If the worktree contradicts a live declaration at commit time** — destination gone, source back —
**refuse the commit, naming the declaration.** Do not silently drop it. A machine discarding a human's
assertion quietly is the exact failure §4g exists to prevent.

Crash safety follows the queue's own conventions; do not invent a second durability story.

## 4. `worktree-status` shows live declarations (§4o.3)

In the human output and as a **machine-branchable field** in `--format json`. A declaration made and
forgotten becomes permanent history at the next commit, so it must be visible in the command whose job
is *what will this commit do*.

## 5. REQUIRED controls

**The authoring↔replay agreement is the property this round must prove**, not merely exercise:

1. **A declared move authors one `RenamePath`, and replay accepts it** — end to end: `prikk mv`,
   `commit`, `seal`, then verify. This is the first sealed history containing a rename; say so in the
   report.
2. **A chained declaration authors the net move** — `mv a b`, `mv b c`, commit; assert **one** operation
   `a→c`, and that seal and verify both accept it.
3. **A round-trip declaration authors nothing** — `mv a b`, `mv b a`, commit; assert no `RenamePath`.
4. **A declared move whose destination is deleted authors a plain `DeleteNode`**, no rename.
5. **A worktree contradicting a live declaration refuses the commit**, with the declaration named.
6. **The two-node swap, end to end** — the case increments 1 and 2 were built for, now reachable for the
   first time. Author both declarations in one commit, seal, verify.

**Perturb at least one of these.** A control that cannot fail is this project's most repeated defect;
pick the one whose passing you would most take for granted and break the implementation to confirm it
fails.

## 6. What this round must NOT do

- **No commit-time hint** (§4o.4), **no thirteenth witness** (§4o.5), **no read-type change** (§4o.6).
- Do not change `patch_replay`, the seal path, or `rename_nodes_checked_batch` — increments 1 and 2 are
  accepted and this round is their first real consumer, not their revision. **If you find a defect in
  them, report it; do not fix it inside this round.**
- Do not relabel `SamePathCreate` (§4i.2 defers it).
- `MILESTONES.md` untouched.

## 7. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9. **New user-visible command, so `CHANGELOG.md`
gets an entry** — and `commands.rs`'s `help_lines` is the single source for both dispatch and `--help`,
so `prikk mv` must appear there. (0.37.0 shipped a flag that never reached its command's help; do not
repeat it.) The guide needs a page, and `reference/commands.md` an entry.

**State rule 9's cross-target result, or that it does not apply and why** — the amended rule requires it
either way.

Report to `.git-exclude/review-request/`. Lead with control 1 and control 6, and state plainly that this
is the first sealed prikk history to contain a rename.
