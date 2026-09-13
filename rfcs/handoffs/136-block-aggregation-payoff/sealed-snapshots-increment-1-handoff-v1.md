# RFC 136 §10 — sealed snapshots, increment 1: the format and the writer

**SUPERSEDED 2026-09-13 by `sealed-snapshots-handoff-v2.md`** after the dev team's hold (RFC 136 §10.1a): the readers give the field the opposite meaning, and sealing is four places, not one. Kept for its §2 controls, which v2 reuses. Originally: **Live 2026-09-13** (0.43.0's theme, approved by the owner). After the RFC 133 instrument item, which is
smaller. Read RFC 136 §6, §8, §9.2.4, §9.2.5 and §10 before starting; §6 is the rule everything here obeys.

## 1. The change

1. **`SnapshotManifest` v2** in `snapshot.rs`: magic `PRIKK-SNAPSHOT-MANIFEST-v2\n`; entries are the
   block's `StateRootEntry` set in canonical path order, encoded with the same field grammar
   `state_leaf_preimage` uses (path, NodeId, kind, mode, Blob id or symlink target) — reuse the
   preimage's encoder rather than writing a second grammar; say so. `decode` dispatches on the magic and
   keeps the v1 decoder for fixtures. **`SnapshotManifest::recomputed_state_root()`** returns the root of
   the decoded entry set; readers compare it to the block's `state_merkle_root` and refuse a mismatch as
   `Integrity` (*"snapshot manifest does not recompute to its block's state root"*).
2. **`CHECKPOINT_CADENCE: u32 = 64`**, one named constant in the store, read by both the reanchor
   (`REANCHOR_BOUND` becomes an alias of it or is replaced — one number, one place) and the seal path.
3. **The writer**, in the one place sealing computes the state root: if the new block's depth from the
   nearest snapshotted ancestor on the ref is `>= CHECKPOINT_CADENCE`, or there is no snapshotted
   ancestor, encode the v2 manifest, write it as a Blob through the ordinary object write (under the
   object-store lock, like every object), and set `snapshot_blob_ref` **before** the block is encoded and
   signed — the ref is inside the canonical encoding (§8). `merge` and `sync accept` seal through the same
   function; if they do not today, route them through it rather than duplicating the decision.
4. **`verify`**: for every block with a snapshot, the Blob exists (already) **and** the manifest
   recomputes to the block's root; a failure is an `Integrity` finding in the objects stage.
5. No read-side acceleration in this increment; `checkout --snapshot-*` keeps working on v2 through
   the decoder (it materializes bytes by Blob id — confirm the materializer reads blobs by id; if it
   assumed inline bytes, that is increment 2's work and this increment says so).

## 2. Controls

- Seal 65 blocks on one ref: snapshots at depth 1 and 65 only; each recomputes to its block's root.
- **Determinism**: two repositories sealing the same patches in the same order produce byte-identical
  blocks (same ids) — the §8 property; assert on ids, not on "a snapshot exists".
- Tamper one manifest byte → `verify` reports the finding, `checkout --snapshot-plan` refuses as
  `Integrity`; the block itself still verifies by replay (§6: the snapshot never certifies).
- Bundle export/import round-trips a snapshotted block and its manifest Blob; a bundle that omits the
  Blob is refused where `bundle verify` refuses missing reachable blobs today.
- `patch_set_digest` of a snapshotted block equals that of an otherwise identical unsnapshotted one
  (cross-repository identity unchanged).
- Storage: report bytes per snapshot on the RFC 139 corpus profile (§10.4's first number).
- Full gate set; the addendum from the diff (object writes are platform-neutral; say so). CHANGELOG
  waits for increment 3 — nothing user-visible changes yet except `verify`'s new finding; add a
  `### Added` line for that alone.
- Report: `.git-exclude/review-request/rfc136-increment-1-report-v1.md`.

## 3. Hard stops

A snapshot that would be read by `verify` to skip replay; a cadence decision that depends on anything but
history; any write outside the object-store lock; any v1 manifest written. Stop and report.
