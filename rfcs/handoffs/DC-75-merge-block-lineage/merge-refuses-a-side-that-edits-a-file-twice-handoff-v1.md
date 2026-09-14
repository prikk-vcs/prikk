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
