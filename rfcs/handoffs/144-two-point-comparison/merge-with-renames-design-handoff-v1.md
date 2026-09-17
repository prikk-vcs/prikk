# Merge of a branch containing a rename — design-round handoff v1

**Order of work for 0.45.0 (the four handoffs of 2026-09-17):** 1. `136-block-aggregation-payoff/warm-cache-commit-anomaly-handoff-v1.md` (measure-only); 2. `135-first-run-entrance-and-configuration/distinct-default-key-ids-handoff-v1.md`; 3. `144-two-point-comparison/merge-with-renames-design-handoff-v1.md` (report only); 4. `132-error-taxonomy-structure/absent-and-received-ref-refusals-handoff-v1.md`. Take them one at a time, each reported and reviewed before the next.

**Live 2026-09-17.** 0.45.0 item 2 (ROADMAP row 15b). **This is a design round: it ends in a report, and no product
code changes.** Implementation is handed off only after the architect rules on the report.

## 1. Measured 2026-09-17, binary of `fab4aded`

Each case starts from a fresh repository: one sealed base with `a.txt` and `c.txt`, then `heads/feature` branched at it,
merged with `merge --allow-no-audit --baseline-block <base> --into heads/main --from heads/feature`.

| Case | `heads/feature` | `heads/main` after base | Result |
|---|---|---|---|
| S1 | `prikk mv a.txt b.txt` | edits `c.txt` | exit 1, `outcome: Unsupported, reason: unsupported_operation` |
| S2 | `prikk mv a.txt b.txt` | **nothing** | exit 1, same |
| S3 | `prikk mv a.txt b.txt` | edits `a.txt` | exit 1, same |

**Even S2 refuses**, although it has nothing to reconcile. `prikk mv` has shipped since 0.38.0, and stikk surfaces
renames, so any branch that renames a file is unmergeable.

**At source:**
- `patch_algebra/facts.rs:148` `deferred_reason` returns `RenameDeferred` for every `RenamePath`.
- `commutation.rs:187` (`ensure_flat_sequence`) defers on it before replaying.
- The replay oracle refuses `RenamePath` outright (`replay_oracle.rs:124`, together with symlinks).
- RFC 144 §4r.1 already fixed pairwise **classification** reachability for renames, without special-casing
  `RenameDeferred`. What remains is **replay**, and whatever merge execution must do with a replayed rename.
- `checkout`'s patch replay **does** apply renames (RFC 144 increment 1, `increment-1-patch-replay-rename-handoff-v1.md`).
  That is a second implementation of rename semantics, which the oracle does not share.

## 2. Constraints the design must meet

1. **Not rename-specific at the shared layer.** RFC 144 §4r.1: nothing in `commutation.rs` may branch on which deferred
   reason it is. Symlinks stay deferred, and their controls must still pass unchanged.
2. **One definition of what a rename does.** The oracle must not grow a third rename semantics. Say whether it
   reuses checkout's replay, or both move onto one shared definition, and what that costs.
3. **Node identity decides.** Operations name nodes, so a rename and an edit of the same node should commute by
   construction. The report must show that from the code, not assume it.
4. **A merge result must be byte-identical to replaying either order** where the proof says confluent. That is the
   existing confluence contract, unchanged.
5. **No format change.** If the design needs one, stop and say so.

## 3. What the report must answer

1. **Rename replay in the oracle:** preconditions (the node is live at `from`; `to` is free, or what else is legal),
   effects, and failure modes. For each failure mode, whether it is `ReplayFailure` or a new witness kind.
2. **A pair table: rename against every operation kind**, in both orders and on same and different nodes/paths:
   - create
   - delete
   - edit text
   - replace binary
   - mode change
   - rename (same node to the same path, same node to different paths, different nodes to one path, a swap, a
     chain `a→b` against `b→c`)
   - symlink create/delete, which stays deferred

   For each pair, the classification RFC 144's classifier gives **today** (cite the test, or measure it) and what it
   gives once replay exists. Flag every row where the two differ.
3. **Merge execution:** what `merge` writes for a confluent rename: the merged tree, the baseline derivation, the
   snapshot at a checkpoint, and rename declarations in the sealed patch. Does any path in execution assume rename-free
   sequences, including DC-75's two-edits fold?
4. **`merge-evidence` and `merge-plan` output** for renames, prose and JSON: what changes, and whether it is additive.
5. **Cost:** does rename replay change the complexity of the flat-sequence check or the fold?
6. **The smallest correct increment plan**, with a control for each increment that can fail. At minimum S1–S3 above
   as CLI tests, plus the S3 twin where main edits `b.txt` after its own rename.

## 4. Report

`.git-exclude/review-request/merge-with-renames-design-report-v1.md`. **Stop there.** Include what you measured on
the binary, including anything in §1 that does not reproduce.

## Addendum 1 2026-09-17 — design accepted; implement

**The design report is accepted** (`merge-with-renames-design-report-v1.md`, review `merge-with-renames-design-review-v1`).
Its four-piece design (§2.1) and its pair table (§2) are the specification. The architect checked its citations at
source: `rename_nodes_checked_batch`, the flattening `candidate_sequence`, the fold's `replay_operations(…).ok()?` bail,
the missing preimage rename arm, and the saved "today" measurement.

**Rulings on §7:**
1. **Patch boundaries: (a).** `candidate_sequence` carries each operation's patch index, and the oracle batches
   consecutive renames sharing an index through `rename_nodes_checked_batch`. A boundary is declared, never inferred
   from `op_seq`.
2. **`classify_path_relation`: skip the create/delete-typed helpers for renames**, as recommended, on one condition:
   **every `Independent` verdict on a pair containing a rename must be proven by `prove_pair_replay`**, with no path
   around it. Control: perturb the proof to be skipped for rename pairs, and a row-8/10/12-style test must fail.
3. **Identical renames on both sides stay `Conflict`** (rows 14, 23). Changing the identical-operation policy is a
   whole-policy question, not this round's. It is recorded as a ROADMAP candidate.
4. **Row 5 keeps `DeleteMutationConflict`.** A rename is a mutation of the node the other side deletes. No new witness
   kind.

**One correction to the increment plan.** §6 says increment 1 has "no CLI behaviour yet reachable except via
evidence". That is not so: `merge` runs the same confluence check (S1's refusal came from `merge` itself). So
**increments 1, 2 and 3 are one round**, and the reviewed state must never be a commit where `merge` accepts renames
without its CLI controls. Commits inside the round may be split; the report is one.

**Constraints still in force** (handoff §2):
- nothing in `commutation.rs` branches on a deferred reason, and every symlink control passes unchanged;
- one rename semantics in the algebra (`rename_nodes_checked_batch`), and no third implementation;
- no format change.

**Controls:** the report's §6 lists for increments 1 and 2, plus rule 2's proof control above.

**Docs and CHANGELOG** (increment 3):
- `guide/merge.md` drops the "not merged yet" rename paragraph and describes what refuses (identical renames, rename
  versus delete of the same node, cross-side chains and swaps);
- `merge-evidence`'s `unsupported_operation` now means symlinks only;
- CHANGELOG `### Added — merge accepts branches containing renames`, naming that change of meaning for consumers who
  read the reason.

**Report:** `.git-exclude/review-request/merge-with-renames-report-v1.md`.

**Order (2026-09-17):** this implementation comes **before** the refusal sweep
(`132-error-taxonomy-structure/absent-and-received-ref-refusals-handoff-v1.md`). Both touch `merge`, so they run one
after the other, never in parallel.
