# Repository Status

```sh
prikk status
```

Unlike most other commands, `status` takes no path or `--ref` argument: it always reports on the
repository rooted at the current working directory, using its default active-session WAL.

For each invocation, the CLI reports:

- the repository's `.prikk` directory path;
- the number of active (unsealed) WAL records, and any trailing partial bytes from an interrupted
  append;
- `heads/main`'s current `RefState` ID, specifically — not whichever ref the active queue is targeting,
  which is reported separately;
- the queued-patch count and the ref it targets, when the queue is non-empty;
- when the worktree was written by `checkout --snapshot-materialize` and `prikk verify` has not replayed
  the repository since, `provisional worktree: materialized from the snapshot of <block> on <ref>; not
  replay-verified — run prikk verify` (`provisional_worktree` in `--format json`, `null` otherwise; see
  [snapshot materialization](checkout/snapshot-materialization.md));
- when a checkout or `branch switch` stopped part-way (a crash, or a file changed during it),
  `interrupted materialization: a checkout or branch switch stopped part-way; move aside any file it
  named, then run prikk checkout --patch-materialize --ref <branch> or prikk branch switch <branch>`,
  naming the current branch (`interrupted_materialization` in `--format json`, holding those two
  `routes`, `null` otherwise; see [troubleshooting](troubleshooting.md));
- in `--format json`, `queue.already_sealed`: `null` for an ordinary queue; `{"block_id": "<id>", "route": "prikk seal
  --allow-no-audit"}` when a `seal` published its Block and was stopped before it cleaned the queue. The field means
  exactly this: **the queue is the patches of `block_id`, which the ref already points at** (the state `verify` calls
  `PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING`, decided by the same predicate), so `queue.patches` lists what that Block
  sealed, not new work, and `route` is the command that finishes the cleanup (see
  [troubleshooting](troubleshooting.md#seal-stopped-after-it-published-before-it-cleaned-the-queue)). It is an
  addition within `status-report-v1`: no existing field moved. Through 0.57.0, `status --format json` exited 1 in this
  state (`integrity error: lifecycle replay: … node_id is already live`);
- a warning once the queued-patch count reaches the recommended threshold or the configured hard
  limit (`PRIKK_ACTIVE_PATCH_WARN`/`PRIKK_ACTIVE_PATCH_LIMIT`; see NFR-PERF-02), the same thresholds
  [`prikk commit`](patches/worktree-patch.md) enforces on the write path.
- when the next `commit` or `seal` would refuse, `warning: the next commit or seal will refuse: <reason>`,
  where the reason is the one that write would give;
- otherwise, when a queued commit was written durably but never confirmed (a previous command was
  interrupted after its own write, or an older prikk wrote it), `note: a queued commit was already written
  but not confirmed`; the two cases cannot be told apart (`crates/prikk-cli/src/main.rs:624-641`).

`status` does not run [`verify`](../reference/integrity-recovery.md)'s integrity checks, and does not
report the worktree's own divergence from the replay baseline — that is
[`worktree-status`](worktree-status.md). Multi-operation text diff minimization and audit plugins are
not yet implemented; the command's own trailing line notes this.
