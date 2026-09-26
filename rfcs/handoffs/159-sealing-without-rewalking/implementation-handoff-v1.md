# Seal from the nearest authenticated anchor — RFC 159 implementation, handoff v1

**Live 2026-09-26, and it is next.** RFC 159 was accepted by the owner (*"Yes. Accepted."*), with all seven rulings of
its §8. **This ships as 0.48.0**, cut as soon as it is accepted, ahead of RFC 158 Stages B and C.

**Read first:**
- `rfcs/accepted/159-sealing-without-rewalking-the-lineage.md` §8;
- your own design report, `.git-exclude/review-request/rfc159-design-round-report-v1.md`;
- its review, `.git-exclude/reviewed/rfc159-design-round-review-v1.md`.

Your prototype (`rfc159-prototype-v1.patch`) is the starting point, **not** the design. The rulings below change it.

**Next after this, in order:** a time budget and a watcher for the measurement harnesses (owner-approved 2026-09-26;
it goes live when this round is accepted), then 0.48.0 release prep, then stack B (a later increment), then the gate plan.

## 1. What lands

1. **One anchor-trust function.** It is the only way a snapshot's *state* may be used. It returns a usable anchor
   only if **all four** hold:
   - the block is in the replay-verified record;
   - its manifest passes `validate_snapshot_manifest`;
   - **its maintainer signature verifies against a key in this repository's adopted maintainer trust policy** (RFC
     159 §8.2);
   - it is at most 63 blocks back.

   Anything else means no anchor, and the caller runs today's full derivation.
2. **Stack A: block-state derivation from that anchor.** Your prototype's `anchored_parent_state`, hooked where
   `derive_next_state_for_candidate` is reached, so **one function serves `seal`, `merge` and seal-from-accepted**.
   - The id-only walk keeps every check that made items 1–8 of your §1 class (b): cycle, presence, type, schema,
     shape, genesis, and every patch's presence, type, schema and decode.
   - The fold after the anchor is the ordinary fold, with a root comparison per folded block.
3. **The same function now governs RFC 136 2b's anchored worktree writes.** There are two sites, and both trust the
   record alone today:
   - `patch_replay.rs::replay_for_verified_worktree_write`: anchored `checkout --patch-materialize*` and
     `branch switch`;
   - `worktree.rs:86`: whether `--snapshot-materialize` may skip the provisional marker. An anchor that fails the
     new rule means **the marker is set**, and the derivation gate applies.
4. **2c's anchored *text* is unchanged**: it is hash-checked, so it certifies itself.
5. **Failures are classified, not folded** (the 2c pattern). An anchor that is simply not usable is **silent**, and
   the full walk runs; a missing record, an unadopted signer and too great a distance all count here. A **recorded**
   anchor whose manifest fails validation, **or whose signature fails to verify against an adopted key**, is an
   integrity signal: **name it on stderr** through `take_anchor_fallbacks`.
6. **The snapshot writer's presence check** (§8.6). Where the checkpoint writer reads a blob only to learn whether it
   is stored, it asks the index instead. Carry an identity control on every snapshot the corpus writes.
7. **Remove the prototype's switches**: `PRIKK_RFC159_ANCHOR`, `_TRACE` and `_SKIP_RECORDED_ROOTS`. **No
   environment knob decides how a signed root is derived.** For controls, use `cfg(test)` or `test-support` probes.
8. **What `seal_block` records.** It records the blocks it folded, plus the new block, as the prototype does.

**Not in this round:**
- stack B (the cold baseline, `merge-evidence`'s baseline) — a later increment;
- the reanchor and `verify_divergence`, which are never anchored;
- any history cache;
- `CHECKPOINT_CADENCE`;
- any format change.

## 2. Controls — landed, each shown red

**Identity**
1. **Whole-state identity at every block** of a non-ignored fixture history. The history crosses checkpoints, keeps a
   tombstone, restores a node, renames, and holds a merge block below and above the anchor. The comparison is
   against an independent forward replay. **Perturb:** skip `seed_tombstone`.
2. **The negative restoration.** A restoration that does not match its tombstone is refused by both derivations. This
   is the control that sees the history fields; byte-identity of commands cannot, as your round showed.
3. **Byte-identity of commands**, the corpus-scale `#[ignore]`d instrument: blocks, signatures, refs and snapshots
   identical with and without anchoring, for `seal`, `merge` and seal-from-accepted.

**Anchor trust (ruling 2), each on both users: stack A and the 2b worktree writes**

4. **An anchor whose signer is not adopted is never used.** The full walk runs, and the worktree write sets the
   provisional marker. **Perturb:** drop the signature condition.
5. **The laundering case, closed.** Take the lying-snapshot fixture and **write its id into the record by hand**. With
   an unadopted signer, it is refused. Your prototype's `…_and_then_it_is_believed` control becomes this one.
   **Perturb:** drop the signature condition, and it is believed again.
6. **A recorded anchor whose signature fails against an adopted key is named on stderr**, and the result is the full
   walk's.
7. **Every other condition, one control each:** not recorded, manifest invalid, more than 63 blocks back. Beware a
   masked guard, the shape this project has now met three times. Each control must reach its own condition past
   every earlier one.

**The (b) items, each with a damaged object below the anchor**

8. A missing patch, a patch of an unadmitted schema, a block of the wrong shape or schema, and an undecodable block.
   Each must still make `seal` refuse. Your round could not build these fixtures. **Build them now**, with a store
   test helper that writes the damaged object directly.

**C1 (ruling 1)**

9. **Damaged content below the anchor that no live node names.** `seal` succeeds, and `verify` reports it. **This
   pins the accepted behavior deliberately.**

**Per call site**

10. `prikk seal`, `merge`, seal-from-accepted, `replay_for_verified_worktree_write`, and `worktree.rs:86`. Remove the
    anchoring (or the new trust check) at each site, and its control, and only its control, goes red.

## 3. Measurement — release, as the design round measured

Repeat, with **this commit's** binary against 0.47.0's, on the same plan, three samples, alternating:
- the build-cost curve to 1,024 (and to 2,048 under the 2-hour rule);
- one-seal time and peak RSS at depths 256 and 1,024, ordinary and checkpoint;
- the 64-block seal-from-accepted catch-up.

**The acceptance figure is peak RSS of one seal at depth 1,024**: the prototype's 30 MiB, against 0.47.0's 1.8 GiB.
Report the snapshot writer's presence check separately: checkpoint seal before and after. Load, binaries by sha256
and `opt_level`, and nothing from the architect while you measure, as always.

## 4. Docs and CHANGELOG

- **CHANGELOG** (`### Changed`): `seal`, `merge` and a `sync` catch-up no longer walk the whole history, with release
  figures. Also a line saying that **a checkpoint is now used only when an adopted maintainer signed it**, and that
  otherwise the command does the full walk, as before.
- **`current-state.md`**: the sealing bullets say what changes in 0.48.0, with this round's figures, beside 0.47.0's.
- **One sentence where `seal` is documented:** it no longer re-reads the content of files no current file refers to;
  `verify` still checks them (C1).

## 5. The report

`.git-exclude/review-request/rfc159-implementation-report-v1.md`: the gates on the exact final commit, each control
with the perturbation that turned it red, the measurement tables, and anything in this handoff that is not true at
source.

## Addendum 1 — 2026-09-26: the interrupted measurement session, ruled

Report: `.git-exclude/review-request/rfc159-measurement-session-interrupted-report-v1.md`. The session stopped because **the
host was powered off** (`systemctl poweroff` from the user session, at 02:04:37), not because of the product or the
instruments. The acceptance figure is already in hand: one `seal` at depth 1,024, **1,803 MiB → 29 MiB** peak and
4,312 → 460 ms, three rounds each.

1. **Combining boots: yes.** The completed build curves and seal table (boot 22:04–02:04) may stand beside the remaining
   steps from a later boot, on two conditions:
   - every **compared pair** (base against impl, and nopres against impl) is measured **adjacent and within one boot**;
   - every table states its boot id, the load at start, and the binary sha256s.

   No ratio may pair a base figure from one boot with an impl figure from another.
2. **A + C: approved.**
   - **A** is the resumable per-step driver, with `.done` records carrying the boot id, binaries and load, and started
     with `setsid`. It lives in your scratch area, not in the repository.
   - **C** splits the work into units:
     - (i) the `sync` catch-up ×6, timed, alternating;
     - (ii) the corpus-scale identity probes and `identity_by_cli`;
     - (iii) `identity_of_every_snapshot` and the two shapes histories.

     Each unit can be rerun alone.
3. **B (holding shutdown): not approved.** The owner's machine is the owner's to switch off. With A and C, a power-off
   costs one step. The architect has asked the owner only whether a window is convenient. **Do not use
   `systemd-inhibit`.**
4. **The sequence: confirmed.**
   1. Unit (i) first. **Nothing heavy runs beside it**, including anything of the architect's.
   2. Then (ii) and (iii). They are correctness checks, so load does not matter to them, but nothing else of yours runs
      in the main tree meanwhile.
   3. Then the docs, then the 14 gates on the exact final commit, then the report.

   **The review waits for the corpus-scale identity**: it is §2 control 3, and the report is not complete without it.

**ACCEPTED 2026-09-26** (`4e11ce26`, `5fc00556`, `02f10add`, `0a5d6ead`; review `rfc159-implementation-review-v1`;
14/14 gates re-run by the architect; the acceptance figure reproduced on the final commit's own release binary: an ordinary
`seal` at depth 1,024 peaks at about 30 MiB, against 0.47.0's 1.8 GiB). **This handoff is closed.** Next, live:
`rfcs/handoffs/133-performance-cost-and-its-evidence/measurement-budget-handoff-v1.md`.

## Addendum 2 — 2026-09-26: `main` is red on Windows; fix first, before the measurement-budget round

**Live, and it is next.** CI run `36202895907` on `08c87271`: 15/16 green, **Windows mutation test suite red**.
`block_state::anchored_parent::tests::no_environment_knob_and_no_production_never` flags
`crates\prikk-store\src\snapshot\tests\writer.rs:1086` as a production file naming `StateAnchoring::Never`. The scan
skips test files by matching the **string** `"/tests/"`. On Windows the path uses `\`, so the test file is not
skipped. **A test defect, not a product defect.** Linux and macOS are green because their separator is `/`.

1. **Classify by path components, not by the path's spelling.** For example, skip when any component of the path
   *relative to `src`* is `tests`, the file name ends with `tests.rs`, or a component is `test_gates`. Grep the whole
   workspace for other string-matched `/tests/` or `/` path filters in test code, and fix them the same way; list each
   one you find, or say there are none.
2. **The control keeps its teeth.** Put `StateAnchoring::Never` into `merge/execute.rs` again: the test goes red **on
   Linux**, as before.
3. **Proof is the Windows job itself.** Local gates run no Windows tests (cross-target clippy only compiles). The
   architect pushes your fix and reads the **Windows mutation test suite by name**. The round is closed only when it is
   green.

Gates on the exact final commit, as always. Report: `.git-exclude/review-request/rfc159-windows-path-fix-report-v1.md`.
Then the measurement-budget round, which is unchanged.
