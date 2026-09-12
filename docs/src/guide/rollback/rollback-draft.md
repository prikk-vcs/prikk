# Rollback Draft

`prikk rollback-draft` appends an explicit rollback-draft Patch for the supported patch-operation subset.

The command is:

```sh
prikk rollback-draft --append-inverse [path] [--ref REF] -m "rollback message"
```

The command performs the same supported inverse validation used by `inverse-plan` and
`rollback-preview`, marks the inverse Patch payload as `PatchPurpose::RollbackDraft`, signs it with a
real role-bound Ed25519 AUTHOR signature, then appends the Patch envelope to the active WAL. Key material
uses the same key material as `prikk commit`: the AUTHOR seed from your key directory (or
`PRIKK_AUTHOR_SEED_FILE`), with `PRIKK_AUTHOR_KEY_ID` naming the key id.

Safety rules:

- `--append-inverse` is required.
- `-m <message>` is required and must not be empty.
- the target ref must be published and must resolve to a supported single-parent block chain.
- the supported replay/inverse subset must validate successfully.
- the active WAL must be empty.
- the active WAL must not contain a trailing partial record.

What it mutates:

- appends one AUTHOR-signed rollback-draft Patch envelope to `.prikk/active/default/queue.wal`.

What it does not mutate:

- it does not write object files directly.
- it does not publish refs.
- it does not modify the worktree.
- it does not authorize rollback by policy.

PR-029 adds a pre-seal verification command:

```sh
prikk rollback-draft-verify [path] [--ref REF]
```

After reviewing and verifying the draft, the existing local seal scaffold can publish it:

```sh
prikk seal --allow-no-audit
```

The seal path is unchanged, but `prikk log` and `prikk verify` classify the sealed Block as a rollback
Block when it contains Patch objects with `PatchPurpose::RollbackDraft`.

Supported inverse operation subset:

- `CreateFile` -> `DeleteFile`
- `DeleteFile` -> `CreateFile`
- deterministic arbitrary-span `EditText` -> `EditText`
- `ReplaceBinary` -> `ReplaceBinary` (old/new blob swapped; DC-73)
- `ChangePerm` -> `ChangePerm` (old/new mode swapped; DC-73)

Deferred:

- `RenamePath` and `CreateSymlink` inverse — not a node-model gap, an authoring one: `commit` never
  produces `RenamePath` (renames become delete+create) and symlink authoring is refused outright, so
  there is nothing in ordinary history for either inverse to act on (DC-73)
- rollback-specific ref publication policy
- authorization and audit policy for rollback
- rollback branch/reflog semantics
- worktree rollback materialization policy
- commutation, confluence, and conflict witnesses
- plugin execution and remote sync
