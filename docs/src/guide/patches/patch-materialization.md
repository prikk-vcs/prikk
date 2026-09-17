# Supported Patch Materialization

PR-021 added an explicit materialization command for the supported patch replay result:

```sh
prikk checkout --patch-materialize [path] [--ref REF]
```

This command reuses the read-only replay support from `checkout --patch-plan` and writes the
resulting file manifest into the worktree through a conservative, mode-aware materializer. It is a
separate code path from snapshot checkout's materializer (DC-73): the replayed manifest carries mode
bits derived from `CreateFile`/`ChangePerm` history, which the snapshot-blob wire format does not
encode, so the two are deliberately not unified.

PR-022 adds deletion-aware materialization as a separate opt-in command:

```sh
prikk checkout --patch-materialize-delete [path] [--ref REF]
```

Supported operation subset:

- `CreateFile`
- `DeleteFile`
- deterministic arbitrary-span `EditText`
- `ReplaceBinary` (DC-73)
- `ChangePerm` (DC-73) — the mode bit is written, not only the content
- `RenamePath` (RFC 144 increment 1) — the node's content is written at its new path; a rename in one
  patch that swaps two files is applied as one step

Safety boundaries:

- Existing files with identical bytes and mode are left unchanged.
- Existing files with different bytes are refused.
- Existing files with identical bytes but a different mode have their mode corrected in place.
- `--patch-materialize` never deletes files.
- `--patch-materialize-delete` deletes only files explicitly removed by replayed `DeleteFile` operations.
- Deletion is refused unless the current worktree bytes still match the old Blob precondition.
- Extra untracked files are never deleted.
- Symlinked parents, symlink targets, non-file targets, and `.prikk/` metadata paths remain refused.
- Neither mode removes a renamed file's *old* path from the worktree: `--patch-materialize` never deletes,
  and `--patch-materialize-delete` removes only files a `DeleteFile` removed.
- Symlinks, merge conflicts, and full patch algebra remain later increments. `commit` refuses a symlink
  outright, so there is nothing in ordinary history for materialization to act on (DC-73).

This command is useful for exercising the current Prikk object/WAL/ref/block pipeline end-to-end,
but it is not yet a complete checkout implementation. For the shared write-safety boundary and its
race caveats, see [path and worktree safety](../../reference/path-safety.md).
