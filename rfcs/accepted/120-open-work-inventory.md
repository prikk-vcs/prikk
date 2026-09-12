# RFC 120 — Open-work inventory: one place the candidate set is complete

**Status.** **ACCEPTED by the project owner 2026-08-27**, at the reduced scope ruled in §6 —
`rfcs/proposed/*.md` bound to one thin index section in `ROADMAP.md`, with a "findings without a file"
section beside it. The milestone half (§6 Q2) and `rfcs/accepted/` (§6 Q3) are **deliberately excluded**
and need their own decisions.

**§9 AMENDMENT, 2026-09-05, written at the owner's instruction — and it does not do what it was asked
to do.** The request was to widen the gate to `rfcs/accepted/`. Measured, that folder has gone from 13
files to **52**, at least 29 of them finished, so widening as-is would list roughly forty-five shipped
RFCs as open work — **Q3's own refusal, at four times the scale it was refused at.** §9 proposes
draining the folder first and widening after, and **§9.4 carries the one question the owner must
answer**: drain now, drain gradually, or never widen. **Nothing is implemented; §9 is a proposal.**

Recorded by the architect at the owner's instruction, 2026-08-26, after answering *"what are our
remaining themes?"* with an incomplete list. **The scope reduction came from evidence, not from wanting
less work** — see §5a.

**Tracks.** A correction to how this project records open work, not a product capability.

---

## 1. What went wrong, precisely

Asked what remained, the architect read `ROADMAP.md`'s **Future Themes** section and answered from it.
That section names four items. The real open surface is spread across at least four places, and the
answer omitted most of it.

**Measured, not asserted:**

```
ROADMAP.md mentions of `proposed/`:                     0
RFC 108 (Workspace)          referenced in ROADMAP/MILESTONES: 0
RFC 109 (agent-native interface)                              0
RFC 110 (agent safety and provenance)                         0
RFC 113 (history import foundations)                          0
DC-43 / DC-44 / DC-49                                    7 / 1 / 4
```

**The four newest proposed RFCs — all authored or originated by the project owner — are unreachable
from either document a reader consults to ask what remains.** The older `DC-*` items are referenced
only because they predate the RFC-100 numbering and were woven in at the time.

**No amount of care fixes this.** The index does not point at them; finding them requires already
knowing they exist.

## 2. The defect class is the project's own

This is the pattern RFC 118 exists to remove — **an inventory that lives in more than one place with
nothing asserting the set is complete.** It is `parent_patch_ids` documented at six sites, or the
object-taxonomy table drifting from `ObjectType`.

**One difference makes it worse.** Those were facts *transcribed* into several places, so any copy
could be checked against the source. This is a fact **partitioned** across places, where **no source
claims to be whole**. Nothing was stale. Nothing was wrong. The answer was simply incomplete, and
nothing could have said so.

## 3. What a mechanism can and cannot guarantee

**Stating this plainly, because the value of the proposal depends on it.**

- **Enumeration completeness — mechanisable.** Every file in `rfcs/proposed/` and every incomplete
  milestone row appears in one index, or a gate fails.
- **Description accuracy — not mechanisable.** A gate binds *existence*, not truth. A one-line summary
  can go stale exactly as `ROADMAP.md`'s Sync heading did — accurate body, false framing. This needs
  the same currency discipline as every other document.
- **Priority — not mechanisable, and should not be.** Cost, risk, dependency and owner intent are
  judgment. **No gate produces a ranking, and one that appeared to would be lying.**

**What this buys is narrow and worth having: the candidate set is complete.** A priority decision is
then made over everything rather than over what someone remembered. **An omission is worse than a
mis-ranking** — a ranking can be disagreed with; an omission cannot be seen.

## 4. The gap a gate alone does not close

**Open work does not reliably have a file.**

This session produced at least three open items recorded in **no tracked file**: the `sync build`
*"already in sync"* wording question (RFC 116 §4 behaviour that misleads a user who committed without
sealing), the missing local reproduction path for the cross-host CI jobs, and `MILESTONES.md:334`'s
stale `M5` row. They live in `.git-exclude/reviewed/` — **which is git-excluded, and therefore not in
the repository at all** — and in the architect's session memory.

**A gate over two directories would enumerate them faithfully and omit these silently.**

**So the proposal is two things, and the second is a habit:**

1. **Gate the enumeration** (§5).
2. **Record open work found in review as a tracked line** when it is not fixed immediately.

**A habit is exactly what the owner asked not to depend on.** The gate cannot enforce that a line was
*written*, but it can enforce that **every source it knows about is represented** — so the habit's
absence is visible for anything with a file, and only genuinely fileless findings can escape. **That is
a reduction in exposure, not elimination, and it should not be described as more.**

## 5. The gate, if this is accepted

**Bind two derivable sets to one authored index:**

- every `rfcs/proposed/*.md`;
- every milestone row in `MILESTONES.md` not marked complete.

**Fail when a member of either set is not named in the index.** Do not check ordering, wording, or
priority — only presence.

**Precedents, all in-repo and proven:** `trust_gated_operations_binding_gate.rs` (enum bound to a
markdown section), `object_type_table_binding_gate.rs` (enum bound to a markdown table, both
directions), the MSRV transcription gate (`Cargo.toml` bound to six sites), and `reference-check`'s
inventory-plus-scanner. **This is a fifth application of a shape that already works here — not a new
mechanism.**

**Scope the index to `ROADMAP.md`**, which is where the question naturally goes. **Binding
`MILESTONES.md` means editing that file, which requires the owner's explicit instruction** — so the
milestone half of §5 is separable and may be deferred without weakening the RFC-half.

## 5a. Findings from the §6 research, 2026-08-27

**The dev team researched §6 and built nothing, as the status line requires.** Two of its evidence
claims were wrong in ways that changed the rulings, and one produced a new finding.

**`ROADMAP.md`'s backlog tables are not historical.** The research reported them "fully populated with
`Done`/`Released` rows — nothing new has landed." They hold **three `Open` rows**:

| TASK-14 | consolidated non-goals / deferred features | *start when deferred-feature lists begin drifting* |
| TASK-15 | roles & user-classes orientation | *start when the docs need a clearer audience map* |
| TASK-16 | error taxonomy & diagnostics | *start with TASK-07 or when diagnostics need user-facing intent* |

**Both the researcher and the architect read that section as historical and skipped it.** It is a third
instance of §1's own defect — open work sitting where a reader stops looking — and this one is *inside
`ROADMAP.md`*, the document §5 proposes to make authoritative. **TASK-14's own trigger arguably fired
this week**, given how many deferred-feature entries were deleted.

**`rfcs/accepted/` holds thirteen RFCs, not five.** The research cited 115–119. It also contains 100,
102, 103, 105, 106, 107, 112, and 114 — long finished. **That difference decides Q3.**

## 6. Rulings

**Q1 — thin lines, and do not absorb the existing tables.** The gate checks presence only, so any
column it does not read is unverified transcription that can drift. **But the backlog tables are live
(§5a) and their `Trigger / next action` column is what makes TASK-14/15/16 actionable rather than a
nag.** So: **a thin gated index, which *references* those tables rather than replacing them.** Do not
flatten a working structure to fit a new one.

**Q2 — defer the milestone half.** `MILESTONES.md`'s "State today" column is free prose (`MET,
2026-08-22`; `OPEN, and deliberately so — not a blocker`), and a regex over it is fragile in both
directions. **Adopting the researcher's reasoning verbatim: encoding a heuristic over prose the gate
does not control is a worse failure than not gating yet.** Revisit only if a structured status
convention is adopted, which needs the owner's instruction independently.

**Q3 — `rfcs/proposed/` only; exclude `rfcs/accepted/`.** Thirteen accepted RFCs are dominated by
finished work, and distinguishing "accepted and still open" from "accepted and shipped" needs the same
prose-marker that Q2 just refused. **An index that lists eight finished RFCs teaches readers to ignore
it** — the opposite failure from the one this RFC exists to fix, and no less damaging.

**Q4 — a named section in the same document.** Adopting the researcher's reasoning: a separate tracked
file recreates a second place a reader must know to check. **The gate still cannot enforce that a
fileless finding was ever written down** (§4), and that limit stays stated.

**Net scope: gate `rfcs/proposed/*.md` against one thin index section in `ROADMAP.md`, with a
"findings without a file" section beside it.** Substantially smaller than §5 first implied, and every
reduction came from evidence rather than from wanting less work.

## 7. Open questions a design must answer

1. **Does the index carry one line per item, or the existing backlog table's shape**
   (`ID | Tier | Owner | Status | Trigger / next action | Completion condition`)? The richer shape
   invites staleness; the thinner one carries less.
2. **What marks a milestone row "incomplete"** in a way a gate can read without interpreting prose?
3. **Do accepted-but-unfinished RFCs belong in the set?** `rfcs/accepted/` holds work in progress;
   including it widens the gate meaningfully.
4. **Where do fileless findings go** — a tracked backlog file, or a line in the index itself?
   **This is §4's unresolved half and the one that decides whether the index is genuinely single.**

## 8. What this is not

- **Not a priority mechanism** (§3).
- **Not a claim that the index is true**, only that it is complete with respect to its sources.
- **Not a replacement for review records.** `.git-exclude/reviewed/` stays where reasoning lives; the
  index carries only that an item is open.

## 9. Amendment 2026-09-05 — Q3 revisited, and the answer is not the one it was asked for

**Raised by the project owner 2026-09-05**, after they caught that four owner-accepted RFCs had been
left in `rfcs/proposed/` with "ACCEPTED" written in their status text. Correcting that moved 123,
130, 131 and 137 into `rfcs/accepted/` — and **§6 Q3 excludes `rfcs/accepted/` from this gate**, so
accepted-but-unshipped work is now visible only in `ROADMAP.md` prose that nothing checks.

Two RFCs had already noticed this and worked around it: **130 and 131 each carried a status paragraph
saying they would stay in `rfcs/proposed/` until implemented, precisely so they would not fall off the
gated board.** That is the gap making itself felt — a gate's scope bending the lifecycle to compensate.
Both paragraphs are now corrected; the workaround is gone and the gap is exposed.

### 9.1 Q3's premise, measured then and measured now

Q3 was not a principle. It was an evidence-based call about one folder's contents:

> Thirteen accepted RFCs are dominated by finished work... **An index that lists eight finished RFCs
> teaches readers to ignore it.**

| | At the ruling, 2026-08-27 | Measured 2026-09-05 |
|---|---:|---:|
| files in `rfcs/accepted/` | 13 | **52** |

**The premise did not weaken; it got four times stronger.** A crude sweep — each file's own status text
plus `EXECUTION-ORDER.md`'s completion column — puts **at least 29 of the 52** as finished, and the true
figure is higher, because the sweep misses ones that are demonstrably live in the codebase today:
RFC 114's format-stability contract, RFC 118's derive-never-transcribe gate, DC-51's dependency
placement gate. **Genuinely open work in that folder is a handful: 102, 103, 130, 131, 137, and part
of 108.**

### 9.2 Therefore the amendment is not "widen the gate"

**Widening the gate to `rfcs/accepted/` as it stands today would list roughly forty-five finished RFCs
as open work** — exactly the "teaches readers to ignore it" failure Q3 named, at four times the scale
it was refused at. Q3 stays correct on the folder as it is.

**The amendment is: drain the folder, then widen the gate.**

1. **Classify all 52**, on evidence rather than heuristic — a shipped RFC left behind hides nothing,
   but an unshipped one moved to `done/` hides open work, so the classification must be per-file and
   checkable, not a regex.
2. **Move the finished ones to `rfcs/done/`**, each with the full retirement checklist: the file, its
   own status text, `ROADMAP.md`, `rfcs/README.md`, any live docs link, **and repointing its
   handoffs** — the step that did not exist until the 2026-09-05 sweep found 35 files citing an RFC at
   a folder it had left.
3. **Then widen the gate** to bind `rfcs/proposed/` ∪ `rfcs/accepted/` in both directions, unchanged
   in every other respect.

### 9.3 Why the order matters, and what the gate buys once it holds

**The gate then enforces the drain it depended on.** A shipped RFC left in `rfcs/accepted/` would
appear in the open-work index as open work and be visibly wrong to any reader — so the folder cannot
silently refill. Today nothing notices, which is how 13 became 52.

That is the same shape as this RFC's own §2 thesis: the failure was never that someone forgot, it was
that **nothing asserted the set was complete.** Q3 was the right call on the evidence available; it
left the *reason* for the exclusion unaddressed, and the reason has been compounding for nine days.

### 9.4 The cost, stated plainly

**Roughly 45 file moves, each with reference repointing.** The 2026-09-05 sweep found 35 stale paths
from four moves plus a handful of earlier retirements — so a drain of this size will surface
substantially more, and every one is a link that currently resolves and would stop resolving.

**This is a day of careful, mechanical work with a real chance of hiding open work if done
carelessly.** It is not obviously worth doing purely to widen a gate. **The question for the owner is
whether the folder is drained now, drained gradually as each RFC is next touched, or left as it is
with `rfcs/accepted/` accepted as a historical archive and the index widened never.**

**The architect's recommendation: drain gradually, widen when it is empty of finished work.** Every
retirement already carries the checklist; making it a standing rule that *an RFC moves to `done/` in
the release that ships it* — which 0.32.0 did for RFC 123 — drains the folder as a side effect of
ordinary work, with no separate campaign. The gate widens when the count of finished files in
`rfcs/accepted/` reaches zero, and until then `ROADMAP.md`'s prose pointer carries the four open ones.

### 9.4a A second, much cheaper gate — proposed 2026-09-06 after the same error recurred

**Separate from §9.2's drain, needing nothing drained:** *an RFC in `rfcs/proposed/` must not have a
directory in `rfcs/handoffs/`.* A handoff instructs an implementer to start; RFC-000 says such an RFC
is `accepted/`. **Both cannot be true, and the check is a directory listing.**

**Why now:** the project owner has caught the same lifecycle miss twice in two days — RFCs 123, 130,
131, 137 on 2026-09-05, and RFC 135 on 2026-09-06. Measured before the 135 move, that rule had
**exactly one violation**; after it, none. Each earlier case would have tripped it the day its handoff
was issued.

**This does not depend on §9.2.** It binds two directories that are both already accurate, so it can
land whenever the owner wants it — including before any drain, and including if the drain is never
done. See RFC 135 §9.9, where it was first written down.

### 9.4b Re-reviewed 2026-09-13 — the recommendation, with today's numbers

**23 finished RFCs sit in `rfcs/accepted/`** (status line says done/delivered/closed) of 59. The §9.4 recommendation stands: drain gradually — *an RFC moves to `done/` in the release that ships it* — and widen the index when the folder is empty of finished work. **§9.4a should land now**: measured today it would have fired on **RFC 133 and RFC 136**, both accepted by the owner (2026-09-03, 2026-09-04) and both still filed in `proposed/` until this commit moved them — the third and fourth instances of the miss it exists to catch. Its cost is a directory listing inside `boundary-check`. **RULED yes 2026-09-13** (owner: *"Will be handed to dev team"*); handoff `120-open-work-inventory/rfc-lifecycle-gate-handoff-v1.md`. **Built 2026-09-13; its only real-tree finding was DC-43** — a handoff *prepared in advance* for a proposed RFC. Refiled by the architect (appendix in the RFC, directory removed); the gate lands in the follow-up commit.

### 9.5 What is not proposed

No change to Q1, Q2 or Q4. No change to the gate's implementation until §9.2 step 3. No change to
`MILESTONES.md`, which Q2 excluded for reasons this amendment does not touch.
