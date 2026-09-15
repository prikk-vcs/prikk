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

## 7. Rulings on the fix report — 2026-09-15

**Sources:** the report is `merge-two-edits-report-v1.md`, and the review is
`.git-exclude/reviewed/merge-two-edits-review-v1.md`.

**The five commits `b5b92373`..`70afbc8a` are accepted in substance:**
- all fourteen gates are green on `70afbc8a` in the architect's run, with 1,998 tests on both toolchains;
- 2 / 0 and 0 / 2 merge on the binary;
- the inventory classes are right.

**They are not pushed yet.** The architect's own ruling R1.4 (*"a net no-op drops that node"*) makes them
claim confluence that a merge cannot realize.

### 7.1 The hole — measured by the architect on a binary built from `70afbc8a`

The file `e.txt` is on both sides. The adopted side edits a word and then restores it; the target side
edits another word in the same file.

| file | direction | `merge-evidence` | `merge` |
|---|---|---|---|
| short (anchors cover both words) | side into main | **`proven_confluent`** | **exit 1: `integrity error: lifecycle replay: EditText span … could not be localized`** |
| short | main into side | `proven_confluent` | exit 0, verify clean, content right |
| long (words 200 lines apart) | both | `proven_confluent` | exit 0, verify clean |

**The cause.**
- `fold.rs`'s `Net::Drop` removes a node whose run restores the baseline. `fold_side` never sees the other
  side, so the evidence has no same-node pair for that file.
- Execution then replays the adopted side's *original* edits onto the other side's tip, where their
  anchors no longer match.

**Severity.** The failed merge leaves the repository verifying clean and `heads/main` unmoved, so the
defect is a false `proven_confluent` followed by a failure reported as damage, not a wrong tree. On `main`
today the same history refuses honestly. The error is the ruling's, not the implementation's.

### 7.2 Required before push — the next commit

1. **A net no-op drops its node only when the other side has no operation on that node.** Otherwise that
   node's run is judged as authored. The rule is symmetric.
2. **Write the principle into `fold.rs`'s module doc.** The evidence judges folds, but execution replays
   the *originals* onto the other side's tip. A fold is admissible only when the other side has no
   operation on the folded node and the run changes no path before its end. Then the originals replay
   onto that tip exactly as they did on their own side.
3. **Controls.**
   - The short-file case in both directions refuses with a same-node witness, not `proven_confluent`.
   - The long-file case still merges in both directions.
   - Perturb rule 1 away, and the short case claims confluence again.
   - **Execution soundness.** For every fold kind, with the other side (a) untouched and (b) editing a
     *different* file, `merge-evidence` says `proven_confluent` **and** `merge` succeeds in both
     directions, with `verify` clean and the expected tree.
4. **The gate slip is recorded, not rewritten.** `b5b92373`..`3df5c354` fail two release-policy pins
   that `70afbc8a` fixes, and history stays as it is. From now on, `cargo test --workspace --locked` runs
   before every commit.

### 7.3 The §6 asks and R4 — ruled

Each is its own commit, after 7.2, under the same guard and with controls in both directions.

- **R6 — mixed kinds on one node: fold.** No path changes before the end, so it is admissible under
  7.2.2.
  - **The net:** an `EditText` if the text changed, a `ChangePerm` if the mode changed, or one
    `DeleteNode` with the baseline preimage if the run ends in a delete.
  - **The tombstone exception widens** to what such a delete legitimately changes: a folded delete's
    tombstone may differ in blob id *and mode*; path and kind may not.
  - **Controls:** the three mixed shapes of your §6 merge in both directions.
- **R7 — create-then-delete: never fold.** The run holds a path for part of the side's history. Replaying
  the original create onto the other side's tip can find that path taken, so a net "nothing" would be
  7.1 again. It stays refused.
- **R8 — the label.** An operation that does not replay alone against the baseline, and that has an
  earlier operation on the *same node* on its side, is `sequence_internal_dependency_deferred`, not
  `pair_replay_failed`. Widen `has_prefix_dependency`'s escape to a same-node predecessor.
  - **The class stays `Precondition`.**
  - **After R6–R8,** report every ordinary shape that still reaches `pair_replay_failed`, and make
    `patch-algebra.md`'s row say the result.
- **R4 — create-then-edit: accepted as proposed,** with four conditions.
  - (a) **The fallback.** `current_text` falls back only for a node not live in the baseline, to
    `blob_content(candidate_scope, blob)`, where `blob` is that node's live blob in the oracle state. It
    refuses unless the kind is `Text`. This is the evidence read `create.rs:203` already makes, not a new
    one. Pass the caller's scope, never a weaker one.
  - (b) **No read of the final content.** The folded `CreateFile`'s final content id is never read; its
    kind comes from the original create's Blob.
  - (c) **Admissible only when the create persists** to the run's end. A run ending in a delete falls
    under R7.
  - (d) **A cross-side node-id reuse** that degrades to `MissingCandidateEvidence` is accepted.

### 7.4 Docs, CHANGELOG, reports

- **Docs and CHANGELOG follow the result:** `merge.md`'s "What cannot be merged yet", `patch-algebra.md`'s
  row, and the `### Fixed` entry, which currently names mixed kinds and create-then-edit as still
  refusing.
- **Reports:**
  - 7.2 is `.git-exclude/review-request/merge-two-edits-noop-report-v1.md`; the push follows its review.
  - 7.3 is `.git-exclude/review-request/merge-two-edits-folds-report-v1.md`.
