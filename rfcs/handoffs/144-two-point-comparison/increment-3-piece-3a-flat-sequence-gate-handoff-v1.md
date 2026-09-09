# RFC 144 §4r.1 — make the thirteenth witness reachable from `prikk merge-evidence`

**Ruled:** `rfcs/accepted/144-two-point-comparison.md` **§4r.1**. Follows `0e04164a`, which is
**accepted** — its classification is correct and this round does not revise it.

**This round closes a reachability gap in shared machinery.** It is not rename-specific, and it must not
become rename-specific.

## 1. The gap, verified at source twice

`check_confluence` (`patch_algebra/commutation.rs:44-58`):

```rust
let left_sequence_result = ensure_flat_sequence(baseline, evidence, candidate_scope, left)?;
let right_sequence_result = ensure_flat_sequence(baseline, evidence, candidate_scope, right)?;
if let Some(result) = left_sequence_result { return Ok(result); }   // ← returns here
if let Some(result) = right_sequence_result { return Ok(result); }
for (left_index, left_operation) in left.iter().enumerate() {        // ← never reached
```

`ensure_flat_sequence` sets `Unknown { reason }` for any operation whose `deferred_reason` is `Some`,
and for `Action::RenamePath` that is **unconditional** (`facts.rs:150`).

**So any `merge-evidence` call touching a rename returns `Unknown { RenameDeferred }` before pairwise
classification runs.** The thirteenth witness — and `same-path-create` for a rename operand — exist and
are correct, and no user can observe either.

## 2. What to change, and the constraint that shapes it

**The per-operation deferral must stop pre-empting pairwise classification** — by moving it after the
pairwise loop, interleaving it, or another shape you can defend.

**The constraint: `deferred_reason` is answering a real question and must keep answering it.** It is
about *single-operation replay support* — whether the oracle can replay this operation alone — not about
pairwise relation. A sequence containing an operation the replay oracle cannot handle **must still
report `Unknown`** when no pairwise conflict is found. **Do not delete the check; change when it decides.**

**Every witness kind flows through this path, not just the thirteenth.** A reordering that makes renames
classify while changing what symlinks, or unsupported operations, or malformed ones report is a
regression in twelve other kinds. **That is the risk of this round and where its controls belong.**

## 3. REQUIRED controls

1. **The thirteenth reaches `prikk merge-evidence`'s real entry point** — `analyze_merge_evidence`, not
   `analyze_pair_merge_evidence`. This is the round's whole point.
2. **`same-path-create` for a rename operand reaches it too** — the other newly-reachable case from
   `0e04164a`.
3. **A sequence with a deferred operation and no pairwise conflict still reports `Unknown` with its
   original reason** — the property §2 says must survive. Cover **symlink** deferral as well as rename,
   since they are different reasons through the same gate.
4. **A regression sweep across the other witness kinds**: for each kind real classification can produce,
   assert `analyze_merge_evidence` still reports what it did before this round. **Capture the before
   values from the current commit first**, and say in the report how you captured them.

**Perturb control 3.** It is the one whose passing is most easily vacuous — an implementation that simply
deleted the deferral check would pass 1 and 2 and fail nothing else obvious. Break the ordering so a
deferred sequence reports `Confluent` instead of `Unknown`, and confirm control 3 fails.

## 4. What this round must NOT do

- **Do not make the fix rename-specific.** A special case for `RenameDeferred` that leaves symlink
  deferral pre-empting classification would close the reported gap and leave the real one.
- **Do not add a resolution mechanism.** Still classification only; nothing chooses a winner.
- Do not revise `0e04164a`'s classification, `classify_rename_destination_conflict`, or
  `SamePathCreate`'s logic or label.
- Do not touch `patch_replay`, the seal path, `rename_nodes_checked_batch`, the declaration store, the
  disclosure lines, or the move hint.
- No read-type change (§4o.6) — that is piece 4 and still wants its own round.
- `MILESTONES.md` untouched.

## 5. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9. **State rule 9's cross-target result, or that
it does not apply and why.**

**`CHANGELOG.md` needs correcting, not only extending.** The open `## Unreleased` entry currently opens
*"a thirteenth conflict witness, **not yet reachable end to end**"* and leads with the gap. When this
round lands, that framing is stale — rewrite the entry to describe the shipped state rather than
appending a second entry that contradicts the first. `docs/src/reference/patch-algebra.md`'s Deferred
Work paragraph naming this gap must go too.

Report to `.git-exclude/review-request/`. Lead with control 1, then control 4's regression sweep and how
its before-values were captured — **the sweep is the part I will read hardest**, because this round's risk
is not the feature but the twelve kinds it moves underneath.
