# RFC 136 increment 2b — the replay-verified block record, and anchored worktree writes

**Live 2026-09-15.** Read RFC 136 §10.3c ruling 2 first. 2a's review is
`.git-exclude/reviewed/rfc136-increment-2a-review-v1.md`.

**The rule this increment implements.** State from a snapshot may reach the worktree only through an
anchor this repository has replay-verified. Every worktree write that cannot find such an anchor replays
from genesis, exactly as today.

**Commits:**
1. the record;
2. anchored worktree writes, and the `--snapshot-materialize` marker skip;
3. the carried items;
4. docs.

Deliver one report.

## 0. Two carried items from 2a's review (their own commit)

1. **`lifecycle-cache-divergence` does not block clearing the provisional marker.**
   - **Set** `blocks_provisional_clear: false`, with the reason in the row: the DC-64 cache is rebuildable
     and never authoritative, a divergence is a cache fault, and `verify`'s own replay decides block state.
     Counting it keeps a marker set for a reason unrelated to the snapshot, with a recovery no user would
     find.
   - **Control:** a stale lifecycle cache plus a set marker, then `verify` clears the marker.
2. **A `test-support` helper that writes a block whose snapshot is damaged, or lies** (§2 below). Add a
   binary control for 2a's read-only fallback: stdout equals full replay, stderr has the one warning line,
   exit 0.

## 1. The record

**What it is.** A monotonic set of block ids whose `state_merkle_root` this repository confirmed by replay.
- **Where:** `.prikk/cache/` (`layout.cache_dir()`), named through `RepositoryLayout`.
- **Its standing:** rebuildable and never authoritative, exactly like the DC-64 lifecycle cache
  (`lifecycle_cache/incremental.rs`: `write_file_atomically`, a magic, a schema version, a checksum, and
  `load` returning nothing on any failure).

**Format:**
- **Keyed by the replay semantics.** The file records the `prikk-store` crate version. A file from any
  other version reads as empty, so a replay fix never inherits an old verdict.
- **Fails safe.** A missing, truncated, mis-checksummed or other-version file is the empty set: no error,
  and replay runs in full.
- **Writes are best-effort** read-merge-write. A lost update only drops entries, which is the safe
  direction. Say how a concurrent writer can at worst lose entries, never add one.

**Writers** — only operations that have just confirmed roots by replay:
1. **`seal_block`**, which covers `seal`, `merge`, `sync seal --claim` and the RFC 111 simulation. Its
   derivation already replays and checks every block on the parent lineage: `verify_v2_lineage_roots`
   inserts each block into `LineageStateMemo::verified` only after `computed == payload.state_merkle_root`
   (`block_state.rs:693-702`). The new block's root is computed, not asserted.
   - **Record:** the memo's keys plus the new block id, after the Block is written.
   - **To reach them:** thread the memo out of `derive_next_state_for_candidate`.
2. **`verify`:** every block whose `BlockStateStatus` is `Verified`, whatever else the run found. Each such
   outcome is individually sound.

**Never writers:** `bundle import`, `sync accept`, `branch create`, any `checkout`, `status`,
`worktree-status`. None of them replays roots.

## 2. Anchored worktree writes

**The variant.** `Anchoring` gains `VerifiedWorktreeWrite`: anchor only at the nearest snapshot whose block
id is in the record **and** that passes the loader; otherwise replay from genesis.
- **Rollback preview stays `Never`** (§10.3a ruling 3).
- **The guard test is extended:** name, per variant, the files allowed to use it. A new file using either
  anchored variant must fail, naming the file.

**Callers:**
- `checkout --patch-materialize`;
- `--patch-materialize-delete`, whose `deleted_files` come from decoding the pre-anchor chain, as in 2a;
- `branch switch`, whose target manifest is state only (`branch_switch.rs:126-179`).

**A verified anchor that fails the loader** (damage after verification) falls back to genesis, with 2a's
stderr line. The worktree output is unchanged.

**`checkout --snapshot-materialize`:** if the target block is in the record, write no provisional marker,
because the content is what replay gives. Otherwise the marker and the gate apply exactly as in 2a. State
which one happened in the command's prose output.

## 3. Controls (each must be able to fail)

- **Writers.** A store test: after `seal`, `merge`, `sync seal` and `verify`, the expected ids are recorded.
  After `bundle import`, `sync accept` and `branch create`, no new id is recorded.
- **Byte identity, verified anchors.** At a depth past two checkpoints, with edits, a pre-checkpoint delete
  whose stale file is back in the worktree, a rename, `chmod` and a binary:
  - `--patch-materialize`, `--patch-materialize-delete` and `branch switch` produce identical worktrees
    (every file's bytes and executable bit, deleted paths) and identical reports, anchored against genesis;
  - the snapshot-load counter is above 0 for the anchored run.
- **The lying anchor — the reason for this increment.** Use the §0 helper to build a received history:
  1. a block whose snapshot recomputes to its **signed** root, while its patches produce a different root;
  2. import it as a bundle, then `branch create heads/x --from remotes/...`.

  Then:
  - `--patch-materialize --ref heads/x` does **not** anchor (counter 0) and refuses exactly as full replay
    does;
  - `--snapshot-materialize --ref heads/x` sets the marker and `commit` refuses;
  - `verify` reports the block's state failure, the marker stays, and the lying block is **not** recorded.
  - **Perturb:** skip the membership check. The materialize writes the lie, and the control fails.
- **Record failure is safe.** Flip a byte, truncate the file, or change the recorded version: each gives
  full replay, identical output, and no error.
- **The marker skip:** `--snapshot-materialize` of a sealed local tip writes no marker, and `commit`
  proceeds. The same tip after deleting the record file sets the marker.

## 4. Cost

Measure `--patch-materialize` and `branch switch` wall time and peak memory at the five §9.3 depths,
before (`2d96196b`) and after, on the RFC 139 corpus. Use one repository, sealed locally, so every
checkpoint is verified. Write to `.git-exclude/measurements/rfc136/`. If the record costs more than it
saves, report and stop.

## 5. Docs, CHANGELOG, discipline, report

**Docs:** `snapshot-materialization.md` states when the marker is skipped, where the record lives, and that
deleting it is always safe.

**CHANGELOG:** `### Changed`, twice:
- `--snapshot-materialize` of a replay-verified block no longer marks the worktree;
- `lifecycle-cache-divergence` no longer keeps the marker.

**Before every commit:** fmt, workspace clippy with 0 errors, workspace tests with 0 failed, and the
staged list equal to `git status`. Run the full gates on the final commit. Report:
`.git-exclude/review-request/rfc136-increment-2b-report-v1.md`.

**Hard stops, reported:**
- a worktree write anchored at an unrecorded block;
- a record writer that did not replay the roots it records;
- a record read that errors instead of reading empty;
- a lying anchor that reaches the worktree.
