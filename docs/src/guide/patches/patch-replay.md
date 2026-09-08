# Supported Patch Replay Planning

Read-only patch replay handles the current conservative operation subset, including DC-12
arbitrary-span text edits.

The command is:

```sh
prikk checkout --patch-plan [path] [--ref REF]
```

It walks the single-parent block chain from oldest to newest, loads any snapshot Blob attached to a
block, and applies supported Patch operations in block patch order.

Supported operations:

- `CreateFile`
- `DeleteFile`
- `EditText` for deterministic content-anchored arbitrary spans
- `ReplaceBinary` (DC-73)
- `ChangePerm` (DC-73)

Unsupported operations still fail the plan clearly:

- `RenamePath` — not a node-model gap: `commit` never authors it, renames become delete+create
- `CreateSymlink` — not a node-model gap: symlink authoring is refused outright
- merge/conflict algebra

This command does not write the worktree. It only proves that the current sealed history can be
replayed into an in-memory snapshot manifest using the operation subset implemented so far.

## Content at a point (RFC 143)

The replay above already reconstructs every file's bytes internally, then discards them once the
plan is proven. `--format json` exposes those bytes instead, for exactly the paths you name:

```sh
prikk checkout --patch-plan --format json --content-path <repo-relative path> [--ref REF]
```

`--content-path` is repeatable and required for `--format json` to report anything — there is no
whole-tree default. A requested path that never resolved in the replay degrades to `not_found`
(not an error); an unsupported operation anywhere in the walked chain still fails the whole call,
never folds into a partial-but-successful response. A bare `--format json` with no `--content-path`
reports metadata only — an empty `content` and `not_found`, a populated `coverage` — since there is
nothing to scope content to and nothing to guess at.

`patch-plan-content-v1`'s `coverage` field is not a completeness promise: it names which
operation kinds this specific replay actually applied (`create-file`, `delete-file`, `edit-text`,
`replace-binary`, `change-perm`) and that the walk was `single-parent` — the same conservative
subset [`--patch-plan` supports above](#supported-patch-replay-planning), not a claim that every
operation kind in history is covered. `RenamePath` and `CreateSymlink` still fail the call outright,
exactly as they fail the read-only replay itself.

Binary content is never emitted as bytes: a `BinaryFile` node reports its current blob id and
declared size only. A path that a snapshot seeded but the replayed chain never subsequently touched
has no way to know its own text-vs-binary kind — snapshots carry no kind field — so it reports as
`opaque` (size only) rather than guessing.

This surface exists to let another tool compare two replayed points without prikk owning comparison
semantics itself — prikk still has no `diff`, and this does not add one. See
[`show`](../show.md) for the sibling case: one block or patch's own recorded content, no replay
needed for path-addressed operations.
