# Worktree Patch Authoring

`prikk commit` authors the current worktree into a node-addressed patch and appends it to the active
WAL. The patch carries a real role-bound Ed25519 `AUTHOR` signature.

```sh
# Key material is supplied via the environment (a minimal key-input mechanism, not a trust store):
export PRIKK_AUTHOR_KEY_ID="dev-author"
export PRIKK_AUTHOR_SEED_FILE="<path to a 0600 seed file>"   # or leave unset to use ~/.config/prikk/author.seed

prikk commit -m "record changes"
# --text-edits is accepted for compatibility; text nodes author EditText either way:
prikk commit --text-edits -m "record text changes"
# Explicit unborn local branch genesis:
prikk commit --ref heads/topic -m "start topic"
```

`--from-worktree` is still accepted for backward compatibility but is now the only behavior, so it can
be omitted.

## Baseline

Authoring compares the worktree against a baseline node lifecycle state:

- **Published ref:** the baseline is reconstructed from authoritative node-addressed replay of the
  `heads/main` (or `--ref`) lineage — never from a snapshot manifest.
- **Genesis (fresh local branch):** when a valid `heads/*` target ref has never been published, the first commit
  authors against an empty baseline, so every worktree file becomes a `CreateFile`. The following
  `seal --ref heads/<branch>` publishes the first block as a Root block.

`commit --ref heads/topic` on an unborn ref creates an independent Root history from the current
worktree. It does not copy/fork `heads/main`, switch the checkout branch, or create a merge base.

The active WAL is single-commit for this stage. A second commit before seal fails closed, and the active
WAL records the target ref so `seal --ref heads/main` cannot publish a patch authored for
`heads/topic`.

## Operation mapping

Existing-node kind is authoritative:

- new file → `CreateFile` (fresh CSPRNG-minted `node_id`, normalized mode)
- removed tracked file → `DeleteNode`
- modified text file → deterministic arbitrary-span `EditText`
- modified binary file → `ReplaceBinary`
- permission-only change → `ChangePerm`

Path handling is strict: non-UTF-8 worktree paths fail closed, and traversal/reserved-name/collision
rules apply as elsewhere. For the exact repository path and worktree scanner boundary, see
[path and worktree safety](../../reference/path-safety.md). A path matched by a `.prikkignore` rule
at the repository root is skipped by this scan entirely — never authored, never reported — see
[Ignoring Worktree Paths](../ignore.md).

## Out of scope (this stage)

- symlink authoring (fails closed on all symlinks)
- text↔binary kind transitions (fail closed)
- rename detection (a move is a `DeleteNode` + `CreateFile`)
- branch switching or branch copy/fork from an existing tip
- multi-commit queued active sessions or per-ref active WALs
- multi-operation text diff minimization, commutation, conflict witnesses

Signature scope: worktree commits are role-bound Ed25519 `AUTHOR`-signed. This does not imply
trust-store enforcement, key management, `MAINTAINER`/publication signing, rollback authorization, or
publication-grade repository trust.
