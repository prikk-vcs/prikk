# RFC 136 increment 2a — the provisional marker, the derivation gate, anchored read-only reports

**Live 2026-09-15.** Before starting, read RFC 136 §6, §10.3, §10.3a, §10.3b and **§10.3c**. §10.3c splits
increment 2 because anchored state may reach the worktree only through a replay-verified anchor, and that
record is 2b.

**This handoff is 2a only.** No anchored worktree write and no anchored baseline reconstruction in this
increment: `checkout --patch-materialize`, `--patch-materialize-delete`, `branch switch` and `commit`'s
baseline all keep replaying in full.

Commit in three pieces, in this order, and deliver one report:
1. marker and gate;
2. anchored reports;
3. docs.

## 0. First section of the report: the classification table

For each reader this increment anchors, list every output field with three columns:
- **State or history:** state is a function of the tip's state; history is a function of the chain.
- **Source when anchored:** the snapshot, or the whole chain decoded without applying.
- **The line that produces it today.**

**Readers:** `checkout --patch-plan` (prose and `patch-plan-content-v1` JSON, including `coverage` and
`--content-path`), `--patch-delete-plan`, `bundle preview`.

**The anchor rule:** a field on no row does not anchor. Checkout's file-level replay checks only live
state (`patch_replay/apply.rs:62-122`: path occupancy, already-live node id, delete preimage), so its
history is exactly `deleted_files` and the counts and kinds.

## 1. The provisional marker (RFC 136 §10.3b.1–2)

**Where it lives.** A file under `.prikk/`, named through `RepositoryLayout`, beside the RFC 102
worktree-dirty marker (`worktree_marker`). It holds the ref and the snapshot's block id.

**Who writes it.** `materialize_snapshot_checkout` (`worktree.rs:48`) writes it durably **before** its
first worktree write, next to `mark_worktree_dirty`. A crash after the marker and before any file leaves
the marker set; that is fail-closed.

**Who clears it.** Only `prikk verify`. The run must hold no `verify_verdict::VERDICT_CONDITIONS` condition
about objects, WAL, refs or block state.
- **Your table:** list every condition, marked counts or excluded, with the reason. A publication-trust
  issue alone does not block clearing.
- **Compare-and-remove, under the active lock:** read the marker's bytes at the start of `verify`, and
  remove the file only if the bytes are unchanged at the end. A `--snapshot-materialize` that runs during
  a `verify` must keep its marker.

**Who reports it.**
- `status`, prose and JSON: `provisional worktree: materialized from the snapshot of <block> on <ref>;
  not replay-verified — run prikk verify`.
- `doctor` lists it.

## 2. The derivation gate (§10.3b.3)

**One function, one message.** A single check returns `Precondition` while the marker is set, naming the
ref, the block and `prikk verify`.

**Callers, before any write:**
- `commit` and `mv`;
- `seal`, `merge`, `sync accept` and `sync seal`;
- `rollback-draft --append-inverse`;
- `branch switch`.

**The guard test** is modelled on `every_object_store_writer_goes_through_a_locked_caller`: a table of the
eight entry points, and a test that each calls the check. Removing one call must name that command.

**Controls:**
- materialize, then every gated command refuses: one row each, exit 1, the message, nothing written;
- `verify` clean, then `commit` proceeds;
- `verify` with a damaged object: the marker stays;
- `verify` with only a trust issue: the marker clears;
- the compare-and-remove race: the marker is replaced mid-verify and survives;
- a crash injected after the marker write and before the first file: the marker is set and `commit`
  refuses.

## 3. Anchored read-only reports (§10.3c ruling 1, §10.3a ruling 5)

**Which reports.** `replay_supported_patch_chain` gains an explicit purpose.
- **Read-only reports:** `--patch-plan`, the content report, `--patch-delete-plan`.
- **Worktree writes:** `--patch-materialize`, `--patch-materialize-delete`, `branch switch`.
- **Only read-only reports anchor.** Say how a future caller cannot pick the wrong purpose by accident.
- **Bundle preview** anchors each walked chain the same way.

**The anchor.** The nearest block on the single-parent chain whose snapshot passes `load_block_snapshot`.
- **Seeded from the snapshot:** files and live nodes (path, kind, mode, blob).
- **Replayed:** only the blocks after it.

**History fields over the whole chain, decoded, never applied:**
- `patch_count` from block payloads;
- `applied_operation_count` and `coverage.applied_operation_kinds` by decoding every patch;
- `deleted_files` by walking decoded `DeleteFile`s in order. A later `CreateFile` at the same path removes
  the entry, the same rule as `apply.rs:72` and `:122`.

**A snapshot that fails the loader** (§10.3b.4) falls back to full replay.
- **Stdout** is unchanged.
- **Stderr** gets one line naming the block, the `Integrity` finding and `prikk verify`.
- **Exit 0.**

**Controls, each able to fail:**
- **Byte-identity.** A 130-block history with:
  - edits across blocks;
  - a pre-checkpoint `DeleteFile` whose stale file is in the worktree;
  - a rename;
  - `chmod`;
  - a binary replacement.

  Every anchored report, prose and JSON, must equal the same report with anchoring disabled, as must a
  bundle preview of that history. Keep the test-only switch pattern of `without_checkpoints_for_test`.
  **Perturb:** stop decoding pre-anchor patches, and `coverage` and `deleted_files` must differ.
- **A tampered manifest at the anchor:** stdout equals full replay, stderr names the finding, exit 0.
  **Perturb:** remove the fallback, and the command refuses or differs.
- **Worktree writes stay unanchored:** `--patch-materialize` on the same history reads no snapshot. Assert
  it through a counter, not through timing.
- **Cost:** `--patch-plan` wall time and peak memory at the five §9.3 depths, before and after, on the
  RFC 139 corpus. The measurement writes under `.git-exclude/measurements/rfc136/`. If decoding the chain
  eats the saving, report the numbers and stop.

## 4. Docs, CHANGELOG

**Docs:**
- `snapshot-materialization.md`: the marker, the gate, and how `verify` clears it;
- `troubleshooting.md`: the gate's message and the stderr fallback line;
- `status.md`: the new line.

**CHANGELOG:**
- `### Added`: the provisional marker and the derivation gate;
- `### Changed`: the commands that refuse while it is set.

Anchored reports need no entry, because their output is unchanged.

## 5. Discipline and report

- **Before every commit:** `cargo fmt --all -- --check`, workspace clippy `-D warnings`, and
  `cargo test --workspace --locked --no-fail-fast`, gated on 0 failed.
- **On the final commit:** the full gate set.
- **Cross-target addendum:** the marker's file handling.
- **Report:** `.git-exclude/review-request/rfc136-increment-2a-report-v1.md`.

**Hard stops, reported:**
- an anchored path that writes the worktree;
- a history field taken from a snapshot;
- a gate you cannot route through the one check;
- a race you cannot close with compare-and-remove.
