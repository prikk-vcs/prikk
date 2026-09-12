# Supported Patch Inverse Planning

PR-026 adds read-only inverse planning for the supported patch-operation subset.

```sh
prikk inverse-plan [path] [--ref REF]
```

`--ref` takes any published ref, not only a branch. A tag ref resolves through its tag object to
the Block that object names, so `prikk inverse-plan --ref tags/v1` plans the inverse of the tagged
Block's patch, which need not be the branch tip. The same is true of `prikk rollback-preview` and
`prikk rollback-draft-verify`, which reach the same resolution. `prikk rollback-draft
--append-inverse` still requires a branch ref: it appends to that ref's active WAL, and a tag names
no WAL.

The command walks the same single-parent sealed block chain used by supported patch replay. While
validating and replaying the chain, Prikk derives an unsigned inverse Patch payload in reverse
application order.

Supported inverse shapes in PR-026:

- `CreateFile` → inverse `DeleteFile`
- `DeleteFile` → inverse `CreateFile`

Safety boundaries:

- The command is read-only.
- The inverse Patch is not written to the object store.
- The reported inverse Patch ID is only an unsigned deterministic planning hint.
- `EditText` direct inverse for arbitrary spans remains deferred until the required round-trip vectors
  land.
- Rollback refs, authorization policy, conflict witnesses, commutation, and confluence remain later
  increments.
