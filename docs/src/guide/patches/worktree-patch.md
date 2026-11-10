# Worktree Patch Authoring

`prikk commit` authors the current worktree into a node-addressed patch and appends it to the active
WAL. The patch carries a real role-bound Ed25519 `AUTHOR` signature.

```sh
# `prikk setup` wrote your AUTHOR seed to the key directory (~/.config/prikk/author.seed), where
# commit looks for it: nothing to export. To sign with another seed instead, name it:
#   export PRIKK_AUTHOR_SEED_FILE="<path to a 0600 seed file>"
#   export PRIKK_AUTHOR_KEY_ID="<an id>"           # optional: overrides the key-id file beside the seed

prikk commit -m "record changes"
# --text-edits is accepted for compatibility; text nodes author EditText either way:
prikk commit --text-edits -m "record text changes"
# Explicit unborn local branch genesis:
prikk commit --ref heads/topic -m "start topic"
```

`--from-worktree` is still accepted for backward compatibility but is now the only behavior, so it can
be omitted.

## Taking back your newest commit

`prikk commit` queues a patch; until `prikk seal` seals the queue, you can take back the newest one:

```sh
prikk status --format json        # lists the queued commits, with their patch ids
prikk uncommit <patch-id> --plan-only
prikk uncommit <patch-id>
```

Nothing is wrong when you do this. The patch leaves the queue (and is kept in the recovery log), the files in your worktree are
not changed, and the next `prikk commit` includes them. Sealed history is never touched; to undo a sealed change, use
[`prikk rollback-draft`](../rollback/rollback-draft.md). Only the newest queued commit can be taken back: if others were queued
after the one you mean, take those back first (committing them again gives them new patch ids). `--plan-only` says what will
happen, including what the next `commit` does about each operation, and a run prints the same. If it is interrupted, type the
same command again. To undo it: `prikk doctor --recovery-restore <run>` (the run id is printed), which refuses once anything has
changed since.

Taking a commit back also clears the refusal `sync seal --claim` gives while a commit is queued. The files stay in your
worktree, and `sync seal` onto the current branch needs a clean worktree, so remove them (or keep them elsewhere) first.

## Baseline

Authoring compares the worktree against a baseline node lifecycle state:

- **Published ref:** the baseline is reconstructed from authoritative node-addressed replay of the
  `heads/main` (or `--ref`) lineage — never from a snapshot manifest.
- **Genesis (fresh local branch):** when a valid `heads/*` target ref has never been published, the first commit
  authors against an empty baseline, so every worktree file becomes a `CreateFile`. The following
  `seal --ref heads/<branch>` publishes the first block as a Root block.

`commit --ref heads/topic` on an unborn ref creates an independent Root history from the current
worktree. It does not copy/fork `heads/main`, switch the checkout branch, or create a merge base.
A later `prikk branch switch heads/topic` brings the current-branch pointer in line with what the
worktree already holds.

## Commands that move the current branch

Four commands can move the branch your worktree is on, and each writes the result into the worktree
itself, so the worktree and the branch end in step and the next `prikk commit` finds nothing stale:

- `merge --into <the current branch>`;
- `branch create --from <ref>`, when the new branch is the unborn current branch;
- `sync seal <ref> --claim <id>` (or `--claims <file>`) onto the current branch;
- `archive import --adopt`, for a ref that becomes the current branch.

Each one checks two things before it writes anything, and refuses if either fails:

- **The worktree is clean against its own recorded baseline.** A worktree with uncommitted changes
  refuses the command, naming the paths `prikk worktree-status` lists. Commit them or restore them,
  then run the command again.
- **No file is in the way.** A file that is not tracked, sitting at a path the new tip's files
  occupy, refuses the command: `<n> file(s) are in the way of <ref>'s files: <path> (…); move them
  aside before <action> (nothing was written)`. Move the file aside and run the command again.

A command that refuses leaves `.prikk` and the worktree as it found them (apart from `.prikk`'s own
cache), so nothing needs undoing. `archive import --adopt` makes both checks before it admits
anything from the archive.

For `sync seal` with several claims, claims seal in order and sealing stops at the first claim that
fails, keeping the seals before it; the worktree is then written to match what actually sealed, and
then the error is reported. A batch that seals nothing writes nothing into the worktree. If an
`archive import` is interrupted after a ref is adopted but before its files are written, running the
same import again finishes the write and reports the ref as adopted, completed by the earlier run;
no `branch switch` is needed.

If a file appears in the way after the branch has moved (created by a process outside prikk),
the error says the branch was published, names the file, and says to move it aside and run
`prikk branch switch <the current branch>`, which finishes the write.

A command that moves a branch which is *not* the current one leaves the worktree alone.
`rollback-draft --append-inverse` also leaves the worktree alone, and says so: `prikk seal`, then
`prikk branch switch <ref>`, applies the inverse. See [Command Surface](../../reference/commands.md)
for the exact messages.

The active WAL queues commits until `seal`, up to the queue limit described in the [FAQ](../faq.md). The
active WAL records the one ref it belongs to, and a commit for a different ref is refused while the queue is
non-empty, so `seal --ref heads/main` cannot publish a patch authored for `heads/topic`
(`crates/prikk-store/src/commit_boundary/active.rs:301-314`).

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
- multi-commit queued active sessions or per-ref active WALs
- multi-operation text diff minimization, commutation, conflict witnesses

Signature scope: worktree commits are role-bound Ed25519 `AUTHOR`-signed. This does not imply
trust-store enforcement, key management, `MAINTAINER`/publication signing, rollback authorization, or
publication-grade repository trust.
