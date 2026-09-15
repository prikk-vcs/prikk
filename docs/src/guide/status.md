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
- a warning once the queued-patch count reaches the recommended threshold or the configured hard
  limit (`PRIKK_ACTIVE_PATCH_WARN`/`PRIKK_ACTIVE_PATCH_LIMIT`; see NFR-PERF-02), the same thresholds
  [`prikk commit`](patches/worktree-patch.md) enforces on the write path.

`status` does not run [`verify`](../reference/integrity-recovery.md)'s integrity checks, and does not
report the worktree's own divergence from the replay baseline — that is
[`worktree-status`](worktree-status.md). Multi-operation text diff minimization and audit plugins are
not yet implemented; the command's own trailing line notes this.
