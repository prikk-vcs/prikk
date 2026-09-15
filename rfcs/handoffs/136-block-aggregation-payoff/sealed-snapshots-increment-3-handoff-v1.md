# RFC 136 increment 3 — measurement, docs, CHANGELOG

**Live 2026-09-16.** This is the last round before the 0.43.0 prep.

**Read first:** RFC 136 §9.3, §10.3a–§10.3c, §10.4, and the 2a/2b reviews
(`.git-exclude/reviewed/rfc136-increment-2a-review-v1.md`, `…-2b-review-v1.md`).

**No product code changes in this round,** except a fix a measurement proves necessary. Report it, and hold it for
review.

**RFC 136 is not moved to `done/` by the team.** The architect moves it at the 0.43.0 cut (RFC-000).

## 1. Measurements

**Where and how:**
- **Corpus:** RFC 139 (`profiles/prikk-self.toml`), at the five §9.3 depths.
- **Samples:** **three per cell**; report the median and the range. One sample is not a measurement: the 2a and 2b
  memory figures were single samples, and this round answers them.
- **Output:** instruments go under `tools/corpus/tests/` (extend the existing `rfc136_*` and `snapshot_bytes.rs`,
  and RFC 139's `two_measurements.rs`, rather than duplicating them). Numbers go under
  `.git-exclude/measurements/rfc136/`. Every table names both binaries' commits and sha256.

**The five measurements:**
1. **Storage.**
   - **What to measure:** bytes per checkpoint (the manifest, plus the edited-text content Blobs a checkpoint had
     to store), and total repository size and object count, at each depth.
   - **Compared against:** the same history sealed by a build from before the writer (`c786e7f9`).
   - **The answer wanted:** what a checkpoint costs a repository.
2. **Cost, consolidated.** One table for `checkout --patch-plan`, `--patch-materialize`, `branch switch` and
   `bundle preview`, before (`26e8c528`, which has no anchoring) and after (this tree). Wall time and peak memory.
3. **What is still unanchored — this decides 2c.**
   - **What to measure:** `merge-evidence` (baseline via `replay_derived_state`, `merge/evidence.rs:55`), and
     `commit`'s baseline with the DC-64 cache both warm and cold (delete `.prikk/cache/lifecycle-state.v1`).
   - **Where:** the same depths, before and after.
   - **What to report:** the growth exponent against depth. Recommend, don't decide, whether 2c is worth scheduling.
4. **Peak memory.**
   - **Scope:** every command above.
   - **The questions carried from earlier rounds:** `--patch-plan` was +12 % at depth 256 (2a), and
     `--patch-materialize` +19 % (2b), each a single sample.
   - **If anchored peak memory grows faster than the tree** (file count and content bytes), find where. The
     suspect is the snapshot's files and the decoded history held at once. Report it; fix it only if it is small
     and plainly within §10.3a ruling 1.
5. **The derivation gate, exercised end to end, recorded.** Materialize from the snapshot of a block not in the
   record (delete the record file), then `commit` refuses, then `verify`, then `commit` proceeds. Time each step
   at the deepest depth.

## 2. Docs

1. **A reference section on checkpoints**, in `docs/src/reference/` (choose the page and say why). It covers:
   - they fall at a ref's first block and every 64th after it;
   - a snapshot is a v2 manifest of the block's own state, plus the content Blobs it stores;
   - `verify` checks every snapshot and never replays less;
   - a checkpoint changes cost, never output (§10.3a);
   - read-only reports may start at any valid snapshot, and worktree writes only at a replay-verified one;
   - the replay-verified record (a cache, safe to delete) and the provisional marker;
   - the storage numbers from §1.1.

   Link the guides that already describe the marker, the gate and the record, rather than repeating them.
2. **Two false statements, measured by the architect on 2026-09-15** (ROADMAP findings from the planeter letters):
   - **`docs/src/guide/backup-restore.md`:** *"Turning `remotes/heads/main` into your own working `heads/main` is
     an ordinary `merge`"*. False: a fresh repository cannot merge into an unpublished `heads/main`, and
     `branch create --from remotes/…` and `checkout --ref remotes/…` refuse too. State what a restored bundle
     does give (verifiable history at `remotes/`), and what it does not. Check every sentence on the binary.
   - **`docs/src/guide/sync.md`:** *"A sender's block and the receiver's sealed block for the same patches are
     different objects … expect the ids to differ"*. False when the receiver seals the same patches in the same
     grouping: block and RefState ids are identical, and only the signed envelopes differ (an id excludes
     signatures). Rewrite it, and name the one-envelope-per-id consequence: a repository cannot hold the same
     history under two signers.
3. **A currency check** of `snapshot-checkout.md` and `snapshot-materialization.md` against the binary.

## 3. CHANGELOG `## Unreleased`

1. **`### Added — sealed snapshots`.** Checkpoints at a ref's first block and every 64th. A bundle's object count
   includes the checkpoint manifests. `show` reports content a checkpoint stored, where it used to say
   *unavailable*. Read-only reports and worktree writes are faster at depth, with the §1.2 numbers (for example,
   depth 256), and their output is unchanged.
2. **`### Fixed`.** The replay materializer re-syncs the directory on a mode-fixing retry. Found in 1a's review.
   Before, a rename whose first directory sync failed could stay undurable.
3. **Reconcile** with the entries already there, so each change appears once.

## 4. Discipline and report

- **Before every commit:** fmt, workspace clippy with 0 errors, workspace tests with 0 failed, the staged list equal
  to `git status`, **and the cross-target addendum for any new `cfg`**. The last round's slip was a test seam with
  a Linux-only caller.
- **On the final commit:** the full gates.
- **Report:** `.git-exclude/review-request/rfc136-increment-3-report-v1.md`.
- **Hard stops, reported:**
  - any output difference with and without checkpoints;
  - a storage cost per checkpoint above 10 % of the repository size at depth 64;
  - an anchored peak memory above 1.5× the unanchored run at any depth.
