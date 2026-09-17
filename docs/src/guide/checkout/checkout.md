# Checkout Planning

PR-017 includes a read-only checkout plan:

```sh
prikk checkout --plan-only [path] [--ref heads/main]
```

The command resolves the current RefState to its Block and reports whether checkout would need
snapshot materialization or patch application.

`--ref` takes any published ref, not only a branch: a tag ref resolves through its tag object to the
Block that object names, so `--ref tags/v1` plans against the tagged Block rather than the branch
tip. That applies to `--patch-plan` and its `--content-path` reporting too.

The read-only modes — `--plan-only`, `--snapshot-plan`, `--patch-plan` (with or without
`--content-path`) and `--patch-delete-plan` — also take a **bare block id**, as `log` prints it. Any
block the repository holds resolves, including an older block on a branch, so two blocks on one ref
can be read side by side:

```sh
prikk checkout --patch-plan --format json --content-path a.txt --ref <block-id>
```

A block id replays exactly as a ref naming the same block does, from the same checkpoint. The prose
header then reads `block: <id>` instead of `ref: <name>`. An id the repository does not hold refuses
with `block <id> is not in this repository`; an id naming another kind of object refuses naming its
type, for example `object <id> is a patch, not a block`. The modes that write the worktree refuse a
block id, because the next `commit` authors against a branch and a block id names none.

For snapshot-backed blocks, first validate the snapshot manifest:

```sh
prikk checkout --snapshot-plan [path] [--ref heads/main]
```

Then explicitly materialize validated snapshot files:

```sh
prikk checkout --snapshot-materialize [path] [--ref heads/main]
```

## A refused checkout writes nothing

`--patch-materialize`, `--patch-materialize-delete` and `--snapshot-materialize` check every file they
would write before writing any. An existing file with different content, a directory or other non-file
at a path, or a symlinked parent directory is a conflict, and a conflict refuses the whole checkout:

```text
error: precondition not met: refusing to materialize: 2 path(s) in the way: dev-only.txt (an existing file with different content), shared.txt (an existing file with different content); move them aside, or commit them on their own branch, and run the checkout again (nothing was written)
```

Every conflicting path is named, not only the first. Nothing is written, no marker is set, and `commit`
keeps working. `--patch-materialize-delete` likewise refuses, naming each file, when a file it would
delete has changed. See [troubleshooting](../troubleshooting.md) for the one case that still leaves a
worktree part-written.

Snapshot materialization writes only validated regular files. Supported patch replay and
materialization are available through `--patch-plan`, `--patch-materialize`, and
`--patch-materialize-delete`, but full patch algebra remains deferred. For the shared path and
worktree safety boundary, see the [path and worktree safety](../../reference/path-safety.md) reference.
