# RFC 136 §10 — sealed snapshots, handoff v2: increments 0, 1a, 1b

**Live 2026-09-13**, replacing `sealed-snapshots-increment-1-handoff-v1.md` after your hold
(`rfc152-docs-rfc136-increment-1-hold-report-v1.md` §2). The hold was right, and it was the right kind
of stop: a snapshot whose readers misread it is the class the hard stops exist for. Ruling is RFC 136
§10.1a — read it first; §10.5 is the revised plan. In one line: **a snapshot on block B is B's own
state; a reader that seeds from it does not apply B's patches; v1 is retired; sealing becomes one
function before anything writes a snapshot.** Readers before writers, as every split here.

Each increment is its own commit and its own report; do not start the next before the previous is
reviewed. Sizes: 0 is a move, 1a is the substance, 1b is small once 0 and 1a stand.

## Increment 0 — one seal function

1. One `pub` store function (name it; `seal_block` is fine) that takes what the four callers already
   have — parent, kind, patch ids, mainline/baseline ids where applicable, the signer — derives the state
   root, builds the `BlockPayload`, signs and writes the envelope, and returns the block id. The CLI's
   `seal.rs:167-178`, `merge/execute.rs:166-175`, `seal_from_accepted.rs:228` and
   `rfc111_seal_simulation.rs:82-92` call it and build no `BlockPayload` themselves. `snapshot_blob_ref`
   is still `None`, set in exactly one place.
2. **Control — byte identity.** Before the refactor, on the current binary, seal a history that exercises
   all four callers (`seal`, a `merge`, a `sync accept`, and the RFC 111 simulation) and record every
   block id. After it, the same history yields the same ids. Pin the ids in the test, not "the count".
   The coupling gate decides where the function lives (the layer of `seal_from_accepted`, presumably);
   say which module and why.
3. Nothing else changes. Report: `.git-exclude/review-request/rfc136-increment-0-report-v1.md`.

## Increment 1a — readers on v2, v1 retired, no writer

1. **Format.** `SnapshotManifest` v2: magic `PRIKK-SNAPSHOT-MANIFEST-v2\n`, then the block's
   `StateRootEntry` set in canonical path order with the field grammar `state_leaf_preimage` uses —
   reuse that encoder; say so. `recomputed_state_root()` over the decoded set. **The v1 decoder and
   `SNAPSHOT_MAGIC` v1 are deleted.** Any other magic, including v1's, is `Integrity` naming the magic
   found (*"snapshot manifest magic `…` is not PRIKK-SNAPSHOT-MANIFEST-v2"*).
2. **Loading.** `load_snapshot_files` (and whatever `SnapshotEntry` becomes) yields per path: bytes read
   by Blob id, NodeId, kind, mode, or the symlink target. A missing Blob is `Integrity`. Then check
   `recomputed_state_root() == block.state_merkle_root` **at every reader**, `Integrity` on mismatch
   (*"snapshot manifest does not recompute to its block's state root"*).
3. **Meaning, at every reader.** `patch_replay.rs`, `patch_inverse.rs`, `bundle/preview.rs`: when a block
   carries a snapshot, seed `files`, `live_nodes` (now with real NodeIds, kinds and modes) and the
   baseline from it, clear `deleted_files`, and **do not apply that block's patches** — `continue`. The
   "mode-unaware" and "None for a path seeded by a snapshot" caveats in `patch_replay.rs:226-300`
   become false; rewrite or delete each, and say which. `checkout --snapshot-plan/-materialize`
   materialize by Blob id and mode. `node_authoring.rs:445`'s refusal (no node files and a snapshot ref):
   re-read it under the new meaning and report whether it still names a real state.
4. **Fixtures.** The three copies of `publish_snapshot_then_patch_block` (`patch_checkout/tests.rs:180`,
   `patch_inverse/tests.rs:59`, `test_support/snapshot_history.rs:17`) become one shared v2 fixture
   whose snapshot block **has patches** and whose manifest is that block's post-state (the state the
   block's own `state_merkle_root` hashes). `snapshot/tests.rs`'s zero-patch snapshot block gains a
   patch too, so the meaning is exercised there as well; a fixture on which both meanings agree tests
   neither.
5. **Controls** (each must be able to fail; perturb and show the message):
   - Your §2.2 measurement as a permanent test: post-state snapshot on a block whose patch creates
     `a.txt` → `replay_supported_patch_chain`, the deletion plan, materialization and
     `prepare_patch_inverse_plan` all succeed and yield `a.txt` once; delete the `continue` and the test
     must fail with the overwrite error you saw.
   - Snapshot on a block with patches, then two more patch blocks: checkout by replay equals checkout
     from the same history with the snapshot removed, byte for byte (readers with and without an
     anchor agree).
   - Tamper one manifest byte → every reader refuses as `Integrity`; the block still verifies by replay.
   - A v1-magic Blob in `snapshot_blob_ref` → `Integrity` naming the magic.
   - `checkout --snapshot-materialize` on the shared fixture yields the tip's state (not its parent's).
6. **Nothing writes a snapshot yet.** `verify` unchanged in this increment. No CHANGELOG line (nothing
   user-visible changes: no released binary ever wrote a snapshot). Full gate set; addendum from the
   diff. Report: `.git-exclude/review-request/rfc136-increment-1a-report-v1.md`.

## Increment 1b — the writer

1. `CHECKPOINT_CADENCE: u32 = 64`, one constant read by the reanchor (`REANCHOR_BOUND` becomes it) and
   by the seal function of increment 0: write a v2 snapshot when the new block's depth from the nearest
   snapshotted ancestor on its ref is `>= CHECKPOINT_CADENCE`, or there is none. The manifest is the
   **new block's** state — computed from the same state derivation that produces `state_merkle_root`,
   so the two cannot disagree; write it as a Blob under the object-store lock; set `snapshot_blob_ref`
   before the block is encoded and signed.
2. `verify`: for every block with a snapshot, the Blob exists (already) and the manifest recomputes to
   the block's root; a failure is an `Integrity` finding in the objects stage.
   `CHANGELOG.md` `### Added` for that finding alone.
   **Content Blobs (ruled 2026-09-15, increment 1a review).** For each file entry, if the store does not
   hold its `blob_id`, write the Blob — the same schema-1 envelope `write_content_blob`
   (`node_authoring.rs`) writes for a fresh create — under the object-store lock, before the manifest
   Blob. Assert on write that the stored id equals the entry's `blob_id`; a mismatch is a bug, refused as
   `Integrity`, never a snapshot written anyway. Symlink entries are written as the state has them (the
   manifest must recompute); readers refuse them as replay already refuses symlink operations.
   **E3 (ruled):** delete the refusal at `node_authoring.rs` (the `baseline_files.is_empty() &&
   baseline_symlinks.is_empty()` arm with a snapshot reference) and its test in this commit; a commit on
   an emptied tree at a checkpoint must author normally — add that as a control.
3. Controls: handoff v1 §2 in full (65 blocks → snapshots at depth 1 and 65; two repositories, same
   history, same block ids; tamper → `verify` finding + `--snapshot-plan` `Integrity`; bundle round-trip
   and the missing-Blob refusal; `patch_set_digest` unchanged; bytes per snapshot on the RFC 139 corpus
   profile). Plus: a repository sealed with 1b, read by 1a's readers, checks out byte-equal to the same
   history sealed without snapshots — **on a history with at least one `EditText` before the
   checkpoint** (the case that needs the content Blob), and perturbing the Blob write away must make it
   fail at the loader's missing-Blob step. `show` on a block whose edited content is now stored reports
   it instead of *unavailable*: say so in the report and in `show.md` if its text changes. Hard stops of v1 §3 stand. Report:
   `.git-exclude/review-request/rfc136-increment-1b-report-v1.md`.

## Increment 1b — revised 2026-09-15 after your hold (read RFC 136 §10.3a first)

Your hold was right, and it was the right stop: six tests would have been edited into a new meaning for
rollback. The cause was my 1a handoff's *"every reader seeds and skips"*. Where this section conflicts
with the 1b section above, this section wins. Your WIP stands, with these changes.

**Asked, answered.**
1. Rollback and inverse: **A**, and permanently (§10.3a ruling 3). Rollback never anchors.
2. Coverage: **none of A, B or C in 1b**, because no reader anchors in 1b (ruling 2), so coverage cannot
   change. For increment 2, coverage stays whole-chain (ruling 4).
3. Declaring `block_state` a hub: **yes**, with the reason "the one seal function derives, decides the
   checkpoint and writes" in `DECLARED_HUBS`. The size pin moves in this commit: yes.

**Changes to the WIP.**
- **Readers do not anchor.** Remove seed-and-`continue` from `patch_replay.rs`, `patch_inverse.rs` and
  `bundle/preview.rs`, so they replay every block's patches and never read `snapshot_blob_ref`. Delete
  `replay_state_from_snapshot` if nothing else uses it. `load_block_snapshot` stays for
  `checkout --snapshot-*`.
- **One validator.** A single `validate_snapshot_manifest(reader, block_id, block)` decodes, recomputes
  and checks each content Blob's presence without reading bytes. `load_block_snapshot`, `verify` and
  bundle export call it. Export validates *before* enumerating content ids, so a tampered manifest is
  refused for the real reason, not as *missing blob*.
- **Nothing to invert** becomes a `Precondition` naming the ref (RFC 132 mould), in its own commit in this
  round. `CHANGELOG.md` gets `### Fixed`.
- **`rollback-preview.md:20`** changes from *"the latest snapshot baseline in that replay window"* to the
  empty state before the replayed chain.
- **`checkout --snapshot-plan`'s `Precondition`** can no longer say *"no block-creating path writes one
  yet"*. It says the block is not a checkpoint, and that checkpoints fall at genesis and every 64 blocks.

**The six held tests pass unmodified.** That is the evidence ruling 1 holds; do not touch them. Name them
in the report as passing at the final commit.

**Increment 1a's controls, revised to the ruling.**
- **C1 and C2 stand.**
- **C3 and C5:** with a tampered or v1-magic manifest present, every replay reader's output equals the
  output with no snapshot, because they do not read it. `checkout --snapshot-plan`, snapshot
  materialization and `verify` refuse it.
- **Perturbation:** make `patch_replay` read the snapshot again; the identity control must fail.

**New controls.**
- **Checkpoints are invisible.** On one history containing, before the second checkpoint:
  - an `EditText`;
  - a `DeleteFile` whose path is still present in the worktree with its old bytes;
  - a rename and a `ChangePerm`;

  every report must be equal, prose and JSON, between the repository as sealed and the same history
  sealed with no checkpoints. The reports: `--patch-plan` (with `coverage`), the content report,
  `--patch-delete-plan` and `--patch-materialize-delete` (the stale file is deleted), `inverse-plan`,
  `rollback-preview`, `rollback-draft --append-inverse`, bundle preview and branch switch. Block and
  RefState ids differ by design: normalize them, or seal the comparison repository through a test-only
  path that writes no snapshot. Say which, and why it cannot leak into production.
  **Perturbation:** restore 1a's anchor in `patch_replay` and the delete plan must lose the stale file.
- **A checkpoint never fails a seal that would succeed without it.** Seal 130 blocks with `EditText`
  scattered across blocks, a delete of an edited node, a rename of an edited node, and a `merge` whose
  mainline reaches the cadence. Every seal succeeds, every checkpoint loads, `verify` is clean.
  - **Why it should hold:** the cache is complete by construction. `resolved_parent_state` takes the
    parent's (state, cache) from the replay memo (`block_state.rs:356-362`, stored at `:390`), and every
    `EditText` inserts (`effect.rs:220`) with no eviction.
  - **Perturbation:** evict one entry; the writer must refuse as `Integrity` and the seal must fail
    visibly, never write a partial snapshot.
- **Re-pin increment 0's ids** with old and new shown, plus two assertions:
  - which blocks carry a snapshot (the root only);
  - a perturbation forcing `checkpoint_due` to `false` restores all four old pins, proving the change is
    exactly that field.

**The other seven mechanical failures** are accepted as you listed them. Update the counts and their
explanations, and give `row2b`'s fixture a real v2 manifest.

**Release note for the report.** Increments 1b and 2 ship in one release (ruling 6). Do not propose a cut
after 1b.

## Also in this round: nothing else

The RFC 152 docs commits (`26fea4f8`, `52838ed7`) are accepted; one wording change to `SECURITY.md` and
the release-notes paragraph (a checksum proves transport, not origin) was made by the architect on top.
