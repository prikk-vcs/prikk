# Troubleshooting

Refusals a newcomer actually hits, in the [Tutorial](tutorial.md)'s own sequence, each quoted
verbatim from the CLI. If a message here reads as confusing, that is worth reporting — this page
explains what exists today; it does not change any wording.

For the three key-material refusals below — a missing seed, a seed others can read, and an
undecodable one — `prikk key status` reports the same state without needing a commit to provoke it,
and exits `0` while doing so. It names the file in effect, whether it is usable, and why not. See
[Can I Sign Here?](security-setup.md#can-i-sign-here).

## `error: author signing is required: no seed at <path>`

prikk looked for your AUTHOR seed and did not find it. Every commit produces a signed Patch; there is
no unsigned path. The message names the exact file and three ways to get one — create it with
`prikk key generate --out <that path>`, run `prikk setup` in a new project directory, or point
`PRIKK_AUTHOR_SEED_FILE` at a seed file you already have. `seal` says the same for the MAINTAINER
seed. See [First Run](first-run.md) for where the key directory is on each platform.

`prikk key status --role author` shows which of the two inputs it is resolving and to which path —
useful when the file you created is not the file it is looking for. An override that is set and
points at nothing reports `reason: override-missing`, which distinguishes "you have no key" from
"your `PRIKK_AUTHOR_SEED_FILE` is wrong".

prikk 0.40 also reported `error: precondition not met: PRIKK_AUTHOR_SEED is no longer read` when a
seed was still exported as an environment variable. That refusal was deliberately one release wide
and is gone as of 0.41: a stale `PRIKK_AUTHOR_SEED` or `PRIKK_MAINTAINER_SEED` is now simply unread,
and `prikk key status` is where you check which key is in effect instead.

## `error: <path> is readable by group or other (mode 0644); run `chmod 600 <path>``

A seed file anyone but you can read is refused before signing. Run the `chmod` the message names.
(Unix only — on Windows the key directory relies on `%APPDATA%`'s per-user ACL instead.)
`prikk key status` reports the same file as `usable: false` with the mode in its `reason`, and
running it again is how you confirm the `chmod` took.

## `error: <path> must be 64 hex characters, got 8`

A seed file must hold exactly 64 lowercase hex characters (32 raw bytes) — the number after `got`
reports how many your file actually had, so it will differ from the `8` shown here. A shorter,
longer, or non-hex value is rejected before anything is signed; nothing is written to the repository
when this fires. `prikk key status` reports such a file as `reason: undecodable`.

## `error: invalid name: worktree has no node-addressed changes to commit`

You ran `commit` with nothing changed since the last commit or seal — no new, edited, or deleted
files for Prikk to record. Change something first.

## `error: maintainer signing is required: no seed at <path>`

`seal` needs a *maintainer* key, distinct from the *author* key `commit` used — see
[why the tutorial needs a second key](tutorial.md#sealing--and-the-second-key). Same three routes as
the AUTHOR case above, with `maintainer.seed` in place of `author.seed`.

## `error: precondition not met: no maintainer key is adopted in this repository yet`

Your maintainer key is configured but not yet trusted by this repository. Run
`prikk trust maintainer add --key-id ID --public-key HEX` with the public half of your maintainer
key, then seal again. This is a repository-local, trust-on-first-use step — every fresh repository
needs it once, even with a key you have used elsewhere.

The message's parenthetical names the one other way this refusal can be reached: a trust policy
container damaged so badly it replays as empty is indistinguishable, at this read, from one never
written. If you know a key *was* adopted here before, run `prikk doctor`.

Earlier releases reported this same refusal as `error: integrity error: publication trust policy is
missing or unreadable`, which sent readers looking for corruption that was not there.

## `error: precondition not met: <path>: worktree symlink authoring is out of scope`

`commit` found a symlink where it needs a regular file — an untracked symlink in the worktree, or a
tracked file that has been replaced by one, including a **dangling** symlink whose target does not
exist. Symlink authoring is not implemented yet,
so the commit refuses rather than authoring something it cannot faithfully replay. Replace the
symlink with a regular file, or move it outside the repository (or add it to `.prikkignore` if it is
not meant to be tracked), then commit again.

You do not have to run `commit` to find these. `prikk worktree-status` reports the same paths, with
the same reason, under `refused paths:` — see [Worktree Status](worktree-status.md).

Earlier releases reported this as `error: integrity error: worktree authoring: unsupported symlink
authoring: ...`, which sent readers looking for repository corruption. Nothing is damaged.

## `error: precondition not met: <path>: existing TextFile cannot accept non-UTF-8 content`

A path tracked as a text node now holds bytes that are not valid UTF-8. Changing a node's kind from
text to binary (or back) is out of scope, so the commit refuses rather than silently changing what
the node is. Restore valid UTF-8 content at that path, or delete the path and add it again as a new
node, then commit again.

Earlier releases reported this as `error: integrity error: worktree authoring: unsupported kind
transition: ...`. Same refusal, correct class: it is a state you fix, not damage.

## `error: precondition not met: --target names a tag (tags/<t>); pass the block id it points at, or a branch ref`

You ran `prikk tag create --target` against another tag. A tag ref is one hop from its Block — ref →
tag object → block — and a tag of a tag is outside that model, so Prikk refuses rather than quietly
tagging the Block instead: that would make "tag this tag" and "tag this block" produce identical
history. Pass the block id (`prikk tag list` shows each tag's target block) or a branch ref.

`prikk branch create --from` is the other way round: it *does* dereference a tag, because `--from`
means "at the block this ref names".

Earlier releases reported this as `error: object type mismatch: expected block, got tag`, which read
as repository damage rather than as a correctable argument.

## `error: precondition not met: <path> already holds a repository`

You ran `prikk setup` on a directory that already has a `.prikk` in it. `setup` is the first-run
command: it initializes a repository *and* mints a fresh key pair, so there is nothing sensible for
it to do on a repository that already has adopted keys. It refuses before doing anything — no
`init`, no keys minted, no seed file written, even when you passed `--author-seed-out` or
`--maintainer-seed-out`.

Two ways on, both named in the message: to work in the existing repository with keys you already
hold, run `prikk trust maintainer add --key-id ID --public-key HEX` (use `prikk key public --role
maintainer` to derive the public half); to start a new project, point `setup` at a different
directory.

Earlier releases got further before failing: they printed `initialized Prikk repository at …`, minted
two keys, wrote any `--*-seed-out` file, and only then reported `maintainer key id maintainer is
already adopted with a different public key`. If you hit that version, the seed files it left behind
belong to keys **no repository adopted** — they are not your keys, and can be deleted.

## `error: integrity error: index entry for <id> resolves to an envelope with computed id <other>`

The object index points at the wrong record: the entry claims an object lives at a location that
actually holds a different one. Your objects are intact — the containers are self-describing and
carry every byte needed — it is only the index that is wrong. Run

```sh
prikk doctor --repair-index
```

which rebuilds the index by scanning the containers, touches no container bytes, and reports how many
entries it moved. Then `prikk verify` again.

This happened when two commands that both write objects ran at the same moment — for example a
`prikk commit` and a `prikk tag create`. Since prikk 0.40 that is prevented: the second command is
refused with `lock conflict` instead, so a repository cannot reach this state any more. The repair
exists for repositories damaged before that.

## `error: precondition not met: checkout target for <ref> does not contain a snapshot blob`

The block you asked to check out has no snapshot, which is the normal state of most blocks: `seal`
writes one only at a ref's first block and every 64 blocks after it. Use the patch-replay route instead,
which does not need a snapshot:

```sh
prikk checkout --patch-plan --ref <ref>
```

Earlier releases reported this as `error: integrity error: checkout target for <ref> does not contain
a snapshot blob`, which read as damage. Nothing is damaged, and nothing was ever missing.

## `error: precondition not met: the worktree was materialized from the snapshot of Block <block> on <ref> and is not replay-verified; run `prikk verify` …`

You ran `checkout --snapshot-materialize`, and the worktree it wrote is the block's signed state rather
than a replay of its history. Until the repository has been replayed, `commit`, `mv`, `seal`, `merge`,
`sync accept`, `sync seal`, `rollback-draft --append-inverse` and `branch switch` refuse, so nothing
derived from an unverified worktree becomes history. Run:

```sh
prikk verify
```

When verify finds nothing about objects, the WAL, refs or block state, it prints `provisional worktree:
replay-verified; marker cleared` and the commands work again. If it prints `provisional worktree: kept`,
fix what verify reported first. See [snapshot materialization](checkout/snapshot-materialization.md).

## `error: precondition not met: <old> -> <new>: the source is present in the worktree again, …`

A `prikk mv` declaration says `<old>` became `<new>`, and the worktree says otherwise — most often
because the move was undone with a shell `mv` rather than with `prikk mv`. The commit refuses rather
than guess which one is the tracked node, and **the worktree still reads `clean`**: nothing is
missing, modified or untracked, so `worktree-status` reports it as `refused declarations: 1` instead.

The refusal names the way out for the state you are actually in, and each command was measured:

```sh
prikk mv <new> <old>    # drop the declaration: it nets to no move and is removed
prikk mv <old> <new>    # or make the move again, and commit authors the rename
```

A shell `mv` back is not one of them: it recreates the state that produced this refusal.

If **both paths exist**, `prikk mv` refuses in either direction until one copy is set aside — delete
`<old>` and commit to author the rename, or delete `<new>` and then run `prikk mv <new> <old>`. If the
destination is **another tracked node** this commit does not also move or delete, `prikk mv <new> <old>`
drops the declaration; `<old>` keeps its content and `<new>` stays deleted, so the commit authors that
deletion. See [Declared Moves](patches/declared-move.md) and [Worktree Status](worktree-status.md).

## `warning: the snapshot of Block <block> failed validation (…); this report replayed the whole history instead -- run `prikk verify``

`checkout --patch-plan`, `--patch-delete-plan` or `bundle preview` tried to start at a snapshot, and the
snapshot did not validate. The report did not use it: it replayed the whole history, and its output and
exit code are what they would have been with no snapshot. The warning is a sign of a damaged or
tampered snapshot object. Run `prikk verify`, which names the block and the finding.

## `error: active WAL has no patch records to seal`

You ran `seal` with nothing queued — every commit since the last seal has already been published.
There is nothing to fix; commit something before sealing again.

## `error: invalid name: ref topic is not a local branch ref; expected heads/<name>`

You passed a bare name (`topic`) where Prikk expects a fully-qualified ref
(`heads/topic`). `branch create`, `branch close`, `tag create`, and any `--ref` flag all take the
qualified form — see [why refs are fully qualified](faq.md#why-is-it-headstopic-and-not-topic) for
the reasoning. Re-run the same command with `heads/` (or `tags/` for a tag) in front of the name.

## `error: i/o error: repository mutation requires Linux, macOS, or Windows root-scoped filesystem capabilities`

You are running `init`, `commit`, or `seal` on a platform other than Linux, macOS, or Windows.
Reading commands (`verify`, `log`, `status`, `doctor`) work anywhere Prikk builds; mutation does
not, by design — see [Platform Support](../reference/platform-support.md).

## `error: precondition not met: heads/<name> does not exist; run `prikk branch create heads/<name>` first`

`prikk branch switch` only moves to a branch that already exists. Create it from the branch you are
on with `prikk branch create heads/<name>`, then switch. The same command says `heads/<name> is
closed` for a closed branch: `prikk branch list` shows the open ones.

## `error: precondition not met: the active WAL holds unsealed work for heads/<name>; seal it …`

You committed on one branch and have not sealed, and are switching to another. Unsealed work belongs
to the branch it was committed on, so the switch refuses rather than leave it stranded. Seal it
(`prikk seal --allow-no-audit --ref heads/<name>`, exactly as the message prints), then switch.

## `error: precondition not met: the worktree is not clean against heads/<name>: <path> (modified), …`

The switch replaces the worktree's tracked files with the other branch's, so it refuses while any of
them differs from the branch you are on — it would otherwise overwrite or delete your edits. The
message lists each path and whether it is `modified`, `missing` or an `unsupported-path`;
`prikk worktree-status` shows the same list. Commit the changes, or restore the files, and switch
again. Untracked files do not count and are never touched.

## `error: precondition not met: refusing to switch to heads/<name>: <n> in the way of its files: …`

A file the other branch has would land on a path that already holds something that is not part of
the branch you are on — usually an untracked file with the same name. Nothing was written. Move the
listed paths aside and switch again.

**If a switch was interrupted** (a crash, a full disk), no file is torn and the current branch is still
the old one; `commit` then refuses with `worktree materialization was interrupted` (below). Run the same
`prikk branch switch heads/<name>` again: it recognises the half-switched files and completes.

## `error: precondition not met: refusing to materialize: <n> path(s) in the way: …`

`checkout --patch-materialize`, `--patch-materialize-delete` or `--snapshot-materialize` found files
that would have to be overwritten: each is listed with why, such as `an existing file with different
content` or `not a regular file`. Nothing was written. This is the ordinary result of checking out
another branch's files over your own: move the listed paths aside, or commit them, and run the checkout
again. Earlier releases wrote every file before the first conflict, left the worktree part-written and
reported `integrity error: refusing to overwrite existing file with different content`.

## `error: precondition not met: refusing checkout deletion because <n> candidate(s) are unsafe: …`

`--patch-materialize-delete` would delete files that no longer match what history deleted; each is named
with the reason. Nothing was written. Earlier releases reported this as an `integrity error:` without the
paths.

## `error: precondition not met: <path> changed during the checkout, so the worktree is partly written; …`

A file the checkout had checked changed before it was written, usually because another program wrote it
at the same moment. The checkout stopped part-way and the worktree is marked as interrupted. Move that
file aside, then run one of the two routes below.

## `error: precondition not met: worktree materialization was interrupted, …`

A checkout or `branch switch` stopped part-way: a crash, or the change above. `prikk status` prints
`interrupted materialization: …` naming your current branch, `status --format json` carries
`interrupted_materialization` with the two routes, and `prikk doctor` warns
`PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION`. Until it is cleared, `commit` refuses, because it cannot tell
a file you deleted from one the stopped checkout never wrote. Move aside any file the stopped checkout
named, then run either
`prikk checkout --patch-materialize --ref heads/<current branch>` or
`prikk branch switch heads/<current branch>`; both write the branch's files again and clear it. A file
the stopped checkout had already written stays in the worktree as an untracked file, and `commit` authors
the whole worktree as it always does. Earlier releases reported this as an `integrity error:` whose
suggested route could refuse again.

## `error: precondition not met: no object <id> in the object store or the active WAL; …`

`prikk show` was given an id that names nothing here — usually a typo, or an id from another
repository. Sealed patch and block ids are listed by `prikk log`; queued (committed, not yet sealed)
patch ids by `prikk status --format json`, and `show` accepts both. Earlier releases reported this as
`error: integrity error: no object <id>`, which read as repository damage; nothing is damaged.

## `error: precondition not met: .prikk/current-branch names heads/<name>, which does not exist; …`

A command whose `--ref` you did not give defaults to the current branch, and `.prikk/current-branch`
names a branch that is not there. The same message says `which is closed` for a closed branch, and
`.prikk/current-branch is malformed (…)` when the file does not hold exactly one `heads/<name>` line.
Nothing is damaged, and `prikk doctor` reports it as the warning `PRIKK-DOCTOR-CURRENT-BRANCH`. Either
run `prikk branch switch heads/<name>` to an existing, open branch — with an unusable pointer the switch
writes only what is absent and deletes nothing — or `prikk branch create` the branch the file names.
Any command still works with `--ref` given explicitly meanwhile.

## `error: invalid name: backslashes are not allowed in repository paths`

A file in the worktree has a name no repository path can hold — here a backslash; a name that is not
UTF-8 says `worktree path is not valid UTF-8: <name>` instead. `prikk commit` refuses the whole commit
rather than skip the file silently. `prikk worktree-status` lists it as an `unsupported-path` entry with
this same reason. Rename the file, or exclude it with a `.prikkignore` rule
([Ignoring Worktree Paths](ignore.md)).

## Something not listed here

Run [`prikk doctor`](tutorial.md#doctor) — it is the diagnostic-first command, and its recommendation
lines usually name the next step. If it does not, the message is real (Prikk does not invent
placeholder diagnostics) but this page has not caught up with it yet.
