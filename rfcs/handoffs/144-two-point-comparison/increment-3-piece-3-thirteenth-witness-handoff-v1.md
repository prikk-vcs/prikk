# RFC 144 increment 3, piece 3 — the thirteenth conflict witness

**Design:** `rfcs/accepted/144-two-point-comparison.md` **§4o.5**, resting on §4i.2's own narrowing.
**Follows** `9a1b7dc2`, `cabe2487`, `25c45734` — all accepted.

**Piece 3 only.** Not the structural honesty invariant (§4o.6), which changes read types across four
render surfaces and gets its own round with nothing else moving.

## 1. What the thirteenth is, and what it is not

**Both sides rename one node to two *disjoint* destinations.** Resolution: *choose which destination
wins **for this node***.

**`SamePathCreate` stays exactly as it is.** §4i.2 ruled the pair a genuine dual, and it is the clearest
statement of the distinction:

> **both-sides is "one node, two paths — pick the path"; occupied-path is "two nodes, one path — pick the
> node."**

Different resolution shapes, so different witnesses. **Relabelling `SamePathCreate` remains deferred
cosmetics** (§4i.2) — do not touch it.

**This became reachable three rounds ago and is unreachable no longer.** Until `prikk mv` existed nothing
could author a rename, so no merge could ever see two. Two people can now each declare a move of the same
node.

## 2. The enabling change, established so you do not discover it mid-round

**`Action::RenamePath` currently carries only `node_id`** (`patch_algebra/types.rs:212`). The destination
goes into the path-effect sets — `newly_occupied`, `occupied_after`, `required_free`
(`facts.rs:99-106`) — so **the node→destination pairing is not recoverable from the action**, and the
witness cannot be computed without it.

**So `Action::RenamePath` must carry the destination path.** That is a change to a shared type in
`patch_algebra`, and it is the load-bearing part of this round. Verified at source before writing this;
do not take it on trust either — check it yourself before building on it.

**Why the existing classification cannot already cover it**, also verified: `classify_path_relation`
(`classify.rs:94-99`) fires `SamePathCreate` on a `newly_occupied` **intersection**. Two disjoint
destinations do not intersect, so **nothing fires today** — which is exactly what §4i.2 means by "there
is no resolution path at all."

## 3. The enum is already guarded — use the guard, do not work around it

`ConflictWitnessKind`, its `ALL`, and its `label()` are generated from **one variant list** by the
`conflict_witness_kinds!` macro (`types.rs:95-131`), whose own comment says:

> *"A thirteenth kind cannot be added to the enum without simultaneously giving it a label; there is
> nowhere else to add it and nothing else to keep in sync."*

**Add the variant there and take the label with it.** The label is **an external interface** once it
reaches `MergeEvidenceDisplayItem::witness_kind` — pick it once, kebab-case, in the established style,
and expect never to change it.

`every_conflict_witness_kind_reaches_the_report_item` is the existing completeness sweep. **It should
fail before your report-item work and pass after.** Say whether it did — if it passed all along, the
sweep is weaker than it looks and that is a finding.

## 4. REQUIRED — report what happens today for the same-destination case, and change nothing

**Both sides renaming one node to the *same* destination is a third shape**, and §4i.2 rules on neither
it nor its label. Today `newly_occupied` intersects, so `SamePathCreate` fires.

**Do not change that in this round.** Report what it currently classifies as and whether the label reads
defensibly, and I will rule. **A negative result is a result** — "it fires `SamePathCreate`, and the
resolution *pick one* is still correct even though nothing was created" is a complete answer.

I am asking because it is adjacent enough that you will meet it while building, and because deciding it
inside a round scoped to the disjoint case is how scope grows quietly.

## 5. Controls

1. **Both sides rename one node to disjoint destinations → the thirteenth fires**, with both destinations
   recoverable from the witness. This is the case with no resolution path today.
2. **Rename onto an occupied path still fires `SamePathCreate`**, unchanged — a regression guard on the
   dual, not a new behaviour.
3. **A one-sided rename, and two renames of *different* nodes, fire nothing.** The witness must not
   over-fire; a spurious conflict blocks a merge that should proceed.
4. **The witness reaches the report item and carries its label** — through the real path, not a
   constructed value.

**Perturb one.** Control 3 is the one whose passing is most easily vacuous: a witness that never fires at
all would pass it. Break the detection so it fires unconditionally and confirm control 3 fails.

## 6. What this round must NOT do

- **Do not touch `SamePathCreate`**, its firing condition, or its label.
- **No read-type change** (§4o.6) — that is piece 4 and it moves four render surfaces.
- Do not touch `patch_replay`, the seal path, `rename_nodes_checked_batch`, the declaration store, the
  disclosure lines, or the move hint.
- Do not add a resolution *mechanism*. This round makes the conflict **visible and named**; choosing a
  winner is a merge-execution question and is not in §4i.2's scope.
- `MILESTONES.md` untouched.

## 7. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9. **State rule 9's cross-target result, or that
it does not apply and why.**

`CHANGELOG.md`: a new witness kind is a user-visible addition to merge evidence and its label is an
external interface — it earns a line. Check whether the merge-evidence documentation enumerates the
kinds; if it does, the thirteenth belongs there too.

Report to `.git-exclude/review-request/`. Lead with control 1, state the chosen label verbatim, answer
§4's question, and say whether the completeness sweep failed before your change.
