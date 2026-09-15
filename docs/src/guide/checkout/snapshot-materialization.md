# Snapshot Materialization

PR-017 adds an explicit, opt-in snapshot materialization path:

```sh
prikk checkout --snapshot-materialize [path] [--ref REF]
```

The command writes files only from a validated snapshot manifest. It does not apply patch algebra,
does not remove extra files, and refuses to overwrite existing files with different content. It also
refuses symlinked parent directories and symlink targets so snapshot checkout cannot be used to
write outside the repository worktree.

## A snapshot worktree is provisional until `prikk verify`

A snapshot proves only that its files match the state root its block signs. It does not prove that
replaying the history before that block produces the same files: only replay proves that, and only
`prikk verify` replays the whole repository. So a worktree written from a snapshot is **provisional**.

- **The marker.** Before its first write, `checkout --snapshot-materialize` records the ref and the
  snapshot's block in `.prikk/worktree.provisional`. A crash after that point leaves the marker set.
- **The derivation gate.** While the marker is set, every command that would turn the worktree, or the
  repository's current state, into history refuses as a precondition and names the route:
  `commit`, `mv`, `seal`, `merge`, `sync accept`, `sync seal`, `rollback-draft --append-inverse` and
  `branch switch`. Reading commands, `checkout --patch-*`, `tag create`, `branch create`/`close`,
  `trust` and `bundle` stay available.
- **Clearing it.** Run `prikk verify`. The marker clears when verify finds nothing about objects, the
  WAL, refs or block state. A publication-trust finding alone does not keep it: who signed a block says
  nothing about whether replay reproduces the snapshot. A materialization that runs while `verify` is
  running keeps its marker, and verify says so.
- **When no marker is written.** If this repository has already confirmed the block's state by replay,
  the snapshot's files are exactly what replay gives, so `--snapshot-materialize` writes no marker and
  prints `provisional: no`. Otherwise it prints `provisional: yes` and the marker applies. A block is
  confirmed when `seal`, `merge` or `sync seal` sealed it or replayed it as part of its parent history,
  or when `prikk verify` replay-verified it. Receiving a block (`bundle import`, `sync accept`) never
  confirms it.
- **The record.** Those confirmed block ids live in `.prikk/cache/replay-verified-blocks.v1`. It is a
  cache, never authoritative: a damaged, truncated or other-version file reads as empty, and deleting it
  is always safe. Without it, every snapshot materialization is provisional and every worktree write
  replays from genesis, until `seal` or `verify` records the blocks again.
- **Where it shows.** `prikk status` prints `provisional worktree: materialized from the snapshot of
  <block> on <ref>; not replay-verified — run prikk verify`, `status --format json` carries
  `provisional_worktree`, and `prikk doctor` lists `PRIKK-DOCTOR-PROVISIONAL-WORKTREE`.

## Reports that start at a snapshot

`checkout --patch-plan` (prose and `--format json`, including `--content-path`),
`checkout --patch-delete-plan` and `bundle preview` read nothing but write nothing either, so they may
start at the nearest snapshot on the ref's history instead of genesis. Their output is byte-identical
either way: the file manifest comes from the snapshot, and everything about history — patch and
operation counts, `coverage`, files history deleted — is still computed over the whole chain. A snapshot
that fails validation is never used: the report replays from genesis, prints the same output, exits `0`,
and adds one line on stderr naming the block and `prikk verify`.

Commands that write the worktree — `checkout --patch-materialize`, `--patch-materialize-delete` and
`branch switch` — start at a snapshot only when its block is in the record above and the snapshot passes
validation; otherwise they replay the whole history. A received block's snapshot is never a starting
point for a worktree write. The written files are identical either way. `rollback-preview` always
replays the whole history.

The path validator remains conservative: non-ASCII paths are deferred until Unicode NFC
normalization is implemented, and paths targeting `.prikk/` are rejected. For the exact validator and
write-safety caveats, see [path and worktree safety](../../reference/path-safety.md).
