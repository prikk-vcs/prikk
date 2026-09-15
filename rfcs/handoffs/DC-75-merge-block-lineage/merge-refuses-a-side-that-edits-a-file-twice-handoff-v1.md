# Merge refuses a side that edits one file twice after the baseline — and calls it damage

**Live 2026-09-15.** This is a pre-existing product defect found while reviewing RFC 136 increment 1b.
The team's 130-block control dodged it and described it as *"a merge limitation independent of
checkpoints"*. That description is right that checkpoints are unrelated, but wrong to leave it as a
footnote.

- **Order:** this goes before RFC 136 increment 2.
- **Release:** 0.43.0 does not cut without it.

## 1. Measured by the architect, not inferred

**The history** is isolated and built with the real binary:
1. `setup`;
2. create `e.txt` and `f.txt`, `commit`, `seal`, which is the baseline;
3. `branch create heads/side`;
4. N text edits of `e.txt` on `main` (each a `commit` + `seal`);
5. `branch switch heads/side`, then M text edits of `f.txt` (each sealed);
6. `branch switch heads/main`;
7. `merge-evidence --baseline-block <baseline> --left-ref heads/main --right-ref heads/side`, then
   `merge --allow-no-audit --baseline-block <baseline> --into heads/main --from heads/side`.

A side with no edits adds one new file instead.

| edits of one file after the baseline, main / side | evidence `reason` | `merge` |
|---|---|---|
| 1 / 0 | `proven_confluent` | exit 0, `verify` clean |
| 1 / 1 | `proven_confluent` | exit 0 |
| **2 / 0** | **`pair_replay_failed`** | **exit 1** |
| **0 / 2** (on the adopted side) | **`pair_replay_failed`** | **exit 1** |
| **2 / 2** | **`pair_replay_failed`** | **exit 1** |

Also refused: one edit before the branch and three on `main` after it, and seventy edits before the
branch and three after.

**The message** is `error: integrity error: merge refused: heads/side is not confluent with heads/main
from baseline …`.

**Identical on two builds:** `c786e7f9`, which is before RFC 136 increment 1b, and `f3723bd4`. So it
predates the checkpoint writer, and very likely every release since merge shipped (DC-74, 0.19.0).

**Two things are wrong:**
1. **A false refusal.** In the 2 / 0 row the sides touch different files, so the histories are
   trivially confluent. Editing a file twice on a branch and then merging is ordinary use.
2. **The wrong class.** `integrity error` tells the user the repository is damaged, and `verify` says
   it is not (RFC 132).

## 2. Find the cause first — do not take mine

The neighbourhood below is not a diagnosis:
- `patch_algebra/commutation.rs:140` `prove_pair_replay` replays pairs in both orders against the
  baseline state.
- `patch_algebra/tests/confluence.rs:81` defers a sequence-internal dependency. Two edits of one node
  within one side are exactly that.
- After `EditText`, a text node's `blob_id` is an unstored content identity
  (`text_span.rs:105`, DC-65). The missing-text fallback exists only for queued WAL patches
  (`lifecycle_cache/replay.rs:449`).
- DC-75's note `reachability-vs-state-derivation-answer-v1.md` §2 records `PairReplayFailed` as *"the
  proof engine breaking on an input it was never built to receive"*.

**Method:**
1. Write the smallest failing store-level test first. Two same-node edits on one side and an unrelated
   create on the other.
2. Name the cause with the line that fails.
3. If the fix changes what the confluence proof accepts, report the cause and the proposed change
   before implementing it. That is a design ruling.

## 3. Required outcomes

**Must pass:**
- **The five table rows,** plus ≥ 20 edits of one file on one side, a rename of an edited file, and a
  delete of an edited file. Each proves confluent, merges, and leaves `verify` clean. The merged tree
  must equal the tree replaying both sides gives.
- **A history crossing a checkpoint** (post-1b): 70 blocks, branch, then edits after the branch on both
  sides, merged.

**Must still refuse, with the right reason:**
- `main` edits `e.txt` twice and the side edits `e.txt` once. This is a real overlap and must refuse as a
  conflict (`pair_conflict` or the designed witness), not as `pair_replay_failed`.
- **DC-75's re-adoption guard:** `dc75_merge_block_lineage.rs` passes unchanged. Repeated merges
  between the same pair still exclude baseline-reachable patches.

**Also fix:** restore post-baseline edits in RFC 136's control
`snapshot::tests::writer::a_checkpoint_never_fails_a_seal_that_would_succeed_without_it`
(`writer.rs:489`). Put edits on `main` between the branch at block 100 and the merge at 129, as the
first run had them.

## 4. The refusal class — ruled

A merge refused because the evidence does not prove confluence is not damage.

- **The ruling:** refuse as `Precondition`. The exit code stays 1 and the message keeps its words; only
  the `integrity error:` class prefix changes. Do this as its own commit, per site in the RFC 132 mould.
- **What stays `Integrity`:** a merge whose *inputs* are damaged (a missing block, a malformed patch).
- **Inventory:** list every refusal site in `merge/execute.rs` and `merge/evidence.rs` with its class
  before and after, and name the ones you leave as they are.
- **`pair_replay_failed` itself:** once the false case is gone, if it can still be reached, report what
  reaches it. If it now signals only an engine fault, the prose should say so.

## 5. Controls, docs, report

- **Every control must be able to fail.** Perturb the fix away: the 2 / 0 row must refuse again.
  Perturb the class back: the class control must fail.
- **`CHANGELOG.md`:**
  - `### Fixed`: merging a branch that edits a file more than once after the branch point;
  - `### Changed`: the non-confluent refusal is a precondition, not an integrity error.
- **Docs:** check `merge.md` and `merge-evidence.md`. If either states what a refusal means, it must
  still be true.
- **Gates:** the full set, plus the cross-target addendum from the diff.
- **Report:** `.git-exclude/review-request/merge-two-edits-report-v1.md`.

## 6. Rulings on the cause report — 2026-09-15

The cause report is `merge-two-edits-cause-report-v1.md`, and the review is
`.git-exclude/reviewed/merge-two-edits-cause-review-v1.md`. The cause is verified at source and by running
the ignored test. `749365ee` and `a76ff611` are accepted.

**R1 — Option A, with these conditions.**
1. **Validate before folding.** Every original operation passes provenance and sealed-evidence validation
   before any fold, in today's scan order. A fold never hides an evidence error or a malformed operation.
2. **The guard.** A fold is used only when replaying the folded side equals replaying the original side
   (`OracleState`, texts included). Otherwise the original sequence is judged exactly as today.
3. **Fold kinds.** Name each fold kind you implement: `EditText` runs; edits then `DeleteNode`; and, if
   the guard holds, `ChangePerm` or `ReplaceBinary` runs. Give each its own control. Nothing else folds:
   renames and symlinks stay deferred, and create-then-edit follows R4.
4. **A net no-op** (edits that restore the baseline bytes) drops that node from the side. A side that
   folds to nothing is judged as empty. Add a control for both.
5. **Deterministic.** The same history gives byte-identical `merge-evidence` output, prose and JSON.
6. **Report indices refer to the original sequence, not the folded one.** A folded item names the range
   of original operations it stands for: the first and last index, and their `op_seq`s. The sequence
   summary keeps the original lengths. A user reads these lines to find the operation, and "the last
   operation folded in" hides the others. The new JSON fields are additive; no known consumer reads
   `merge-evidence`.
7. **Execution unchanged, and the §3 controls stand.** The merged tree equals replaying both sides'
   original patches, and `verify` is clean. Perturb the fold away and the 2 / 0 row must refuse again.

**R2 — yes.** Both designed witnesses are accepted for main 2 / side 1 on one file.
- **Why `pair_conflict` stays honest:** `span_id` v2 (`text_span.rs:187-203`) hashes the node, the
  replaced baseline bytes and both anchors. An identical `span_id` across sides means both sides' net
  changes replace the same baseline region, so the claim is `pair_conflict` (`TextSpanOverlap`).
- **Otherwise** the claim is `SameNodeTextCommutationDeferred`. Report the exact labels the CLI prints
  for both.

**R3 — dropped from this handoff.**
- **What is removed:** the must-pass row "a rename of an edited file" cannot pass here.
- **Measured by the architect on the binary:** *any* branch containing `prikk mv` refuses to merge
  (`unsupported_operation`). That holds for a rename alone, a rename then an edit, and an edit then a
  rename. It is RFC 144 §4r.1's designed deferral (`RenameDeferred`).
- **This round:** `merge.md` says it plainly — a side containing a rename is not merged yet, and why.
- **Later:** rename replay in the confluence oracle is its own ROADMAP row.

**R4 — in scope, as its own commit after A.**
- **The shape:** create-then-edit folds into one `CreateFile` of the side's final content.
- **Kind evidence:** it comes from the *original* create's stored Blob, since `EditText` never changes a
  text file's kind.
- **What the oracle must not do:** read the final, unstored content id.
- **When to stop:** if the oracle needs more than the kind from that Blob, report before implementing.
- **The guard (R1.2)** applies.

**The inventory — ruled per site, as its own commit in the fix round (RFC 132 mould):**

| site | ruling |
|---|---|
| `merge/execute.rs:142` — {from} has no patches to adopt since baseline | `Precondition`: nothing to do is state, not damage |
| `merge/evidence.rs:330` — baseline Block is not an ancestor of target | `Precondition`: `--baseline-block` is user input |
| `merge/execute.rs:162` — ref advanced during evidence gathering; retry | `LockConflict` if its documented meaning is "another writer got there first; retry", otherwise `Precondition`. Quote the variant's doc to justify the choice |
| `execute.rs:150`, `evidence.rs:117` ref not published; `evidence.rs:141` received ref does not exist | **left as they are.** The same condition is `Integrity` in `refs.rs:148` and `InvalidName` in `rollback/draft.rs:216`, so fixing merge alone would add a third answer. One cross-command sweep, recorded in ROADMAP §C |

**CHANGELOG entries:**
- `### Fixed` for the merge;
- `### Changed` for each class moved.

**After the fix,** report what can still reach `pair_replay_failed`. If only crafted or damaged input, or
an engine fault, can reach it, say so. The class for that case is ruled from your measurement, not before
it.

**Report:** `.git-exclude/review-request/merge-two-edits-report-v1.md`.
