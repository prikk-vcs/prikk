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
A later `prikk branch switch heads/topic` brings the current-branch pointer in line with what the
worktree already holds.

**0.56.0 step W3b:** `merge --into` the current branch now writes the merged files into the
worktree itself, when the worktree was clean against its own recorded baseline beforehand — it is
no longer only `heads/main`'s ref that moves while the worktree is left to find out later, through
a stale `commit` refusal or a `branch switch heads/main` resync. A dirty worktree still refuses the
merge, before any write. `archive import --adopt`, `sync seal --claim`, and `branch create --from`
onto the unborn current branch are not yet changed and still leave the worktree exactly as it was,
recoverable the same way the `commit --ref heads/topic` walkthrough above is.

**0.56.0 step W3c-1:** "before any write" above is now actually true. As W3b first landed it, the
clean check ran after the merge's own Block/RefState/RefUpdate objects were already built and
written, so a refused merge still left an orphaned, unreferenced Block behind in the object store
(invisible in the worktree, but reported by `prikk verify` as an unreached object). The clean
check now runs after every one of the merge's own read-only refusals (its ref-ownership check and
RFC 164 Rule D's own pointer-index check among them), before it builds or writes anything of its
own; the dirty-worktree marker is set immediately before the publish instead. A
refused merge into the current branch now writes nothing at all, anywhere in the repository.

**0.56.0 step W3c-2:** `branch create --from`, of the unborn current branch, now writes the
`--from` target's files into the worktree itself too, the same way `merge` does above, with the
same "before any write" guarantee W3c-1 gave `merge`: a dirty worktree refuses the create outright,
before any write. `archive import --adopt` and `sync seal --claim` are not yet changed and still
leave the worktree exactly as it was, recoverable the same way the `commit --ref heads/topic`
walkthrough above is.

**0.56.0 step W3c-3:** a clean worktree does not always end current, either. A file already on
disk at a path the `--from` target's own files occupy — present but untracked, so the clean check
above does not refuse it — is also "in the way," the same reason `branch switch` can already
refuse on. Before this round, that collision was only discovered *after* `branch create --from`
had already published the new branch (or, for `merge --into`, after the merge had already moved
the current branch's tip): the command still exited with an error, but the ref had already moved,
leaving the dirty marker set and a later `commit` or `branch switch heads/<branch>` to resync —
exactly the state a reader of W3c-1/W3c-2 above would not expect from "before any write." For
`branch create --from`, this is fixed: the same in-the-way collision now refuses before the create
publishes anything, naming the colliding path and saying nothing was written, so moving it aside
and running the create again is all a caller needs to do.

**0.56.0 step W3c-3b:** `merge --into` is fixed the same way. Its own target was not available to
check this way directly: the confluence proof computes the merged state but discards it behind its
own, differently-shaped representation (`OracleState`, a proof device), and widening that proof's
own evidence API to expose it would have coupled the worktree write to the proof's own internals.
Instead, `merge` now derives its own merge Block's candidate state directly — the identical
`derive_candidate` call `seal_block_at` itself makes again moments later, from the same mainline
parent, the same adopted patch ids, and the same merge anchor site — before `seal_block_at` ever
runs, and turns the derived state into a file list the same way `extract --claim` already builds
one from a derivation no Block holds (resident bytes from the text cache first, the Blob only when
absent). The identical `require_nothing_in_the_way` planner then runs against it, before any write,
naming the colliding path the same way `branch create --from`'s own refusal does. `archive import
--adopt` and `sync seal --claim` are unaffected either way, since neither one yet writes the
worktree at all.

**0.56.0 step W3c-4:** `sync seal <ref> --claim`/`--claims <file>`, onto the current branch, now
writes the sealed claim's own content into the worktree itself too, with the complete guarantee
the paragraphs above build up in parts: a dirty worktree, or a file already in the way of the
claim's own target files, refuses before any claim is sealed, and the claim (or, for a batch of
several, every claim up to and including the last one that actually publishes) is materialized
into the worktree afterward. A single `--claim <id>` is a batch of one claim, not a separate
behavior. Sealing every claim in a batch in order and keeping an earlier one's seal even when a
later one fails is unchanged: when that happens, the tip the batch actually produced is still
materialized into the worktree before the batch's own error is reported, so the worktree never
disagrees with what the batch really sealed. `archive import --adopt` is the one producer this
round leaves unaffected, still leaving the worktree exactly as it was, recoverable the same way the
`commit --ref heads/topic` walkthrough above is -- until the next paragraph closes it too.

**0.56.0 step W3c-5:** `archive import --adopt`, of a ref destined for the current branch, now
writes the adopted content into the worktree too. A courtesy clean check runs before anything from
the archive is written at all, so an unclean worktree refuses as early as possible; it is purely a
courtesy, since it cannot survive a crash and resume (a resumed import is a new process invocation
with no memory of an earlier attempt). The real guarantee runs again, from scratch, at the one
point immediately before the adoption's own publish, on both a fresh import and a resumed one
alike: the worktree must be clean, and only then does the publish move the current branch and the
adopted content get materialized into the worktree afterward.

**0.56.0 step W3c-5b** corrected two defects review found in this shape before release. First, a
file in the way of the adopted tip's own files now refuses before *anything* is written, the same
as every other producer in this series: `archive import --adopt` builds a seeking reader over the
archive's own carried Patch/Blob content and checks for a collision before the journal, before the
archive's own objects are ever admitted locally, rather than only after. A real backstop still runs
at the old, later point too, for the one window the early check cannot close (a file appearing
between the two checks, from outside prikk, under the same lock) — but it is now genuinely a
backstop, caught only in that rare case, not the normal path. Second, resuming an import that
crashed between a `--adopt` landing's own publish and its materialize now reports that landing as
what it actually is: adopted, completed by the earlier run, not a fresh fall-through to a received
`remotes/` pointer the uninterrupted import would never have written — and if that earlier run's
own materialize never finished either, this resume finishes it directly, so there is no second
`branch switch` step left for the operator to run by hand. A landing not destined for the current
branch, or one that cannot be adopted as a real local ref at all, is unaffected by either fix.

**0.56.0 step W3c-5c** closed one remaining gap in that second fix. A ref this import never
touched at all could already hold the identical content — not because an earlier attempt of this
import adopted it, but because some *other*, earlier import already had, and this import's own
archive happens to carry the same, unchanged tip for that ref. Resuming now also checks what the
ref held before this import's own plan was first computed, not only whether the content matches on
resume, so that case reports the ordinary fall-through too — exactly what an uninterrupted run of
the same import would have reported, rather than a false claim that this import adopted something
it never touched. An import still in progress from before this fix resumes correctly as well: the
ordinary fall-through applies there too, never a false adoption.

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
- branch switching or branch copy/fork from an existing tip
- multi-commit queued active sessions or per-ref active WALs
- multi-operation text diff minimization, commutation, conflict witnesses

Signature scope: worktree commits are role-bound Ed25519 `AUTHOR`-signed. This does not imply
trust-store enforcement, key management, `MAINTAINER`/publication signing, rollback authorization, or
publication-grade repository trust.
