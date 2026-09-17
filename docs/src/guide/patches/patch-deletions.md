# Supported Patch Deletions

PR-022 adds an explicit deletion plan for the supported patch replay result:

```sh
prikk checkout --patch-delete-plan [path] [--ref REF]
```

It also adds an opt-in materialization mode that removes eligible files:

```sh
prikk checkout --patch-materialize-delete [path] [--ref REF]
```

Deletion is intentionally narrow. Prikk deletes only files that the replayed patch chain removed
with a `DeleteFile` operation. Before removing a worktree file, Prikk checks that the current file
bytes still match the operation's old Blob precondition.

Safety boundaries:

- Arbitrary untracked files are never deleted.
- Modified deleted files are refused.
- Symlink targets and non-file targets are refused.
- General checkout pruning remains deferred.
- The replay this plan reads applies text edits, renames and mode changes, as `--patch-materialize` does,
  but only a `DeleteFile` makes a file eligible for deletion: a renamed file's old path is never deleted.
- Symlinks, merge conflicts, inverse logic, and full patch algebra remain later increments.

For exact deletion and materialization safety boundaries, see
[path and worktree safety](../../reference/path-safety.md).
